# Build stage
FROM rust:1.85-slim AS builder

# Install system dependencies required for building
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    libsqlite3-dev \
    build-essential \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy manifests
COPY Cargo.toml Cargo.lock ./
COPY build.rs ./

# Copy C code for build.rs
COPY c_code ./c_code

# Copy source code
COPY src ./src
COPY locales ./locales
COPY migrations ./migrations
COPY migration.sql ./

# Build the application
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
# hadolint ignore=DL3008
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    ffmpeg \
    gosu \
    python3 \
    python3-pip \
    sqlite3 \
    wget \
    && rm -rf /var/lib/apt/lists/*

# Install yt-dlp
RUN wget --progress=dot:giga https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp -O /usr/local/bin/yt-dlp && \
    chmod a+rx /usr/local/bin/yt-dlp

# Install Python dependencies for browser cookie extraction (optional)
# hadolint ignore=DL3013
RUN pip3 install --no-cache-dir --break-system-packages keyring pycryptodomex

WORKDIR /app

# Copy the compiled binary from builder
COPY --from=builder /app/target/release/doradura /app/doradura

# Copy migration script (for fallback)
COPY migration.sql ./

# Create startup script
RUN cat <<'EOF' > /app/entrypoint.sh && chmod +x /app/entrypoint.sh
#!/bin/bash
set -e

if [ "$(id -u)" -eq 0 ]; then
  if [ -d /data ]; then
    chown -R botuser:botuser /data || true
  fi
fi

echo "Database initialization..."

# Use DATABASE_URL if provided, else default to /data/database.sqlite
DB_PATH=${DATABASE_URL:-/data/database.sqlite}
DB_DIR=$(dirname "$DB_PATH")

# Ensure directory exists and has correct permissions
mkdir -p "$DB_DIR" || true
chmod 755 "$DB_DIR" 2>/dev/null || true
chown -R botuser:botuser "$DB_DIR" 2>/dev/null || true

echo "🔍 Testing write permissions for $DB_DIR..."
if ! touch "$DB_DIR/.rw_test" 2>/dev/null; then
  echo "⚠️  Database directory not writable: $DB_DIR"
  echo "Directory info:"
  ls -lah "$DB_DIR" || echo "Directory does not exist"
  echo "Current user: $(whoami)"
  echo "Falling back to /app/database.sqlite"
  DB_PATH="/app/database.sqlite"
  DB_DIR="/app"
  mkdir -p "$DB_DIR"
  chmod 755 "$DB_DIR"
fi
rm -f "$DB_DIR/.rw_test"

echo "🔍 Checking for database at: $DB_PATH"
ls -lah "$DB_DIR" 2>/dev/null || echo "Directory $DB_DIR is empty or does not exist"

if [ -f "$DB_PATH" ]; then
  echo "✅ Using existing database at $DB_PATH"
  echo "📊 Database file size: $(du -h "$DB_PATH" | cut -f1)"
  echo "📅 Last modified: $(stat -c %y "$DB_PATH" 2>/dev/null || stat -f "%Sm" "$DB_PATH" 2>/dev/null || echo "unknown")"
else
  echo "⚠️  Database not found, creating from migration.sql at $DB_PATH..."
  sqlite3 "$DB_PATH" < /app/migration.sql
  echo "✅ Database created at $DB_PATH"
fi

export DATABASE_URL="$DB_PATH"

# Write YouTube cookies from env if provided (base64)
if [ -n "$YTDL_COOKIES_B64" ]; then
  COOKIES_PATH=${YTDL_COOKIES_FILE:-/data/youtube_cookies.txt}
  COOKIES_DIR=$(dirname "$COOKIES_PATH")
  mkdir -p "$COOKIES_DIR"
  echo "$YTDL_COOKIES_B64" | base64 -d > "$COOKIES_PATH"
  chown botuser:botuser "$COOKIES_PATH"
  chmod 644 "$COOKIES_PATH"
  export YTDL_COOKIES_FILE="$COOKIES_PATH"
  echo "✅ Wrote YouTube cookies to $COOKIES_PATH"
fi

# Run any pending migrations from Rust code
echo "Ready to start bot (migrations will run if needed)"

echo "Starting bot..."
exec gosu botuser /app/doradura "$@"
EOF

# Create necessary directories and non-root user
RUN mkdir -p downloads logs backups /data && \
    useradd -m -u 1000 botuser && \
    chown -R botuser:botuser /app /data

# Expose port for webapp (optional)
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=40s --retries=3 \
    CMD pgrep doradura || exit 1

# Run the bot via entrypoint script
CMD ["/app/entrypoint.sh"]
