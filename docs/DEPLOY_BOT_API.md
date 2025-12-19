# Деплой Telegram Bot API на Railway

## Доступные варианты Dockerfile

1. **Dockerfile.bot-api** - с захардкоженными credentials (быстрый старт)
2. **Dockerfile.bot-api.secure** - с использованием ENV переменных (рекомендуется)

## Рекомендуемый способ: Деплой через Railway Web Dashboard

### Шаг 1: Создайте новый сервис

1. Откройте [Railway Dashboard](https://railway.app/dashboard)
2. Выберите ваш проект или создайте новый
3. Нажмите "New Service" → "GitHub Repo"
4. Выберите репозиторий `doradura`

### Шаг 2: Настройте сервис

1. В настройках сервиса найдите секцию "Settings"
2. Измените следующие параметры:
   - **Service Name**: `telegram-bot-api`
   - **Dockerfile Path**: `Dockerfile.bot-api`
   - **Custom Start Command**: (оставьте пустым, команда уже в Dockerfile)

### Шаг 3: Настройте порты

1. В секции "Settings" → "Networking"
2. Добавьте публичный домен (если нужен внешний доступ)
3. Убедитесь что порт 8081 прокинут

### Шаг 4: Deploy

1. Railway автоматически начнет деплой после настройки
2. Следите за логами в разделе "Deployments"
3. После успешного деплоя сервис будет доступен

## Конфигурация

Сервис настроен со следующими параметрами:
- **API ID**: YOUR_API_ID
- **API Hash**: YOUR_API_HASH
- **HTTP Port**: 8081
- **Mode**: --local

## Проверка работы

После деплоя проверьте:

```bash
curl https://your-service-url.railway.app/
```

Или в логах Railway должны быть сообщения об успешном запуске.

## Альтернативный метод: Railway CLI (если заработает)

```bash
# Убедитесь что авторизованы
railway login

# Создайте новый проект или подключитесь к существующему
railway link

# Задеплойте с указанием на Dockerfile
railway up --dockerfile Dockerfile.bot-api
```

## Использование в основном боте

После деплоя обновите переменную окружения в основном сервисе бота:

```bash
BOT_API_URL=https://your-bot-api-service.railway.app
```

## Важные замечания

⚠️ **Безопасность**: API ID и Hash захардкожены в Dockerfile. Для production рекомендуется:

1. Использовать переменные окружения Railway
2. Создать отдельный Dockerfile который принимает ENV переменные
3. Настроить secrets в Railway Dashboard

⚠️ **Персистентность**: Данные bot-api хранятся в контейнере. Для сохранения данных между деплоями нужно настроить Railway Volumes.
