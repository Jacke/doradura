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

echo "Database initialization..."

# Use DATABASE_URL if provided, else default to /data/database.sqlite
DB_PATH=${DATABASE_URL:-/data/database.sqlite}
DB_DIR=$(dirname "$DB_PATH")
mkdir -p "$DB_DIR"

if [ -f "$DB_PATH" ]; then
  echo "✅ Using existing database at $DB_PATH"
else
  echo "⚠️  Database not found, creating from migration.sql at $DB_PATH..."
  sqlite3 "$DB_PATH" < /app/migration.sql
  echo "✅ Database created"
fi

export DATABASE_URL="$DB_PATH"

# Run any pending migrations from Rust code
echo "Ready to start bot (migrations will run if needed)"

echo "Starting bot..."
exec /app/doradura "$@"
EOF

# Create necessary directories and non-root user
RUN mkdir -p downloads logs backups && \
    useradd -m -u 1000 botuser && \
    chown -R botuser:botuser /app

USER botuser

# Expose port for webapp (optional)
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=40s --retries=3 \
    CMD pgrep doradura || exit 1

# Run the bot via entrypoint script
CMD ["/app/entrypoint.sh"]
