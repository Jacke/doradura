#!/bin/bash

# Run bot in staging environment for testing new features

set -e

echo "🚀 Starting Doradura Bot in STAGING mode"
echo "=========================================="
echo ""
echo "Using configuration from .env.staging"
echo "Bot Token: 8224275354:AAF***"
echo "Database: database_staging.sqlite"
echo "Downloads: ~/downloads/dora-staging"
echo ""

# Check if .env.staging exists
if [ ! -f .env.staging ]; then
    echo "❌ Error: .env.staging not found"
    echo "Please create .env.staging file"
    exit 1
fi

# Export environment variables from .env.staging
export $(grep -v '^#' .env.staging | xargs)

# Check if cargo is installed
if ! command -v cargo &> /dev/null; then
    echo "❌ Error: cargo not found"
    echo "Please install Rust: https://rustup.rs"
    exit 1
fi

echo "✅ Environment loaded"
echo "✅ Building and running bot..."
echo ""

# Build and run
cargo run --release

echo ""
echo "🛑 Bot stopped"
