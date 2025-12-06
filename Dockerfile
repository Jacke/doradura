# Build stage
FROM rust:1.83-slim as builder

# Install system dependencies required for building
RUN apt-get update && apt-get install -y \
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
COPY migration.sql ./

# Build the application
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    ffmpeg \
    python3 \
    python3-pip \
    sqlite3 \
    wget \
    && rm -rf /var/lib/apt/lists/*

# Install yt-dlp
RUN wget https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp -O /usr/local/bin/yt-dlp && \
    chmod a+rx /usr/local/bin/yt-dlp

# Install Python dependencies for browser cookie extraction (optional)
RUN pip3 install --no-cache-dir --break-system-packages keyring pycryptodomex

WORKDIR /app

# Copy the compiled binary from builder
COPY --from=builder /app/target/release/doradura /app/doradura

# Copy migration script (for fallback)
COPY migration.sql ./

# Copy database from git (contains all users and settings)
COPY database.sqlite ./

# Create startup script
RUN echo '#!/bin/bash\n\
set -e\n\
\n\
echo "Database initialization..."\n\
\n\
# Database is already copied from git\n\
if [ -f /app/database.sqlite ]; then\n\
  echo "✅ Using database from git repository"\n\
  echo "   Database synced with local development"\n\
else\n\
  echo "⚠️  Database not found, creating from migration.sql..."\n\
  sqlite3 /app/database.sqlite < /app/migration.sql\n\
  echo "✅ Database created from migration"\n\
fi\n\
\n\
# Run any pending migrations from Rust code\n\
echo "Ready to start bot (migrations will run if needed)"\n\
\n\
echo "Starting bot..."\n\
exec /app/doradura "$@"\n\
' > /app/entrypoint.sh && chmod +x /app/entrypoint.sh

# Create necessary directories
RUN mkdir -p downloads logs backups

# Create non-root user
RUN useradd -m -u 1000 botuser && \
    chown -R botuser:botuser /app

USER botuser

# Expose port for webapp (optional)
EXPOSE 8080

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=40s --retries=3 \
    CMD pgrep doradura || exit 1

# Run the bot via entrypoint script
CMD ["/app/entrypoint.sh"]
