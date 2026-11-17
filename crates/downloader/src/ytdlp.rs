//! Async wrapper around the yt-dlp process. Commands are argv arrays (never a
//! shell), always end with `--` before the URL and carry a hard timeout.

use common::config::Config;
use common::domain::{QualityTier, VideoKey, VideoMeta};
use common::error::DownloadError;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tracing::debug;

#[derive(Clone)]
pub struct YtDlp {
    binary: String,
    ffmpeg: String,
    metadata_timeout: Duration,
    download_timeout: Duration,
}

#[derive(Debug, Deserialize, Default)]
struct Info {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    uploader: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(default)]
    webpage_url: Option<String>,
    #[serde(default)]
    thumbnails: Vec<Thumb>,
}

#[derive(Debug, Deserialize)]
struct Thumb {
    url: String,
}

impl YtDlp {
    pub fn new(config: &Config) -> Self {
        Self {
            binary: config.ytdlp_path.clone(),
            ffmpeg: config.ffmpeg_path.clone(),
            metadata_timeout: config.metadata_timeout,
            download_timeout: config.download_timeout,
        }
    }

    /// Full metadata via `yt-dlp -J --no-playlist`. Called only when a real
    /// download starts, never on every inline query.
    pub async fn metadata(&self, url: &str, key: &VideoKey) -> Result<VideoMeta, DownloadError> {
        let args = [
            "--ignore-config",
            "--dump-single-json",
            "--no-playlist",
            "--skip-download",
            "--no-warnings",
            "--",
            url,
        ];
        let output = self.run(&args, self.metadata_timeout).await?;
        if !output.status.success() {
            return Err(classify_failure(&String::from_utf8_lossy(&output.stderr)));
        }
        let info: Info = serde_json::from_slice(&output.stdout).map_err(|error| {
            DownloadError::SourceUnavailable(format!("invalid yt-dlp JSON: {error}"))
        })?;
        if info.title.as_deref().unwrap_or_default().trim().is_empty() {
            return Err(DownloadError::NotFound);
        }
        Ok(meta_from(info, key, url))
    }

    /// Download a single video in a Telegram-friendly format. Returns the
    /// path printed by `--print after_move:filepath` (or a directory scan).
    pub async fn download(
        &self,
        url: &str,
        quality: QualityTier,
        dir: &Path,
        max_bytes: u64,
    ) -> Result<PathBuf, DownloadError> {
        let selector = format_selector(quality.height());
        let template = dir.join("video.%(ext)s").to_string_lossy().into_owned();
        let max_bytes_arg = max_bytes.to_string();
        let args = [
            "--ignore-config",
            "--no-playlist",
            "--no-warnings",
            "--no-progress",
            "-f",
            selector.as_str(),
            "--merge-output-format",
            "mp4",
            "-N",
            "4",
            "--socket-timeout",
            "15",
            "--retries",
            "3",
            "--max-filesize",
            max_bytes_arg.as_str(),
            "--ffmpeg-location",
            self.ffmpeg.as_str(),
            "--output",
            template.as_str(),
            "--print",
            "after_move:filepath",
            "--",
            url,
        ];
        let output = self.run(&args, self.download_timeout).await?;
        if !output.status.success() {
            return Err(classify_failure(&String::from_utf8_lossy(&output.stderr)));
        }
        if let Some(path) = last_line(&output.stdout)
            && Path::new(&path).is_file()
        {
            return Ok(PathBuf::from(path));
        }
        find_video(dir).await
    }

    async fn run(&self, args: &[&str], timeout: Duration) -> Result<std::process::Output, DownloadError> {
        debug!(binary = %self.binary, ?args, "running yt-dlp");
        let mut command = Command::new(&self.binary);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        tokio::time::timeout(timeout, command.output())
            .await
            .map_err(|_| DownloadError::Timeout)?
            .map_err(|error| DownloadError::SourceUnavailable(format!("cannot start yt-dlp: {error}")))
    }
}

fn format_selector(height: u32) -> String {
    // Prefer H.264 (avc1) + AAC (m4a) in mp4; fall back to other codecs only
    // when the source has no compatible stream (then media/transcodes).
    format!(
        "bv*[vcodec^=avc1][height<={h}]+ba[ext=m4a]/b[vcodec^=avc1][height<={h}][ext=mp4]/bv*[height<={h}]+ba/b[height<={h}]/b",
        h = height
    )
}

fn meta_from(info: Info, key: &VideoKey, fallback_url: &str) -> VideoMeta {
    let thumbnail = info.thumbnail.clone().or_else(|| {
        info.thumbnails
            .iter()
            .max_by_key(|thumb| thumb.url.len())
            .map(|thumb| thumb.url.clone())
    });
    VideoMeta {
        key: key.clone(),
        title: info.title.unwrap_or_default().trim().to_owned(),
        uploader: info
            .uploader
            .or(info.channel)
            .filter(|value| !value.trim().is_empty()),
        duration: info
            .duration
            .filter(|value| value.is_finite() && *value > 0.0)
            .map(|value| Duration::from_secs(value.round() as u64)),
        thumbnail_url: thumbnail,
        page_url: info.webpage_url.unwrap_or_else(|| fallback_url.to_owned()),
    }
}

fn last_line(stdout: &[u8]) -> Option<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .last()
        .map(ToOwned::to_owned)
}

async fn find_video(dir: &Path) -> Result<PathBuf, DownloadError> {
    let mut entries = tokio::fs::read_dir(dir).await?;
    let mut found = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    matches!(extension.to_ascii_lowercase().as_str(), "mp4" | "webm" | "mkv" | "mov")
                })
        {
            found.push(path);
        }
    }
    found.sort();
    found.into_iter().next().ok_or_else(|| {
        DownloadError::SourceUnavailable("yt-dlp produced no video file".into())
    })
}

/// Map yt-dlp stderr patterns onto friendly error variants. Keep all patterns
/// in one place so they can be covered by tests.
pub fn classify_failure(stderr: &str) -> DownloadError {
    let lower = stderr.to_ascii_lowercase();
    if lower.contains("private video") || lower.contains("login required") {
        DownloadError::Private
    } else if lower.contains("age-restricted") || lower.contains("age restricted") {
        DownloadError::AgeRestricted
    } else if lower.contains("not available in your country")
        || lower.contains("geo-restricted")
        || lower.contains("geo restricted")
    {
        DownloadError::RegionLocked
    } else if lower.contains("max-filesize") || lower.contains("file is larger") {
        DownloadError::TooLarge {
            limit: 0,
            actual: 0,
        }
    } else if lower.contains("video unavailable")
        || lower.contains("not found")
        || lower.contains("has been removed")
        || lower.contains("no video formats")
    {
        DownloadError::NotFound
    } else if lower.contains("timed out") || lower.contains("timeout") {
        DownloadError::Timeout
    } else {
        DownloadError::SourceUnavailable(common::i18n::truncate(stderr.trim(), 300))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_prefers_h264_720() {
        let selector = format_selector(720);
        assert!(selector.contains("vcodec^=avc1"));
        assert!(selector.contains("height<=720"));
        assert!(selector.contains("ba[ext=m4a]"));
    }

    #[test]
    fn classifies_common_errors() {
        assert!(matches!(
            classify_failure("ERROR: Private video"),
            DownloadError::Private
        ));
        assert!(matches!(
            classify_failure("not available in your country"),
            DownloadError::RegionLocked
        ));
        assert!(matches!(
            classify_failure("age-restricted video"),
            DownloadError::AgeRestricted
        ));
        assert!(matches!(
            classify_failure("some random failure"),
            DownloadError::SourceUnavailable(_)
        ));
    }
}
