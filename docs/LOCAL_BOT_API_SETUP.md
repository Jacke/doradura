# 🚀 Настройка локального Telegram Bot API сервера

Локальный Bot API сервер позволяет:
- ✅ Отправлять файлы до **2 ГБ** (вместо 50 МБ)
- ✅ Уменьшить сетевые задержки
- ✅ Больше гибкости в настройке вебхуков

## 📋 Требования

1. **API ID и API Hash** от Telegram
2. **Docker** (рекомендуется) или компилятор C++ для сборки из исходников

## 🔑 Шаг 1: Получение API ID и API Hash

1. Перейдите на https://my.telegram.org
2. Войдите с вашим номером телефона
3. Перейдите в раздел **API development tools**
4. Создайте новое приложение (или используйте существующее)
5. Скопируйте `api_id` и `api_hash`

## 🐳 Шаг 2: Установка через Docker (рекомендуется)

### Быстрый старт

1. Создайте файл `.env.bot-api` с вашими данными:
```bash
API_ID=YOUR_API_ID
API_HASH=YOUR_API_HASH
```

2. Запустите сервер:
```bash
docker run -d \
  --name telegram-bot-api \
  -p 8081:8081 \
  --env-file .env.bot-api \
  -v $(pwd)/bot-api-data:/var/lib/telegram-bot-api \
  aiogram/telegram-bot-api:latest
```

3. Проверьте, что сервер работает:
```bash
curl http://localhost:8081/botYOUR_BOT_TOKEN/getMe
```

### Использование docker-compose (удобнее)

Создайте файл `docker-compose.bot-api.yml`:

```yaml
version: '3.8'

services:
  telegram-bot-api:
    image: aiogram/telegram-bot-api:latest
    container_name: telegram-bot-api
    restart: unless-stopped
    ports:
      - "8081:8081"
    environment:
      - API_ID=${API_ID}
      - API_HASH=${API_HASH}
    volumes:
      - ./bot-api-data:/var/lib/telegram-bot-api
    command: --local --api-id=${API_ID} --api-hash=${API_HASH} --http-port=8081
```

Запуск:
```bash
# Запустить
docker-compose -f docker-compose.bot-api.yml up -d

# Остановить
docker-compose -f docker-compose.bot-api.yml down

# Просмотр логов
docker-compose -f docker-compose.bot-api.yml logs -f
```

## 📦 Шаг 3: Установка из исходников (альтернатива)

Если Docker недоступен, можно собрать из исходников:

```bash
# Клонируем репозиторий
git clone --recursive https://github.com/tdlib/telegram-bot-api.git
cd telegram-bot-api

# Собираем
mkdir build
cd build
cmake -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX:PATH=.. ..
cmake --build . --target install

# Запускаем
cd ..
./bin/telegram-bot-api \
  --local \
  --api-id=YOUR_API_ID \
  --api-hash=YOUR_API_HASH \
  --http-port=8081
```

## ⚙️ Шаг 4: Настройка бота

После запуска локального сервера, настройте переменную окружения:

```bash
# В .env файле или при запуске бота
export BOT_API_URL=http://localhost:8081
```

Или добавьте в `.env`:
```env
BOT_API_URL=http://localhost:8081
```

## ✅ Проверка работы

1. **Проверьте сервер:**
```bash
curl http://localhost:8081/botYOUR_BOT_TOKEN/getMe
```

2. **Проверьте логи бота:**
При запуске бота вы должны увидеть:
```
[INFO] Local Bot API server detected (BOT_API_URL=http://localhost:8081), using 2 GB limit
```

3. **Проверьте отправку файла:**
Попробуйте скачать видео размером больше 50 МБ - должно работать!

## 🔧 Дополнительные настройки

### Изменение порта

По умолчанию используется порт `8081`. Чтобы изменить:

```bash
# В docker-compose
ports:
  - "9000:9000"  # Внешний:Внутренний

# В команде запуска
--http-port=9000

# В .env бота
BOT_API_URL=http://localhost:9000
```

### Настройка для production

Для production рекомендуется:
- Использовать HTTPS (через reverse proxy, например nginx)
- Настроить firewall
- Использовать systemd для автозапуска

Пример systemd сервиса (`/etc/systemd/system/telegram-bot-api.service`):

```ini
[Unit]
Description=Telegram Bot API Server
After=network.target

[Service]
Type=simple
User=your-user
WorkingDirectory=/path/to/telegram-bot-api
ExecStart=/path/to/telegram-bot-api/bin/telegram-bot-api \
  --local \
  --api-id=YOUR_API_ID \
  --api-hash=YOUR_API_HASH \
  --http-port=8081
Restart=always

[Install]
WantedBy=multi-user.target
```

## 📚 Полезные ссылки

- [Официальная документация Telegram Bot API](https://core.telegram.org/bots/api#using-a-local-bot-api-server)
- [Репозиторий telegram-bot-api](https://github.com/tdlib/telegram-bot-api)
- [Docker образ aiogram/telegram-bot-api](https://hub.docker.com/r/aiogram/telegram-bot-api)

## 🐛 Решение проблем

### Сервер не запускается

1. Проверьте, что порт 8081 свободен:
```bash
lsof -i :8081
```

2. Проверьте логи:
```bash
docker logs telegram-bot-api
```

### Бот не подключается к локальному серверу

1. Убедитесь, что `BOT_API_URL` установлена правильно
2. Проверьте, что сервер доступен:
```bash
curl http://localhost:8081/botYOUR_BOT_TOKEN/getMe
```

3. Проверьте firewall/iptables

### Файлы все еще блокируются на 50 МБ

1. Убедитесь, что `BOT_API_URL` установлена и не указывает на `api.telegram.org`
2. Проверьте логи бота - должно быть сообщение о детекции локального сервера
3. Перезапустите бота после изменения `BOT_API_URL`

## 💡 Рекомендации

- Используйте Docker для простоты развертывания
- Храните `API_ID` и `API_HASH` в безопасном месте (не коммитьте в git!)
- Используйте `.env` файл для конфигурации
- Настройте резервное копирование данных сервера (папка `bot-api-data`)

