# 🚀 Staging Environment – Quick Start

## What’s new
Two bots now:
1. **Production** — main bot for users.
2. **Staging** — test bot for new features.

## How to run
### Production bot
```bash
./run_production.sh
```

### Staging bot
```bash
./scripts/run_staging.sh
```

## Why
**Before:**
```
Code → Commit → Push → Railway deploys → 😱 Bug hits users
```
**Now:**
```
Code → Test on staging → Works → Push → Railway → ✅ Users happy
```
