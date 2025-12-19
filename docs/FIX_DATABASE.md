# 🛠️ Fixing the Railway Database

## Problem
```
[ERROR] Failed to get user: no such column: send_as_document
```
The Railway database was created from an older version and is missing new columns.

---

## ✅ Solution 1: Recreate the DB (Recommended)

### Method A: Via Railway Dashboard
1. **Open a shell in the container:**
   - Railway Dashboard → your project
   - Deployments → Latest → kebab menu (⋮)
   - **Open Shell** / **SSH**
2. **Delete the old DB:**
   ```bash
   rm -f /app/database.sqlite
   rm -f /app/database.sqlite-shm
   rm -f /app/database.sqlite-wal
   ```
3. **Restart the service:** Settings → Restart Deployment
4. **A fresh DB will be created with the correct schema.**

### Method B: Via an env var
Add a temporary env var to force recreation:
1. Railway Dashboard → Variables
2. Add:
   ```
   Name: RESET_DATABASE
   Value: true
   ```
3. Update code to act on this variable (see below).

---

## ✅ Solution 2: Run migrations manually

### 1) Add a migration script
Create `migrate_db.sh`:
```bash
#!/bin/bash
# Railway database migration script
DB_PATH="${DATABASE_URL:-/app/database.sqlite}"

echo "Running database migrations..."

sqlite3 "$DB_PATH" <<EOSQL
-- Add missing columns if they don't exist
ALTER TABLE users ADD COLUMN send_as_document INTEGER DEFAULT 0;
ALTER TABLE users ADD COLUMN send_audio_as_document INTEGER DEFAULT 0;
ALTER TABLE users ADD COLUMN subscription_expires_at DATETIME DEFAULT NULL;
ALTER TABLE users ADD COLUMN telegram_charge_id TEXT DEFAULT NULL;
EOSQL

echo "Migrations completed!"
```

### 2) Run in Railway Shell
```bash
chmod +x migrate_db.sh
./migrate_db.sh
```

---

## ✅ Solution 3: Sync local DB to Railway
**Not recommended for production**, but acceptable for testing.

### Option A: Export/Import via SQL
1. **Export schema locally:**
   ```bash
   sqlite3 database.sqlite .schema > schema.sql
   ```
2. **Commit it:**
   ```bash
   git add schema.sql
   git commit -m "Add database schema"
   git push
   ```
3. **Import on Railway:**
   ```bash
   sqlite3 /app/database.sqlite < schema.sql
   ```

### Option B: Dockerfile with auto-migration
Update `Dockerfile` to always run migrations at startup:
```dockerfile
# In runtime stage, after COPY
COPY migration.sql ./

RUN echo '#!/bin/bash\n\
if [ ! -f /app/database.sqlite ]; then\n\
  sqlite3 /app/database.sqlite < /app/migration.sql\n\
fi\n\
sqlite3 /app/database.sqlite <<EOSQL\n\
ALTER TABLE users ADD COLUMN IF NOT EXISTS send_as_document INTEGER DEFAULT 0;\n\
ALTER TABLE users ADD COLUMN IF NOT EXISTS send_audio_as_document INTEGER DEFAULT 0;\n\
EOSQL\n\
exec /app/doradura\n\
' > /app/start.sh && chmod +x /app/start.sh

CMD ["/app/start.sh"]
```

---

## 🎯 Recommended path
Rust already has `migrate_schema()` in `src/storage/db.rs`. SQLite, however, does not support `ALTER TABLE ADD COLUMN IF NOT EXISTS`—so existing DBs miss the new columns.

**Best fix:** recreate the DB on Railway.

---

## 🚀 Quick fix (5 minutes)

### Step 1: Add a startup script in Dockerfile
```dockerfile
RUN echo '#!/bin/bash\n\
set -e\n\
if [ -f /app/database.sqlite ]; then\n\
  echo "Database exists, running migrations..."\n\
else\n\
  echo "Creating new database..."\n\
  sqlite3 /app/database.sqlite < /app/migration.sql\n\
fi\n\
exec /app/doradura "$@"\n\
' > /app/entrypoint.sh && chmod +x /app/entrypoint.sh

CMD ["/app/entrypoint.sh"]
```

### Step 2: Or simply delete the DB on Railway
1. Railway Dashboard → Settings → Restart Deployment
2. Or in Shell:
   ```bash
   rm /app/database.sqlite && exit
   ```
3. Railway restarts and creates a fresh DB.

---

## 📊 Post-fix check
Logs should show:
```
[INFO] Creating new database...
[INFO] Running migrations...
[INFO] Database initialized successfully
[INFO] Starting bot...
```
And no errors like "no such column".

---

## 💾 If you must keep existing data
1. **Export:**
   ```bash
   sqlite3 /app/database.sqlite <<EOSQL
.mode csv
.output /tmp/users_backup.csv
SELECT * FROM users;
.quit
EOSQL
   ```
2. **Backup via CLI:** `railway run sqlite3 /app/database.sqlite .dump > backup.sql`
3. **Restore after recreation:** `railway run sqlite3 /app/database.sqlite < backup.sql`

---

## ⚠️ Important
Do **not** commit `database.sqlite`.
- Contains user data
- Can be large
- Already git-ignored

Use `migration.sql` + code-based migrations + Railway volumes instead.

---

## 🎯 Final action plan
**Option 1 (fast):**
1) Railway Dashboard → Open Shell
2) `rm /app/database.sqlite`
3) Restart Deployment
4) ✅ Done

**Option 2 (automatic):**
1) Update Dockerfile (see above)
2) Commit & push
3) Railway rebuilds and fixes itself
4) ✅ Done

Recommended: **Option 1** for speed. 🚀
