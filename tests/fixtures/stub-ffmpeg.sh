#!/bin/sh
# Stub ffmpeg: copies the -i input to the final argument (output path).
input=""
prev=""
for arg in "$@"; do
    if [ "$prev" = "-i" ]; then input="$arg"; fi
    prev="$arg"
done
out="$prev"
if [ -z "$input" ] || [ -z "$out" ]; then exit 1; fi
printf 'FAKE' > "$out"
