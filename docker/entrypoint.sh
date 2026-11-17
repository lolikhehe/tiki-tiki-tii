#!/bin/sh
# Refresh the pinned yt-dlp from the latest release on container start, then
# exec the bot. Failures leave the pinned binary in place.
log() { echo "[entrypoint] $*"; }

PINNED_VERSION="2026.10.26"
PINNED_BIN="/usr/local/bin/yt-dlp"
UPDATE_DIR="/data/downloads/bin"
UPDATE_BIN="$UPDATE_DIR/yt-dlp"
AUTO_UPDATE="${YTDLP_AUTO_UPDATE:-true}"

if [ "$AUTO_UPDATE" != "true" ]; then
    log "using pinned yt-dlp $PINNED_VERSION"
    exec "$@"
fi

tag_json=$(curl --fail --silent --show-error --retry 2 --max-time 30 \
    https://api.github.com/repos/yt-dlp/yt-dlp/releases/latest 2>&1)
tag=$(printf '%s' "$tag_json" | grep -o '"tag_name": *"[^"]*"' | head -n 1 | cut -d'"' -f 4)

if [ -z "$tag" ]; then
    log "release lookup failed; using pinned yt-dlp $PINNED_VERSION"
elif [ "$tag" = "$PINNED_VERSION" ]; then
    log "pinned yt-dlp $PINNED_VERSION is current"
elif [ -x "$UPDATE_BIN" ] && [ "$("$UPDATE_BIN" --version 2>/dev/null)" = "$tag" ]; then
    log "yt-dlp $tag already cached"
    YTDLP_PATH="$UPDATE_BIN"; export YTDLP_PATH
else
    log "updating yt-dlp $PINNED_VERSION -> $tag"
    mkdir -p "$UPDATE_DIR" || log "cannot create $UPDATE_DIR"
    if curl --fail --silent --show-error --retry 2 --max-time 300 -L \
        "https://github.com/yt-dlp/yt-dlp/releases/download/${tag}/yt-dlp_linux" \
        -o "$UPDATE_BIN.new" \
        && chmod 755 "$UPDATE_BIN.new" \
        && [ "$("$UPDATE_BIN.new" --version 2>/dev/null)" = "$tag" ]; then
        mv "$UPDATE_BIN.new" "$UPDATE_BIN"
        YTDLP_PATH="$UPDATE_BIN"; export YTDLP_PATH
        log "yt-dlp $tag installed"
    else
        rm -f "$UPDATE_BIN.new"
        log "update failed; using pinned yt-dlp $PINNED_VERSION"
    fi
fi

log "starting $*"
exec "$@"
