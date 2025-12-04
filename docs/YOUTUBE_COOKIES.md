# Настройка YouTube Cookies для обхода защиты "Sign in to confirm you're not a bot"

YouTube начал требовать аутентификацию для некоторых видео. Бот использует cookies из браузера для обхода этой защиты.

## Автоматическое извлечение cookies (Рекомендуется)

### Шаг 1: Установка зависимостей Python

yt-dlp требует дополнительные библиотеки для извлечения cookies:

```bash
# Установка зависимостей
pip3 install keyring pycryptodomex

# Или через pip (если pip3 не найден)
pip install keyring pycryptodomex
```

### Шаг 2: Проверка работы

Проверь, что yt-dlp может читать cookies:

```bash
yt-dlp --cookies-from-browser chrome --print "%(title)s" "https://www.youtube.com/watch?v=dQw4w9WgXcQ"
```

Если команда выводит название видео - все работает! ✅

### Шаг 3: Настройка браузера (опционально)

По умолчанию используется **Chrome**. Можно изменить через переменную окружения:

```bash
# Использовать Firefox
export YTDL_COOKIES_BROWSER=firefox

# Использовать Safari (macOS)
export YTDL_COOKIES_BROWSER=safari

# Использовать Brave
export YTDL_COOKIES_BROWSER=brave

# Отключить cookies (не рекомендуется)
export YTDL_COOKIES_BROWSER=""
```

**Поддерживаемые браузеры:**
- `chrome` - Google Chrome (по умолчанию)
- `firefox` - Mozilla Firefox (работает лучше всего, т.к. cookies не зашифрованы)
- `safari` - Safari (только macOS)
- `brave` - Brave Browser
- `chromium` - Chromium
- `edge` - Microsoft Edge
- `opera` - Opera
- `vivaldi` - Vivaldi

### Шаг 4: Авторизация на YouTube

1. Открой браузер (например, Chrome)
2. Зайди на https://youtube.com
3. Войди в свой Google аккаунт
4. Просто посмотри любое видео
5. Готово! yt-dlp будет использовать эти cookies

---

## Альтернатива: Экспорт cookies вручную

Если автоматическое извлечение не работает:

### Способ 1: Расширение для браузера

1. **Установи расширение:**
   - Chrome: [Get cookies.txt LOCALLY](https://chrome.google.com/webstore/detail/get-cookiestxt-locally/cclelndahbckbenkjhflpdbgdldlbecc)
   - Firefox: [cookies.txt](https://addons.mozilla.org/en-US/firefox/addon/cookies-txt/)

2. **Экспортируй cookies:**
   - Открой YouTube.com
   - Нажми на иконку расширения
   - Нажми "Export" → Сохрани как `youtube_cookies.txt`

3. **Используй файл:**
   ```bash
   # Поместить файл в директорию проекта
   mv youtube_cookies.txt ~/youtube_cookies.txt
   ```

4. **Обновить код** (временно):
   
   Замени `--cookies-from-browser chrome` на `--cookies youtube_cookies.txt` в файлах:
   - `src/downloader.rs`
   - `src/preview.rs`

### Способ 2: Использовать Firefox

Firefox хранит cookies в незашифрованном виде, поэтому работает лучше:

```bash
# Просто измени браузер на firefox
export YTDL_COOKIES_BROWSER=firefox
```

---

## Решение проблем

### Ошибка "Signature extraction failed" или "Some formats may be missing"

**Причина:** Устаревшая версия yt-dlp

**Решение:**
```bash
# Обновить yt-dlp
./update_ytdlp.sh

# Или вручную
yt-dlp -U
# или
pip3 install -U yt-dlp --break-system-packages
```

### Ошибка "The following content is not available on this app"

**Причина:** YouTube блокирует определенные клиенты

**Решение:** Уже реализовано в боте! Используется `--extractor-args "youtube:player_client=android,web"` для обхода блокировок через Android клиент.

### Ошибка "Sign in to confirm you're not a bot"

**Причина:** cookies не найдены или устарели

**Решение:**
1. Убедись, что зависимости установлены: `pip3 install keyring pycryptodomex`
2. Открой браузер и зайди на YouTube
3. Попробуй использовать Firefox: `export YTDL_COOKIES_BROWSER=firefox`

### Ошибка "keyring.errors.NoKeyringError"

**Причина:** система не поддерживает keyring (часто на серверах без GUI)

**Решения:**
1. **Использовать Firefox** (не требует keyring):
   ```bash
   export YTDL_COOKIES_BROWSER=firefox
   ```

2. **Экспортировать cookies вручную** (см. выше)

3. **Установить dummy keyring** (для серверов):
   ```bash
   pip3 install keyrings.alt
   ```

### Ошибка "Cryptography module is not available"

**Причина:** отсутствует библиотека для расшифровки

**Решение:**
```bash
pip3 install pycryptodomex
# или
pip3 install pycryptodome
```

### Cookies устарели

**Решение:**
1. Открой браузер
2. Выйди из YouTube
3. Войди снова
4. Перезапусти бота

---

## Проверка настроек

```bash
# Проверить, какой браузер используется
echo $YTDL_COOKIES_BROWSER

# Проверить работу yt-dlp с cookies
yt-dlp --cookies-from-browser ${YTDL_COOKIES_BROWSER:-chrome} --print "%(title)s" "https://www.youtube.com/watch?v=dQw4w9WgXcQ"

# Посмотреть логи бота для диагностики
tail -f logs/bot.log | grep -i cookie
```

---

## Для продакшен серверов

На серверах без браузеров используй файл cookies:

```bash
# 1. На локальной машине экспортируй cookies (см. выше)

# 2. Скопируй на сервер
scp youtube_cookies.txt user@server:/path/to/bot/

# 3. Обнови конфигурацию бота для использования файла
```

---

## Безопасность

⚠️ **Важно:** Файл cookies содержит токены аутентификации!

- ✅ НЕ коммить `youtube_cookies.txt` в git
- ✅ Добавить в `.gitignore`
- ✅ Ограничить права доступа: `chmod 600 youtube_cookies.txt`
- ✅ Регулярно обновлять cookies (раз в месяц)

---

## Дополнительные ресурсы

- [yt-dlp wiki: Cookies](https://github.com/yt-dlp/yt-dlp/wiki/FAQ#how-do-i-pass-cookies-to-yt-dlp)
- [yt-dlp wiki: Exporting YouTube cookies](https://github.com/yt-dlp/yt-dlp/wiki/Extractors#exporting-youtube-cookies)

