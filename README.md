# tiki-tiki-tii

Telegram video downloader bot in Rust. Send a link in a private chat or use
`@bot <link>` inline; the bot downloads the video with `yt-dlp`, prepares a
Telegram-friendly mp4 with `ffmpeg`, and caches the resulting `file_id` in
Postgres so repeat requests are instant.

> Work in progress — see the stage checklist below.

## Architecture

```
crates/
  common/       shared domain types, config, errors, i18n, metrics
  sources/      URL recognition (offline), short-link resolve, oEmbed metadata
  downloader/   yt-dlp wrapper + deduplicating download manager
  media/        ffprobe/ffmpeg: faststart remux, thumbnail, transcode fallback
  storage/      Postgres: file-id cache, job queue, users, selections
  telegram/     handlers, inline mode, job worker, message rendering
  app/          entry point, health/metrics server, graceful shutdown
```

Dependency direction: `common` ← `sources`/`downloader`/`media`/`storage` ←
`telegram` ← `app`.

## Build

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Configuration

See `.env.example`. Required: `BOT_TOKEN`, `DATABASE_URL`. For inline delivery
of uncached videos also set `STORAGE_CHAT_ID`. `WEBHOOK_URL` empty = polling.

## Legal

Download only content you are allowed to copy. The bot does not bypass DRM,
logins or paywalls, and stores only metadata plus Telegram file ids. Use
`/purge <video_key>` to remove a cache entry on a rights complaint.

License: Apache-2.0.
