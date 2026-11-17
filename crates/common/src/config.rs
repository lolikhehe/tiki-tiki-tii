//! Environment-only configuration. Invalid or unsafe values fail fast at
//! startup instead of surfacing in the middle of a download. Secrets are
//! never written to logs.

use crate::domain::{Lang, SourceKind};
use anyhow::{Context, Result, bail};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Config {
    pub token: String,
    pub telegram_api_url: Option<String>,
    pub webhook_url: Option<String>,
    pub webhook_port: u16,
    pub database_url: String,
    pub storage_chat_id: Option<i64>,
    pub enabled_sources: HashSet<SourceKind>,
    pub max_concurrent_downloads: usize,
    pub max_concurrent_transcodes: usize,
    pub max_concurrent_uploads: usize,
    pub max_duration_sec: u64,
    pub max_file_mb: u64,
    pub default_max_height: u32,
    pub download_dir: PathBuf,
    pub admin_ids: HashSet<i64>,
    pub log_level: String,
    pub log_json: bool,
    pub metrics_addr: Option<SocketAddr>,
    pub default_lang: Lang,
    pub rate_limit_per_min: u32,
    pub ytdlp_path: String,
    pub ffmpeg_path: String,
    pub ffprobe_path: String,
    pub download_timeout: Duration,
    pub metadata_timeout: Duration,
    pub media_timeout: Duration,
    pub quick_meta_timeout: Duration,
    pub bot_api_wait: Duration,
    pub proxy: Option<String>,
    pub disk_budget_bytes: u64,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let token = env_required("BOT_TOKEN")
            .or_else(|_| env_required("TELOXIDE_TOKEN"))
            .context("BOT_TOKEN (or TELOXIDE_TOKEN) is required")?;
        if token.trim().is_empty() {
            bail!("BOT_TOKEN is empty");
        }

        let telegram_api_url = env_optional("TELEGRAM_API_URL").map(|value| {
            if value.starts_with("http://") || value.starts_with("https://") {
                value.trim_end_matches('/').to_owned()
            } else {
                format!("http://{}", value.trim_end_matches('/'))
            }
        });

        let webhook_url = env_optional("WEBHOOK_URL");
        let webhook_port = match &webhook_url {
            Some(url) => url::Url::parse(url)
                .context("WEBHOOK_URL is not a valid URL")?
                .port_or_known_default()
                .context("WEBHOOK_URL has no port")?,
            None => 8080,
        };

        let database_url =
            env_required("DATABASE_URL").context("DATABASE_URL is required (postgres://…)")?;
        if !database_url.starts_with("postgres://") && !database_url.starts_with("postgresql://") {
            bail!("DATABASE_URL must be a postgres:// connection string");
        }

        let enabled_sources =
            parse_sources(&env_optional("ENABLED_SOURCES").unwrap_or_else(|| "all".to_owned()))?;

        let cpus = std::thread::available_parallelism()
            .map(|value| value.get())
            .unwrap_or(4);
        let max_concurrent_downloads = env_parse("MAX_CONCURRENT_DOWNLOADS", 3usize)?;
        let max_concurrent_transcodes = env_parse("MAX_CONCURRENT_TRANSCODES", (cpus / 2).max(1))?;
        let max_concurrent_uploads = env_parse("MAX_CONCURRENT_UPLOADS", 4usize)?;
        if max_concurrent_downloads == 0 || max_concurrent_downloads > 64 {
            bail!("MAX_CONCURRENT_DOWNLOADS must be between 1 and 64");
        }
        if max_concurrent_transcodes == 0 || max_concurrent_transcodes > 32 {
            bail!("MAX_CONCURRENT_TRANSCODES must be between 1 and 32");
        }

        let max_duration_sec = env_parse("MAX_DURATION_SEC", 900u64)?;
        let max_file_mb = env_parse("MAX_FILE_MB", 200u64)?.clamp(1, 2000);
        let default_max_height = env_parse("DEFAULT_MAX_HEIGHT", 720u32)?;
        if ![480, 720, 1080].contains(&default_max_height) {
            bail!("DEFAULT_MAX_HEIGHT must be 480, 720 or 1080");
        }

        let mut download_dir = PathBuf::from(
            env_optional("DOWNLOAD_DIR").unwrap_or_else(|| "./data/downloads".into()),
        );
        if download_dir.as_os_str().is_empty() {
            download_dir = PathBuf::from("./data/downloads");
        }

        let admin_ids = env_optional("ADMIN_IDS")
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(|part| {
                        part.parse::<i64>()
                            .with_context(|| format!("invalid admin id: {part}"))
                    })
                    .collect::<Result<HashSet<_>>>()
            })
            .transpose()?
            .unwrap_or_default();

        let storage_chat_id = env_optional("STORAGE_CHAT_ID")
            .map(|value| {
                value
                    .trim()
                    .parse::<i64>()
                    .with_context(|| format!("STORAGE_CHAT_ID must be numeric: {value}"))
            })
            .transpose()?;

        let metrics_addr = match env_optional("METRICS_ADDR") {
            None => Some("0.0.0.0:9090".parse().context("METRICS_ADDR")?),
            Some(value) if value.is_empty() || value.eq_ignore_ascii_case("off") => None,
            Some(value) => Some(value.parse().context("METRICS_ADDR must be host:port")?),
        };

        Ok(Self {
            token,
            telegram_api_url,
            webhook_url,
            webhook_port,
            database_url,
            storage_chat_id,
            enabled_sources,
            max_concurrent_downloads,
            max_concurrent_transcodes,
            max_concurrent_uploads,
            max_duration_sec,
            max_file_mb,
            default_max_height,
            download_dir,
            admin_ids,
            log_level: env_optional("LOG_LEVEL").unwrap_or_else(|| "info".into()),
            log_json: env_optional("LOG_FORMAT")
                .map(|value| value.eq_ignore_ascii_case("json"))
                .unwrap_or(false),
            metrics_addr,
            default_lang: env_optional("DEFAULT_LANG")
                .and_then(|value| Lang::from_code(&value))
                .unwrap_or(Lang::Uk),
            rate_limit_per_min: env_parse("RATE_LIMIT_PER_MIN", 10u32)?,
            ytdlp_path: env_optional("YTDLP_PATH").unwrap_or_else(|| "yt-dlp".into()),
            ffmpeg_path: env_optional("FFMPEG_PATH").unwrap_or_else(|| "ffmpeg".into()),
            ffprobe_path: env_optional("FFPROBE_PATH").unwrap_or_else(|| "ffprobe".into()),
            download_timeout: Duration::from_secs(env_parse("DOWNLOAD_TIMEOUT_SECS", 300u64)?),
            metadata_timeout: Duration::from_secs(env_parse("METADATA_TIMEOUT_SECS", 30u64)?),
            media_timeout: Duration::from_secs(env_parse("MEDIA_TIMEOUT_SECS", 180u64)?),
            quick_meta_timeout: Duration::from_millis(env_parse("QUICK_META_TIMEOUT_MS", 500u64)?),
            bot_api_wait: Duration::from_secs(env_parse("BOT_API_WAIT_SECS", 20u64)?),
            proxy: env_optional("PROXY"),
            disk_budget_bytes: env_parse("DISK_BUDGET_MB", 4096u64)?.saturating_mul(1_000_000),
        })
    }

    pub fn is_admin(&self, user_id: i64) -> bool {
        self.admin_ids.contains(&user_id)
    }

    pub fn source_enabled(&self, source: SourceKind) -> bool {
        self.enabled_sources.contains(&source)
    }

    pub fn max_file_bytes(&self) -> u64 {
        // Cloud Bot API rejects uploads above 50 MB. A local Bot API server
        // (TELEGRAM_API_URL set) accepts roughly 2 GB.
        let ceiling = if self.telegram_api_url.is_some() {
            2_000_000_000
        } else {
            49_000_000
        };
        self.max_file_mb.saturating_mul(1_000_000).min(ceiling)
    }
}

fn env_optional(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn env_required(key: &str) -> Result<String> {
    env_optional(key).with_context(|| format!("{key} is required"))
}

fn env_parse<T>(key: &str, default: T) -> Result<T>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    match env_optional(key) {
        Some(value) => value
            .parse::<T>()
            .map_err(|error| anyhow::anyhow!("{key}: {error}")),
        None => Ok(default),
    }
}

fn parse_sources(value: &str) -> Result<HashSet<SourceKind>> {
    if value.trim().eq_ignore_ascii_case("all") {
        return Ok(SourceKind::ALL.iter().copied().collect());
    }
    let mut result = HashSet::new();
    for part in value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        let source = SourceKind::from_str(part)
            .map_err(|_| anyhow::anyhow!("unknown source in ENABLED_SOURCES: {part}"))?;
        result.insert(source);
    }
    if result.is_empty() {
        bail!("at least one source must be enabled");
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_source_lists() {
        let parsed = parse_sources("youtube, tiktok, x").expect("sources");
        assert!(parsed.contains(&SourceKind::YouTube));
        assert!(parsed.contains(&SourceKind::TikTok));
        assert!(parsed.contains(&SourceKind::X));
        assert!(!parsed.contains(&SourceKind::Reddit));
    }

    #[test]
    fn rejects_unknown_sources() {
        assert!(parse_sources("youtube, myspace").is_err());
        assert!(parse_sources("").is_err());
    }
}
