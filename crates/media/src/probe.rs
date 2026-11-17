//! Read real stream parameters with ffprobe. Rotation is important: Telegram
//! expects `width`/`height` to match the *displayed* frame, not the raw one.

use common::config::Config;
use common::error::DownloadError;
use serde_json::Value;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;
use tracing::debug;

#[derive(Debug, Clone)]
pub struct MediaInfo {
    pub width: u32,
    pub height: u32,
    /// Dimensions after applying the display-matrix rotation.
    pub display_width: u32,
    pub display_height: u32,
    pub duration_sec: u32,
    pub video_codec: String,
    pub audio_codec: String,
    pub pix_fmt: String,
    pub rotation: i32,
    pub size_bytes: u64,
}

pub async fn probe(path: &Path, config: &Config) -> Result<MediaInfo, DownloadError> {
    let output = run_ffprobe(path, config, config.media_timeout).await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(DownloadError::Media(format!(
            "ffprobe failed: {}",
            common::i18n::truncate(&stderr, 500)
        )));
    }
    let json: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| DownloadError::Media(format!("invalid ffprobe JSON: {error}")))?;

    let duration_sec = json
        .get("format")
        .and_then(|format| format.get("duration"))
        .and_then(Value::as_str)
        .and_then(|value| value.parse::<f64>().ok())
        .map(|value| value.round().max(0.0) as u32)
        .unwrap_or(0);

    let mut info = MediaInfo {
        width: 0,
        height: 0,
        display_width: 0,
        display_height: 0,
        duration_sec,
        video_codec: String::new(),
        audio_codec: String::new(),
        pix_fmt: String::new(),
        rotation: 0,
        size_bytes: json
            .get("format")
            .and_then(|format| format.get("size"))
            .and_then(Value::as_str)
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_default(),
    };

    if let Some(streams) = json.get("streams").and_then(Value::as_array) {
        for stream in streams {
            match stream.get("codec_type").and_then(Value::as_str) {
                Some("video") => {
                    info.video_codec = stream
                        .get("codec_name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    info.pix_fmt = stream
                        .get("pix_fmt")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    info.width = stream
                        .get("width")
                        .and_then(Value::as_u64)
                        .and_then(|value| u32::try_from(value).ok())
                        .unwrap_or(0);
                    info.height = stream
                        .get("height")
                        .and_then(Value::as_u64)
                        .and_then(|value| u32::try_from(value).ok())
                        .unwrap_or(0);
                    info.rotation = read_rotation(stream);
                }
                Some("audio") => {
                    info.audio_codec = stream
                        .get("codec_name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                }
                _ => {}
            }
        }
    }

    if info.video_codec.is_empty() {
        return Err(DownloadError::Media("no video stream found".into()));
    }
    let rotated = matches!(info.rotation.rem_euclid(360), 90 | 270);
    info.display_width = if rotated { info.height } else { info.width };
    info.display_height = if rotated { info.width } else { info.height };
    if info.display_width == 0 || info.display_height == 0 {
        return Err(DownloadError::Media("invalid video dimensions".into()));
    }
    debug!(?info, "probed video");
    Ok(info)
}

fn read_rotation(stream: &Value) -> i32 {
    if let Some(rotation) = stream
        .get("side_data_list")
        .and_then(Value::as_array)
        .and_then(|list| list.iter().find_map(|item| item.get("rotation")))
        .and_then(Value::as_i64)
    {
        return rotation as i32;
    }
    stream
        .get("tags")
        .and_then(|tags| tags.get("rotate"))
        .and_then(Value::as_str)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

async fn run_ffprobe(
    path: &Path,
    config: &Config,
    timeout: Duration,
) -> Result<std::process::Output, DownloadError> {
    let args = [
        "-v",
        "error",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
        &path.to_string_lossy(),
    ];
    let mut command = Command::new(&config.ffprobe_path);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| DownloadError::Media("ffprobe timed out".into()))?
        .map_err(|error| DownloadError::Media(format!("cannot start ffprobe: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_detection_from_side_data() {
        let json: Value = serde_json::from_str(
            r#"{"side_data_list":[{"rotation":-90},{"rotation":0}]}"#,
        )
        .expect("json");
        assert_eq!(read_rotation(&json), -90);
    }

    #[test]
    fn rotation_detection_from_tags() {
        let json: Value = serde_json::from_str(r#"{"tags":{"rotate":"90"}}"#).expect("json");
        assert_eq!(read_rotation(&json), 90);
    }
}
