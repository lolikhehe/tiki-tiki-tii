#!/bin/sh
# Stub ffprobe: always reports a Telegram-compatible 1280x720 h264/aac stream.
printf '%s\n' '{"format":{"duration":"30","size":"1000"},"streams":[{"codec_type":"video","codec_name":"h264","pix_fmt":"yuv420p","width":1280,"height":720},{"codec_type":"audio","codec_name":"aac"}]}'
