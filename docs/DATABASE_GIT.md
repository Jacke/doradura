# 💾 Database in Git

## Concept
The `database.sqlite` file now lives in git and syncs automatically between:
- Local machine (development)
- Railway (production)

## ✅ Benefits
1. **Data sync** — users/settings stay aligned everywhere.
2. **Versioning** — roll back to previous versions if needed.
3. **Backups** — git history holds DB snapshots.
4. **Simplicity** — no extra storage setup.

## 📋 DB files
| File | In Git? | Description |
|------|---------|-------------|
| `database.sqlite` | ✅ Yes | Main DB |
| `database.sqlite-shm` | ❌ No | Shared memory (temp) |
| `database.sqlite-wal` | ❌ No | Write-Ahead Log (temp) |

## 🔄 Workflow

### Local development
1. Make DB changes (add users, adjust settings).
2. Commit the DB:
   ```bash
   git add database.sqlite
   git commit -m "chore: update database with new users"
   git push
   ```
3. Railway picks up the updated DB automatically.

### On Railway
1. Railway deploys new code.
2. Dockerfile copies `database.sqlite` from git.
3. The bot uses the current DB version.

## 📝 Examples

### Add a test user locally
```bash
cargo run
# Send /start to the bot
# User is added to DB

git add database.sqlite
git commit -m "chore: add test user to database"
git push
```
Railway receives the updated DB on next deploy.

### Update user settings
```bash
# Change settings via the bot (format, quality, etc.)
git add database.sqlite
git commit -m "chore: update user preferences"
git push
```

### Sync DB with Railway
```bash
git pull   # get changes made remotely
# make local changes
git push
```

## ⚠️ Notes

### 1) File size
SQLite grows over time. Current size example: **147KB**.
- ✅ Up to 10MB — fine for git.
- ⚠️ 10–50MB — acceptable; monitor.
- ❌ >50MB — consider alternatives.

Check size:
```bash
ls -lh database.sqlite
```
Clean old data:
```bash
sqlite3 database.sqlite "DELETE FROM download_history WHERE downloaded_at < datetime('now', '-30 days')"
sqlite3 database.sqlite "VACUUM"
```

### 2) Merge conflicts
If DB changes locally and on Railway:
```bash
git pull
# If conflict:
# Option 1: keep local
git checkout --ours database.sqlite
# Option 2: keep remote
git checkout --theirs database.sqlite
# Option 3: export/import as needed
```

### 3) Binary file
SQLite is binary; git diff is not readable.
View contents:
```bash
sqlite3 database.sqlite ".tables"
sqlite3 database.sqlite "SELECT * FROM users"
sqlite3 database.sqlite .dump > database_backup.sql
```

## 🔧 Utilities
- Export to SQL:
  ```bash
  sqlite3 database.sqlite .dump > database.sql
  ```
- Import from SQL:
  ```bash
  sqlite3 database.sqlite < database.sql
  ```
- Create backup with timestamp:
  ```bash
  cp database.sqlite "backups/database_$(date +%Y%m%d_%H%M%S).sqlite"
  sqlite3 database.sqlite .dump > "backups/database_$(date +%Y%m%d_%H%M%S).sql"
  ```
- Check schema:
  ```bash
  sqlite3 database.sqlite .schema
  ```

## 🚀 Deploy with updated DB
### Automatic (recommended)
```bash
git add database.sqlite
git commit -m "chore: update database"
git push
```
Railway deploys with the new DB.

### Manual
```bash
sqlite3 database.sqlite .dump > temp_db.sql
railway run sqlite3 /app/database.sqlite < temp_db.sql
railway restart
```

## 📊 Monitor DB size
- Local:
  ```bash
  du -h database.sqlite
  sqlite3 database.sqlite "SELECT COUNT(*) FROM users"
  sqlite3 database.sqlite "SELECT COUNT(*) FROM download_history"
  ```
- Railway logs on startup can show size/counts.

## 🔒 Security
Do **not** store secrets, passwords, or payment data in a git-tracked DB.
Safe to store: Telegram user IDs, user settings, download history (if not sensitive).

## 📈 If DB grows too large
- **Git LFS:**
  ```bash
  git lfs track "*.sqlite"
  git add .gitattributes
  git commit -m "chore: use git-lfs for database"
  ```
- **Railway volumes:** store DB at `/app/data/database.sqlite` and mount a volume.
- **External DB:** consider PostgreSQL/MySQL/managed DB for production.

## ✅ Current status
- DB size: ~147KB
- In git: ✅
- Auto-sync: ✅
- Backups: ✅ via git history

## 🎯 Best practices
1. Commit the DB regularly with descriptive messages.
2. Clean old data monthly (`DELETE` old history + `VACUUM`).
3. Watch file size; switch strategies if it exceeds ~10–50MB.

**The database is now versioned!** 🎉
