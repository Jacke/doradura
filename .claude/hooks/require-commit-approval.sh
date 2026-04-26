#!/bin/bash
# Gate `git commit` / `git push` behind a fresh user-approval file.
#
# Workflow:
#   1. Claude proposes a commit and asks "Можно?"
#   2. User runs `!touch .claude/commit-approved` in chat (or any other
#      shell action that creates / refreshes the file). Doing it via
#      the `!`-prefix proves the user — not Claude — created the flag.
#   3. Hook sees file exists with mtime < TTL → allows commit + push
#      within the same window.
#   4. Old / missing file → block, with instructions to refresh.
#
# Window (TTL_SECONDS) is short on purpose: re-approval is cheap,
# stale "yes" from yesterday should not authorise today's deploy.
#
# Exit 0 = allow. Exit 2 = block + send reason to Claude.

set -e
INPUT=$(cat)
CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty')

if [[ -z "$CMD" ]]; then
  exit 0
fi

# Block --no-verify / --no-gpg-sign across ALL git subcommands
if [[ "$CMD" =~ --no-verify|--no-gpg-sign ]]; then
  echo "BLOCKED: '--no-verify' / '--no-gpg-sign' bypasses git hooks. Fix the underlying issue instead." >&2
  exit 2
fi

# Block force-push outright
if [[ "$CMD" =~ git[[:space:]]+push.*--force|git[[:space:]]+push.*-f([[:space:]]|$) ]]; then
  echo "BLOCKED: force-push is destructive. Ask user explicitly before proceeding." >&2
  exit 2
fi

# Block git reset --hard
if [[ "$CMD" =~ git[[:space:]]+reset[[:space:]]+(--hard|--keep) ]]; then
  echo "BLOCKED: 'git reset --hard' is destructive. Ask user explicitly before proceeding." >&2
  exit 2
fi

# Gate commit/push behind a fresh approval file.
if [[ "$CMD" =~ ^[[:space:]]*git[[:space:]]+(commit|push) ]] || \
   [[ "$CMD" =~ ([[:space:]]|^)git[[:space:]]+-C[[:space:]]+[^[:space:]]+[[:space:]]+(commit|push) ]]; then
  PROJECT_DIR="${CLAUDE_PROJECT_DIR:-$(pwd)}"
  APPROVAL="$PROJECT_DIR/.claude/commit-approved"
  TTL_SECONDS=600  # 10 minutes — covers commit + push + Railway-deploy verify

  if [[ -f "$APPROVAL" ]]; then
    # Cross-platform mtime: GNU `stat -c` first, fall back to BSD `stat -f`.
    MTIME=$(stat -c %Y "$APPROVAL" 2>/dev/null || stat -f %m "$APPROVAL" 2>/dev/null || echo 0)
    AGE=$(( $(date +%s) - MTIME ))
    if (( AGE < TTL_SECONDS )); then
      # Fresh approval — let the command through.
      exit 0
    fi
    REASON="approval file is $AGE s old (TTL $TTL_SECONDS s) — re-approve."
  else
    REASON="no approval file at $APPROVAL."
  fi

  echo "BLOCKED: '$CMD'" >&2
  echo "" >&2
  echo "Reason: $REASON" >&2
  echo "" >&2
  echo "CLAUDE.md rule: explicit user approval required for commit/push." >&2
  echo "" >&2
  echo "How to approve (in chat, with the '!' prefix so the touch comes from YOU,"
  echo "not Claude):" >&2
  echo "" >&2
  echo "  !touch .claude/commit-approved" >&2
  echo "" >&2
  echo "Approval is good for $((TTL_SECONDS / 60)) minutes and covers the next" >&2
  echo "git commit AND git push. Stale approvals expire automatically." >&2
  echo "Run 'rm .claude/commit-approved' to revoke early." >&2
  exit 2
fi

exit 0
