# Railway Deployment Troubleshooting

Решения распространённых проблем при деплое на Railway.

## 🔴 Bot Panic: teloxide-core/src/bot.rs:319:43

### Симптомы:
```
[ERROR] Panic caught: PanicHookInfo { payload: Any { .. },
  location: Location { file: ".../teloxide-core/src/bot.rs", line: 319, col: 43 }
```

### Причина:
Неверный или отсутствующий `TELOXIDE_TOKEN`.

### Решение:

1. **Проверьте токен в Railway Dashboard:**
   ```
   Railway Dashboard → Variables → TELOXIDE_TOKEN
   ```

2. **Убедитесь, что токен правильный:**
   - Формат: `123456789:ABCdefGHIjklMNOpqrsTUVwxyz`
   - Получите новый от [@BotFather](https://t.me/BotFather) если потерян

3. **Обновите токен через CLI:**
   ```bash
   railway variables --set "TELOXIDE_TOKEN=YOUR_NEW_TOKEN"
   ```

4. **Или через Dashboard:**
   - Variables → Edit TELOXIDE_TOKEN
   - Вставьте новый токен
   - Save
   - Railway автоматически перезапустит сервис

---

## ⚠️ YouTube Downloads Fail

### Симптомы:
```
[ERROR] ❌ NO COOKIES CONFIGURED - YouTube downloads will FAIL!
```

### Причина:
Отсутствуют cookies для YouTube аутентификации.

### Решение:

**Вариант 1: Добавить файл cookies (Рекомендуется)**

Файл `youtube_cookies.txt` уже в репозитории, но Railway нужно указать где его искать:

```bash
railway variables --set "YTDL_COOKIES_FILE=youtube_cookies.txt"
```

**Вариант 2: Использовать браузер для извлечения cookies**

```bash
railway variables --set "YTDL_COOKIES_BROWSER=chrome"
```

Требует установки дополнительных зависимостей (уже в Dockerfile):
- `keyring`
- `pycryptodomex`

**Вариант 3: Обновить cookies вручную**

1. Экспортируйте свежие cookies из браузера:
   - Chrome Extension: "Get cookies.txt LOCALLY"
   - Firefox Extension: "cookies.txt"

2. Сохраните как `youtube_cookies.txt`

3. Закоммитьте и запушьте:
   ```bash
   git add youtube_cookies.txt
   git commit -m "Update YouTube cookies"
   git push
   ```

---

## 🔧 Build Fails

### Cargo.lock version error
```
lock file version 4 was found, but this version of Cargo does not understand this lock file
```

**Решение:** Обновлен в commit `c257ecb` (Rust 1.75 → 1.83)

### Missing c_code directory
```
cc1: fatal error: c_code/foo.c: No such file or directory
```

**Решение:** Исправлено в commit `f2b742c` (добавлено `COPY c_code ./c_code`)

### Cannot find -lsqlite3
```
/usr/bin/ld: cannot find -lsqlite3: No such file or directory
```

**Решение:** Исправлено в commit `a870bdf` (добавлен `libsqlite3-dev`)

---

## 💾 Database Not Persisting

### Симптомы:
База данных сбрасывается после каждого редеплоя.

### Решение:

1. **Создайте Volume в Railway:**
   - Railway Dashboard → Settings → Volumes
   - Add Volume

2. **Настройте mount path:**
   - Mount Path: `/app`
   - Size: 1 GB (достаточно для SQLite)

3. **Сохраните и переdeployте**

Теперь `database.sqlite` будет сохраняться между деплоями.

---

## 📡 Bot Not Responding

### Проверочный список:

1. **Проверьте статус деплоя:**
   ```
   Railway Dashboard → Deployments
   ```
   Статус должен быть "Active" ✅

2. **Проверьте логи:**
   ```
   Railway Dashboard → View Logs
   ```
   Ищите:
   ```
   ✅ Starting bot...
   ✅ Bot username: @yourbot
   ✅ Starting bot in long polling mode
   ```

3. **Проверьте переменные окружения:**
   ```bash
   railway variables
   ```
   Должна быть минимум `TELOXIDE_TOKEN`

4. **Проверьте в Telegram:**
   - Найдите бота по username
   - Отправьте `/start`
   - Если не отвечает - проверьте логи на ошибки

---

## 🔄 Deploy Stuck

### Симптомы:
Деплой висит более 20 минут.

### Решение:

1. **Отмените текущий деплой:**
   - Railway Dashboard → Deployments → Cancel

2. **Очистите кэш и редеплойте:**
   - Settings → Clear Cache
   - Redeploy

3. **Проверьте лимиты:**
   - Settings → Resource Limits
   - Увеличьте Memory если нужно

---

## 🚫 Out of Memory

### Симптомы:
```
[ERROR] Out of memory (OOM)
Container killed
```

### Решение:

**Временное:**
```bash
railway variables --set "CARGO_BUILD_JOBS=1"
```
Ограничивает параллельную компиляцию.

**Постоянное:**
- Railway Dashboard → Settings
- Увеличьте Memory Limit
- Минимум рекомендуемый: 2GB для сборки, 512MB для работы

---

## 📝 Environment Variables

### Основные переменные:

| Переменная | Обязательная | Описание |
|-----------|--------------|----------|
| `TELOXIDE_TOKEN` | ✅ | Telegram Bot Token |
| `YTDL_COOKIES_FILE` | ❌ | Путь к cookies файлу |
| `YTDL_COOKIES_BROWSER` | ❌ | Браузер для извлечения cookies |
| `ADMIN_IDS` | ❌ | Telegram User IDs админов |
| `WEBAPP_PORT` | ❌ | Порт для Mini App |
| `WEBAPP_URL` | ❌ | URL для Mini App |

### Установка через CLI:

```bash
# Основное
railway variables --set "TELOXIDE_TOKEN=your_token"

# YouTube
railway variables --set "YTDL_COOKIES_FILE=youtube_cookies.txt"

# Админ
railway variables --set "ADMIN_IDS=123456789"

# Mini App
railway variables --set "WEBAPP_PORT=8080"
railway variables --set "WEBAPP_URL=https://your-project.railway.app"
```

---

## 🔍 Debug Mode

Для включения детальных логов:

```bash
railway variables --set "RUST_LOG=debug"
```

Или для конкретного модуля:
```bash
railway variables --set "RUST_LOG=doradura=debug,teloxide=info"
```

---

## 📞 Получить помощь

1. **Проверьте логи:**
   ```bash
   railway logs | tail -100
   ```

2. **Проверьте статус:**
   ```bash
   railway status
   ```

3. **Перезапустите сервис:**
   ```bash
   railway restart
   ```

4. **Откройте issue:**
   - [GitHub Issues](https://github.com/Jacke/doradura/issues)
   - Приложите логи и описание проблемы

---

## ✅ Checklist для успешного деплоя

- [ ] Rust 1.83+ в Dockerfile
- [ ] `libsqlite3-dev` в зависимостях
- [ ] `c_code/` директория копируется
- [ ] `TELOXIDE_TOKEN` установлен
- [ ] `youtube_cookies.txt` в репозитории
- [ ] `YTDL_COOKIES_FILE` переменная установлена
- [ ] Volume создан для database.sqlite
- [ ] Memory limit минимум 2GB для сборки
- [ ] Логи показывают "Starting bot in long polling mode"
- [ ] Бот отвечает на `/start` в Telegram

---

**Если проблема не решена - проверьте полные логи и создайте issue!** 🛠️
