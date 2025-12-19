# Быстрый деплой Telegram Bot API на Railway

## Шаг 1: Откройте Railway Dashboard

Перейдите на [railway.app/dashboard](https://railway.app/dashboard)

## Шаг 2: Создайте новый сервис

1. Откройте ваш проект (или создайте новый)
2. Нажмите **"+ New"** → **"GitHub Repo"**
3. Выберите репозиторий `doradura`
4. Railway автоматически обнаружит Dockerfile

## Шаг 3: Настройте сервис

В настройках сервиса (Settings):

### General
- **Service Name**: `telegram-bot-api`

### Source
- **Dockerfile Path**: `Dockerfile.bot-api`

### Networking
- Добавьте **Public Domain** если нужен внешний доступ
- Порт: `8081` (автоматически)

## Шаг 4: (Опционально) Безопасная конфигурация

Если используете `Dockerfile.bot-api.secure`:

### Environment Variables
Добавьте в Settings → Variables:

```
TELEGRAM_API_ID=YOUR_API_ID
TELEGRAM_API_HASH=YOUR_API_HASH
TELEGRAM_HTTP_PORT=8081
```

## Шаг 5: Deploy

Railway автоматически начнет деплой. Следите за прогрессом в разделе **Deployments**.

## Готово!

После успешного деплоя ваш Bot API будет доступен по адресу:
```
https://your-service-name.up.railway.app
```

## Что дальше?

Используйте этот URL в вашем боте:

```bash
# В переменных окружения основного бота
BOT_API_URL=https://your-bot-api-service.up.railway.app
```

## Проверка

```bash
curl https://your-bot-api-service.up.railway.app/
```

Должен вернуть ответ от Telegram Bot API.
