#!/usr/bin/env bash
# Check for drift between env vars referenced in Rust code and declared in .env.schema.
#
# Fails if code reads `std::env::var("FOO")` where FOO is not declared in
# .env.schema. Used by CI to prevent "bot panics at runtime because Railway
# doesn't know about the new env var you added last week" incidents.
#
# Runs purely offline (grep-based). No varlock required.

set -euo pipefail

SCHEMA=".env.schema"

if [[ ! -f "$SCHEMA" ]]; then
    echo "❌ $SCHEMA not found" >&2
    exit 3
fi

# Extract env vars read by code. Matches both `std::env::var("FOO")` and
# `env::var("FOO")`. Only scans crates/*/src/ (production code), not tests
# or integration fixtures.
CODE_VARS=$(
    grep -rh 'std::env::var\|env::var(' crates/*/src/ 2>/dev/null \
        | grep -oE '"[A-Z_][A-Z0-9_]+"' \
        | tr -d '"' \
        | sort -u
)

# Extract vars declared in .env.schema (ignore comments).
SCHEMA_VARS=$(
    grep -E '^[A-Z_][A-Z0-9_]*=' "$SCHEMA" \
        | cut -d= -f1 \
        | sort -u
)

# Find vars in code but not in schema.
MISSING=$(comm -23 <(echo "$CODE_VARS") <(echo "$SCHEMA_VARS"))

# POSIX/system vars that are intentionally never in schema.
# HOME is standard POSIX; not configuration.
INTENTIONAL_EXCLUSIONS=(
    "HOME"
)

# Filter out intentional exclusions.
FILTERED_MISSING=""
for var in $MISSING; do
    excluded=false
    for excl in "${INTENTIONAL_EXCLUSIONS[@]}"; do
        if [[ "$var" == "$excl" ]]; then
            excluded=true
            break
        fi
    done
    if ! $excluded; then
        FILTERED_MISSING="$FILTERED_MISSING$var"$'\n'
    fi
done

FILTERED_MISSING=$(echo "$FILTERED_MISSING" | grep -v '^$' || true)

if [[ -n "$FILTERED_MISSING" ]]; then
    echo "❌ Env schema drift detected." >&2
    echo "" >&2
    echo "These env vars are read by code but NOT declared in $SCHEMA:" >&2
    echo "$FILTERED_MISSING" | sed 's/^/  - /' >&2
    echo "" >&2
    echo "Add each to $SCHEMA with appropriate annotations:" >&2
    echo "  # Description" >&2
    echo "  # @sensitive      (for secrets)" >&2
    echo "  # @type=number    (for numeric values)" >&2
    echo "  # @type=boolean   (for true/false)" >&2
    echo "  # @required       (if the bot cannot boot without it)" >&2
    echo "  VAR_NAME=" >&2
    exit 1
fi

CODE_COUNT=$(echo "$CODE_VARS" | wc -l | tr -d ' ')
SCHEMA_COUNT=$(echo "$SCHEMA_VARS" | wc -l | tr -d ' ')
echo "✅ No env schema drift ($CODE_COUNT vars in code, $SCHEMA_COUNT in schema)"
