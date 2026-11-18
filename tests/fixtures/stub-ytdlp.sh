#!/bin/sh
# Stub yt-dlp for tests. Returns fixed metadata for -J and writes a tiny fake
# mp4 for downloads. Counts real download invocations via $STUB_COUNTER.
is_meta=0
out=""
prev=""
for arg in "$@"; do
    if [ "$arg" = "--dump-single-json" ]; then is_meta=1; fi
    if [ "$prev" = "-o" ]; then out="$arg"; fi
    prev="$arg"
done

if [ "$is_meta" = 1 ]; then
    printf '%s\n' '{"title":"Test video","uploader":"Tester","duration":30,"webpage_url":"https://youtu.be/dedup-test-id"}'
    exit 0
fi

counter="${STUB_COUNTER:-/tmp/yt-dlp-count}"
printf 'x\n' >> "$counter"
if [ -z "$out" ]; then out="video.mp4"; fi
path=$(printf '%s' "$out" | sed 's/%(ext)s/mp4/')
mkdir -p "$(dirname "$path")"
printf 'FAKEVIDEO' > "$path"
printf '%s\n' "$path"
