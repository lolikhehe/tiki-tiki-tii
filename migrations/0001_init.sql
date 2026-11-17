-- Clean schema for the video bot (ТЗ §6.3). No music-domain columns.
CREATE TYPE job_status AS ENUM ('queued', 'running', 'done', 'failed');

-- Cache of Telegram file ids. Unit of caching: (video, quality).
CREATE TABLE IF NOT EXISTS videos (
    source         TEXT        NOT NULL,
    source_id      TEXT        NOT NULL,
    quality        TEXT        NOT NULL,
    file_id        TEXT        NOT NULL,
    file_unique_id TEXT        NOT NULL,
    title          TEXT        NOT NULL,
    uploader       TEXT,
    duration_sec   INTEGER     NOT NULL,
    width          INTEGER     NOT NULL,
    height         INTEGER     NOT NULL,
    size_bytes     BIGINT      NOT NULL,
    page_url       TEXT        NOT NULL DEFAULT '',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    hits           BIGINT      NOT NULL DEFAULT 0,
    PRIMARY KEY (source, source_id, quality)
);

-- Persistent job queue. Status survives restarts and is shared by replicas.
CREATE TABLE IF NOT EXISTS jobs (
    id         UUID        PRIMARY KEY,
    source     TEXT        NOT NULL,
    source_id  TEXT        NOT NULL,
    quality    TEXT        NOT NULL,
    url        TEXT        NOT NULL,
    status     job_status  NOT NULL DEFAULT 'queued',
    attempts   INTEGER     NOT NULL DEFAULT 0,
    error      TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- At most one active job per (video, quality), even across bot replicas.
CREATE UNIQUE INDEX IF NOT EXISTS jobs_active_uq
    ON jobs (source, source_id, quality)
    WHERE status IN ('queued', 'running');

-- Everyone waiting for a job's result (private and inline).
CREATE TABLE IF NOT EXISTS job_waiters (
    id                     BIGSERIAL PRIMARY KEY,
    job_id                 UUID    NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    user_tg_id             BIGINT  NOT NULL,
    chat_id                BIGINT,
    placeholder_message_id INTEGER,
    inline_message_id      TEXT,
    lang                   TEXT    NOT NULL DEFAULT 'uk',
    delivered              BOOLEAN NOT NULL DEFAULT false
);

CREATE INDEX IF NOT EXISTS job_waiters_job_idx ON job_waiters (job_id, delivered);

-- Short ids for callback_data and inline result ids. Payload lives in JSONB.
CREATE TABLE IF NOT EXISTS selections (
    id         TEXT        PRIMARY KEY,
    payload    JSONB       NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE IF NOT EXISTS users (
    tg_id      BIGINT      PRIMARY KEY,
    lang       TEXT        NOT NULL DEFAULT 'uk',
    max_height INTEGER     NOT NULL DEFAULT 720,
    banned     BOOLEAN     NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A failed file_id is removed so the next request re-downloads.
CREATE INDEX IF NOT EXISTS videos_last_used_idx ON videos (last_used_at);
