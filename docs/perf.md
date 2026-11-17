# Performance benchmarks

## How to measure

TZ §11 defines five scenarios. Run them against a local Bot API mock and a
stub `yt-dlp` that writes a small mp4 fixture (see `docs/ASSUMPTIONS.md`).
Record the p50/p95/p99 and write the numbers here.

| Scenario | What it measures | Target |
| --- | --- | --- |
| S1 | request served from cache (`file_id`) | < 300 ms |
| S2 | new 30–60 s 720p video, no transcode | 4–8 s |
| S3 | same video with transcode | measure (CPU-bound) |
| S4 | 20 concurrent requests, 5 distinct videos | exactly 5 downloads |
| S5 | burst of inline queries | < 1–2 s answer time |

Counters/histograms are exported at `:9090/metrics` under the `video_bot_`
prefix: `cache_hits_total`, `cache_misses_total`, `downloads_total`,
`transcodes_total`, `remux_total`, `download_seconds`, `ffmpeg_seconds`,
`upload_seconds`, `inline_answer_seconds`, `queue_wait_seconds`,
`downloads_inflight`, `queue_length`.

## Status

Not yet measured on production-like hardware. Unit tests cover URL parsing,
error classification, cache-key derivation and metadata mapping. The e2e/load
harness with the Bot API mock is the next milestone.
