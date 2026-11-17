//! Decide whether the downloaded file can be sent as-is (fast remux only) or
//! needs a costly transcode, then produce the final mp4 plus thumbnail.

use crate::ffmpeg;
use crate::probe::probe;
use common::config::Config;
use common::domain::{PreparedVideo, QualityTier, VideoMeta};
use common::error::DownloadError;
use std::path::Path;
use tempfile::TempDir;
use tokio::sync::Semaphore;
use tracing::{info, warn};

pub async fn prepare_video(
    input: &Path,
    meta: VideoMeta,
    quality: QualityTier,
    config: &Config,
    transcode_semaphore: &Semaphore,
    dir: TempDir,
) -> Result<PreparedVideo, DownloadError> {
    let info = probe(input, config).await?;

    if u64::from(info.duration_sec) > config.max_duration_sec {
        return Err(DownloadError::TooLong {
            limit: config.max_duration_sec,
            actual: u64::from(info.duration_sec),
        });
    }
    if info.size_bytes > config.max_file_bytes() {
        return Err(DownloadError::TooLarge {
            limit: config.max_file_bytes(),
            actual: info.size_bytes,
        });
    }

    let already_compatible = info.video_codec == "h264"
        && info.audio_codec == "aac"
        && (info.pix_fmt == "yuv420p" || info.pix_fmt.is_empty())
        && input
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mp4"));

    let output = dir.path().join("video.mp4");
    let transcoded = if already_compatible {
        info!(key = %meta.key, "compatible stream: fast remux only");
        metrics::counter!("video_bot_remux_total").increment(1);
        ffmpeg::remux_faststart(input, &output, config).await?;
        false
    } else {
        warn!(
            key = %meta.key,
            video = %info.video_codec,
            audio = %info.audio_codec,
            pix_fmt = %info.pix_fmt,
            "incompatible stream: transcoding"
        );
        let _permit = transcode_semaphore
            .acquire()
            .await
            .map_err(|_| DownloadError::Internal("transcode semaphore closed".into()))?;
        metrics::counter!("video_bot_transcodes_total").increment(1);
        ffmpeg::transcode(input, &output, quality.height(), config).await?;
        true
    };

    let size_bytes = tokio::fs::metadata(&output).await?.len();
    if size_bytes > config.max_file_bytes() {
        return Err(DownloadError::TooLarge {
            limit: config.max_file_bytes(),
            actual: size_bytes,
        });
    }

    let thumbnail = {
        let path = dir.path().join("thumb.jpg");
        match ffmpeg::thumbnail(&output, &path, config).await {
            Ok(()) => Some(path),
            Err(error) => {
                warn!(%error, "thumbnail generation skipped");
                None
            }
        }
    };

    Ok(PreparedVideo::new(
        output,
        thumbnail,
        meta,
        quality,
        info.display_width,
        info.display_height,
        info.duration_sec,
        size_bytes,
        transcoded,
        dir,
    ))
}
