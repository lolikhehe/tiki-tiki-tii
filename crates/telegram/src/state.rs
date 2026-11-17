//! Shared application state, injected into every handler.

use crate::rate_limit::RateLimiter;
use common::config::Config;
use downloader::DownloadManager;
use std::sync::Arc;
use storage::Db;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: Db,
    pub manager: Arc<DownloadManager>,
    pub bot_username: String,
    pub rate_limiter: Arc<RateLimiter>,
}

impl AppState {
    pub fn new(
        config: Arc<Config>,
        db: Db,
        manager: Arc<DownloadManager>,
        bot_username: String,
    ) -> Self {
        let rate_limiter = Arc::new(RateLimiter::new(config.rate_limit_per_min));
        Self {
            config,
            db,
            manager,
            bot_username,
            rate_limiter,
        }
    }
}
