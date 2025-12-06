# Railway Deployment Guide

Это руководство поможет вам задеплоить Telegram бота Doradura на Railway.

## Предварительные требования

1. Аккаунт на [Railway](https://railway.app)
2. Railway CLI установлен локально
3. Telegram Bot Token от [@BotFather](https://t.me/BotFather)
4. YouTube cookies (опционально, но рекомендуется)

## Шаг 1: Авторизация в Railway

```bash
# Войдите в аккаунт Railway
railway login
```

Или используйте токен напрямую:
```bash
export RAILWAY_TOKEN=your_railway_token_here
```

## Шаг 2: Инициализация проекта

```bash
# Создайте новый проект или подключитесь к существующему
railway init
```

Или создайте проект программно:
```bash
railway project create doradura-bot
```

## Шаг 3: Настройка переменных окружения

### Обязательные переменные:

```bash
# Telegram Bot Token
railway variables --set "TELOXIDE_TOKEN=your_bot_token_here"
```

### Опциональные, но рекомендуемые переменные:

```bash
# YouTube cookies для доступа к YouTube (ВАЖНО!)
railway variables --set "YTDL_COOKIES_FILE=youtube_cookies.txt"

# Или используйте браузер для извлечения cookies
railway variables --set "YTDL_COOKIES_BROWSER=chrome"

# Admin user IDs (для административных команд)
railway variables --set "ADMIN_IDS=your_telegram_id"

# Mini App configuration (если нужен веб-интерфейс)
railway variables --set "WEBAPP_PORT=8080"
railway variables --set "WEBAPP_URL=https://your-domain.railway.app"
```

### Настройка базы данных:

Railway автоматически создаст том (volume) для SQLite базы данных. Если хотите использовать постоянное хранилище:

```bash
# База данных будет храниться в контейнере по умолчанию
# Для постоянного хранения Railway создаст volume автоматически
```

## Шаг 4: Настройка YouTube cookies

Если у вас есть файл `youtube_cookies.txt`:

```bash
# Закодируйте файл cookies в base64
base64 youtube_cookies.txt > cookies_base64.txt

# Установите как переменную окружения
railway variables --set "YOUTUBE_COOKIES_BASE64=$(cat cookies_base64.txt)"
```

Затем добавьте в `Dockerfile` (уже добавлено):
```dockerfile
# Декодирование cookies из base64 при старте
RUN if [ ! -z "$YOUTUBE_COOKIES_BASE64" ]; then \
      echo "$YOUTUBE_COOKIES_BASE64" | base64 -d > youtube_cookies.txt; \
    fi
```

## Шаг 5: Деплой

```bash
# Задеплойте проект
railway up

# Или используйте Git push (если подключен GitHub)
git add .
git commit -m "Deploy to Railway"
git push
```

## Шаг 6: Мониторинг

```bash
# Просмотр логов
railway logs

# Проверка статуса
railway status

# Открыть dashboard
railway open
```

## Переменные окружения (полный список)

| Переменная | Описание | Обязательная | Пример |
|-----------|----------|--------------|--------|
| `TELOXIDE_TOKEN` | Telegram Bot Token | ✅ | `123456:ABC-DEF1234ghIkl-zyx57W2v1u123ew11` |
| `YTDL_COOKIES_FILE` | Путь к файлу cookies | ❌ | `youtube_cookies.txt` |
| `YTDL_COOKIES_BROWSER` | Браузер для извлечения cookies | ❌ | `chrome`, `firefox` |
| `YTDL_BIN` | Путь к yt-dlp | ❌ | `yt-dlp` (по умолчанию) |
| `DOWNLOAD_FOLDER` | Папка для загрузок | ❌ | `~/downloads` |
| `ADMIN_IDS` | Admin user IDs | ❌ | `123456789,987654321` |
| `WEBAPP_PORT` | Порт для Mini App | ❌ | `8080` |
| `WEBAPP_URL` | URL для Mini App | ❌ | `https://bot.railway.app` |
| `BOT_API_URL` | Custom Bot API URL | ❌ | `http://localhost:8081` |
| `WEBHOOK_URL` | Webhook URL | ❌ | `https://bot.railway.app/webhook` |

## Troubleshooting

### Бот не отвечает на сообщения

1. Проверьте логи: `railway logs`
2. Убедитесь, что `TELOXIDE_TOKEN` установлен правильно
3. Проверьте статус деплоя: `railway status`

### YouTube downloads не работают

1. Проверьте, что cookies настроены правильно
2. Убедитесь, что ffmpeg установлен (уже в Dockerfile)
3. Проверьте логи: `railway logs | grep -i youtube`

### База данных теряется после редеплоя

1. Убедитесь, что используете Railway Volume для постоянного хранения
2. Настройте volume в Railway Dashboard: Project Settings → Volumes

### Out of Memory ошибки

1. Увеличьте лимит памяти в Railway Dashboard
2. Уменьшите `MAX_CONCURRENT_DOWNLOADS` в конфигурации

## Рекомендации по production

1. **Всегда используйте cookies** для YouTube downloads
2. **Настройте persistent volume** для базы данных
3. **Мониторьте логи** регулярно
4. **Настройте backups** базы данных
5. **Используйте webhook** вместо long polling для лучшей производительности

## Обновление бота

```bash
# Пересоберите и задеплойте
railway up --detach

# Или через git
git push
```

## Удаление проекта

```bash
railway project delete
```

## Полезные ссылки

- [Railway Documentation](https://docs.railway.app)
- [Doradura GitHub](https://github.com/Jacke/doradura)
- [Telegram Bot API](https://core.telegram.org/bots/api)
