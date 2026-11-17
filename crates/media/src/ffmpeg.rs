//! Child-process wrappers around ffmpeg. Every invocation uses argv arrays
//! (never a shell), `-nostdin`, `kill_on_drop` and a hard timeout.

use common::config::Config;
use common::error::DownloadError;
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;
use tracing::warn;

async fn run(args: Vec<String>, config: &Config) -> Result<std::process::Output, DownloadError> {
    let mut command = Command::new(&config.ffmpeg_path);
    command
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(config.media_timeout, command.output())
        .await
        .map_err(|_| DownloadError::Media("ffmpeg timed out".into()))?
        .map_err(|error| DownloadError::Media(format!("cannot start ffmpeg: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(stderr = %common::i18n::truncate(&stderr, 500), "ffmpeg failed");
    }
    Ok(output)
}

/// Fast, lossless remux with `moov` atom moved to the front so Telegram can
/// start playback before the whole file is downloaded.
pub async fn remux_faststart(
    input: &Path,
    output: &Path,
    config: &Config,
) -> Result<(), DownloadError> {
    let args = vec![
        "-nostdin".into(),
        "-y".into(),
        "-i".into(),
        input.to_string_lossy().into_owned(),
        "-c".into(),
        "copy".into(),
        "-map".into(),
        "0:v:0".into(),
        "-map".into(),
        "0:a:0?".into(),
        "-movflags".into(),
        "+faststart".into(),
        "-threads".into(),
        "2".into(),
        output.to_string_lossy().into_owned(),
    ];
    let output_result = run(args, config).await?;
    if !output_result.status.success() {
        return Err(DownloadError::Media("faststart remux failed".into()));
    }
    Ok(())
}

/// Fallback path for VP9/AV1/HEVC, 10-bit or 4:4:4 video: re-encode to the
/// universally supported H.264 + AAC, scaled to the requested quality tier.
pub async fn transcode(
    input: &Path,
    output: &Path,
    height: u32,
    config: &Config,
) -> Result<(), DownloadError> {
    let scale = format!("scale=-2:min({height},ih)");
    let args = vec![
        "-nostdin".into(),
        "-y".into(),
        "-i".into(),
        input.to_string_lossy().into_owned(),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-crf".into(),
        "23".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-vf".into(),
        scale,
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "128k".into(),
        "-movflags".into(),
        "+faststart".into(),
        "-threads".into(),
        "2".into(),
        output.to_string_lossy().into_owned(),
    ];
    let result = run(args, config).await?;
    if !result.status.success() {
        return Err(DownloadError::Media("transcode failed".into()));
    }
    Ok(())
}

/// Generate a small JPEG thumbnail (<200 KB, sides <= 320 px). The encoder
/// quality is lowered until the image fits Telegram's thumbnail budget.
pub async fn thumbnail(input: &Path, output: &Path, config: &Config) -> Result<(), DownloadError> {
    const MAX_BYTES: u64 = 200_000;
    for quality in [4u8, 6, 8, 10, 13, 16] {
        let args = vec![
            "-nostdin".into(),
            "-y".into(),
            "-ss".into(),
            "1".into(),
            "-i".into(),
            input.to_string_lossy().into_owned(),
            "-frames:v".into(),
            "1".into(),
            "-vf".into(),
            "scale=min(320,iw):-2".into(),
            "-q:v".into(),
            quality.to_string(),
            output.to_string_lossy().into_owned(),
        ];
        let result = run(args, config).await?;
        if !result.status.success() {
            continue;
        }
        if let Ok(metadata) = tokio::fs::metadata(output).await
            && metadata.len() <= MAX_BYTES
        {
            return Ok(());
        }
    }
    Err(DownloadError::Media(
        "could not create a thumbnail under 200 KB".into(),
    ))
}
