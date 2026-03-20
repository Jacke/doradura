# init-data: Оптимизация старта Bot API

## Проблема

Каждый рестарт контейнера занимает **~150 секунд** из-за холодного старта Bot API.
Bot API вынужден заново скачивать все file references с серверов Telegram.

## Почему так сейчас

### Инцидент 2026-03-09

Попытка сохранить binlog для быстрого рестарта уронила прод на ~1 час:

1. Хотели сохранять binlog между рестартами (10с старт вместо 150с)
2. `chown -R 1000:2000 /data` менял владельца binlog файлов на `botuser`
3. `telegram-bot-api` (uid отличается от 1000) не мог прочитать свои файлы → crash loop
4. Crash loop повредил внутренний state → Bot API застрял в вечном "restart"
5. Решение: ядерный вариант — удалять ВСЁ при каждом рестарте

### Текущий init-data скрипт (Dockerfile:155-212)

```
Шаг 1: mkdir /data, chmod /tmp                     ← OK
Шаг 2: Удаление temp файлов (*.png, *.binlog.lock) ← OK
Шаг 3: chown telegram-bot-api:shareddata /data     ← OK (только top-level dir)
Шаг 4: chown -R 1000:2000 /app                     ← OK
Шаг 5: Подготовка DB directory                      ← OK
Шаг 6: chown -R 1000:2000 /data    ← ❌ ПРОБЛЕМА: меняет ВСЁ в /data на botuser
Шаг 7: chown/chmod *.sqlite*                        ← OK (но избыточно после шага 6)
Шаг 8: Удаление ВСЕХ Bot API директорий + binlog   ← ❌ ЯДЕРНАЯ ОЧИСТКА
```

**Корень проблемы — Шаг 6**: `chown -R 1000:2000 /data` рекурсивно меняет владельца ВСЕХ файлов, включая Bot API state. После этого Bot API не может их прочитать, поэтому Шаг 8 их удаляет.

## Решение: Гранулярный chown

### Принцип

Вместо `chown -R` на весь `/data` — менять владельца только тех файлов, которые нужны конкретному процессу.

### Что кому принадлежит в /data

| Файлы | Владелец | Зачем |
|-------|---------|-------|
| `*.sqlite*` | `botuser:shareddata` (1000:2000) | SQLite БД бота |
| `app.log` | `botuser:shareddata` (1000:2000) | Лог файл |
| `*.png`, temp файлы | `botuser:shareddata` (1000:2000) | Временные скриншоты |
| `/data/` (сама директория) | `telegram-bot-api:shareddata` | Bot API пишет сюда |
| `/data/*/` (поддиректории с binlog) | `telegram-bot-api:shareddata` | **Bot API state — НЕ ТРОГАТЬ** |
| `*.binlog` | `telegram-bot-api:shareddata` | **Bot API state — НЕ ТРОГАТЬ** |

### Новый init-data скрипт (псевдокод)

```sh
# 1. Создать директории
mkdir -p /data /tmp
chmod 1777 /tmp

# 2. Удалить temp файлы
find /data -maxdepth 1 -name "refresh_error_*.png" -delete
find /data -maxdepth 1 -name "signout_detected_*.png" -delete
find /data -name "*.binlog.lock" -delete       # только stale locks!

# 3. /data — владелец telegram-bot-api (он создаёт поддиректории)
chown telegram-bot-api:shareddata /data
chmod 775 /data

# 4. /app — владелец botuser
chown -R 1000:2000 /app
chmod 755 /app

# 5. SQLite — владелец botuser
chown 1000:2000 /data/*.sqlite* 2>/dev/null || true
chmod 664 /data/*.sqlite* 2>/dev/null || true

# 6. Логи и temp — владелец botuser
chown 1000:2000 /data/app.log 2>/dev/null || true
chown 1000:2000 /data/*.png 2>/dev/null || true

# 7. Bot API директории — ВЕРНУТЬ владельца telegram-bot-api
#    (на случай если volume сохранил старые permissions)
for d in /data/*/; do
  if [ -d "$d" ]; then
    chown -R telegram-bot-api:shareddata "$d"
  fi
done
find /data -name "*.binlog" -exec chown telegram-bot-api:shareddata {} \;

# 8. НЕ УДАЛЯЕМ binlog и Bot API директории!
echo "Bot API state preserved for warm restart"
```

### Что изменилось

| Было | Стало |
|------|-------|
| `chown -R 1000:2000 /data` (всё подряд) | `chown` только конкретных файлов botuser'а |
| `rm -rf` всех Bot API директорий | Не удаляем, а `chown` обратно на `telegram-bot-api` |
| `find -name "*.binlog" -delete` | Удаляем только `*.binlog.lock` (stale locks) |
| Холодный старт ~150с | Тёплый старт ~10с |

## Риски

### 1. Corrupted state после crash'а
- **Вероятность**: Низкая. Bot API корректно обрабатывает replay из binlog
- **Митигация**: Если Bot API не стартует за 180с (wait-for-bot-api скрипт), можно добавить автоматический fallback на полную очистку

### 2. Несовместимость версий Bot API
- **Когда**: При обновлении base image `aiogram/telegram-bot-api`
- **Митигация**: При смене версии Bot API — одноразово очистить `/data/*/` вручную или через env var

### 3. Диск заполняется binlog'ами
- **Вероятность**: Низкая. Binlog файлы обычно небольшие (несколько MB)
- **Митигация**: Мониторинг в health-monitor

## Fallback-механизм (опционально)

Можно добавить env var `FORCE_CLEAN_BOT_API=1` для принудительной очистки:

```sh
if [ "${FORCE_CLEAN_BOT_API:-0}" = "1" ]; then
  echo "FORCE_CLEAN_BOT_API=1: deleting all Bot API state"
  for d in /data/*/; do
    if [ -d "$d" ]; then
      rm -rf "$d"
    fi
  done
  find /data -name "*.binlog" -delete
fi
```

Это позволит при необходимости вернуть текущее поведение без изменения кода — просто выставить переменную в Railway.

## План тестирования

1. **Собрать образ** с новым init-data
2. **Первый запуск** (холодный) — проверить что Bot API стартует нормально (~150с)
3. **Рестарт** (тёплый) — проверить что Bot API стартует быстро (~10с)
4. **Проверить permissions** — `ls -la /data/` после рестарта
5. **Проверить что бот работает** — отправить команду, скачать аудио
6. **Тест FORCE_CLEAN_BOT_API=1** — убедиться что очистка работает
7. **Тест crash recovery** — `docker kill` + restart → Bot API должен восстановиться
