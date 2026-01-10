# Railway Deployment Guide для Telegram Bot API с Persistent Storage

## Обзор

Этот гайд поможет настроить Local Telegram Bot API Server на Railway с **persistent volume** для хранения файлов размером до 2GB.

## Что даёт persistent storage?

✅ Файлы до **2GB** (вместо 20MB лимита официального API)
✅ Файлы **сохраняются** между перезапусками
✅ **Быстрый доступ** к файлам через прямое копирование
✅ **Fallback** на api.telegram.org при проблемах

## Стоимость

Railway Volume: **~$5-10/месяц** за 1GB storage
(Точная цена зависит от региона и использования)

---

## Пошаговая инструкция

### Шаг 1: Создать Volume на Railway

1. Открой Railway Dashboard: https://railway.app
2. Выбери проект Bot API (или создай новый)
3. Перейди в раздел **Variables**
4. Нажми **New Variable** → **Volume**
5. Настройки volume:
   - **Name:** `telegram-bot-api-data`
   - **Mount Path:** `/var/lib/telegram-bot-api`
   - **Size:** 1GB (можно увеличить позже)

### Шаг 2: Настроить переменные окружения

В Railway Dashboard → Variables добавь:

```bash
# Обязательные переменные (уже должны быть)
TELEGRAM_API_ID=<your_api_id>
TELEGRAM_API_HASH=<your_api_hash>
TELEGRAM_HTTP_PORT=8081

# НОВАЯ переменная для основного бота
BOT_API_DATA_DIR=/var/lib/telegram-bot-api
```

**Важно:** `BOT_API_DATA_DIR` должна быть установлена в **основном боте**, а не в Bot API сервере!

### Шаг 3: Деплой обновлённой конфигурации

```bash
# 1. Закоммить изменения
git add bot-api/
git commit -m "feat: add persistent volume support for Bot API"

# 2. Запушить на Railway
git push railway main

# 3. Railway автоматически пересоберёт контейнер с volume
```

### Шаг 4: Проверка

После деплоя проверь логи Bot API:

```
Starting Telegram Bot API with persistent storage...
Data directory: /var/lib/telegram-bot-api
```

Если видишь эти строки - всё работает! ✅

---

## Тестирование

### Тест 1: Загрузка большого файла

1. Отправь видео боту (>20MB)
2. Попробуй сделать clip/cut
3. Проверь логи - должен использоваться direct copy:

```
📂 Local Bot API: attempting direct file copy from /var/lib/telegram-bot-api/...
✅ File exists locally, copying directly...
✅ File copied successfully
```

### Тест 2: Fallback на api.telegram.org

1. Отправь файл <20MB
2. Если файл не найден на Local API:

```
⚠️ File not found on local Bot API server, falling back to api.telegram.org
```

Это нормально - бот автоматически скачает с официального API.

---

## Архитектура

### Текущая схема (С volume)

```
User → Telegram → Railway Bot API → Volume (/var/lib/telegram-bot-api)
                         ↓
                    Main Bot (direct copy)
                         ↓
                    Processing ✅
```

### Fallback схема (Без volume или при 404)

```
User → Telegram → Railway Bot API → ❌ 404 Not Found
                         ↓
                    Main Bot → Fallback to api.telegram.org
                         ↓
                    Download via HTTP ✅
```

---

## Переменные окружения

### В Bot API сервере (Railway)

```bash
TELEGRAM_API_ID=<your_api_id>
TELEGRAM_API_HASH=<your_api_hash>
TELEGRAM_HTTP_PORT=8081
```

### В основном боте (Railway/VPS)

```bash
BOT_API_URL=https://telegram-bot-api-production-d892.up.railway.app
BOT_API_DATA_DIR=/var/lib/telegram-bot-api  # ← ВАЖНО!
```

**Примечание:** Если `BOT_API_DATA_DIR` не установлена, бот будет использовать HTTP fallback.

---

## Мониторинг Volume

### Проверка использования диска

В Railway Dashboard → Metrics можно посмотреть:
- Использование volume (GB)
- I/O операции
- Стоимость

### Очистка старых файлов

Telegram Bot API автоматически удаляет старые файлы через 1 час.
Но можно настроить manual cleanup:

```bash
# SSH в Railway container (если нужно)
railway run bash

# Проверить размер
du -sh /var/lib/telegram-bot-api

# Удалить старые файлы (>24ч)
find /var/lib/telegram-bot-api -type f -mtime +1 -delete
```

---

## Troubleshooting

### Проблема: "BOT_API_DATA_DIR not set"

**Решение:** Установи переменную окружения в **основном боте**:
```bash
BOT_API_DATA_DIR=/var/lib/telegram-bot-api
```

### Проблема: "File not found" (404)

**Причины:**
1. Volume не примонтирован - проверь Railway Dashboard
2. Файл уже удалён Telegram (>1 час)
3. Permissions issue - проверь логи Bot API

**Решение:** Бот автоматически fallback на api.telegram.org

### Проблема: Permission denied

**Решение:** В Dockerfile уже есть `chown`, но если проблема повторяется:

```bash
# В entrypoint.sh
chown -R telegram-bot-api:telegram-bot-api /var/lib/telegram-bot-api
```

### Проблема: Volume full (нет места)

**Решение:** Увеличь размер volume в Railway Dashboard или настрой auto-cleanup:

```bash
# В cron (если нужно)
0 */6 * * * find /var/lib/telegram-bot-api -type f -mtime +1 -delete
```

---

## Откат изменений

Если что-то пошло не так, можно вернуться к HTTP-only режиму:

1. Убери `BOT_API_URL` из переменных окружения
2. Бот автоматически переключится на `api.telegram.org`
3. Лимит файлов вернётся к 20MB

---

## FAQ

**Q: Сколько стоит volume?**
A: ~$5-10/месяц за 1GB на Railway

**Q: Можно ли увеличить размер?**
A: Да, в Railway Dashboard → Volume → Resize

**Q: Что если volume недоступен?**
A: Бот автоматически fallback на api.telegram.org (лимит 20MB)

**Q: Нужно ли бэкапить volume?**
A: Нет, файлы временные (Telegram удаляет через 1 час)

**Q: Можно ли использовать S3 вместо volume?**
A: Telegram Bot API не поддерживает S3 напрямую, только local filesystem

---

## Полезные ссылки

- [Railway Volumes Documentation](https://docs.railway.app/reference/volumes)
- [Telegram Bot API Documentation](https://core.telegram.org/bots/api)
- [aiogram/telegram-bot-api Docker Image](https://hub.docker.com/r/aiogram/telegram-bot-api)

---

## Поддержка

Если возникли проблемы, проверь:
1. Логи Bot API сервера в Railway
2. Логи основного бота
3. Railway Dashboard → Metrics → Volume usage

Нашёл баг? Создай issue в GitHub!
