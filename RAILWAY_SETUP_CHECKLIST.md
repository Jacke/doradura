# ✅ Railway Setup Checklist

Пошаговая проверка перед запуском бота.

## 📋 Pre-Deploy Checklist

### 1. GitHub Repository ✅ ГОТОВО
- [x] Код запушен в GitHub
- [x] Dockerfile создан и настроен
- [x] youtube_cookies.txt в репозитории
- [x] Railway конфигурация готова

### 2. Railway Project
- [ ] Проект создан на [railway.app](https://railway.app)
- [ ] Репозиторий подключен (Deploy from GitHub)
- [ ] Первая сборка завершилась успешно

### 3. Environment Variables ⚠️ **НУЖНО НАСТРОИТЬ!**

#### Обязательные:

```bash
railway variables --set "TELOXIDE_TOKEN=6310079371:AAH5D08Tvmt5W7Lo8PDHBL_qxq4Cgv1wpUw"
```

#### Для YouTube (ВАЖНО!):

```bash
railway variables --set "YTDL_COOKIES_FILE=youtube_cookies.txt"
```

#### Опциональные:

```bash
# Ваш Telegram User ID для админ-команд
railway variables --set "ADMIN_IDS=your_telegram_id"

# Для Mini App (если нужно)
railway variables --set "WEBAPP_PORT=8080"
railway variables --set "WEBAPP_URL=https://your-project.railway.app"
```

---

## 🚀 Quick Setup (Copy-Paste)

### Вариант 1: Через Railway CLI

```bash
# 1. Войдите в Railway
railway login

# 2. Подключитесь к проекту (если еще не подключены)
railway link

# 3. Установите все переменные одной командой
railway variables \
  --set "TELOXIDE_TOKEN=6310079371:AAH5D08Tvmt5W7Lo8PDHBL_qxq4Cgv1wpUw" \
  --set "YTDL_COOKIES_FILE=youtube_cookies.txt"

# 4. Проверьте, что переменные установлены
railway variables

# 5. Перезапустите сервис
railway restart
```

### Вариант 2: Через Railway Dashboard (Рекомендуется)

1. **Откройте Railway Dashboard:**
   - https://railway.app
   - Выберите ваш проект `doradura-bot`

2. **Перейдите в Variables:**
   - Нажмите на вкладку **Variables**

3. **Добавьте переменные:**

   **Переменная 1:**
   ```
   Имя: TELOXIDE_TOKEN
   Значение: 6310079371:AAH5D08Tvmt5W7Lo8PDHBL_qxq4Cgv1wpUw
   ```

   **Переменная 2:**
   ```
   Имя: YTDL_COOKIES_FILE
   Значение: youtube_cookies.txt
   ```

4. **Сохраните и дождитесь перезапуска**
   - Railway автоматически перезапустит бота

---

## 🔍 Проверка после деплоя

### 1. Проверьте логи:

```bash
railway logs
```

Должны увидеть:
```
✅ YTDL_COOKIES_FILE: /app/youtube_cookies.txt
   File exists and will be used for YouTube authentication
✅ Cookies configured - YouTube downloads should work
✅ Bot username: @your_bot
✅ Starting bot in long polling mode
```

### 2. Проверьте переменные:

```bash
railway variables
```

Должны быть установлены:
- `TELOXIDE_TOKEN=6310079371:...`
- `YTDL_COOKIES_FILE=youtube_cookies.txt`

### 3. Тест в Telegram:

1. Найдите бота в Telegram
2. Отправьте `/start`
3. Бот должен ответить стикером и приветствием
4. Попробуйте скачать что-то с YouTube

---

## ❌ Что НЕ ТАК сейчас

Судя по логам:
```
⚠️  YTDL_COOKIES_FILE: not set
⚠️  YTDL_COOKIES_BROWSER: not set
❌ NO COOKIES CONFIGURED - YouTube downloads will FAIL!
```

### Проблема:
Переменные окружения НЕ УСТАНОВЛЕНЫ на Railway!

### Решение:
Установите переменные как описано выше ⬆️

---

## 🎯 После установки переменных

1. **Railway автоматически перезапустит бота**
2. **Проверьте логи через 1-2 минуты**
3. **Должны увидеть:**
   ```
   ✅ YTDL_COOKIES_FILE: /app/youtube_cookies.txt
   ✅ Cookies configured - YouTube downloads should work
   ```
4. **Протестируйте в Telegram**

---

## 📊 Итоговая конфигурация

После выполнения всех шагов у вас должно быть:

```
✅ TELOXIDE_TOKEN=6310079371:AAH5D08Tvmt5W7Lo8PDHBL_qxq4Cgv1wpUw
✅ YTDL_COOKIES_FILE=youtube_cookies.txt
✅ youtube_cookies.txt файл в /app/ (из git)
✅ Бот запущен в long polling режиме
✅ YouTube downloads работают
```

---

## 🆘 Если что-то не работает

1. **Проверьте TROUBLESHOOTING.md**
2. **Проверьте логи:** `railway logs`
3. **Перезапустите:** `railway restart`
4. **Проверьте переменные:** `railway variables`

---

**Следуйте этому чеклисту и бот заработает!** 🚀
