//! Postgres persistence. Only metadata and Telegram file ids are stored;
//! video bytes never leave the temporary download directory.
//!
//! Queries use the runtime `query_as` API rather than the compile-time `query!`
//! macro, so the workspace builds without a live database during CI. See
//! `docs/decisions.md` for the rationale (deviation from ТЗ §5.1).

use chrono::{Duration as ChronoDuration, Utc};
use common::domain::{CachedVideo, Lang, QualityTier, SourceKind, Stats, UserRecord, VideoKey};
use common::error::{AppError, AppResult};
use moka::future::Cache;
use serde_json::Value;
use sqlx::PgPool;
use std::time::Duration;
use tracing::warn;
use uuid::Uuid;

type VideoRow = (String, String, String, i32, i32, i32, i64, String);
type WaiterRow = (i64, i64, Option<i64>, Option<i32>, Option<String>, String);

#[derive(Debug, Clone)]
pub struct Job {
    pub id: Uuid,
    pub key: VideoKey,
    pub quality: QualityTier,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct Waiter {
    pub id: i64,
    pub user_tg_id: i64,
    pub chat_id: Option<i64>,
    pub placeholder_message_id: Option<i32>,
    pub inline_message_id: Option<String>,
    pub lang: Lang,
}

#[derive(Clone)]
pub struct Db {
    pool: PgPool,
    /// L1 cache: (video key, quality) -> full cached entry.
    l1: Cache<String, CachedVideo>,
}

impl Db {
    pub async fn connect(database_url: &str) -> AppResult<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(10)
            .min_connections(1)
            .acquire_timeout(Duration::from_secs(10))
            .connect(database_url)
            .await
            .map_err(|error| AppError::Database(format!("cannot connect to Postgres: {error}")))?;

        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .map_err(|error| AppError::Database(format!("migration failed: {error}")))?;

        let l1 = Cache::builder()
            .max_capacity(10_000)
            .time_to_live(Duration::from_secs(7 * 24 * 3600))
            .build();

        let db = Self { pool, l1 };
        let _ = db.cleanup_expired_selections().await;
        db.requeue_stale(600).await;
        Ok(db)
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    pub async fn ping(&self) -> bool {
        sqlx::query_scalar::<_, i32>("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .map(|value| value == 1)
            .unwrap_or(false)
    }

    // ----- file-id cache ----------------------------------------------------

    fn cache_id(key: &VideoKey, quality: QualityTier) -> String {
        format!("{}:{}", key, quality.as_str())
    }

    pub async fn get_video(
        &self,
        key: &VideoKey,
        quality: QualityTier,
    ) -> AppResult<Option<CachedVideo>> {
        let id = Self::cache_id(key, quality);
        if let Some(cached) = self.l1.get(&id).await {
            return Ok(Some(cached));
        }

        let row: Option<VideoRow> = sqlx::query_as(
            "SELECT file_id, file_unique_id, title, duration_sec, width, height, size_bytes, page_url
             FROM videos WHERE source = $1 AND source_id = $2 AND quality = $3",
        )
        .bind(key.source.as_str())
        .bind(&key.id)
        .bind(quality.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

        let Some((
            file_id,
            file_unique_id,
            title,
            duration_sec,
            width,
            height,
            size_bytes,
            page_url,
        )) = row
        else {
            return Ok(None);
        };
        // Uploader is nullable; fetch it separately to keep the tuple small.
        let uploader: Option<String> = sqlx::query_scalar(
            "SELECT uploader FROM videos WHERE source = $1 AND source_id = $2 AND quality = $3",
        )
        .bind(key.source.as_str())
        .bind(&key.id)
        .bind(quality.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

        let cached = CachedVideo {
            key: key.clone(),
            quality,
            file_id,
            file_unique_id,
            meta: common::domain::VideoMeta {
                key: key.clone(),
                title,
                uploader,
                duration: u32::try_from(duration_sec)
                    .ok()
                    .map(|secs| std::time::Duration::from_secs(u64::from(secs))),
                thumbnail_url: None,
                page_url,
            },
            width: u32::try_from(width).unwrap_or(0),
            height: u32::try_from(height).unwrap_or(0),
            duration_sec: u32::try_from(duration_sec).unwrap_or(0),
            size_bytes: u64::try_from(size_bytes).unwrap_or(0),
        };
        self.l1.insert(id, cached.clone()).await;
        Ok(Some(cached))
    }

    pub async fn put_video(&self, video: &CachedVideo) -> AppResult<()> {
        let width = i32::try_from(video.width).unwrap_or(i32::MAX);
        let height = i32::try_from(video.height).unwrap_or(i32::MAX);
        let duration = i32::try_from(video.duration_sec).unwrap_or(i32::MAX);
        let size = i64::try_from(video.size_bytes).unwrap_or(i64::MAX);

        sqlx::query(
            "INSERT INTO videos
                (source, source_id, quality, file_id, file_unique_id, title, uploader,
                 duration_sec, width, height, size_bytes, page_url, last_used_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, now())
             ON CONFLICT (source, source_id, quality) DO UPDATE SET
                 file_id = excluded.file_id,
                 file_unique_id = excluded.file_unique_id,
                 title = excluded.title,
                 uploader = excluded.uploader,
                 duration_sec = excluded.duration_sec,
                 width = excluded.width,
                 height = excluded.height,
                 size_bytes = excluded.size_bytes,
                 page_url = excluded.page_url,
                 last_used_at = now()",
        )
        .bind(video.key.source.as_str())
        .bind(&video.key.id)
        .bind(video.quality.as_str())
        .bind(&video.file_id)
        .bind(&video.file_unique_id)
        .bind(&video.meta.title)
        .bind(video.meta.uploader.as_deref())
        .bind(duration)
        .bind(width)
        .bind(height)
        .bind(size)
        .bind(&video.meta.page_url)
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;

        let id = Self::cache_id(&video.key, video.quality);
        self.l1.insert(id, video.clone()).await;
        Ok(())
    }

    /// Bookkeeping write; spawned fire-and-forget so it never delays a send.
    pub async fn touch_video(&self, key: &VideoKey, quality: QualityTier) {
        let pool = self.pool.clone();
        let source = key.source.as_str().to_owned();
        let id = key.id.clone();
        let quality = quality.as_str().to_owned();
        tokio::spawn(async move {
            if let Err(error) = sqlx::query(
                "UPDATE videos SET hits = hits + 1, last_used_at = now()
                 WHERE source = $1 AND source_id = $2 AND quality = $3",
            )
            .bind(source)
            .bind(id)
            .bind(quality)
            .execute(&pool)
            .await
            {
                warn!(%error, "could not update cache usage counters");
            }
        });
    }

    pub async fn purge_video(&self, key: &VideoKey) -> AppResult<bool> {
        let result = sqlx::query("DELETE FROM videos WHERE source = $1 AND source_id = $2")
            .bind(key.source.as_str())
            .bind(&key.id)
            .execute(&self.pool)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        for quality in QualityTier::ALL {
            self.l1.remove(&Self::cache_id(key, quality)).await;
        }
        Ok(result.rows_affected() > 0)
    }

    // ----- users ------------------------------------------------------------

    pub async fn get_user(&self, tg_id: i64) -> AppResult<Option<UserRecord>> {
        let row: Option<(String, i32, bool)> =
            sqlx::query_as("SELECT lang, max_height, banned FROM users WHERE tg_id = $1")
                .bind(tg_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(row.map(|(lang, max_height, banned)| UserRecord {
            lang: Lang::from_code(&lang).unwrap_or(Lang::Uk),
            max_height: u32::try_from(max_height).unwrap_or(720),
            banned,
        }))
    }

    pub async fn ensure_user(&self, tg_id: i64, lang: Lang, max_height: u32) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO users (tg_id, lang, max_height) VALUES ($1, $2, $3)
             ON CONFLICT (tg_id) DO NOTHING",
        )
        .bind(tg_id)
        .bind(lang.code())
        .bind(i32::try_from(max_height).unwrap_or(720))
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(())
    }

    pub async fn set_user_quality(&self, tg_id: i64, max_height: u32) -> AppResult<()> {
        sqlx::query("UPDATE users SET max_height = $1 WHERE tg_id = $2")
            .bind(i32::try_from(max_height).unwrap_or(720))
            .bind(tg_id)
            .execute(&self.pool)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(())
    }

    pub async fn set_user_lang(&self, tg_id: i64, lang: Lang) -> AppResult<()> {
        sqlx::query("UPDATE users SET lang = $1 WHERE tg_id = $2")
            .bind(lang.code())
            .bind(tg_id)
            .execute(&self.pool)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(())
    }

    pub async fn set_banned(&self, tg_id: i64, banned: bool) -> AppResult<()> {
        sqlx::query("UPDATE users SET banned = $1 WHERE tg_id = $2")
            .bind(banned)
            .bind(tg_id)
            .execute(&self.pool)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(())
    }

    pub async fn stats(&self) -> AppResult<Stats> {
        let videos: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM videos")
            .fetch_one(&self.pool)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&self.pool)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        let sends: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(hits), 0)::bigint FROM videos")
            .fetch_one(&self.pool)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(Stats {
            videos,
            users,
            sends,
        })
    }

    // ----- selections -------------------------------------------------------

    pub async fn create_selection(&self, payload: Value, ttl: Duration) -> AppResult<String> {
        let id = Uuid::new_v4().simple().to_string();
        let expires_at =
            Utc::now() + ChronoDuration::seconds(i64::try_from(ttl.as_secs()).unwrap_or(3600));
        sqlx::query("INSERT INTO selections (id, payload, expires_at) VALUES ($1, $2, $3)")
            .bind(&id)
            .bind(&payload)
            .bind(expires_at)
            .execute(&self.pool)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(id)
    }

    pub async fn get_selection(&self, id: &str) -> AppResult<Option<Value>> {
        let row: Option<Value> = sqlx::query_scalar(
            "SELECT payload FROM selections WHERE id = $1 AND expires_at > now()",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(row)
    }

    async fn cleanup_expired_selections(&self) -> AppResult<()> {
        sqlx::query("DELETE FROM selections WHERE expires_at < now()")
            .execute(&self.pool)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(())
    }

    // ----- job queue --------------------------------------------------------

    /// Enqueue a job. Returns `false` when another active job already exists
    /// for this (video, quality) — callers then just add themselves as waiters.
    pub async fn enqueue_job(
        &self,
        key: &VideoKey,
        quality: QualityTier,
        url: &str,
    ) -> AppResult<(Uuid, bool)> {
        let id = Uuid::new_v4();
        let result = sqlx::query(
            "INSERT INTO jobs (id, source, source_id, quality, url)
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(key.source.as_str())
        .bind(&key.id)
        .bind(quality.as_str())
        .bind(url)
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
        let created = result.rows_affected() > 0;

        let actual_id: Uuid = if created {
            id
        } else {
            sqlx::query_scalar(
                "SELECT id FROM jobs
                 WHERE source = $1 AND source_id = $2 AND quality = $3
                   AND status IN ('queued', 'running')
                 LIMIT 1",
            )
            .bind(key.source.as_str())
            .bind(&key.id)
            .bind(quality.as_str())
            .fetch_one(&self.pool)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?
        };
        Ok((actual_id, created))
    }

    pub async fn add_waiter(
        &self,
        job_id: Uuid,
        user_tg_id: i64,
        chat_id: Option<i64>,
        placeholder_message_id: Option<i32>,
        inline_message_id: Option<&str>,
        lang: Lang,
    ) -> AppResult<()> {
        sqlx::query(
            "INSERT INTO job_waiters
                (job_id, user_tg_id, chat_id, placeholder_message_id, inline_message_id, lang)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(job_id)
        .bind(user_tg_id)
        .bind(chat_id)
        .bind(placeholder_message_id)
        .bind(inline_message_id)
        .bind(lang.code())
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(())
    }

    /// Atomically claim the oldest queued job. Safe across replicas thanks to
    /// `FOR UPDATE SKIP LOCKED`.
    pub async fn claim_job(&self) -> AppResult<Option<Job>> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;
        let row: Option<(Uuid, String, String, String, String)> = sqlx::query_as(
            "SELECT id, source, source_id, quality, url FROM jobs
             WHERE status = 'queued'
             ORDER BY created_at
             LIMIT 1
             FOR UPDATE SKIP LOCKED",
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
        let Some((id, source, source_id, quality, url)) = row else {
            return Ok(None);
        };
        sqlx::query("UPDATE jobs SET status = 'running', attempts = attempts + 1, updated_at = now() WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        tx.commit()
            .await
            .map_err(|e| AppError::Database(e.to_string()))?;

        let Ok(source_kind) = source.parse::<SourceKind>() else {
            warn!(%source, "unknown source in job; marking failed");
            self.fail_job(id, "unknown source").await?;
            return Ok(None);
        };
        let Some(quality) = QualityTier::from_label(&quality) else {
            self.fail_job(id, "unknown quality").await?;
            return Ok(None);
        };
        Ok(Some(Job {
            id,
            key: VideoKey::new(source_kind, source_id),
            quality,
            url,
        }))
    }

    pub async fn finish_job(&self, id: Uuid) -> AppResult<()> {
        sqlx::query("UPDATE jobs SET status = 'done', updated_at = now() WHERE id = $1")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(())
    }

    pub async fn fail_job(&self, id: Uuid, error: &str) -> AppResult<()> {
        sqlx::query(
            "UPDATE jobs SET status = 'failed', error = $1, updated_at = now() WHERE id = $2",
        )
        .bind(common::i18n::truncate(error, 2000))
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(())
    }

    /// Requeue jobs that were left `running` by a crashed replica.
    pub async fn requeue_stale(&self, threshold_secs: i64) {
        let cutoff = Utc::now() - ChronoDuration::seconds(threshold_secs);
        if let Err(error) = sqlx::query(
            "UPDATE jobs SET status = 'queued', updated_at = now()
             WHERE status = 'running' AND updated_at < $1",
        )
        .bind(cutoff)
        .execute(&self.pool)
        .await
        {
            warn!(%error, "could not requeue stale jobs");
        }
    }

    /// Atomically take every undelivered waiter for a job exactly once.
    pub async fn take_waiters(&self, job_id: Uuid) -> AppResult<Vec<Waiter>> {
        let rows: Vec<WaiterRow> = sqlx::query_as(
                "UPDATE job_waiters SET delivered = true
                 WHERE id IN (SELECT id FROM job_waiters WHERE job_id = $1 AND delivered = false)
                 RETURNING id, user_tg_id, chat_id, placeholder_message_id, inline_message_id, lang",
            )
            .bind(job_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|error| AppError::Database(error.to_string()))?;
        Ok(rows
            .into_iter()
            .map(
                |(id, user_tg_id, chat_id, placeholder_message_id, inline_message_id, lang)| {
                    Waiter {
                        id,
                        user_tg_id,
                        chat_id,
                        placeholder_message_id,
                        inline_message_id,
                        lang: Lang::from_code(&lang).unwrap_or(Lang::Uk),
                    }
                },
            )
            .collect())
    }

    pub async fn queue_length(&self) -> AppResult<i64> {
        sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE status = 'queued'")
            .fetch_one(&self.pool)
            .await
            .map_err(|error| AppError::Database(error.to_string()))
    }
}
