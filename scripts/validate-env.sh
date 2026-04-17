#!/usr/bin/env bash
# Validate the current environment against .env.schema using varlock.
# Usable from pre-commit hooks, CI, or ad-hoc checks.
#
# Exits 0 on success, non-zero if:
#   - varlock is not installed
#   - .env.schema is missing
#   - any @required var is missing / any @type annotation fails to coerce

set -euo pipefail

SCHEMA=".env.schema"

if ! command -v varlock >/dev/null 2>&1; then
    echo "❌ varlock not installed. Get it at https://varlock.dev" >&2
    exit 2
fi

if [[ ! -f "$SCHEMA" ]]; then
    echo "❌ $SCHEMA not found" >&2
    exit 3
fi

# `varlock load` parses .env.schema + any .env / .env.{env} files and
# validates every declared var against its @type / @required annotation.
# Silent on success (only prints resolved table). Redirect on-success
# output so the hook stays quiet.
if varlock load --format=env >/dev/null; then
    echo "✅ Env schema validated ($(grep -cE '^[A-Z_][A-Z0-9_]*=' "$SCHEMA") declared vars)"
    exit 0
else
    echo "❌ Env schema validation failed — see varlock output above" >&2
    exit 1
fi
