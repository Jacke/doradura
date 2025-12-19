# 📊 Как Смотреть Метрики - Полное Руководство

## 🎯 3 Способа Просмотра Метрик

### 1️⃣ Grafana - Красивые Дашборды (Рекомендуется)
### 2️⃣ Prometheus - Запросы и Графики
### 3️⃣ Telegram - Прямо в Боте

---

## 1️⃣ Grafana - Визуальные Дашборды

### Открыть Grafana

```bash
open http://localhost:3000
# Или просто откройте в браузере
```

**Логин:**
- Username: `admin`
- Password: `admin` (при первом входе попросит сменить)

### Найти Дашборд

После входа:

1. **Слева в меню** → нажмите на иконку **"Dashboards"** (4 квадратика)
2. Увидите дашборд **"Doradura Bot - Overview"**
3. Нажмите на него

### Что Увидите

**9 панелей с метриками:**

#### Performance
- **Download Rate** - Загрузок в секунду (success vs failure)
- **Success Rate** - Процент успешных загрузок (gauge)
- **Queue Depth** - Текущая очередь задач
- **Download Duration** - p50, p95, p99 (медиана, 95-й и 99-й перцентили)

#### Business
- **Daily Active Users** - Активные пользователи сегодня
- **Total Revenue** - Общий доход в Stars
- **Active Subscriptions** - Количество активных подписок

#### Formats & Errors
- **Downloads by Format** - MP3 vs MP4 vs Subtitles
- **Errors by Category** - ytdlp, network, rate_limit и т.д.

### Настройки Дашборда

**Временной диапазон** (справа вверху):
- Last 5 minutes
- Last 15 minutes
- Last 1 hour
- Last 6 hours ← по умолчанию
- Last 24 hours
- Last 7 days
- Custom range

**Auto-refresh** (справа вверху):
- Off
- 5s
- 10s
- 30s ← по умолчанию
- 1m

### Drill Down в Метрики

**Клик на график** → увидите детали
**Hover над точкой** → tooltip с точными значениями
**Legend** → клик чтобы включить/выключить серию

---

## 2️⃣ Prometheus - Запросы и Исследование

### Открыть Prometheus

```bash
open http://localhost:9091
```

### Graph Tab - Визуализация

1. Перейдите на вкладку **"Graph"**
2. В поле **"Expression"** введите запрос (PromQL)
3. Нажмите **"Execute"**
4. Переключайтесь между **"Graph"** и **"Table"**

### Примеры PromQL Запросов

#### Базовые Метрики

```promql
# Текущая глубина очереди
doradura_queue_depth

# Daily Active Users
doradura_daily_active_users

# Total Revenue
doradura_revenue_total_stars

# Активные подписки
doradura_active_subscriptions
```

#### Rate - Скорость за Период

```promql
# Загрузок в секунду (за последние 5 минут)
rate(doradura_download_success_total[5m])

# Ошибок в секунду
rate(doradura_download_failure_total[5m])

# По формату
rate(doradura_format_requests_total{format="mp3"}[5m])
```

#### Aggregate - Суммирование

```promql
# Всего загрузок в секунду (все форматы)
sum(rate(doradura_download_success_total[5m]))

# По формату
sum by (format) (rate(doradura_download_success_total[5m]))

# По качеству
sum by (quality) (rate(doradura_download_success_total[5m]))
```

#### Calculations - Вычисления

```promql
# Success Rate (%)
sum(rate(doradura_download_success_total[5m])) /
(sum(rate(doradura_download_success_total[5m])) +
 sum(rate(doradura_download_failure_total[5m]))) * 100

# Error Rate (%)
sum(rate(doradura_download_failure_total[5m])) /
(sum(rate(doradura_download_success_total[5m])) +
 sum(rate(doradura_download_failure_total[5m]))) * 100

# Conversion Rate (%)
rate(doradura_new_subscriptions_total[1h]) /
rate(doradura_command_usage_total{command="start"}[1h]) * 100
```

#### Histograms - Перцентили

```promql
# Медианная длительность загрузки (p50)
histogram_quantile(0.5,
  rate(doradura_download_duration_seconds_bucket[5m]))

# 95-й перцентиль
histogram_quantile(0.95,
  rate(doradura_download_duration_seconds_bucket[5m]))

# 99-й перцентиль
histogram_quantile(0.99,
  rate(doradura_download_duration_seconds_bucket[5m]))

# По формату
histogram_quantile(0.95,
  rate(doradura_download_duration_seconds_bucket{format="mp3"}[5m]))
```

#### Time Ranges - За Период

```promql
# Загрузок за последний час
increase(doradura_download_success_total[1h])

# Выручка за сегодня
increase(doradura_revenue_total_stars[1d])

# Новых подписок за неделю
increase(doradura_new_subscriptions_total[7d])
```

### Targets - Проверка Источников

1. Перейдите на вкладку **"Status" → "Targets"**
2. Найдите **"doradura-bot"**
3. Должно быть:
   - **State:** `UP` (зелёный)
   - **Endpoint:** `http://host.docker.internal:9094/metrics`
   - **Last Scrape:** недавно (< 15 секунд назад)

### Alerts - Активные Оповещения

1. Перейдите на вкладку **"Alerts"**
2. Увидите все настроенные alerts
3. Активные будут **красными**
4. Неактивные - **зелёными**

---

## 3️⃣ Telegram - Прямо в Боте

### Админские Команды

Отправьте боту (от имени админа):

#### `/analytics` - Общий Дашборд

```
📊 Analytics Dashboard

⚡ Performance (last 24h)
• Downloads: 1,234
• Success rate: 98.5%
• Avg duration: 8.3s

💰 Business
• Revenue: 150⭐
• Active subs: 42
• New today: 5

🏥 Health
• Queue: 3 tasks
• Error rate: 1.5%
• yt-dlp: ✅ OK

👥 Engagement
• DAU: 85
• Commands: 523
• Top format: MP3
```

#### `/health` - Состояние Системы

```
🏥 System Health Report

⏰ Uptime: 2d 5h 23m

📊 Queue Status
• Total: 3 tasks
• High priority: 0
• Medium: 2
• Low: 1

❌ Errors (last 24h)
• ytdlp: 5
• network: 2
• rate_limit: 0

✅ System Status
Bot: Running
Database: OK
yt-dlp: OK
```

#### `/metrics performance` - Performance Метрики

```
⚡ Performance Metrics

📥 Downloads (last 24h)
• Total: 1,234
• Success: 1,215 (98.5%)
• Failed: 19 (1.5%)

⏱️ Duration
• Average: 8.3s
• p95: 15.2s
• p99: 25.8s

📊 Queue
• Current depth: 3
• Avg wait time: 2.1s
```

#### `/metrics business` - Business Метрики

```
💰 Business Metrics

💵 Revenue
• Total: 150⭐
• Premium: 100⭐
• VIP: 50⭐

👥 Subscriptions
• Active: 42
• New (24h): 5
• Cancelled (24h): 1

📈 Conversion
• Rate: 2.3%
• Checkout starts: 218
• Completed: 5
```

#### `/metrics engagement` - User Engagement

```
👥 User Engagement

📊 Activity
• DAU: 85
• MAU: 523
• DAU/MAU: 16.3%

🎵 Format Preferences
• MP3: 65%
• MP4: 30%
• Subtitles: 5%

📱 Commands (24h)
• /download: 523
• /start: 45
• /help: 12
```

#### `/revenue` - Финансовая Аналитика

```
💰 Revenue Analytics

📊 All-time
• Total: 1,250⭐
• Premium: 850⭐
• VIP: 400⭐

📅 This Month
• Revenue: 150⭐
• New subs: 25

🎯 Conversion Funnel
• Visitors: 1,000
• Checkout: 50 (5%)
• Paid: 25 (50%)
```

---

## 📱 Raw Metrics - Для Разработки

### Bot Metrics Endpoint

```bash
# Все метрики в Prometheus формате
curl http://localhost:9094/metrics

# С pipe в less для пролистывания
curl -s http://localhost:9094/metrics | less

# Grep конкретную метрику
curl -s http://localhost:9094/metrics | grep download_success

# Health endpoint
curl http://localhost:9094/health | jq
```

### Prometheus API

```bash
# Query API
curl -s 'http://localhost:9091/api/v1/query?query=doradura_queue_depth' | jq

# Query range (временной диапазон)
curl -s 'http://localhost:9091/api/v1/query_range?query=rate(doradura_download_success_total[5m])&start=2025-12-14T00:00:00Z&end=2025-12-14T23:59:59Z&step=1m' | jq

# Все targets
curl -s http://localhost:9091/api/v1/targets | jq

# Активные alerts
curl -s http://localhost:9091/api/v1/alerts | jq
```

### Grafana API

```bash
# Все дашборды
curl -s -u admin:admin http://localhost:3000/api/search | jq

# Конкретный дашборд
curl -s -u admin:admin http://localhost:3000/api/dashboards/uid/doradura-overview | jq

# Datasources
curl -s -u admin:admin http://localhost:3000/api/datasources | jq
```

---

## 🎓 Практические Сценарии

### Сценарий 1: Проверить Performance

**Вопрос:** Как быстро работают загрузки?

**Grafana:**
1. Откройте дашборд
2. Смотрите панель **"Download Duration"**
3. p95 показывает: 95% загрузок быстрее этого времени

**Prometheus:**
```promql
histogram_quantile(0.95,
  rate(doradura_download_duration_seconds_bucket[5m]))
```

**Telegram:**
```
/metrics performance
```

### Сценарий 2: Найти Проблемы

**Вопрос:** Почему много ошибок?

**Grafana:**
1. Панель **"Errors by Category"**
2. Смотрите какая категория больше всего
3. ytdlp errors → проблема с yt-dlp
4. network errors → проблема с сетью

**Prometheus:**
```promql
# Топ категорий ошибок
topk(5, sum by (category) (rate(doradura_errors_total[1h])))
```

**Telegram:**
```
/health
# Смотрите секцию "Errors"
```

### Сценарий 3: Анализ Выручки

**Вопрос:** Сколько заработали?

**Grafana:**
1. Панель **"Total Revenue"**
2. Видите общую сумму

**Prometheus:**
```promql
# Total
doradura_revenue_total_stars

# По планам
sum by (plan) (doradura_revenue_by_plan)

# Рост за 24 часа
increase(doradura_revenue_total_stars[1d])
```

**Telegram:**
```
/revenue
```

### Сценарий 4: Мониторинг Очереди

**Вопрос:** Не переполнена ли очередь?

**Grafana:**
1. Панель **"Queue Depth"**
2. Если > 50 → может быть проблема

**Prometheus:**
```promql
# Текущая глубина
doradura_queue_depth

# Максимум за час
max_over_time(doradura_queue_depth[1h])

# Alert if > 100
doradura_queue_depth > 100
```

**Telegram:**
```
/health
# Смотрите "Queue Status"
```

---

## 🔍 Advanced: Создание Своих Графиков

### В Grafana

1. Нажмите **"+"** (Add panel) в дашборде
2. Выберите **"Add a new panel"**
3. В **"Query"** введите PromQL
4. Настройте визуализацию:
   - Time series (линии)
   - Gauge (круглая шкала)
   - Stat (число)
   - Bar chart (столбцы)
   - Table (таблица)
5. Нажмите **"Apply"**

**Пример:** График загрузок по часам

```promql
sum(increase(doradura_download_success_total[1h]))
```

### В Prometheus

1. Вкладка **"Graph"**
2. Введите PromQL запрос
3. **"Add Graph"** чтобы добавить ещё один на той же странице
4. Сравнивайте несколько метрик одновременно

---

## 📊 Рекомендуемый Workflow

### Ежедневная Проверка

```bash
# Telegram (быстро)
/analytics
/health
```

### Еженедельный Анализ

1. **Grafana** → смотрите дашборд за последние 7 дней
2. Обращайте внимание на:
   - Тренды Success Rate
   - Рост Revenue
   - Изменения в Error Rate

### При Проблемах

1. **Telegram** `/health` → общее состояние
2. **Grafana** → детальный анализ графиков
3. **Prometheus** → сложные запросы для investigation

### Для Презентаций/Отчётов

1. **Grafana** → Share dashboard → Snapshot
2. Или экспорт в PDF (требует плагин)
3. Или скриншоты панелей

---

## 💡 Pro Tips

### Grafana

- **Shift + Click** на временном графике → zoom in
- **Variables** → создайте переменные для фильтров (format, quality)
- **Annotations** → отметьте важные события (деплои, инциденты)
- **Playlists** → автоматическая ротация дашбордов на TV

### Prometheus

- **`{__name__=~"doradura.*"}`** → все метрики бота
- **Recording rules** → уже созданы для часто используемых запросов
- **Console** → вкладка для экспериментов

### Telegram

- Настройте **cron** для автоматической отправки `/analytics` каждый день
- Используйте для быстрых проверок с телефона

---

## 🎯 Итоговая Шпаргалка

| Что Смотрим | Grafana | Prometheus | Telegram |
|-------------|---------|------------|----------|
| **Быстрый обзор** | ✅ Дашборд | ❌ | ✅ `/analytics` |
| **Детальный анализ** | ✅✅✅ | ✅✅ | ❌ |
| **Сложные запросы** | ✅✅ | ✅✅✅ | ❌ |
| **Мобильный доступ** | ⚠️ Неудобно | ⚠️ Неудобно | ✅✅✅ |
| **Визуализация** | ✅✅✅ | ✅ | ❌ |
| **Экспорт/Share** | ✅✅✅ | ⚠️ | ❌ |

**Рекомендация:**
- **Каждый день:** Telegram
- **Каждую неделю:** Grafana
- **При расследовании:** Prometheus

---

## 📚 Дополнительно

### Обучающие Ресурсы

- [PromQL Tutorial](https://prometheus.io/docs/prometheus/latest/querying/basics/)
- [Grafana Tutorials](https://grafana.com/tutorials/)
- [Query Examples](https://prometheus.io/docs/prometheus/latest/querying/examples/)

### Готовые Дашборды

Ваш дашборд: `grafana/dashboards/doradura_overview.json`

Можете создать дополнительные:
- Business Dashboard (только revenue/subscriptions)
- Technical Dashboard (только performance/errors)
- Executive Dashboard (high-level KPIs)

---

**Начните с Grafana** → http://localhost:3000 🚀
