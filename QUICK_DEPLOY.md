# Quick Deploy to Railway 🚂

Быстрое руководство по деплою Doradura бота на Railway за 5 минут.

## Метод 1: Автоматический (Рекомендуется)

```bash
# 1. Авторизуйтесь в Railway
railway login

# 2. Запустите скрипт деплоя
./deploy.sh
```

Скрипт автоматически:
- Создаст проект на Railway
- Запросит необходимые данные (Bot Token, Admin ID, etc.)
- Настроит переменные окружения
- Задеплоит бота

## Метод 2: Ручной

### Шаг 1: Авторизация

```bash
railway login
```

### Шаг 2: Инициализация проекта

```bash
railway init
```

Выберите "Create a new project" и назовите его `doradura-bot`.

### Шаг 3: Настройка переменных

```bash
# Обязательно
railway variables --set "TELOXIDE_TOKEN=YOUR_BOT_TOKEN"

# Рекомендуется
railway variables --set "YTDL_COOKIES_FILE=youtube_cookies.txt"
railway variables --set "ADMIN_IDS=YOUR_TELEGRAM_ID"
```

### Шаг 4: Деплой

```bash
railway up
```

## Метод 3: Через GitHub

### Шаг 1: Подключите GitHub

1. Перейдите на [railway.app](https://railway.app)
2. Создайте новый проект
3. Выберите "Deploy from GitHub repo"
4. Выберите репозиторий `doradura`

### Шаг 2: Настройте переменные

В Railway Dashboard → Variables → Add Variable:

```
TELOXIDE_TOKEN=your_bot_token
YTDL_COOKIES_FILE=youtube_cookies.txt
ADMIN_IDS=your_telegram_id
```

### Шаг 3: Деплой

Railway автоматически начнет деплой после настройки переменных.

## Проверка деплоя

```bash
# Просмотр логов
railway logs

# Статус
railway status

# Открыть dashboard
railway open
```

## Быстрые команды

```bash
# Обновить бота
railway up

# Перезапустить
railway restart

# Просмотреть переменные
railway variables

# Подключиться к логам в реальном времени
railway logs -f
```

## Получение домена

```bash
# Railway автоматически создаст домен
railway domain

# Или создайте свой
railway domain create
```

После получения домена, обновите WEBAPP_URL:

```bash
railway variables set WEBAPP_URL="https://your-project.railway.app"
```

## Troubleshooting

### Бот не запускается

```bash
# Проверьте логи
railway logs

# Проверьте переменные
railway variables
```

### YouTube не работает

```bash
# Добавьте cookies
railway variables --set "YTDL_COOKIES_FILE=youtube_cookies.txt"

# Или используйте браузер
railway variables --set "YTDL_COOKIES_BROWSER=chrome"
```

### База данных не сохраняется

В Railway Dashboard:
1. Перейдите в Settings → Volumes
2. Создайте новый volume
3. Установите mount path: `/app/database.sqlite`

## Минимальная конфигурация

Для запуска бота нужна только одна переменная:

```bash
railway variables --set "TELOXIDE_TOKEN=YOUR_BOT_TOKEN"
railway up
```

Все остальное опционально!

## Рекомендуемая конфигурация

```bash
# Основное
railway variables --set "TELOXIDE_TOKEN=YOUR_BOT_TOKEN"
railway variables --set "YTDL_COOKIES_FILE=youtube_cookies.txt"

# Админ
railway variables --set "ADMIN_IDS=YOUR_TELEGRAM_ID"

# Mini App (опционально)
railway variables --set "WEBAPP_PORT=8080"
railway variables --set "WEBAPP_URL=https://your-project.railway.app"
```

## Полная документация

Смотрите [RAILWAY_DEPLOY.md](./RAILWAY_DEPLOY.md) для подробных инструкций.
