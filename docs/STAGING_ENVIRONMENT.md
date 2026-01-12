# 🧪 Staging Environment

## Concept
Staging is a separate bot instance for testing new features before releasing to production.

## 📊 Test vs Staging vs Canary
| Type | When to use | Who uses it | Data |
|------|-------------|-------------|------|
| **Test** | Automated tests, CI/CD | Developers | Fake/mocked |
| **Staging** | Manual testing of new features | Devs + QA | Realistic data |
| **Canary** | Gradual rollout to % of users | Production users | Production data |

For this bot:
- **Production bot:** main token for all users
- **Staging bot:** test token for validating new features

## 🚀 How to use Staging

### 1) Run staging bot locally
```bash
./scripts/run_staging.sh
```
Or manually:
```bash
export $(grep -v '^#' .env.staging | xargs)
cargo run --release
```

### 2) Deploy staging to Railway
- Use a separate Railway project or service.
- Set env vars from `.env.staging`.
- Ensure DB is separate from production (different volume/file).

### 3) Test checklist
- Verify /start, /mode, downloads, Mini App, subscriptions (if enabled).
- Check logs for errors.
- Keep cookies fresh if testing YouTube.

## 🎯 Best practices
- Never point staging at the production database.
- Use a distinct bot token and admin IDs.
- Clean up staging DB periodically.
- Clearly label staging bot to avoid user confusion.
