# tiki-tiki-tii

Telegram video downloader bot in Rust. Send a link in a private chat or use
`@bot <link>` inline; the bot downloads the video with `yt-dlp`, prepares a
Telegram-friendly mp4 with `ffmpeg`, and caches the resulting `file_id` in
Postgres so repeat requests are instant.

## Features

- Private chat: link → video with correct aspect ratio, thumbnail and caption.
- Inline mode: `@bot <link>` in any chat; cached videos arrive immediately,
  uncached ones replace a placeholder via `editMessageMedia`.
- Quality tiers 480/720/1080 (default 720) with an "Other quality" button and
  `/settings`.
- `file_id` cache (moka L1 + Postgres L2), Postgres job queue with dedup across
  replicas (`FOR UPDATE SKIP LOCKED` + partial unique index), in-flight
  broadcast dedup in one process.
- No re-encode on the happy path: H.264 + AAC in mp4 is only remuxed with
  `+faststart`; transcoding is a bounded fallback with its own semaphore.
- Per-user rate limiting, admin commands, uk/en messages, Prometheus metrics,
  `/healthz`, graceful shutdown.

## Layout

```
crates/
  common/       domain types, config, errors, i18n, metrics
  sources/      offline URL parsing, short-link resolve, oEmbed metadata
  downloader/   yt-dlp wrapper + deduplicating download manager
  media/        ffprobe/ffmpeg: faststart remux, thumbnail, transcode fallback
  storage/      Postgres: file-id cache, job queue, users, selections
  telegram/     handlers, inline, callbacks, job worker, rendering
  app/          entry point, health/metrics server, janitor
```

Dependency direction: `common` ← `sources`/`downloader`/`media`/`storage` ←
`telegram` ← `app`.

## Before the first run — env checklist

The code is complete and verified; the only remaining step is **your** configuration.
Do this before `docker compose up`:

1. **Bot token** — create a bot with [@BotFather](https://t.me/BotFather) and put
   the token into `.env` as `BOT_TOKEN=...`. The app also accepts the legacy
   name `TELOXIDE_TOKEN`.
2. **Inline mode** — send `/setinline` to BotFather (enable it) and
   `/setinlinefeedback` → **100%**. Without feedback Telegram never sends
   `chosen_inline_result`, so uncached inline videos will not be delivered.
3. **Copy the template:** `cp .env.example .env`, then edit `.env`.
4. **Required for inline uncached videos:** `STORAGE_CHAT_ID` — the numeric id
   of a private channel/chat where the bot can write. Add the bot there first.
5. **For videos > 50 MB (required by the spec):** get `api_id` and `api_hash`
   from [my.telegram.org](https://my.telegram.org), set `TELEGRAM_API_ID`,
   `TELEGRAM_API_HASH` and `TELEGRAM_API_URL=http://bot-api:8081`, then start
   with `docker compose --profile local up -d`. Otherwise leave
   `TELEGRAM_API_URL` empty to use the cloud API (49 MB cap).
6. **Admins (optional):** `ADMIN_IDS=123456,789` for `/stats`, `/ban`,
   `/unban`, `/purge`.

`DATABASE_URL` is already set inside `docker-compose.yml`, so you do not need
to configure Postgres manually. All other variables have sane defaults from
the spec.

## Quick start

After completing the checklist above:

```bash
docker compose up -d          # cloud Bot API
# or, for >50 MB files:
docker compose --profile local up -d
```

This starts Postgres and the bot. The cloud Bot API is used unless
`TELEGRAM_API_URL` points at the local server. Metrics/health are on `:9090`.

### Local Bot API server (files over 50 MB)

The TZ requires `telegram-bot-api --local` for videos above 50 MB.

1. Get `api_id` and `api_hash` from [my.telegram.org](https://my.telegram.org).
2. **One time only**, log the token out of the cloud API (otherwise the local
   server refuses to start). Use a small script or the Bot API method `logOut`
   with the cloud endpoint before switching.
3. Put `TELEGRAM_API_ID`, `TELEGRAM_API_HASH` and
   `TELEGRAM_API_URL=http://bot-api:8081` in `.env`.
4. Start with the profile:

```bash
docker compose --profile local up -d
```

The `downloads` volume is shared at `/data/downloads` between `bot` and
`bot-api`, so the local server can read the file by path. The cache `file_id`s
are bound to one Bot API mode — after switching cloud ↔ local, clear the
`videos` table (or let the self-healing path refresh them).

### yt-dlp / Deno / EJS

The image pins yt-dlp and Deno with SHA256 checksums. `docker/entrypoint.sh`
checks GitHub for a newer yt-dlp on every container start, verifies it and
falls back to the pinned binary on any failure. Set `YTDLP_AUTO_UPDATE=false`
to freeze it. YouTube's EJS challenge scripts are downloaded on demand by
yt-dlp into `DENO_DIR`/`XDG_CACHE_HOME` (on the persistent volume), so no
manual script installation is needed.

## Configuration

See `.env.example`. Notable values:

| Variable | Default | Meaning |
| --- | --- | --- |
| `BOT_TOKEN` | required | Telegram bot token |
| `DATABASE_URL` | required | `postgres://…` |
| `TELEGRAM_API_URL` | unset | local Bot API base URL; unset = cloud |
| `WEBHOOK_URL` | unset | webhook public URL; empty = polling |
| `STORAGE_CHAT_ID` | unset | service chat used for inline file-id bootstrap |
| `ENABLED_SOURCES` | `all` | comma list: youtube,tiktok,instagram,x,reddit,vimeo |
| `MAX_CONCURRENT_DOWNLOADS` | `3` | download semaphore |
| `MAX_CONCURRENT_TRANSCODES` | vCPU/2 | transcode semaphore |
| `MAX_DURATION_SEC` | `900` | 15 minutes |
| `MAX_FILE_MB` | `200` | also capped by Bot API mode |
| `DEFAULT_MAX_HEIGHT` | `720` | 480 / 720 / 1080 |
| `PROXY` | unset | optional outbound proxy |

## Development

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --release --locked
```

See `docs/decisions.md` for deviations from the TZ and `docs/ASSUMPTIONS.md`
for defaults taken on open questions. `docs/perf.md` tracks the benchmarking
plan; results are added as measurements are made.

## Legal

Download only content you are allowed to copy. The bot does not bypass DRM,
logins or paywalls, and stores only metadata plus Telegram file ids.
`/purge <source:id>` removes a cache entry on a rights complaint.

License: Apache-2.0.
