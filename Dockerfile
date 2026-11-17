# syntax=docker/dockerfile:1

# ---------- Dependency planner (cargo-chef) ----------
FROM lukemathwalker/cargo-chef:latest-rust-1-slim-bookworm AS chef
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ---------- Build ----------
FROM chef AS builder
RUN apt-get update \
 && apt-get install -y --no-install-recommends build-essential ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json
COPY . .
RUN cargo build --release --locked --package app --bin video-bot

# ---------- Runtime ----------
FROM debian:bookworm-slim

ARG YTDLP_VERSION=2026.10.26
ARG YTDLP_SHA256=767ba9dce7052aa196f8b12652f433d76de128ba168966fa84b6394d5e6c6795
ARG DENO_VERSION=2.9.8
ARG DENO_SHA256=e890975bc2549807bbcc0b584ffafda72cc7c93349dd99c80483dd784f6c4609

RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl ffmpeg tini unzip \
 && rm -rf /var/lib/apt/lists/* \
 && curl -fSL "https://github.com/yt-dlp/yt-dlp/releases/download/${YTDLP_VERSION}/yt-dlp_linux" \
      -o /usr/local/bin/yt-dlp \
 && echo "${YTDLP_SHA256}  /usr/local/bin/yt-dlp" | sha256sum -c - \
 && chmod 755 /usr/local/bin/yt-dlp \
 && curl -fSL "https://github.com/denoland/deno/releases/download/v${DENO_VERSION}/deno-x86_64-unknown-linux-gnu.zip" \
      -o /tmp/deno.zip \
 && echo "${DENO_SHA256}  /tmp/deno.zip" | sha256sum -c - \
 && unzip /tmp/deno.zip -d /usr/local/bin \
 && chmod 755 /usr/local/bin/deno \
 && rm /tmp/deno.zip \
 && useradd --system --uid 10001 --create-home --shell /usr/sbin/nologin bot

WORKDIR /app
COPY --from=builder /app/target/release/video-bot /usr/local/bin/video-bot
COPY docker/entrypoint.sh /usr/local/bin/docker-entrypoint
RUN chmod 755 /usr/local/bin/docker-entrypoint \
 && mkdir -p /data/downloads && chown -R bot:bot /data

# yt-dlp fetches EJS challenge scripts on demand into its cache directory;
# keep that cache on the writable volume so it survives read_only root fs.
ENV DOWNLOAD_DIR=/data/downloads \
    TMPDIR=/data/downloads \
    XDG_CACHE_HOME=/data/downloads/cache \
    DENO_DIR=/data/downloads/deno \
    YTDLP_PATH=/usr/local/bin/yt-dlp \
    FFMPEG_PATH=/usr/bin/ffmpeg \
    FFPROBE_PATH=/usr/bin/ffprobe \
    YTDLP_AUTO_UPDATE=true \
    METRICS_ADDR=0.0.0.0:9090 \
    LOG_LEVEL=info

VOLUME ["/data/downloads"]
EXPOSE 9090

USER bot
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/docker-entrypoint", "video-bot"]
