//! Minimal Bot API mock on `tokio::net::TcpListener`.
//!
//! The real teloxide client is pointed at it via `set_api_url`; no Telegram
//! token or network is needed. This is the foundation for the full e2e suite
//! described in ТЗ §12 (private/inline flows, `MESSAGE_ID_INVALID` fallback,
//! stale `file_id` self-healing and the load harness).

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const GET_ME: &str = r#"{"ok":true,"result":{"id":123,"is_bot":true,"first_name":"Test","username":"test_bot","can_join_groups":true,"can_read_all_group_messages":false,"supports_inline_queries":true,"can_connect_to_business":false,"has_main_web_app":false}}"#;

async fn mock_api() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("addr");
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let mut buffer = vec![0u8; 16 * 1024];
            let Ok(read) = socket.read(&mut buffer).await else {
                continue;
            };
            let request = String::from_utf8_lossy(&buffer[..read]);
            let method = request
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .split('?')
                .next()
                .unwrap_or("")
                .rsplit('/')
                .next()
                .unwrap_or("")
                .to_owned();
            let body = if method.eq_ignore_ascii_case("getme") {
                GET_ME.to_owned()
            } else if method.eq_ignore_ascii_case("sendmessage") {
                r#"{"ok":true,"result":{"message_id":7,"date":0,"chat":{"id":123,"type":"private","first_name":"T"},"text":"hi"}}"#.to_owned()
            } else {
                r#"{"ok":true,"result":true}"#.to_owned()
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(response.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    (format!("http://{addr}"), handle)
}

#[tokio::test]
async fn teloxide_talks_to_mock_bot_api() {
    let (url, server) = mock_api().await;
    let parsed = reqwest::Url::parse(&url).expect("valid url");
    let bot = teloxide::Bot::new("123:fake-token").set_api_url(parsed);

    let me = bot.get_me().await.expect("getMe");
    assert_eq!(me.user.username.as_deref(), Some("test_bot"));

    use teloxide::prelude::*;
    let sent = bot
        .send_message(teloxide::types::ChatId(123), "hi")
        .await
        .expect("sendMessage");
    assert_eq!(sent.id.0, 7);

    server.abort();
}
