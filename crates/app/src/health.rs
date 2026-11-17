//! Minimal HTTP listener for `/healthz` and `/metrics` without extra web deps.

use metrics_exporter_prometheus::PrometheusHandle;
use std::net::SocketAddr;
use storage::Db;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub async fn serve(addr: SocketAddr, db: Db, metrics: PrometheusHandle) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((socket, _)) => {
                    let db = db.clone();
                    let metrics = metrics.clone();
                    tokio::spawn(async move {
                        if let Err(error) = handle(socket, db, metrics).await {
                            tracing::warn!(%error, "health connection failed");
                        }
                    });
                }
                Err(error) => tracing::warn!(%error, "health accept failed"),
            }
        }
    });
    Ok(())
}

async fn handle(
    mut socket: tokio::net::TcpStream,
    db: Db,
    metrics: PrometheusHandle,
) -> anyhow::Result<()> {
    let mut buffer = [0u8; 4096];
    let read = socket.read(&mut buffer).await?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let path = request
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/");

    let (status, content_type, body) = match path {
        "/healthz" => {
            if db.ping().await {
                ("200 OK", "text/plain; charset=utf-8", "ok".to_owned())
            } else {
                (
                    "503 Service Unavailable",
                    "text/plain; charset=utf-8",
                    "database unavailable".to_owned(),
                )
            }
        }
        "/metrics" => (
            "200 OK",
            "text/plain; version=0.0.4; charset=utf-8",
            metrics.render(),
        ),
        _ => (
            "404 Not Found",
            "text/plain; charset=utf-8",
            "not found".to_owned(),
        ),
    };

    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    socket.write_all(response.as_bytes()).await?;
    socket.shutdown().await?;
    Ok(())
}
