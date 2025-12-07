#!/bin/bash

# Run bot in production environment

set -e

echo "🚀 Starting Doradura Bot in PRODUCTION mode"
echo "============================================"
echo ""
echo "Using configuration from .env"
echo "Bot Token: 6310079371:AAH***"
echo "Database: database.sqlite"
echo "Downloads: ~/downloads/dora-files"
echo ""

# Check if .env exists
if [ ! -f .env ]; then
    echo "❌ Error: .env not found"
    echo "Please create .env file from .env.example"
    exit 1
fi

# Export environment variables from .env
export $(grep -v '^#' .env | grep -v '^$' | xargs)

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
