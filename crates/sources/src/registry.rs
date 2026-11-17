//! Source abstraction: offline URL recognition, bounded short-link
//! resolution, and sub-second oEmbed metadata for inline results.

use async_trait::async_trait;
use common::config::Config;
use common::domain::{Lang, ParsedUrl, SourceKind, VideoKey, VideoMeta};
use std::collections::HashSet;
use std::time::Duration;
use thiserror::Error;
use tracing::warn;
use url::Url;

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("unsupported link")]
    Unsupported,
    #[error("source {0} is disabled")]
    Disabled(SourceKind),
    #[error("short link could not be resolved safely")]
    ResolveFailed,
    #[error("network error: {0}")]
    Network(String),
    #[error("video not found")]
    NotFound,
}

impl From<SourceError> for common::error::DownloadError {
    fn from(error: SourceError) -> Self {
        match error {
            SourceError::Unsupported => common::error::DownloadError::Unsupported("link".into()),
            SourceError::Disabled(source) => common::error::DownloadError::SourceDisabled(source),
            SourceError::NotFound => common::error::DownloadError::NotFound,
            SourceError::ResolveFailed | SourceError::Network(_) => {
                common::error::DownloadError::SourceUnavailable(error.to_string())
            }
        }
    }
}

#[async_trait]
pub trait Source: Send + Sync {
    fn kind(&self) -> SourceKind;
    /// No network access. `None` means "not our link".
    fn parse_url(&self, url: &Url) -> Option<ParsedUrl>;
    /// Fast metadata for inline. Target: under 0.5 s.
    async fn quick_meta(
        &self,
        parsed: &ParsedUrl,
        client: &reqwest::Client,
        timeout: Duration,
        lang: Lang,
    ) -> Result<VideoMeta, SourceError>;
}

/// A source whose metadata comes from a public oEmbed endpoint.
pub struct BuiltinSource {
    kind: SourceKind,
    oembed: Option<&'static str>,
    parser: fn(&Url) -> Option<ParsedUrl>,
}

#[async_trait]
impl Source for BuiltinSource {
    fn kind(&self) -> SourceKind {
        self.kind
    }

    fn parse_url(&self, url: &Url) -> Option<ParsedUrl> {
        (self.parser)(url)
    }

    async fn quick_meta(
        &self,
        parsed: &ParsedUrl,
        client: &reqwest::Client,
        timeout: Duration,
        _lang: Lang,
    ) -> Result<VideoMeta, SourceError> {
        let mut meta = VideoMeta::fallback(&parsed.key, &parsed.page_url);
        let Some(template) = self.oembed else {
            return Ok(meta);
        };
        let encoded: String = url::form_urlencoded::byte_serialize(parsed.page_url.as_str().as_bytes())
            .collect();
        let endpoint = template.replace("{url}", &encoded);

        let response = tokio::time::timeout(timeout, client.get(&endpoint).send())
            .await
            .map_err(|_| SourceError::Network("oEmbed timed out".into()))?
            .map_err(|error| SourceError::Network(error.to_string()))?;
        if !response.status().is_success() {
            return Ok(meta);
        }
        let json: serde_json::Value = tokio::time::timeout(timeout, response.json())
            .await
            .map_err(|_| SourceError::Network("oEmbed body timed out".into()))?
            .map_err(|error| SourceError::Network(error.to_string()))?;

        if let Some(title) = json.get("title").and_then(|value| value.as_str())
            && !title.trim().is_empty()
        {
            meta.title = title.trim().to_owned();
        }
        if let Some(author) = json.get("author_name").and_then(|value| value.as_str())
            && !author.trim().is_empty()
        {
            meta.uploader = Some(author.trim().to_owned());
        }
        if let Some(thumb) = json.get("thumbnail_url").and_then(|value| value.as_str())
            && !thumb.trim().is_empty()
        {
            meta.thumbnail_url = Some(thumb.to_owned());
        }
        Ok(meta)
    }
}

pub struct SourceRegistry {
    sources: Vec<BuiltinSource>,
    enabled: HashSet<SourceKind>,
    client: reqwest::Client,
    quick_timeout: Duration,
    resolve_timeout: Duration,
}

impl SourceRegistry {
    pub fn new(config: &Config) -> Result<Self, SourceError> {
        let client = reqwest::Client::builder()
            .timeout(config.quick_meta_timeout.max(Duration::from_millis(300)))
            .user_agent(crate::http::USER_AGENT)
            .build()
            .map_err(|error| SourceError::Network(error.to_string()))?;
        let sources = vec![
            BuiltinSource {
                kind: SourceKind::YouTube,
                oembed: Some("https://www.youtube.com/oembed?format=json&url={url}"),
                parser: parse_youtube,
            },
            BuiltinSource {
                kind: SourceKind::TikTok,
                oembed: Some("https://www.tiktok.com/oembed?url={url}"),
                parser: parse_tiktok,
            },
            BuiltinSource {
                kind: SourceKind::Instagram,
                oembed: None,
                parser: parse_instagram,
            },
            BuiltinSource {
                kind: SourceKind::X,
                oembed: None,
                parser: parse_x,
            },
            BuiltinSource {
                kind: SourceKind::Reddit,
                oembed: Some("https://www.reddit.com/oembed?url={url}"),
                parser: parse_reddit,
            },
            BuiltinSource {
                kind: SourceKind::Vimeo,
                oembed: Some("https://vimeo.com/api/oembed.json?url={url}"),
                parser: parse_vimeo,
            },
        ];
        Ok(Self {
            sources,
            enabled: config.enabled_sources.clone(),
            client,
            quick_timeout: config.quick_meta_timeout,
            resolve_timeout: Duration::from_secs(5),
        })
    }

    /// Recognize a URL and check the allow-list, all without network access.
    pub fn parse(&self, url: &Url) -> Result<ParsedUrl, SourceError> {
        for source in &self.sources {
            if let Some(parsed) = source.parse_url(url) {
                if !self.enabled.contains(&source.kind()) {
                    return Err(SourceError::Disabled(source.kind()));
                }
                return Ok(parsed);
            }
        }
        Err(SourceError::Unsupported)
    }

    /// Parse without checking the allow-list (used during short-link resolve).
    fn parse_raw(&self, url: &Url) -> Option<ParsedUrl> {
        self.sources.iter().find_map(|source| source.parse_url(url))
    }

    /// Resolve a short link with at most three redirects, https only, and only
    /// onto domains we recognize. Returns the final URL for re-parsing.
    pub async fn resolve(&self, url: Url) -> Result<Url, SourceError> {
        let mut current = url;
        for _ in 0..3 {
            let needs_resolve = self.parse_raw(&current).is_some_and(|parsed| parsed.needs_resolve);
            if !needs_resolve {
                return Ok(current);
            }
            // Only known short-link hosts may be followed.
            let host = host_without_www(&current);
            if !(host.ends_with("tiktok.com") && (host.starts_with("vm.") || host.starts_with("vt."))) {
                return Err(SourceError::ResolveFailed);
            }
            let client = reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::limited(3))
                .timeout(self.resolve_timeout)
                .user_agent(crate::http::USER_AGENT)
                .build()
                .map_err(|error| SourceError::Network(error.to_string()))?;
            let response = tokio::time::timeout(self.resolve_timeout, client.get(current.clone()).send())
                .await
                .map_err(|_| SourceError::Network("redirect timed out".into()))?
                .map_err(|error| SourceError::Network(error.to_string()))?;
            current = response.url().clone();
            if current.scheme() != "https" {
                return Err(SourceError::ResolveFailed);
            }
            let final_host = host_without_www(&current);
            let recognized = self.sources.iter().any(|source| match source.kind() {
                SourceKind::YouTube => final_host.ends_with("youtube.com") || final_host == "youtu.be",
                SourceKind::TikTok => final_host.ends_with("tiktok.com"),
                SourceKind::Instagram => final_host.ends_with("instagram.com"),
                SourceKind::X => final_host == "x.com" || final_host.ends_with("twitter.com"),
                SourceKind::Reddit => final_host.ends_with("reddit.com") || final_host == "redd.it",
                SourceKind::Vimeo => final_host.ends_with("vimeo.com"),
            });
            if !recognized {
                warn!(url = %current, "short link redirected to an unrecognized host");
                return Err(SourceError::ResolveFailed);
            }
        }
        Err(SourceError::ResolveFailed)
    }

    pub async fn quick_meta(
        &self,
        parsed: &ParsedUrl,
        lang: Lang,
    ) -> Result<VideoMeta, SourceError> {
        let source = self
            .sources
            .iter()
            .find(|source| source.kind() == parsed.key.source)
            .ok_or(SourceError::Unsupported)?;
        source
            .quick_meta(parsed, &self.client, self.quick_timeout, lang)
            .await
    }
}

// ----- parsing helpers ------------------------------------------------------

fn host(url: &Url) -> &str {
    url.host_str().unwrap_or_default()
}

fn host_without_www(url: &Url) -> &str {
    let value = host(url);
    value.strip_prefix("www.").unwrap_or(value)
}

fn segments(url: &Url) -> Vec<&str> {
    url.path_segments()
        .map(|segments| segments.filter(|part| !part.is_empty()).collect())
        .unwrap_or_default()
}

fn valid_id(id: &str) -> bool {
    let id = id.trim();
    id.len() >= 3
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '+' | '.'))
}

fn make(kind: SourceKind, id: &str, url: &Url, needs_resolve: bool) -> Option<ParsedUrl> {
    if !valid_id(id) {
        return None;
    }
    Some(ParsedUrl {
        key: VideoKey::new(kind, id),
        page_url: url.clone(),
        is_playlist: false,
        needs_resolve,
    })
}

fn parse_youtube(url: &Url) -> Option<ParsedUrl> {
    let host = host_without_www(url);
    let parts = segments(url);
    if host == "youtu.be" {
        return make(SourceKind::YouTube, parts.first().copied()?, url, false);
    }
    if !host.ends_with("youtube.com") && !host.ends_with("youtube-nocookie.com") {
        return None;
    }
    match parts.first().copied()? {
        "watch" => {
            let id = url
                .query_pairs()
                .find(|(key, _)| key == "v")
                .map(|(_, value)| value.into_owned())?;
            make(SourceKind::YouTube, &id, url, false)
        }
        "shorts" | "embed" | "live" => make(SourceKind::YouTube, parts.get(1).copied()?, url, false),
        "playlist" => {
            let list = url
                .query_pairs()
                .find(|(key, _)| key == "list")
                .map(|(_, value)| value.into_owned())?;
            if !valid_id(&list) {
                return None;
            }
            Some(ParsedUrl {
                key: VideoKey::new(SourceKind::YouTube, list),
                page_url: url.clone(),
                is_playlist: true,
                needs_resolve: false,
            })
        }
        _ => None,
    }
}

fn parse_tiktok(url: &Url) -> Option<ParsedUrl> {
    let host = host_without_www(url);
    if !host.ends_with("tiktok.com") {
        return None;
    }
    let parts = segments(url);
    if (host.starts_with("vm.") || host.starts_with("vt.")) && !host.starts_with("www.") {
        let id = parts.first().copied()?;
        return Some(ParsedUrl {
            key: VideoKey::new(SourceKind::TikTok, id),
            page_url: url.clone(),
            is_playlist: false,
            needs_resolve: true,
        });
    }
    let index = parts.iter().position(|part| *part == "video" || *part == "reel")?;
    make(SourceKind::TikTok, parts.get(index + 1).copied()?, url, false)
}

fn parse_instagram(url: &Url) -> Option<ParsedUrl> {
    let host = host_without_www(url);
    if !host.ends_with("instagram.com") {
        return None;
    }
    let parts = segments(url);
    let index = parts
        .iter()
        .position(|part| matches!(*part, "reel" | "reels" | "p" | "tv"))?;
    make(SourceKind::Instagram, parts.get(index + 1).copied()?, url, false)
}

fn parse_x(url: &Url) -> Option<ParsedUrl> {
    let host = host_without_www(url);
    if host != "x.com" && !host.ends_with("twitter.com") {
        return None;
    }
    let parts = segments(url);
    let index = parts.iter().position(|part| *part == "status")?;
    make(SourceKind::X, parts.get(index + 1).copied()?, url, false)
}

fn parse_reddit(url: &Url) -> Option<ParsedUrl> {
    let host = host_without_www(url);
    let parts = segments(url);
    let id = if host == "redd.it" {
        parts.first().copied()?
    } else if host.ends_with("reddit.com") {
        let index = parts.iter().position(|part| *part == "comments")?;
        parts.get(index + 1).copied()?
    } else {
        return None;
    };
    make(SourceKind::Reddit, id, url, false)
}

fn parse_vimeo(url: &Url) -> Option<ParsedUrl> {
    let host = host_without_www(url);
    if !host.ends_with("vimeo.com") {
        return None;
    }
    let parts = segments(url);
    let id = if host == "player.vimeo.com" {
        let index = parts.iter().position(|part| *part == "video")?;
        parts.get(index + 1).copied()?
    } else {
        parts.first().copied()?
    };
    if !id.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    make(SourceKind::Vimeo, id, url, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(url: &str) -> Option<ParsedUrl> {
        let parsed = Url::parse(url).expect("valid url");
        parse_youtube(&parsed)
            .or_else(|| parse_tiktok(&parsed))
            .or_else(|| parse_instagram(&parsed))
            .or_else(|| parse_x(&parsed))
            .or_else(|| parse_reddit(&parsed))
            .or_else(|| parse_vimeo(&parsed))
    }

    #[test]
    fn youtube_variants() {
        assert_eq!(
            parse("https://youtu.be/abc123XYZ_-")?.key.id,
            "abc123XYZ_-"
        );
        assert_eq!(
            parse("https://www.youtube.com/watch?v=abcd1234&t=30")?.key.id,
            "abcd1234"
        );
        assert_eq!(
            parse("https://youtube.com/shorts/short_id_1?feature=share")?.key.id,
            "short_id_1"
        );
        assert_eq!(
            parse("https://m.youtube.com/embed/embed_id_1")?.key.id,
            "embed_id_1"
        );
        let playlist = parse("https://www.youtube.com/playlist?list=PL123456")?;
        assert!(playlist.is_playlist);
        Ok::<(), ()>(())
    }

    #[test]
    fn other_sources() {
        assert_eq!(
            parse("https://www.tiktok.com/@user/video/7345678901234567890")?.key.source,
            SourceKind::TikTok
        );
        assert_eq!(
            parse("https://www.instagram.com/reel/Cxyz123ABC/")?.key.source,
            SourceKind::Instagram
        );
        assert_eq!(
            parse("https://x.com/user/status/1678901234567890123")?.key.source,
            SourceKind::X
        );
        assert_eq!(
            parse("https://www.reddit.com/r/funny/comments/abc123/some_title/")?.key.id,
            "abc123"
        );
        assert_eq!(
            parse("https://vimeo.com/123456789")?.key.id,
            "123456789"
        );
        Ok::<(), ()>(())
    }

    #[test]
    fn short_links_need_resolution() {
        let parsed = parse("https://vm.tiktok.com/ZMabc123/")?;
        assert!(parsed.needs_resolve);
        assert_eq!(parsed.key.source, SourceKind::TikTok);
        Ok::<(), ()>(())
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse("https://example.com/watch?v=abcd1234").is_none());
        assert!(parse("https://www.youtube.com/").is_none());
    }
}
