#!/bin/bash
# Test script to inspect yt-dlp progress output
# Checks which output format yt-dlp uses with current settings

set -e

echo "Testing yt-dlp progress output..."

yt-dlp \
  --newline \
  --progress \
  --print "%(title)s" \
  --print "%(id)s" \
  --simulate \
  --youtube-skip-dash-manifest \
  "https://www.youtube.com/watch?v=dQw4w9WgXcQ" \
  | head -n 50
