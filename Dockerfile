# syntax=docker/dockerfile:1
# Build stage for Rust application with cargo-chef for dependency caching
FROM rust:1.93-alpine AS chef
# hadolint ignore=DL3018
RUN apk add --no-cache musl-dev && \
    cargo install cargo-chef
WORKDIR /app

FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY locales ./locales
COPY migrations ./migrations
COPY assets ./assets
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS rust-builder
# hadolint ignore=DL3018
RUN apk add --no-cache \
  musl-dev \
  pkgconfig \
  openssl-dev \
  openssl-libs-static \
  sqlite-dev \
  sqlite-static \
  build-base

COPY --from=planner /app/recipe.json recipe.json
# Build dependencies - this is the caching layer
RUN cargo chef cook --release --recipe-path recipe.json

# Build application
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY locales ./locales
COPY migrations ./migrations
COPY assets ./assets

RUN cargo build --release -p doradura && \
    cargo build --release -p health-monitor && \
    cp /app/target/release/doradura /app/doradura-bin && \
    cp /app/target/release/health-monitor /app/health-monitor-bin && \
    strip /app/doradura-bin && \
    strip /app/health-monitor-bin && \
    echo "Binaries built successfully:" && \
    ls -lh /app/doradura-bin /app/health-monitor-bin

# === bgutil builder stage (runs in parallel with rust-builder) ===
FROM aiogram/telegram-bot-api:latest AS bgutil-builder
# hadolint ignore=DL3002,DL3018
USER root
RUN apk add --no-cache \
  nodejs npm git \
  build-base pkgconfig \
  cairo-dev pango-dev jpeg-dev giflib-dev pixman-dev python3

WORKDIR /opt/bgutil
RUN git clone --single-branch --branch 1.2.2 https://github.com/Brainicism/bgutil-ytdlp-pot-provider.git .
WORKDIR /opt/bgutil/server
RUN npm install && npx tsc

# === Runtime stage ===
# hadolint ignore=DL3007
FROM aiogram/telegram-bot-api:latest

# hadolint ignore=DL3002
USER root

# s6-overlay
ARG S6_OVERLAY_VERSION=3.2.0.2
ADD https://github.com/just-containers/s6-overlay/releases/download/v${S6_OVERLAY_VERSION}/s6-overlay-noarch.tar.xz /tmp
ADD https://github.com/just-containers/s6-overlay/releases/download/v${S6_OVERLAY_VERSION}/s6-overlay-x86_64.tar.xz /tmp
RUN tar -C / -Jxpf /tmp/s6-overlay-noarch.tar.xz && \
    tar -C / -Jxpf /tmp/s6-overlay-x86_64.tar.xz && \
    rm /tmp/s6-overlay-*.tar.xz

# Runtime dependencies only (no build-base, no *-dev)
# hadolint ignore=DL3018
RUN apk add --no-cache \
  ca-certificates musl libssl3 libcrypto3 \
  ffmpeg python3 py3-pip sqlite-libs \
  libgcc libstdc++ wget curl bash \
  nodejs npm \
  cairo pango libjpeg-turbo giflib pixman \
  ttf-dejavu fontconfig && \
  fc-cache -f

# curl-impersonate for Instagram TLS fingerprint spoofing
RUN wget -q https://github.com/lexiforest/curl-impersonate/releases/download/v1.4.4/curl-impersonate-v1.4.4.x86_64-linux-musl.tar.gz \
      -O /tmp/curl-impersonate.tar.gz && \
    tar -xzf /tmp/curl-impersonate.tar.gz -C /usr/local/bin/ && \
    rm /tmp/curl-impersonate.tar.gz && \
    chmod +x /usr/local/bin/curl-impersonate*

# Deno for yt-dlp JS runtime
# hadolint ignore=DL3018
RUN apk add --no-cache deno --repository=https://dl-cdn.alpinelinux.org/alpine/edge/testing

# yt-dlp nightly
RUN wget --tries=3 --waitretry=5 --progress=dot:giga \
  https://github.com/yt-dlp/yt-dlp-nightly-builds/releases/latest/download/yt-dlp \
  -O /usr/local/bin/yt-dlp && \
  chmod a+rx /usr/local/bin/yt-dlp

# Python deps
# hadolint ignore=DL3013
RUN pip3 install --no-cache-dir --break-system-packages \
    keyring pycryptodomex bgutil-ytdlp-pot-provider curl_cffi

# Copy pre-built bgutil from builder stage
COPY --from=bgutil-builder /opt/bgutil /opt/bgutil

# Create users and directories
RUN addgroup -g 2000 shareddata && \
  adduser -D -u 1000 -G shareddata botuser && \
  addgroup telegram-bot-api shareddata && \
  addgroup botuser telegram-bot-api && \
  mkdir -p /app /data /home/botuser/.cache && \
  chown 1000:2000 /home/botuser/.cache && \
  chown 1000:2000 /app && \
  chown telegram-bot-api:shareddata /data && \
  chmod 775 /data

# Copy compiled binary and migrations
COPY --from=rust-builder --chown=1000:2000 /app/doradura-bin /app/doradura
COPY --from=rust-builder --chown=1000:2000 /app/health-monitor-bin /app/health-monitor
RUN chmod 755 /app/doradura /app/health-monitor
COPY --from=rust-builder --chown=1000:2000 /app/migrations /app/migrations

WORKDIR /app

# Environment
ENV BOT_API_DATA_DIR=/data
ENV BOT_API_URL=http://localhost:8081
ENV S6_KEEP_ENV=1
ENV S6_BEHAVIOUR_IF_STAGE2_FAILS=2
ENV S6_CMD_WAIT_FOR_SERVICES_MAXTIME=0

# === s6-overlay service definitions ===

RUN mkdir -p /etc/s6-overlay/s6-rc.d/user/contents.d \
             /etc/s6-overlay/s6-rc.d/init-data/dependencies.d \
             /etc/s6-overlay/s6-rc.d/telegram-bot-api/dependencies.d \
             /etc/s6-overlay/s6-rc.d/bgutil-pot-server/dependencies.d \
             /etc/s6-overlay/s6-rc.d/doradura-bot/dependencies.d \
             /etc/s6-overlay/s6-rc.d/health-monitor/dependencies.d \
             /etc/s6-overlay/scripts

# === init-data oneshot service ===
RUN echo "oneshot" > /etc/s6-overlay/s6-rc.d/init-data/type && \
    touch /etc/s6-overlay/s6-rc.d/init-data/dependencies.d/base && \
    echo "/etc/s6-overlay/scripts/init-data" > /etc/s6-overlay/s6-rc.d/init-data/up

# hadolint ignore=SC2016
RUN printf '%s\n' \
    '#!/command/execlineb -P' \
    'foreground { /bin/sh -c "echo \"[init-data] START at $(date +%Y-%m-%dT%H:%M:%S.%3NZ)\" && echo $(($(date +%s%N)/1000000)) > /tmp/init_start_ms && echo $(($(date +%s%N)/1000000)) > /tmp/container_start_ms" }' \
    'foreground { echo "================================================" }' \
    'foreground { echo "Initializing Telegram Bot API + Doradura Bot" }' \
    'foreground { echo "================================================" }' \
    'foreground { mkdir -p /data /tmp }' \
    'foreground { chmod 1777 /tmp }' \
    'foreground { echo "Cleaning up old temp files..." }' \
    'foreground {' \
    '  /bin/sh -c "' \
    '    find /data -maxdepth 1 -name \"refresh_error_*.png\" -delete 2>/dev/null || true' \
    '    find /data -maxdepth 1 -name \"signout_detected_*.png\" -delete 2>/dev/null || true' \
    '    find /data -name \"*.binlog.lock\" -delete 2>/dev/null || true' \
    '    COUNT=\$(find /data -maxdepth 1 -name \"*.png\" 2>/dev/null | wc -l)' \
    '    echo Cleaned up temp files. Remaining files in /data: \$COUNT pngs' \
    '  "' \
    '}' \
    'foreground { chown telegram-bot-api:shareddata /data }' \
    'foreground { chmod 775 /data }' \
    'foreground { chown -R 1000:2000 /app }' \
    'foreground { chmod 755 /app }' \
    'foreground {' \
    '  /bin/sh -c "' \
    '    DB_PATH=\${DATABASE_URL:-/data/database.sqlite}' \
    '    DB_DIR=\$(dirname \"\$DB_PATH\")' \
    '    mkdir -p \"\$DB_DIR\"' \
    '    chmod 755 \"\$DB_DIR\" 2>/dev/null || true' \
    '    echo Database directory ready. Migrations will be applied by refinery on bot startup.' \
    '  "' \
    '}' \
    'foreground { echo "Setting database permissions..." }' \
    'foreground { chown -R 1000:2000 /data }' \
    'foreground { chmod 775 /data }' \
    'foreground { /bin/sh -c "chown 1000:2000 /data/*.sqlite* 2>/dev/null || true" }' \
    'foreground { /bin/sh -c "chmod 664 /data/*.sqlite* 2>/dev/null || true" }' \
    'foreground { echo "Clearing Bot API binlog for clean start..." }' \
    'foreground { /bin/sh -c "rm -f /data/*.binlog /data/tqueue.binlog 2>/dev/null || true" }' \
    'foreground { /bin/sh -c "echo \"[init-data] /data contains $(ls /data | wc -l) files\"" }' \
    'foreground { echo "================================================" }' \
    'foreground { /bin/sh -c "START=$(cat /tmp/init_start_ms 2>/dev/null || echo 0); END=$(($(date +%s%N)/1000000)); ELAPSED=$((END - START)); echo \"[init-data] COMPLETE in ${ELAPSED}ms at $(date +%Y-%m-%dT%H:%M:%S.%3NZ)\"" }' \
    'foreground { echo "Starting services (telegram-bot-api, bgutil, doradura-bot)..." }' \
    'echo "================================================"' \
    > /etc/s6-overlay/scripts/init-data && \
    chmod +x /etc/s6-overlay/scripts/init-data

# === telegram-bot-api longrun service ===
# hadolint ignore=SC2016
RUN echo "longrun" > /etc/s6-overlay/s6-rc.d/telegram-bot-api/type && \
    touch /etc/s6-overlay/s6-rc.d/telegram-bot-api/dependencies.d/init-data && \
    printf '%s\n' \
    '#!/command/execlineb -P' \
    'foreground { /bin/sh -c "echo \"[telegram-bot-api] START at $(date +%Y-%m-%dT%H:%M:%S.%3NZ)\" && echo $(($(date +%s%N)/1000000)) > /tmp/bot_api_start_ms" }' \
    's6-setuidgid telegram-bot-api' \
    'fdmove -c 2 1' \
    '/bin/sh -c "umask 007 && exec telegram-bot-api --local --api-id=${TELEGRAM_API_ID} --api-hash=${TELEGRAM_API_HASH} --http-port=8081 --http-stat-port=8082 --dir=/data --temp-dir=/tmp --verbosity=1"' \
    > /etc/s6-overlay/s6-rc.d/telegram-bot-api/run && \
    chmod +x /etc/s6-overlay/s6-rc.d/telegram-bot-api/run

# === bgutil-pot-server longrun service ===
# hadolint ignore=SC2016
RUN echo "longrun" > /etc/s6-overlay/s6-rc.d/bgutil-pot-server/type && \
    touch /etc/s6-overlay/s6-rc.d/bgutil-pot-server/dependencies.d/init-data && \
    printf '%s\n' \
    '#!/bin/sh' \
    'echo "[bgutil-pot-server] START at $(date +%Y-%m-%dT%H:%M:%S.%3NZ)"' \
    'echo $(($(date +%s%N)/1000000)) > /tmp/bgutil_start_ms' \
    'export HOME=/home/botuser' \
    'export TOKEN_TTL=6' \
    'exec s6-setuidgid botuser node /opt/bgutil/server/build/main.js 2>&1' \
    > /etc/s6-overlay/s6-rc.d/bgutil-pot-server/run && \
    chmod +x /etc/s6-overlay/s6-rc.d/bgutil-pot-server/run

# Wait script for Bot API
# hadolint ignore=SC2016
RUN printf '%s\n' \
    '#!/bin/sh' \
    'BOT_API="${BOT_API_URL:-http://localhost:8081}"' \
    'BOT_TOKEN="${TELOXIDE_TOKEN:-}"' \
    'READY=0' \
    'START_TIME=$(date +%s)' \
    'START_MS=$(($(date +%s%N)/1000000))' \
    'echo "[wait-for-bot-api] Waiting for Bot API at $BOT_API..."' \
    'for i in $(seq 1 180); do' \
    '  if [ $i -le 60 ]; then' \
    '    [ $((i % 5)) -eq 0 ] && echo "[wait-for-bot-api] Still waiting... (${i}s elapsed)"' \
    '  else' \
    '    [ $((i % 10)) -eq 0 ] && echo "[wait-for-bot-api] Still waiting... (${i}s elapsed)"' \
    '  fi' \
    '  if wget -q --spider "$BOT_API" 2>/dev/null; then' \
    '    if [ -n "$BOT_TOKEN" ]; then' \
    '      RESP=$(wget -q -O - "$BOT_API/bot$BOT_TOKEN/getMe" 2>/dev/null || echo "")' \
    '      if echo "$RESP" | grep -q "\"ok\":true"; then' \
    '        END_TIME=$(date +%s)' \
    '        ELAPSED=$((END_TIME - START_TIME))' \
    '        echo "[wait-for-bot-api] Bot API READY after ${ELAPSED}s (${i} checks)"' \
    '        READY=1' \
    '        break' \
    '      elif echo "$RESP" | grep -q "restart"; then' \
    '        [ $((i % 10)) -eq 0 ] && echo "[wait-for-bot-api] Bot API initializing... (${i}s)"' \
    '      fi' \
    '    else' \
    '      echo "[wait-for-bot-api] Bot API server responding"' \
    '      READY=1' \
    '      break' \
    '    fi' \
    '  fi' \
    '  sleep 1' \
    'done' \
    'if [ $READY -eq 0 ]; then' \
    '  echo "[wait-for-bot-api] WARNING: Bot API not ready after 180s, starting anyway..."' \
    'fi' \
    > /etc/s6-overlay/scripts/wait-for-bot-api && \
    chmod +x /etc/s6-overlay/scripts/wait-for-bot-api

# === doradura-bot longrun service ===
# hadolint ignore=SC2016
RUN echo "longrun" > /etc/s6-overlay/s6-rc.d/doradura-bot/type && \
    touch /etc/s6-overlay/s6-rc.d/doradura-bot/dependencies.d/telegram-bot-api && \
    printf '%s\n' \
    '#!/command/execlineb -P' \
    'foreground { /bin/sh -c "echo \"[doradura-bot] START at $(date +%Y-%m-%dT%H:%M:%S.%3NZ)\" && echo $(($(date +%s%N)/1000000)) > /tmp/doradura_start_ms" }' \
    'foreground { /etc/s6-overlay/scripts/wait-for-bot-api }' \
    'foreground { /bin/sh -c "START=$(cat /tmp/doradura_start_ms 2>/dev/null || echo 0); END=$(($(date +%s%N)/1000000)); ELAPSED=$((END - START)); echo \"[doradura-bot] Bot API check complete, waited ${ELAPSED}ms. Launching bot at $(date +%Y-%m-%dT%H:%M:%S.%3NZ)...\"" }' \
    's6-setuidgid botuser' \
    's6-env DATABASE_PATH=/data/database.sqlite' \
    's6-env TEMP_FILES_DIR=/data' \
    's6-env BOT_API_DATA_DIR=/data' \
    's6-env BOT_API_URL=http://localhost:8081' \
    's6-env HOME=/home/botuser' \
    's6-env XDG_CACHE_HOME=/home/botuser/.cache' \
    's6-env LOG_FILE_PATH=/data/app.log' \
    's6-env METRICS_ENABLED=true' \
    's6-env METRICS_PORT=9090' \
    'fdmove -c 2 1' \
    'cd /app' \
    '/bin/sh -c "export CONTAINER_START_MS=$(cat /tmp/container_start_ms 2>/dev/null || echo 0) && exec /app/doradura"' \
    > /etc/s6-overlay/s6-rc.d/doradura-bot/run && \
    chmod +x /etc/s6-overlay/s6-rc.d/doradura-bot/run

# === health-monitor longrun service ===
RUN echo "longrun" > /etc/s6-overlay/s6-rc.d/health-monitor/type && \
    touch /etc/s6-overlay/s6-rc.d/health-monitor/dependencies.d/doradura-bot && \
    printf '%s\n' \
    '#!/command/execlineb -P' \
    'foreground { echo "[health-monitor] START" }' \
    's6-setuidgid botuser' \
    's6-env RUST_LOG=info' \
    'fdmove -c 2 1' \
    '/app/health-monitor' \
    > /etc/s6-overlay/s6-rc.d/health-monitor/run && \
    chmod +x /etc/s6-overlay/s6-rc.d/health-monitor/run

# Enable all services
RUN touch /etc/s6-overlay/s6-rc.d/user/contents.d/init-data \
          /etc/s6-overlay/s6-rc.d/user/contents.d/telegram-bot-api \
          /etc/s6-overlay/s6-rc.d/user/contents.d/bgutil-pot-server \
          /etc/s6-overlay/s6-rc.d/user/contents.d/doradura-bot \
          /etc/s6-overlay/s6-rc.d/user/contents.d/health-monitor

EXPOSE 8080 8081 8082 9090

ENTRYPOINT ["/init"]
