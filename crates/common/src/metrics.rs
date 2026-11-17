//! Prometheus recorder setup. The recorder is installed once at startup so
//! every `metrics::counter!` call is safe; the app crate serves `/metrics` and
//! `/healthz` from the returned handle.

use anyhow::Result;
use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};

pub fn install() -> Result<PrometheusHandle> {
    let handle = PrometheusBuilder::new().install_recorder()?;

    metrics::describe_counter!("video_bot_cache_hits_total", "Served directly from cache");
    metrics::describe_counter!("video_bot_cache_misses_total", "Cache misses");
    metrics::describe_counter!("video_bot_downloads_total", "Cold-path download attempts");
    metrics::describe_counter!("video_bot_transcodes_total", "Fallback transcodes");
    metrics::describe_counter!("video_bot_remux_total", "Fast remuxes without re-encoding");
    metrics::describe_counter!("video_bot_uploads_total", "Telegram uploads attempted");
    metrics::describe_histogram!("video_bot_download_seconds", "Cold-path download latency");
    metrics::describe_histogram!("video_bot_ffmpeg_seconds", "Media processing latency");
    metrics::describe_histogram!("video_bot_upload_seconds", "Telegram upload latency");
    metrics::describe_histogram!("video_bot_inline_answer_seconds", "Inline answer latency");
    metrics::describe_histogram!(
        "video_bot_queue_wait_seconds",
        "Time spent waiting in queue"
    );
    metrics::describe_gauge!("video_bot_downloads_inflight", "Downloads running now");
    metrics::describe_gauge!("video_bot_queue_length", "Queued jobs");
    Ok(handle)
}
