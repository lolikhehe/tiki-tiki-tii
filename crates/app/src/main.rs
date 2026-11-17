//! Entry point: parse config, wire the crates together, run the dispatcher.

mod health;
mod janitor;

use anyhow::Context;
use common::config::Config;
use downloader::{DownloadManager, YtDlp};
use sources::SourceRegistry;
use std::sync::Arc;
use std::time::Duration;
use storage::Db;
use telegram::state::AppState;
use teloxide::prelude::Requester;
use tracing::warn;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let config = Arc::new(Config::from_env()?);
    init_tracing(&config);

    let metrics = common::metrics::install()?;
    tokio::fs::create_dir_all(&config.download_dir).await?;
    janitor::spawn(config.download_dir.clone(), Duration::from_secs(3600));

    let db = Db::connect(&config.database_url).await?;
    if let Some(addr) = config.metrics_addr {
        health::serve(addr, db.clone(), metrics.clone()).await?;
    } else {
        drop(metrics);
    }

    let registry = Arc::new(SourceRegistry::new(&config)?);
    let ytdlp = YtDlp::new(&config);
    let manager = Arc::new(DownloadManager::new(
        config.clone(),
        registry,
        db.clone(),
        ytdlp,
    ));

    let bot = make_bot(&config)?;
    wait_for_bot_api(&config).await;
    let me = bot.get_me().await?;
    let bot_username = me
        .user
        .username
        .clone()
        .unwrap_or_else(|| "video_bot".to_owned());
    tracing::info!(bot_username = %bot_username, "connected to Telegram");

    let state = AppState::new(config.clone(), db, manager, bot_username);
    telegram::bot::run(bot, state).await
}

fn init_tracing(config: &Config) {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(&config.log_level))
        .unwrap_or_else(|_| EnvFilter::new("info"));
    let builder = fmt().with_env_filter(filter);
    if config.log_json {
        builder
            .json()
            .with_current_span(false)
            .with_span_list(false)
            .init();
    } else {
        builder.init();
    }
}

fn make_bot(config: &Config) -> anyhow::Result<teloxide::Bot> {
    let bot = teloxide::Bot::new(config.token.clone());
    match &config.telegram_api_url {
        Some(url) => {
            let parsed = reqwest::Url::parse(url)
                .with_context(|| format!("TELEGRAM_API_URL is not a valid URL: {url}"))?;
            tracing::info!(%parsed, "using custom Bot API endpoint");
            Ok(bot.set_api_url(parsed))
        }
        None => Ok(bot),
    }
}

/// Give a local Bot API server time to become healthy before the first getMe.
async fn wait_for_bot_api(config: &Config) {
    let Some(url) = &config.telegram_api_url else {
        return;
    };
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return;
    };
    let Some(host) = parsed.host_str().map(str::to_owned) else {
        return;
    };
    let port = parsed.port_or_known_default().unwrap_or(80);
    let deadline = std::time::Instant::now() + config.bot_api_wait;
    while std::time::Instant::now() < deadline {
        if tokio::time::timeout(
            Duration::from_secs(2),
            tokio::net::TcpStream::connect((host.as_str(), port)),
        )
        .await
        .is_ok_and(|result| result.is_ok())
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    warn!(host = %host, %port, "Bot API server did not respond in time; continuing anyway");
}
