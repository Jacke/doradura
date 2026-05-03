#!/bin/bash
# cargo-sweep wrapper — clean target/incremental files older than 14 days.
#
# Why: `target/` grows unbounded across feature branches; we've seen it hit
# 40+ GB on a long-lived workspace. cargo-sweep walks build artifacts and
# deletes anything not touched in the last N days. Safe — Cargo just
# rebuilds whatever's missing on next `cargo build`.
#
# Usage (manual):
#   ./scripts/cargo-sweep.sh                    # default: 14 days
#   ./scripts/cargo-sweep.sh 7                  # custom retention (days)
#
# Usage (cron, weekly Sunday 04:00):
#   crontab -e
#   0 4 * * 0  cd /Users/stan/Dev/_PROJ/doradura && ./scripts/cargo-sweep.sh >> /tmp/cargo-sweep.log 2>&1
#
# Install once:
#   cargo install cargo-sweep
#
# Production note: this script is dev-only — Railway/CI environments
# don't keep target/ between builds, so no cron there.

set -euo pipefail

DAYS="${1:-14}"

if ! command -v cargo-sweep >/dev/null 2>&1; then
    echo "❌ cargo-sweep not installed. Run: cargo install cargo-sweep"
    exit 1
fi

echo "🧹 Sweeping target/ files older than ${DAYS} days..."
SIZE_BEFORE=$(du -sh target 2>/dev/null | cut -f1 || echo "?")

cargo sweep --time "${DAYS}"

SIZE_AFTER=$(du -sh target 2>/dev/null | cut -f1 || echo "?")
echo "✅ Done. target/ size: ${SIZE_BEFORE} → ${SIZE_AFTER}"
