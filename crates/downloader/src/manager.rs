//! Cold-path orchestration: metadata, in-flight deduplication, bounded
//! downloads, media preparation and cache recording.

use crate::ytdlp::YtDlp;
use common::config::Config;
use common::domain::{CachedVideo, PreparedVideo, QualityTier, VideoKey, VideoMeta};
use common::error::DownloadError;
use sources::SourceRegistry;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use storage::Db;
use tokio::sync::{Semaphore, broadcast};
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct VideoRequest {
    pub key: VideoKey,
    pub url: String,
    pub meta: Option<VideoMeta>,
}

type InflightResult = Result<PreparedVideo, Arc<DownloadError>>;

pub struct DownloadManager {
    config: Arc<Config>,
    registry: Arc<SourceRegistry>,
    ytdlp: YtDlp,
    db: Db,
    semaphore: Arc<Semaphore>,
    transcode_semaphore: Arc<Semaphore>,
    disk_semaphore: Arc<Semaphore>,
    inflight: Mutex<HashMap<(VideoKey, QualityTier), broadcast::Sender<InflightResult>>>,
}

impl DownloadManager {
    pub fn new(config: Arc<Config>, registry: Arc<SourceRegistry>, db: Db, ytdlp: YtDlp) -> Self {
        let transcode_permits = config.max_concurrent_transcodes;
        let disk_permits = config.disk_budget_bytes.div_ceil(50_000_000).max(1) as usize;
        Self {
            registry,
            ytdlp,
            db,
            semaphore: Arc::new(Semaphore::new(config.max_concurrent_downloads)),
            transcode_semaphore: Arc::new(Semaphore::new(transcode_permits)),
            disk_semaphore: Arc::new(Semaphore::new(disk_permits)),
            inflight: Mutex::new(HashMap::new()),
            config,
        }
    }

    pub fn registry(&self) -> &SourceRegistry {
        &self.registry
    }

    pub async fn get_cached(
        &self,
        key: &VideoKey,
        quality: QualityTier,
    ) -> Result<Option<CachedVideo>, DownloadError> {
        self.db
            .get_video(key, quality)
            .await
            .map_err(|error| DownloadError::Internal(format!("cache lookup failed: {error}")))
    }

    pub async fn touch_cached(&self, key: &VideoKey, quality: QualityTier) {
        self.db.touch_video(key, quality).await;
    }

    pub async fn purge(&self, key: &VideoKey) -> Result<bool, DownloadError> {
        self.db
            .purge_video(key)
            .await
            .map_err(|error| DownloadError::Internal(format!("cache purge failed: {error}")))
    }

    /// Record the Telegram `file_id` returned after a successful upload.
    pub async fn record(
        &self,
        prepared: &PreparedVideo,
        file_id: String,
        file_unique_id: String,
    ) -> Result<(), DownloadError> {
        let cached = CachedVideo {
            key: prepared.meta.key.clone(),
            quality: prepared.quality,
            file_id,
            file_unique_id,
            meta: prepared.meta.clone(),
            width: prepared.width,
            height: prepared.height,
            duration_sec: prepared.duration_sec,
            size_bytes: prepared.size_bytes,
        };
        self.db
            .put_video(&cached)
            .await
            .map_err(|error| DownloadError::Internal(format!("cache write failed: {error}")))
    }

    /// Download once per (video, quality), even if many requests arrive at
    /// the same time. Followers subscribe to the leader's broadcast result.
    pub async fn prepare(
        &self,
        request: VideoRequest,
        quality: QualityTier,
    ) -> Result<PreparedVideo, DownloadError> {
        if !self.config.source_enabled(request.key.source) {
            return Err(DownloadError::SourceDisabled(request.key.source));
        }
        let cache_key = (request.key.clone(), quality);
        let mut receiver = None;
        let mut sender = None;
        {
            let mut inflight = self
                .inflight
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(existing) = inflight.get(&cache_key) {
                receiver = Some(existing.subscribe());
            } else {
                let (tx, _rx) = broadcast::channel(8);
                inflight.insert(cache_key.clone(), tx.clone());
                sender = Some(tx);
            }
        }

        if let Some(mut rx) = receiver {
            loop {
                match rx.recv().await {
                    Ok(Ok(prepared)) => return Ok(prepared),
                    Ok(Err(error)) => return Err((*error).clone()),
                    Err(broadcast::error::RecvError::Closed) => {
                        return Box::pin(self.prepare(request, quality)).await;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        }

        let sender = sender.expect("leader sender must exist");
        let result = self.prepare_leader(request, quality).await;
        let broadcast = result.clone().map_err(Arc::new);
        let _ = sender.send(broadcast);
        let mut inflight = self
            .inflight
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inflight.remove(&cache_key);
        drop(inflight);
        result
    }

    async fn prepare_leader(
        &self,
        request: VideoRequest,
        quality: QualityTier,
    ) -> Result<PreparedVideo, DownloadError> {
        let started = Instant::now();
        metrics::gauge!("video_bot_downloads_inflight").increment(1.0);
        let result = self.prepare_leader_inner(request, quality).await;
        metrics::gauge!("video_bot_downloads_inflight").decrement(1.0);
        metrics::histogram!("video_bot_download_seconds").record(started.elapsed().as_secs_f64());
        match &result {
            Ok(_) => metrics::counter!("video_bot_downloads_total", "result" => "ok").increment(1),
            Err(_) => {
                metrics::counter!("video_bot_downloads_total", "result" => "error").increment(1)
            }
        }
        result
    }

    async fn prepare_leader_inner(
        &self,
        request: VideoRequest,
        quality: QualityTier,
    ) -> Result<PreparedVideo, DownloadError> {
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| DownloadError::Internal("download semaphore closed".into()))?;
        let _disk = self
            .disk_semaphore
            .acquire()
            .await
            .map_err(|_| DownloadError::Internal("disk semaphore closed".into()))?;

        let dir = tempfile::Builder::new()
            .prefix("video-")
            .tempdir_in(&self.config.download_dir)
            .map_err(|error| DownloadError::Internal(format!("cannot create temp dir: {error}")))?;

        let meta = self.ytdlp.metadata(&request.url, &request.key).await?;
        info!(key = %request.key, quality = %quality, "starting video download");

        let mut effective = quality;
        let mut retried = false;
        let downloaded = loop {
            match self
                .ytdlp
                .download(
                    &request.url,
                    effective,
                    dir.path(),
                    self.config.max_file_bytes(),
                )
                .await
            {
                Ok(path) => break path,
                Err(DownloadError::TooLarge { .. }) => {
                    if let Some(lower) = effective.lower() {
                        warn!(key = %request.key, from = %effective, to = %lower, "file too large, retrying smaller");
                        metrics::counter!("video_bot_quality_downgrades_total").increment(1);
                        effective = lower;
                        continue;
                    }
                    return Err(DownloadError::TooLarge {
                        limit: self.config.max_file_bytes(),
                        actual: 0,
                    });
                }
                Err(error) if error.is_transient() && !retried => {
                    retried = true;
                    warn!(key = %request.key, %error, "transient failure, retrying once");
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
                Err(error) => return Err(error),
            }
        };

        let prepared = media::prepare_video(
            &downloaded,
            meta,
            effective,
            &self.config,
            &self.transcode_semaphore,
            dir,
        )
        .await?;
        Ok(prepared)
    }
}
