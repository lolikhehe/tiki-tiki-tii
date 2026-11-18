//! Runtime SQL verification against a real Postgres instance.
//!
//! Set `TEST_DATABASE_URL` to run this suite; otherwise it is skipped so the
//! normal `cargo test` works without external services. This covers every
//! query in `crates/storage/src/db.rs` — migrations, cache upserts, job
//! claiming, waiter de-duplication and selections.

use common::domain::{CachedVideo, Lang, QualityTier, SourceKind, VideoKey, VideoMeta};
use std::time::Duration;
use storage::Db;

async fn db() -> Option<Db> {
    let Ok(url) = std::env::var("TEST_DATABASE_URL") else {
        eprintln!("TEST_DATABASE_URL is not set — skipping Postgres integration test");
        return None;
    };
    Some(
        Db::connect(&url)
            .await
            .expect("connect + migrations must succeed"),
    )
}

#[tokio::test]
async fn full_storage_roundtrip() {
    let Some(db) = db().await else { return };
    assert!(db.ping().await, "healthcheck must pass");

    // Make the suite repeatable against the same database.
    sqlx::query("DELETE FROM users WHERE tg_id = 4242")
        .execute(db.pool())
        .await
        .expect("cleanup users");
    sqlx::query("DELETE FROM videos WHERE source_id = 'integration-test-id'")
        .execute(db.pool())
        .await
        .expect("cleanup videos");

    // Users
    db.ensure_user(4242, Lang::Uk, 720)
        .await
        .expect("ensure user");
    let user = db
        .get_user(4242)
        .await
        .expect("get user")
        .expect("user exists");
    assert_eq!(user.lang, Lang::Uk);
    assert_eq!(user.max_height, 720);
    db.set_user_quality(4242, 480).await.expect("set quality");
    db.set_user_lang(4242, Lang::En).await.expect("set lang");
    let user = db.get_user(4242).await.expect("get user").expect("user");
    assert_eq!((user.max_height, user.lang), (480, Lang::En));

    // Selections with TTL
    let id = db
        .create_selection(serde_json::json!({"kind": "test"}), Duration::from_secs(60))
        .await
        .expect("create selection");
    assert_eq!(
        db.get_selection(&id).await.expect("get selection"),
        Some(serde_json::json!({"kind": "test"}))
    );

    // Cache: write, read, touch, purge
    let key = VideoKey::new(SourceKind::YouTube, "integration-test-id");
    let meta = VideoMeta {
        key: key.clone(),
        title: "Test video".to_owned(),
        uploader: Some("Tester".to_owned()),
        duration: Some(Duration::from_secs(42)),
        thumbnail_url: None,
        page_url: "https://youtu.be/integration-test-id".to_owned(),
    };
    let cached = CachedVideo {
        key: key.clone(),
        quality: QualityTier::P720,
        file_id: "file-id-1".to_owned(),
        file_unique_id: "unique-1".to_owned(),
        meta: meta.clone(),
        width: 1280,
        height: 720,
        duration_sec: 42,
        size_bytes: 1_000_000,
    };
    db.put_video(&cached).await.expect("put video");
    let loaded = db
        .get_video(&key, QualityTier::P720)
        .await
        .expect("get video")
        .expect("cached video exists");
    assert_eq!(loaded.file_id, "file-id-1");
    assert_eq!((loaded.width, loaded.height), (1280, 720));
    db.touch_video(&key, QualityTier::P720).await;
    db.purge_video(&key).await.expect("purge video");
    assert!(
        db.get_video(&key, QualityTier::P720)
            .await
            .expect("get video")
            .is_none()
    );

    // Job queue + waiters: dedup and exactly-once delivery
    let url = "https://youtu.be/integration-test-id";
    let (job_id, created) = db
        .enqueue_job(&key, QualityTier::P720, url)
        .await
        .expect("enqueue job");
    assert!(created, "first enqueue must create a job");
    let (second_id, created_again) = db
        .enqueue_job(&key, QualityTier::P720, url)
        .await
        .expect("enqueue duplicate");
    assert!(!created_again, "duplicate enqueue must be a no-op");
    assert_eq!(second_id, job_id);

    db.add_waiter(job_id, 4242, Some(4242), Some(10), None, Lang::Uk)
        .await
        .expect("add private waiter");
    db.add_waiter(job_id, 4242, None, None, Some("inline-id"), Lang::En)
        .await
        .expect("add inline waiter");

    let claimed = db
        .claim_job()
        .await
        .expect("claim job")
        .expect("a queued job exists");
    assert_eq!(claimed.id, job_id);
    assert_eq!(claimed.key, key);
    assert!(db.claim_job().await.expect("claim again").is_none());

    let waiters = db.take_waiters(job_id).await.expect("take waiters");
    assert_eq!(waiters.len(), 2, "two waiters must be returned once");
    assert!(
        db.take_waiters(job_id)
            .await
            .expect("take waiters again")
            .is_empty()
    );

    db.finish_job(job_id).await.expect("finish job");
    assert!(db.queue_length().await.expect("queue length") >= 0);
    let stats = db.stats().await.expect("stats");
    assert_eq!(stats.users, 1);
}
