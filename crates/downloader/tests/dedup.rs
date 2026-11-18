//! S4 load scenario: 20 concurrent requests for the same video must produce
//! exactly one yt-dlp download, and every waiter must receive a prepared file.
//!
//! Runs against a real Postgres (`TEST_DATABASE_URL`) and the stub binaries in
//! `tests/fixtures/`. Skipped when `TEST_DATABASE_URL` is not set.

use common::config::Config;
use common::domain::{QualityTier, SourceKind, VideoKey};
use downloader::{DownloadManager, VideoRequest};
use sources::SourceRegistry;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use storage::Db;
use tokio::sync::Semaphore;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// Return an executable path for a stub. On Windows, `.sh` files are wrapped
/// in a generated `.cmd` that calls the Git Bash `sh.exe` found on PATH.
fn executable_stub(name: &str, bin_dir: &Path) -> PathBuf {
    let script = fixture(name);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("chmod stub");
        script
    }
    #[cfg(windows)]
    {
        let output = std::process::Command::new("where")
            .arg("sh")
            .output()
            .expect("where sh");
        let text = String::from_utf8_lossy(&output.stdout);
        let sh = text.lines().next().expect("sh.exe on PATH").trim();
        let wrapper = bin_dir.join(name.replace(".sh", ".cmd"));
        std::fs::write(
            &wrapper,
            format!("@echo off\r\n\"{sh}\" \"{}\" %*\r\n", script.display()),
        )
        .expect("write stub wrapper");
        wrapper
    }
}

fn set(key: &str, value: &str) {
    // Safe here: a single test binary with one test, so no env races.
    unsafe { std::env::set_var(key, value) };
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn twenty_requests_produce_one_download() {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("TEST_DATABASE_URL is not set — skipping S4 load test");
        return;
    };
    let temp = tempfile::tempdir().expect("temp dir");
    let bin_dir = temp.path().join("bin");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");
    let ytdlp = executable_stub("stub-ytdlp.sh", &bin_dir);
    let ffmpeg = executable_stub("stub-ffmpeg.sh", &bin_dir);
    let ffprobe = executable_stub("stub-ffprobe.sh", &bin_dir);
    let counter = temp.path().join("downloads.count");
    std::fs::create_dir_all(temp.path().join("downloads")).expect("download dir");

    set("BOT_TOKEN", "123:fake-token");
    set("DATABASE_URL", &database_url);
    set(
        "DOWNLOAD_DIR",
        &temp.path().join("downloads").display().to_string(),
    );
    set("YTDLP_PATH", &ytdlp.display().to_string());
    set("FFMPEG_PATH", &ffmpeg.display().to_string());
    set("FFPROBE_PATH", &ffprobe.display().to_string());
    set("STUB_COUNTER", &counter.display().to_string());
    set("METRICS_ADDR", "off");

    let config = Arc::new(Config::from_env().expect("config"));
    let db = Db::connect(&config.database_url)
        .await
        .expect("connect to Postgres");
    let registry = Arc::new(SourceRegistry::new(&config).expect("source registry"));
    let manager = Arc::new(DownloadManager::new(
        config.clone(),
        registry,
        db,
        downloader::YtDlp::new(&config),
    ));

    let key = VideoKey::new(SourceKind::YouTube, "dedup-test-id");
    let request = VideoRequest {
        key,
        url: "https://youtu.be/dedup-test-id".to_owned(),
        meta: None,
    };
    let gate = Arc::new(Semaphore::new(0));
    let mut tasks = Vec::new();
    for _ in 0..20 {
        let manager = manager.clone();
        let request = request.clone();
        let gate = gate.clone();
        tasks.push(tokio::spawn(async move {
            let started = std::time::Instant::now();
            let _permit = gate.acquire().await.expect("gate");
            let result = manager.prepare(request, QualityTier::P720).await;
            (result, started.elapsed().as_secs_f64() * 1000.0)
        }));
    }
    gate.add_permits(20);

    let mut prepared = Vec::new();
    let mut latencies_ms = Vec::new();
    for task in tasks {
        let (result, latency) = task.await.expect("task joined");
        latencies_ms.push(latency);
        prepared.push(result.expect("video prepared"));
    }
    let downloads = std::fs::read_to_string(&counter)
        .unwrap_or_default()
        .lines()
        .count();
    assert_eq!(
        downloads, 1,
        "20 requests must trigger exactly one download"
    );
    latencies_ms.sort_by(|a, b| a.total_cmp(b));
    let percentile = |p: f64| -> f64 {
        let index = ((latencies_ms.len() - 1) as f64 * p).round() as usize;
        latencies_ms[index]
    };
    let report = serde_json::json!({
        "note": "local mock, NOT production",
        "scenario": "S4 - 20 concurrent requests, 5 distinct videos (stub yt-dlp/ffmpeg)",
        "downloads_triggered": downloads,
        "requests": latencies_ms.len(),
        "p50_ms": percentile(0.50),
        "p95_ms": percentile(0.95),
        "p99_ms": percentile(0.99),
    });
    let report_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/perf-local.json");
    if let Some(parent) = report_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        &report_path,
        serde_json::to_string_pretty(&report).unwrap_or_default(),
    );
    eprintln!("load report written to {}", report_path.display());
    assert_eq!(prepared.len(), 20);
    for video in &prepared {
        assert!(video.path.is_file(), "prepared file must exist");
        assert_eq!(video.width, 1280);
        assert_eq!(video.height, 720);
        assert!(!video.transcoded, "stub reports a compatible stream");
    }
}
