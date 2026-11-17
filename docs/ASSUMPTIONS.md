# Assumptions (open questions from TZ §10)

The TZ says the agent should not wait for answers, but take the defaults and
record them here.

1. **Sources in the first release:** YouTube, TikTok, Instagram Reels, X,
   Reddit, Vimeo — all behind `ENABLED_SOURCES` (default: all).
2. **Limits:** 15 minutes (`MAX_DURATION_SEC=900`) and 200 MB
   (`MAX_FILE_MB=200`, additionally capped by the Bot API mode).
3. **Inline search by title:** not implemented; inline accepts links only.
4. **Audio-only mode:** not implemented.
5. **Carousels / multi-video posts:** first video only, as specified.
6. **Public bot with per-user rate limits**, no allow-list in v1.
7. **Server sizing:** 4 vCPU / 4–8 GB assumed. `MAX_CONCURRENT_DOWNLOADS=3`,
   `MAX_CONCURRENT_TRANSCODES=half of vCPU`.

Additional engineering assumptions:

- Telegram `file_id` is bound to one bot token and Bot API mode. Changing the
  token or switching cloud ↔ local invalidates the `videos` cache; the cache
  self-heals by deleting the bad row and re-downloading.
- `STORAGE_CHAT_ID` is only required for inline delivery of uncached videos.
- Timestamps use `TIMESTAMPTZ`; all SQL is Postgres.
