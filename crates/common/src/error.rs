//! Typed errors. Download errors are cloneable because one cold request is
//! broadcast to every waiter subscribed to the same (video, quality) pair.

use crate::domain::SourceKind;
use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum DownloadError {
    #[error("video is private or requires an account")]
    Private,
    #[error("video is region locked")]
    RegionLocked,
    #[error("video has an age restriction")]
    AgeRestricted,
    #[error("video is too long: {actual}s (limit {limit}s)")]
    TooLong { limit: u64, actual: u64 },
    #[error("file is too large: {actual} bytes (limit {limit} bytes)")]
    TooLarge { limit: u64, actual: u64 },
    #[error("video was not found or was removed")]
    NotFound,
    #[error("playlists and channels are not supported")]
    Playlist,
    #[error("unsupported input: {0}")]
    Unsupported(String),
    #[error("source {0} is disabled")]
    SourceDisabled(SourceKind),
    #[error("source is temporarily unavailable: {0}")]
    SourceUnavailable(String),
    #[error("the operation timed out")]
    Timeout,
    #[error("media processing failed: {0}")]
    Media(String),
    #[error("download pipeline failed: {0}")]
    Internal(String),
}

impl From<std::io::Error> for DownloadError {
    fn from(error: std::io::Error) -> Self {
        DownloadError::Internal(format!("io error: {error}"))
    }
}

impl DownloadError {
    /// Whether this failure is worth one immediate retry.
    pub fn is_transient(&self) -> bool {
        let message = self.to_string().to_ascii_lowercase();
        matches!(self, Self::Timeout | Self::SourceUnavailable(_))
            || message.contains("timed out")
            || message.contains("429")
            || message.contains("403")
            || message.contains("connection reset")
    }
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("download error: {0}")]
    Download(#[from] DownloadError),
    #[error("configuration error: {0}")]
    Config(String),
    #[error("database error: {0}")]
    Database(String),
    #[error("{0}")]
    Message(String),
    #[error(transparent)]
    Other(#[from] Box<dyn std::error::Error + Send + Sync>),
}

pub type AppResult<T> = Result<T, AppError>;
pub type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_errors_are_cloneable() {
        let error = DownloadError::TooLong {
            limit: 900,
            actual: 1000,
        };
        assert_eq!(error.to_string(), error.clone().to_string());
    }

    #[test]
    fn transient_detection() {
        assert!(DownloadError::Timeout.is_transient());
        assert!(!DownloadError::Private.is_transient());
    }
}
