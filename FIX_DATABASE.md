# 🛠️ Исправление базы данных на Railway

## Проблема
```
[ERROR] Failed to get user: no such column: send_as_document
```

База данных на Railway создана старой версией кода и не содержит новых колонок.

---

## ✅ РЕШЕНИЕ 1: Пересоздать БД (Рекомендуется)

### Способ A: Через Railway Dashboard

1. **Подключитесь к контейнеру:**
   - Railway Dashboard → Ваш проект
   - Deployments → Latest → три точки (⋮)
   - **"Open Shell"** или **"SSH"**

2. **Удалите старую БД:**
   ```bash
   rm -f /app/database.sqlite
   rm -f /app/database.sqlite-shm
   rm -f /app/database.sqlite-wal
   ```

3. **Перезапустите сервис:**
   - Settings → Restart Deployment

4. **БД создастся заново с правильной схемой**

### Способ B: Через переменную окружения

Добавьте временную переменную для пересоздания БД:

1. Railway Dashboard → Variables
2. Добавьте:
   ```
   Name: RESET_DATABASE
   Value: true
   ```

3. Обновите код для обработки этой переменной (см. ниже)

---

## ✅ РЕШЕНИЕ 2: Запустить миграции вручную

### 1. Добавьте скрипт миграции

Создайте файл `migrate_db.sh`:

```bash
#!/bin/bash
# Railway database migration script

DB_PATH="${DATABASE_URL:-/app/database.sqlite}"

echo "Running database migrations..."

# Подключаемся к БД и запускаем миграцию
sqlite3 "$DB_PATH" <<EOF
-- Add missing columns if they don't exist

-- Check and add send_as_document
ALTER TABLE users ADD COLUMN send_as_document INTEGER DEFAULT 0;

-- Check and add send_audio_as_document
ALTER TABLE users ADD COLUMN send_audio_as_document INTEGER DEFAULT 0;

-- Check and add subscription_expires_at
ALTER TABLE users ADD COLUMN subscription_expires_at DATETIME DEFAULT NULL;

-- Check and add telegram_charge_id
ALTER TABLE users ADD COLUMN telegram_charge_id TEXT DEFAULT NULL;

EOF

echo "Migrations completed!"
```

### 2. Запустите в Railway Shell

```bash
chmod +x migrate_db.sh
./migrate_db.sh
```

---

## ✅ РЕШЕНИЕ 3: Синхронизировать локальную БД с Railway

**НЕ РЕКОМЕНДУЕТСЯ для production**, но для тестирования:

### Вариант А: Экспорт/Импорт через SQL

1. **Локально экспортируйте схему:**
   ```bash
   sqlite3 database.sqlite .schema > schema.sql
   ```

2. **Добавьте в git:**
   ```bash
   git add schema.sql
   git commit -m "Add database schema"
   git push
   ```

3. **На Railway импортируйте:**
   ```bash
   # В Railway Shell
   sqlite3 /app/database.sqlite < schema.sql
   ```

### Вариант Б: Dockerfile с автоматической миграцией

Обновите `Dockerfile` чтобы всегда запускать миграции при старте:

```dockerfile
# В runtime stage, после COPY
COPY migration.sql ./

# Создайте скрипт запуска
RUN echo '#!/bin/bash\n\
# Initialize database if needed\n\
if [ ! -f /app/database.sqlite ]; then\n\
  sqlite3 /app/database.sqlite < /app/migration.sql\n\
fi\n\
\n\
# Run migrations\n\
sqlite3 /app/database.sqlite <<EOF\n\
-- Safely add missing columns\n\
ALTER TABLE users ADD COLUMN IF NOT EXISTS send_as_document INTEGER DEFAULT 0;\n\
ALTER TABLE users ADD COLUMN IF NOT EXISTS send_audio_as_document INTEGER DEFAULT 0;\n\
EOF\n\
\n\
# Start bot\n\
exec /app/doradura\n\
' > /app/start.sh && chmod +x /app/start.sh

CMD ["/app/start.sh"]
```

---

## 🎯 РЕКОМЕНДУЕМОЕ РЕШЕНИЕ

### Добавьте проверку миграций в код

Rust код уже имеет функцию `migrate_schema()` в `src/storage/db.rs`.

Проблема в том, что SQLite не поддерживает `ALTER TABLE ADD COLUMN IF NOT EXISTS`.

### Обновим migrate_schema:

Код уже правильный! Проблема в том, что **миграция НЕ запускается** для существующей БД.

**Решение:** Пересоздать БД на Railway.

---

## 🚀 БЫСТРОЕ ИСПРАВЛЕНИЕ (5 минут)

### Шаг 1: Добавьте скрипт в Dockerfile

Обновим Dockerfile для автоматического запуска миграций:

```dockerfile
# После COPY migration.sql ./
# Создаём startup script
RUN echo '#!/bin/bash\n\
set -e\n\
\n\
# Check if database exists\n\
if [ -f /app/database.sqlite ]; then\n\
  echo "Database exists, running migrations..."\n\
  # Миграции будут запущены в Rust коде\n\
else\n\
  echo "Creating new database..."\n\
  sqlite3 /app/database.sqlite < /app/migration.sql\n\
fi\n\
\n\
echo "Starting bot..."\n\
exec /app/doradura "$@"\n\
' > /app/entrypoint.sh && chmod +x /app/entrypoint.sh

CMD ["/app/entrypoint.sh"]
```

### Шаг 2: Или просто удалите БД на Railway

Самый простой способ:

1. **Railway Dashboard → Settings → Restart Deployment**

2. **Или в Shell:**
   ```bash
   rm /app/database.sqlite && exit
   ```

3. **Railway перезапустится и создаст новую БД**

---

## 📊 Проверка после исправления

В логах должно быть:

```
[INFO] Creating new database...
[INFO] Running migrations...
[INFO] Database initialized successfully
[INFO] Starting bot...
```

Без ошибок:
```
✅ No "no such column" errors
✅ Bot starts successfully
✅ /start command works
```

---

## 💾 Сохранение данных (если нужно)

Если в БД есть важные данные пользователей:

### 1. Экспортируйте данные:

```bash
# В Railway Shell
sqlite3 /app/database.sqlite <<EOF
.mode csv
.output /tmp/users_backup.csv
SELECT * FROM users;
.quit
EOF
```

### 2. Сохраните локально через `railway` CLI

```bash
railway run sqlite3 /app/database.sqlite .dump > backup.sql
```

### 3. После пересоздания БД импортируйте:

```bash
railway run sqlite3 /app/database.sqlite < backup.sql
```

---

## ⚠️ ВАЖНО

**НЕ добавляйте `database.sqlite` в git!**

База данных:
- Содержит пользовательские данные
- Может быть большой
- Должна быть в `.gitignore`

Вместо этого:
- ✅ Используйте `migration.sql` (уже в git)
- ✅ Используйте автоматические миграции в коде
- ✅ Используйте Railway Volumes для persistence

---

## 🎯 Итоговый план действий

**ВАРИАНТ 1 (Быстрый):**
1. Railway Dashboard → Open Shell
2. `rm /app/database.sqlite`
3. Settings → Restart Deployment
4. ✅ Готово!

**ВАРИАНТ 2 (Автоматический):**
1. Обновите Dockerfile (см. выше)
2. Commit & Push
3. Railway пересоберёт и всё исправит
4. ✅ Готово!

Рекомендую **Вариант 1** - быстрее и проще! 🚀
