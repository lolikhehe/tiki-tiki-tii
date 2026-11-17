//! Remove orphaned download directories left behind by a crashed process.

use std::path::PathBuf;
use std::time::Duration;

pub fn spawn(dir: PathBuf, max_age: Duration) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(300));
        loop {
            interval.tick().await;
            let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
                continue;
            };
            while let Ok(Some(entry)) = entries.next_entry().await {
                let Ok(metadata) = entry.metadata().await else {
                    continue;
                };
                if !metadata.is_dir() {
                    continue;
                }
                let Ok(modified) = metadata.modified() else {
                    continue;
                };
                let Ok(age) = modified.elapsed() else {
                    continue;
                };
                if age > max_age && tokio::fs::remove_dir_all(entry.path()).await.is_ok() {
                    tracing::info!(path = %entry.path().display(), "removed orphan download dir");
                }
            }
        }
    });
}
