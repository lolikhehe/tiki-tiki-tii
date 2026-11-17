//! Core value types shared by all crates. Cache identity is always derived
//! offline from the input URL, so a cache lookup never touches the network.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use tempfile::TempDir;
use url::Url;

/// User interface language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    #[default]
    En,
    Uk,
}

impl Lang {
    pub const fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Uk => "uk",
        }
    }

    pub fn from_code(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "en" | "english" => Some(Self::En),
            "uk" | "ua" | "ukrainian" => Some(Self::Uk),
            _ => None,
        }
    }

    /// Map Telegram's `language_code` to a supported UI language.
    pub fn from_telegram_hint(code: Option<&str>, fallback: Self) -> Self {
        match code {
            Some(value) => {
                let value = value.trim().to_ascii_lowercase();
                if value.starts_with("uk") || value.starts_with("ru") || value.starts_with("be") {
                    Self::Uk
                } else if value.starts_with("en") {
                    Self::En
                } else {
                    fallback
                }
            }
            None => fallback,
        }
    }
}

/// A supported video source. The allow-list is configured at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    YouTube,
    TikTok,
    Instagram,
    X,
    Reddit,
    Vimeo,
}

impl SourceKind {
    pub const ALL: [Self; 6] = [
        Self::YouTube,
        Self::TikTok,
        Self::Instagram,
        Self::X,
        Self::Reddit,
        Self::Vimeo,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::YouTube => "youtube",
            Self::TikTok => "tiktok",
            Self::Instagram => "instagram",
            Self::X => "x",
            Self::Reddit => "reddit",
            Self::Vimeo => "vimeo",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::YouTube => "YouTube",
            Self::TikTok => "TikTok",
            Self::Instagram => "Instagram Reels",
            Self::X => "X",
            Self::Reddit => "Reddit",
            Self::Vimeo => "Vimeo",
        }
    }
}

impl fmt::Display for SourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SourceKind {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "youtube" | "yt" => Ok(Self::YouTube),
            "tiktok" | "tt" => Ok(Self::TikTok),
            "instagram" | "ig" | "reels" => Ok(Self::Instagram),
            "x" | "twitter" | "tw" => Ok(Self::X),
            "reddit" | "rd" => Ok(Self::Reddit),
            "vimeo" | "vm" => Ok(Self::Vimeo),
            _ => Err(()),
        }
    }
}

/// Stable, offline-derived cache identity: (source, video id).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VideoKey {
    pub source: SourceKind,
    pub id: String,
}

impl VideoKey {
    pub fn new(source: SourceKind, id: impl Into<String>) -> Self {
        Self {
            source,
            id: id.into(),
        }
    }

    pub fn as_str(&self) -> String {
        format!("{}:{}", self.source.as_str(), self.id)
    }
}

impl fmt::Display for VideoKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.source.as_str(), self.id)
    }
}

/// Quality step. Determines the yt-dlp height selector and the cache key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QualityTier {
    #[serde(rename = "480")]
    P480,
    #[serde(rename = "720")]
    P720,
    #[serde(rename = "1080")]
    P1080,
}

impl QualityTier {
    pub const ALL: [Self; 3] = [Self::P480, Self::P720, Self::P1080];

    pub const fn height(self) -> u32 {
        match self {
            Self::P480 => 480,
            Self::P720 => 720,
            Self::P1080 => 1080,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::P480 => "h264_480",
            Self::P720 => "h264_720",
            Self::P1080 => "h264_1080",
        }
    }

    /// Nearest supported tier that does not exceed `max_height`.
    pub fn from_max_height(max_height: u32) -> Self {
        if max_height >= 1080 {
            Self::P1080
        } else if max_height >= 720 {
            Self::P720
        } else {
            Self::P480
        }
    }

    /// One step lower, for the "file too large, retry cheaper" path.
    pub const fn lower(self) -> Option<Self> {
        match self {
            Self::P480 => None,
            Self::P720 => Some(Self::P480),
            Self::P1080 => Some(Self::P720),
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "480" | "h264_480" | "p480" => Some(Self::P480),
            "720" | "h264_720" | "p720" => Some(Self::P720),
            "1080" | "h264_1080" | "p1080" => Some(Self::P1080),
            _ => None,
        }
    }
}

impl fmt::Display for QualityTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}p", self.height())
    }
}

impl FromStr for QualityTier {
    type Err = ();

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::from_str(value).ok_or(())
    }
}

/// Small, network-light metadata used to render inline results quickly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoMeta {
    pub key: VideoKey,
    pub title: String,
    pub uploader: Option<String>,
    /// May be unknown in oEmbed responses; filled by yt-dlp on download.
    pub duration: Option<std::time::Duration>,
    pub thumbnail_url: Option<String>,
    pub page_url: String,
}

impl VideoMeta {
    pub fn fallback(key: &VideoKey, page_url: &Url) -> Self {
        Self {
            key: key.clone(),
            title: key.id.clone(),
            uploader: Some(key.source.display_name().to_owned()),
            duration: None,
            thumbnail_url: None,
            page_url: page_url.to_string(),
        }
    }

    pub fn duration_secs(&self) -> Option<u32> {
        self.duration
            .and_then(|value| u32::try_from(value.as_secs()).ok())
    }
}

/// Result of offline URL recognition.
#[derive(Debug, Clone)]
pub struct ParsedUrl {
    pub key: VideoKey,
    pub page_url: Url,
    /// Playlists/channels/carousels are rejected in v1.
    pub is_playlist: bool,
    /// Short links need one bounded redirect-resolution step.
    pub needs_resolve: bool,
}

/// A fully prepared file, ready for `sendVideo`. The temporary directory is
/// kept alive by RAII until the file has been consumed.
#[derive(Debug, Clone)]
pub struct PreparedVideo {
    pub path: PathBuf,
    pub thumbnail: Option<PathBuf>,
    pub meta: VideoMeta,
    pub quality: QualityTier,
    pub width: u32,
    pub height: u32,
    pub duration_sec: u32,
    pub size_bytes: u64,
    pub transcoded: bool,
    _dir: Arc<TempDir>,
}

impl PreparedVideo {
    pub fn new(
        path: PathBuf,
        thumbnail: Option<PathBuf>,
        meta: VideoMeta,
        quality: QualityTier,
        width: u32,
        height: u32,
        duration_sec: u32,
        size_bytes: u64,
        transcoded: bool,
        dir: TempDir,
    ) -> Self {
        Self {
            path,
            thumbnail,
            meta,
            quality,
            width,
            height,
            duration_sec,
            size_bytes,
            transcoded,
            _dir: Arc::new(dir),
        }
    }
}

/// One cached (video, quality) entry. Only metadata and file ids are stored.
#[derive(Debug, Clone)]
pub struct CachedVideo {
    pub key: VideoKey,
    pub quality: QualityTier,
    pub file_id: String,
    pub file_unique_id: String,
    pub meta: VideoMeta,
    pub width: u32,
    pub height: u32,
    pub duration_sec: u32,
    pub size_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct UserRecord {
    pub lang: Lang,
    pub max_height: u32,
    pub banned: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub videos: i64,
    pub users: i64,
    pub sends: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_key_roundtrip() {
        let key = VideoKey::new(SourceKind::YouTube, "abc123");
        assert_eq!(key.as_str(), "youtube:abc123");
        assert_eq!(
            "youtube:abc123".parse::<SourceKind>(),
            Ok(SourceKind::YouTube)
        );
    }

    #[test]
    fn quality_from_max_height() {
        assert_eq!(QualityTier::from_max_height(480), QualityTier::P480);
        assert_eq!(QualityTier::from_max_height(900), QualityTier::P720);
        assert_eq!(QualityTier::from_max_height(2160), QualityTier::P1080);
        assert_eq!(QualityTier::P1080.lower(), Some(QualityTier::P720));
        assert_eq!(QualityTier::P480.lower(), None);
    }

    #[test]
    fn source_kind_parsing() {
        for source in SourceKind::ALL {
            assert_eq!(source.as_str().parse::<SourceKind>(), Ok(source));
        }
        assert_eq!("twitter".parse::<SourceKind>(), Ok(SourceKind::X));
    }
}
