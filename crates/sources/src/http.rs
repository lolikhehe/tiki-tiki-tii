//! Shared HTTP client tuning and bounded byte downloads.

use std::time::Duration;

pub const USER_AGENT: &str = "tiki-tiki-tii/0.1 (Telegram video bot)";

pub fn client(timeout: Duration) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(Duration::from_secs(10))
        .pool_idle_timeout(Duration::from_secs(90))
        .tcp_keepalive(Duration::from_secs(60))
        .user_agent(USER_AGENT)
        .build()
}

/// Download a response body with a hard byte ceiling, streaming in chunks so
/// a misbehaving server cannot make us allocate past the limit.
pub async fn fetch_bytes(client: &reqwest::Client, url: &str, max_bytes: u64) -> Option<Vec<u8>> {
    let mut response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes)
    {
        return None;
    }
    let mut bytes = Vec::with_capacity(
        response
            .content_length()
            .unwrap_or(64 * 1024)
            .min(256 * 1024) as usize,
    );
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len() as u64 + chunk.len() as u64 > max_bytes {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    (!bytes.is_empty()).then_some(bytes)
}
