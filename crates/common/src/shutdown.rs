//! SIGTERM/SIGINT aware shutdown signal.

use tokio::signal;
use tokio_util::sync::CancellationToken;

pub async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        let mut sigterm = signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("can register SIGTERM handler");
        tokio::select! {
            _ = sigterm.recv() => {}
            _ = signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = signal::ctrl_c().await;
    }
}

/// Spawn a task that cancels the token on shutdown and return it.
pub fn cancellation_token() -> CancellationToken {
    let token = CancellationToken::new();
    let child = token.child_token();
    tokio::spawn(async move {
        wait_for_shutdown().await;
        child.cancel();
    });
    token
}
