# 🚨 БЫСТРОЕ ИСПРАВЛЕНИЕ - Railway Variables

## Проблема
```
⚠️  YTDL_COOKIES_FILE: not set
⚠️  YTDL_COOKIES_BROWSER: not set
```

## ⚡ РЕШЕНИЕ ЗА 2 МИНУТЫ

### Шаг 1: Откройте Railway Dashboard

1. Перейдите на https://railway.app
2. Войдите в аккаунт
3. Откройте проект **doradura-bot** (или как вы его назвали)

### Шаг 2: Добавьте переменные

1. Нажмите на вкладку **"Variables"** (слева в меню)

2. Нажмите **"+ New Variable"**

3. Добавьте первую переменную:
   ```
   TELOXIDE_TOKEN
   ```
   Значение:
   ```
   6310079371:AAH5D08Tvmt5W7Lo8PDHBL_qxq4Cgv1wpUw
   ```

4. Нажмите **"+ New Variable"** еще раз

5. Добавьте вторую переменную:
   ```
   YTDL_COOKIES_FILE
   ```
   Значение:
   ```
   youtube_cookies.txt
   ```

6. Нажмите **"Add"** или **"Save"**

### Шаг 3: Дождитесь перезапуска

Railway автоматически перезапустит бот через 10-30 секунд.

### Шаг 4: Проверьте логи

1. В том же проекте нажмите на **"Deployments"**
2. Нажмите на последний активный deployment
3. Нажмите **"View Logs"**
4. Подождите 1-2 минуты

Вы должны увидеть:
```
✅ YTDL_COOKIES_FILE: /app/youtube_cookies.txt
   File exists and will be used for YouTube authentication
✅ Cookies configured - YouTube downloads should work
✅ Starting bot in long polling mode
```

---

## 📱 Тест

1. Откройте Telegram
2. Найдите вашего бота
3. Отправьте `/start`
4. Попробуйте скачать что-то с YouTube

**Готово!** 🎉

---

## 🖼️ Скриншот для ясности

```
Railway Dashboard
├── Ваш проект (doradura-bot)
│   ├── Variables ← СЮДА
│   │   ├── + New Variable
│   │   │   ├── Name: TELOXIDE_TOKEN
│   │   │   └── Value: 6310079371:AAH5D08Tvmt5W7Lo8PDHBL_qxq4Cgv1wpUw
│   │   ├── + New Variable
│   │   │   ├── Name: YTDL_COOKIES_FILE
│   │   │   └── Value: youtube_cookies.txt
│   │   └── [Add/Save]
│   ├── Deployments
│   └── Settings
```

---

## 📋 Копируй-вставляй значения

### Переменная 1
```
Name: TELOXIDE_TOKEN
Value: 6310079371:AAH5D08Tvmt5W7Lo8PDHBL_qxq4Cgv1wpUw
```

### Переменная 2
```
Name: YTDL_COOKIES_FILE
Value: youtube_cookies.txt
```

---

## ❓ Что если не получается через Dashboard?

### Альтернатива: Railway CLI

Если у вас уже настроен Railway CLI и проект подключен:

```bash
# 1. Убедитесь что вы в правильной директории
cd /Users/stan/Dev/_PROJ/doradura

# 2. Подключитесь к проекту (выберите из списка)
railway link

# 3. После подключения установите переменные
railway variables --set "TELOXIDE_TOKEN=6310079371:AAH5D08Tvmt5W7Lo8PDHBL_qxq4Cgv1wpUw"
railway variables --set "YTDL_COOKIES_FILE=youtube_cookies.txt"

# 4. Проверьте
railway variables

# 5. Перезапустите
railway restart
```

---

## ✅ Как понять что всё работает?

### В логах должно быть:

**✅ ХОРОШО:**
```
[INFO] 🍪 Cookies Configuration Check
[INFO] ✅ YTDL_COOKIES_FILE: /app/youtube_cookies.txt
[INFO]    File exists and will be used for YouTube authentication
[INFO] ✅ Cookies configured - YouTube downloads should work
[INFO] Starting bot...
[INFO] Bot username: @your_bot_username
[INFO] Starting bot in long polling mode
```

**❌ ПЛОХО:**
```
[WARN] ⚠️  YTDL_COOKIES_FILE: not set
[WARN] ⚠️  YTDL_COOKIES_BROWSER: not set
[ERROR] ❌ NO COOKIES CONFIGURED - YouTube downloads will FAIL!
```

---

## 💡 Совет

Используйте **Railway Dashboard** - это проще и надежнее, чем CLI для настройки переменных!

---

**Проблема решается за 2 минуты!** Действуйте! 🚀
