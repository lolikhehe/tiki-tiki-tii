# Performance benchmarks

## How to measure

ТЗ §11 defines five scenarios. The load harness lives in
`crates/downloader/tests/dedup.rs`: it starts the real `DownloadManager`
against a Postgres instance (`TEST_DATABASE_URL`) with the stub binaries from
`tests/fixtures/`, fires 20 concurrent requests for the same video, asserts
that exactly one `yt-dlp` download happened, and writes p50/p95/p99 to
`target/perf-local.json` with the label **"local mock, NOT production"**.

Run locally:

```bash
# Postgres must be reachable; see docker-compose.yml
TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/videobot \
  cargo test -p downloader --test dedup -- --nocapture
```

CI runs the same suite with a Postgres service container and uploads the
report as an artifact. `crates/storage/tests/postgres.rs` verifies every SQL
query against a real database in the same job.

| Scenario | What it measures | Target | Status |
| --- | --- | --- | --- |
| S1 | request served from cache (`file_id`) | < 300 ms | plan only |
| S2 | new 30–60 s 720p video, no transcode | 4–8 s | plan only |
| S3 | same video with transcode | measure (CPU-bound) | plan only |
| S4 | 20 concurrent requests, 1 distinct video | exactly 1 download | **measured (local mock)** |
| S5 | burst of inline queries | < 1–2 s answer time | plan only |

## Measured results — local mock, NOT production

S4 on a development machine with stub `yt-dlp`/`ffmpeg`/`ffprobe` (no real
network, no real encoding):

| Metric | Value |
| --- | --- |
| requests | 20 |
| downloads triggered | 1 ✅ |
| p50 | ~402 ms |
| p95 | ~403 ms |
| p99 | ~403 ms |

The absolute latency is dominated by process startup and the stub scripts and
is **not** a production estimate. The meaningful result is the dedup
assertion: 20 concurrent requests produced one download.

Counters/histograms exported at `:9090/metrics` under the `video_bot_` prefix:
`cache_hits_total`, `cache_misses_total`, `downloads_total`,
`transcodes_total`, `remux_total`, `download_seconds`, `ffmpeg_seconds`,
`upload_seconds`, `inline_answer_seconds`, `queue_wait_seconds`,
`downloads_inflight`, `queue_length`.

## Remaining work

S1–S3 and S5 require a real Bot API (or a richer mock covering `SendVideo` /
`EditMessageMediaInline`) plus a real video fixture. The mock Bot API listener
in `crates/app/tests/mock_bot_api.rs` is the foundation for that suite.
