# 🌐 Docker Networking: Доступ между Контейнерами и Хостом

## Проблема

Как из Docker контейнера получить доступ к сервисам на хост-машине?

```
❌ localhost:9094      # Не работает из контейнера (указывает на сам контейнер)
❌ 127.0.0.1:9094      # Тоже не работает
```

## ✅ Решения

### macOS и Windows (Docker Desktop)

Используйте специальный DNS-имя:

```yaml
✅ host.docker.internal:9094
```

**Это автоматически разрешается в IP хост-машины.**

#### Проверка из контейнера

```bash
# Запустить временный контейнер
docker run --rm -it alpine sh

# Внутри контейнера:
ping host.docker.internal
curl http://host.docker.internal:9094/health
```

### Linux

На Linux есть 3 варианта:

#### Вариант 1: `host.docker.internal` через extra_hosts (✅ Используется)

Добавьте в `docker-compose.yml`:

```yaml
services:
  prometheus:
    extra_hosts:
      - "host.docker.internal:host-gateway"
```

**Уже настроено!** Теперь `host.docker.internal` работает и на Linux.

#### Вариант 2: IP адрес Docker bridge

```bash
# Найти IP docker0
ip addr show docker0

# Обычно это:
172.17.0.1
```

В `prometheus.yml`:
```yaml
- targets: ['172.17.0.1:9094']
```

#### Вариант 3: Network mode host

```yaml
services:
  prometheus:
    network_mode: host  # Использовать сеть хоста напрямую
```

⚠️ **Внимание**: В этом режиме порт 9091 будет недоступен (конфликт с портом 9090 на хосте).

---

## 🗺️ Карта Сетевого Доступа

### Из Хоста → Контейнеры

```
localhost:9091  → Prometheus (порт проброшен)
localhost:3000  → Grafana (порт проброшен)
localhost:9093  → AlertManager (порт проброшен)
```

Работает благодаря `ports` в docker-compose:
```yaml
ports:
  - "9091:9090"  # host:container
```

### Из Контейнера → Хост

```
# macOS/Windows:
host.docker.internal:9094  → Bot metrics server

# Linux (с extra_hosts):
host.docker.internal:9094  → Bot metrics server

# Linux (без extra_hosts):
172.17.0.1:9094  → Bot metrics server
```

### Между Контейнерами

Используйте имена сервисов:

```yaml
# Prometheus → Grafana
prometheus:9090

# Grafana → Prometheus
prometheus:9090

# Любой → AlertManager
alertmanager:9093
```

Работает благодаря Docker DNS внутри сети `monitoring`:
```yaml
networks:
  monitoring:
    driver: bridge
```

---

## 📊 Ваша Текущая Архитектура

```
┌─────────────────────────────────────────────────────────────┐
│                      HOST MACHINE                            │
│                                                               │
│  Bot (Rust) :9094                                            │
│      ↑                                                        │
│      │ host.docker.internal:9094                            │
│      │                                                        │
│  ┌───┴──────────────────────────────────────────────────┐   │
│  │              Docker Network: monitoring               │   │
│  │                                                        │   │
│  │  ┌──────────────┐  ┌──────────────┐  ┌────────────┐ │   │
│  │  │ Prometheus   │  │   Grafana    │  │AlertManager│ │   │
│  │  │   :9090      │  │    :3000     │  │   :9093    │ │   │
│  │  │ (внутри)     │  │  (внутри)    │  │ (внутри)   │ │   │
│  │  └──────┬───────┘  └──────────────┘  └────────────┘ │   │
│  │         │                                             │   │
│  │         │ Scrapes: host.docker.internal:9094         │   │
│  │         └─────────────────────────────────────────┐  │   │
│  │                                                     ↓  │   │
│  └─────────────────────────────────────────────────────┘   │
│                                                               │
│  Port Mappings (доступны извне):                            │
│    :9091 → Prometheus:9090                                  │
│    :3000 → Grafana:3000                                     │
│    :9093 → AlertManager:9093                                │
└─────────────────────────────────────────────────────────────┘

Browser:
  http://localhost:9091 → Prometheus UI
  http://localhost:3000 → Grafana UI
  http://localhost:9093 → AlertManager UI
```

---

## 🔍 Диагностика

### Проверить что бот слушает на правильном интерфейсе

```bash
# Бот должен слушать на 0.0.0.0, а не на 127.0.0.1
lsof -i :9094

# Должно быть:
# *:9094 (LISTEN)  ← хорошо, слушает на всех интерфейсах
#
# Не должно быть:
# 127.0.0.1:9094 (LISTEN)  ← плохо, только localhost
```

Проверьте в коде metrics_server:
```rust
// ✅ Правильно
let addr = SocketAddr::from(([0, 0, 0, 0], port));

// ❌ Неправильно
let addr = SocketAddr::from(([127, 0, 0, 1], port));
```

### Проверить доступность из контейнера

```bash
# Запустить shell в контейнере Prometheus
docker exec -it doradura-prometheus sh

# Внутри контейнера:
# Проверить что host.docker.internal резолвится
getent hosts host.docker.internal

# Проверить доступность метрик
wget -O- http://host.docker.internal:9094/metrics
# или
curl http://host.docker.internal:9094/metrics
```

### Проверить targets в Prometheus

```bash
# Из хоста
curl http://localhost:9091/api/v1/targets | jq '.data.activeTargets[] | select(.labels.job=="doradura-bot")'

# Должно показать:
{
  "health": "up",
  "labels": {
    "instance": "doradura-bot",
    "job": "doradura-bot"
  },
  "lastScrape": "2025-12-14T10:00:00Z",
  "scrapeUrl": "http://host.docker.internal:9094/metrics"
}
```

### Проверить логи Prometheus

```bash
docker logs doradura-prometheus

# Если есть ошибки подключения:
# "context deadline exceeded" → бот недоступен
# "connection refused" → порт закрыт или неправильный
# "no such host" → DNS не резолвится
```

---

## 🐛 Типичные Проблемы

### 1. "Connection refused" из контейнера

**Причина**: Бот слушает только на 127.0.0.1

**Решение**: Убедитесь что бот слушает на `0.0.0.0:9094`

```rust
// src/core/metrics_server.rs
let addr = SocketAddr::from(([0, 0, 0, 0], port));
```

### 2. "No such host: host.docker.internal" на Linux

**Причина**: На Linux это имя не работает из коробки

**Решение**: Используйте `extra_hosts` (уже добавлено в docker-compose.yml):
```yaml
extra_hosts:
  - "host.docker.internal:host-gateway"
```

### 3. Firewall блокирует

**macOS/Linux**: Проверьте firewall rules

```bash
# macOS
sudo /usr/libexec/ApplicationFirewall/socketfilterfw --listapps

# Linux (ufw)
sudo ufw status
```

**Решение**: Разрешите входящие подключения на порт 9094

### 4. Неправильная конфигурация prometheus.yml

```yaml
# ❌ Неправильно
- targets: ['localhost:9094']

# ✅ Правильно
- targets: ['host.docker.internal:9094']
```

---

## 🚀 Production: Railway

На Railway сервисы общаются через internal network:

### Internal Domains

```yaml
# prometheus.yml для Railway
scrape_configs:
  - job_name: 'doradura-bot'
    static_configs:
      - targets: ['doradura-bot.railway.internal:9094']
      # Или если в том же проекте:
      - targets: ['doradura-bot:9094']
```

Railway автоматически создает DNS записи для сервисов.

### Проверка в Railway

```bash
# В терминале сервиса
railway run bash

# Внутри:
curl http://doradura-bot.railway.internal:9094/metrics
```

---

## 📝 Checklist

### Development (Local)

- [x] `extra_hosts` добавлен в docker-compose.yml
- [x] `prometheus.yml` использует `host.docker.internal:9094`
- [ ] Бот слушает на `0.0.0.0:9094` (не на `127.0.0.1`)
- [ ] Порт 9094 не заблокирован firewall
- [ ] `curl http://localhost:9094/metrics` работает с хоста
- [ ] Targets в Prometheus показывают "up"

### Production (Railway)

- [ ] Используйте internal domain: `doradura-bot.railway.internal`
- [ ] Или имя сервиса: `doradura-bot`
- [ ] Не используйте `host.docker.internal` в production

---

## 💡 Best Practices

1. **Development**: Используйте `host.docker.internal` с `extra_hosts`
2. **Production**: Используйте internal service names
3. **Metrics Server**: Всегда слушайте на `0.0.0.0`, не на `127.0.0.1`
4. **Docker Networks**: Используйте bridge network для изоляции
5. **Port Mapping**: Пробрасывайте только нужные порты

---

## 🔗 Полезные Ссылки

- [Docker Networking Docs](https://docs.docker.com/network/)
- [Docker Desktop Networking](https://docs.docker.com/desktop/networking/)
- [Railway Private Networking](https://docs.railway.app/reference/private-networking)

---

## ✅ Итог

**Текущая конфигурация работает на:**
- ✅ macOS (Docker Desktop)
- ✅ Windows (Docker Desktop)
- ✅ Linux (благодаря `extra_hosts`)

**Настройка:**
- Prometheus scrapes: `host.docker.internal:9094`
- Работает кроссплатформенно
- Нет ручной настройки IP адресов
