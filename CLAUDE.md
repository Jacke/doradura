# Claude Code Instructions for doradura

## General Rules

Do what the user asks directly. Do not suggest the user do tasks manually — execute them yourself using available tools. If a tool is restricted, find an alternative approach.

## Build & Verify

Primary languages: Rust and Go. Use `cargo check` for Rust and `go build` for Go after every edit. For Rust, ensure no Send/Sync issues with async code — gather DB data before async calls.

# ⛔⛔⛔ STOP! READ THIS FIRST ⛔⛔⛔

## 🚨 ПЕРЕД ЛЮБЫМ git commit/push - ОСТАНОВИСЬ И СПРОСИ! 🚨

### Абсолютные запреты (НИКОГДА не нарушай):

❌ **НИКОГДА** не делай `git commit` без вопроса "Можно закоммитить?"
❌ **НИКОГДА** не делай `git push` без явного подтверждения пользователя
❌ **НИКОГДА** не деплой без разрешения

### Обязательный процесс для git операций:

✅ **ШАГ 1**: Покажи `git diff` или опиши что изменилось
✅ **ШАГ 2**: Спроси: "Можно закоммитить и задеплоить?"
✅ **ШАГ 3**: ЖДИ явного "да" или "нет" от пользователя
✅ **ШАГ 4**: Только после "да" - делай commit/push

### Чеклист перед git командой:

Перед выполнением `git commit` или `git push` ОБЯЗАТЕЛЬНО проверь:

- [ ] Пользователь **ЯВНО** сказал "да" на этот коммит?
- [ ] Я показал что именно изменилось?
- [ ] Я СПРОСИЛ "Можно закоммитить?" и получил ответ?

**Если хотя бы на один вопрос ответ "нет" - НЕ КОММИТЬ!**

---

## Deployment

Always use `git push` to trigger CI/CD pipeline for Railway deployments. NEVER use `railway deploy` directly. Railway auto-deploys when GHCR image updates via Source Image auto-update.

## Git Workflow

When working with git branches, always run `git status` and `git branch` before committing. Never mix unrelated changes into a single commit. Confirm you're on the correct branch before making any edits.

## Bug Fixing

After fixing a bug, search the entire codebase for duplicate implementations of the same logic before considering the fix complete. Use `Grep` to find all instances across all crates.

## TUI Development

When the user says to simplify or reduce visual noise, the problem is usually structural (too many items/options), not cosmetic. Propose structural changes (removing/merging menu entries, collapsing categories) before making style-only edits.

## ⚠️ CRITICAL: yt-dlp Args Testing

**ПЕРЕД ЛЮБЫМ изменением в файлах download/** (ytdlp.rs, metadata.rs, source/*, pipeline.rs):

1. Сделай `cargo check`
2. **ОБЯЗАТЕЛЬНО** запусти smoke test на Railway ПЕРЕД коммитом:
```bash
railway ssh --service doradura -- sh -c '
yt-dlp -o /tmp/t1.mp3 --newline --force-overwrites --no-playlist --age-limit 99 --concurrent-fragments 1 --fragment-retries 10 --socket-timeout 30 --http-chunk-size 10485760 --retries 15 --extract-audio --audio-format mp3 --audio-quality 0 --add-metadata --embed-thumbnail --extractor-args youtubepot-bgutilhttp:base_url=http://127.0.0.1:4416 --cookies /data/youtube_cookies.txt --extractor-args youtube:player_client=default --js-runtimes deno --no-check-certificate -N 4 --postprocessor-args "ffmpeg:-acodec libmp3lame -b:a 320k" "https://youtu.be/jNQXAC9IVRw" 2>&1 | tail -3
ls -lh /tmp/t1.mp3 && echo "PASS" || echo "FAIL"
rm -f /tmp/t1.mp3 /tmp/t1.webm
'
```
3. Если тест FAIL — **НЕ КОММИТЬ**, найди проблему
4. Полный тест (MP3+MP4): `scripts/test-ytdlp-args.sh`

**Причина:** изменение порядка args yt-dlp ломает ВСЕ скачивания. `-N` между `--postprocessor-args` и его значением = production outage.

## ⚠️ CRITICAL: Railway Commands

**`railway run` выполняется ЛОКАЛЬНО на Mac, НЕ в Railway контейнере!**

- ❌ `railway run --service doradura <command>` - запускает команду на ЛОКАЛЬНОЙ машине
- ✅ `railway ssh --service doradura` - заходит В Railway контейнер
- ✅ После `railway ssh` можно выполнять команды внутри контейнера

**Всегда помни:** если нужно проверить что-то ВНУТРИ Railway контейнера - используй `railway ssh`, НЕ `railway run`!

## Code Audits

When performing code audits, ALWAYS read and follow the severity criteria defined in [`docs/SEVERITY_CRITERIA.md`](docs/SEVERITY_CRITERIA.md). Do NOT inflate severity to fill quotas — if only 3 issues are CRITICAL, report 3, not 50.

## CRITICAL RULES

### Commits and Deployments
- **NEVER make commits without explicit user confirmation**
- **NEVER push to GitHub without explicit user confirmation**
- **NEVER deploy without explicit user confirmation**
- Always ask: "Можно закоммитить и задеплоить?" and WAIT for response
- Do NOT commit, push, or deploy automatically - ALWAYS ask first
- After making code changes, show what changed and ASK before committing

### Code Changes
- Explain what you're going to change before doing it
- For large changes, show the plan first

### Версионирование (SemVer)

**ОБЯЗАТЕЛЬНО** обновляй `version` в `Cargo.toml` при каждом коммите по правилам SemVer:

Проект pre-1.0 (`0.x.y`), поэтому:

| Изменение | Bump | Пример |
|-----------|------|--------|
| **Ломающие изменения API**, удаление публичных функций, смена сигнатур, миграции БД | **MINOR** (`0.X.0`) | `0.2.0` → `0.3.0` |
| **Новая фича**, новый модуль, новый endpoint, заметное расширение функционала | **MINOR** (`0.X.0`) | `0.2.0` → `0.3.0` |
| **Баг-фикс**, мелкие правки, рефакторинг без изменения поведения, обновление зависимостей | **PATCH** (`0.0.X`) | `0.2.0` → `0.2.1` |

**Процесс:**
1. Определи степень изменений (breaking/feature/fix)
2. Обнови `version` в `Cargo.toml` **до** коммита
3. Укажи новую версию в commit message если bump значительный

**Примеры commit prefixes:**
- `feat:` → MINOR bump (новая фича)
- `fix:` → PATCH bump (баг-фикс)
- `refactor:` → PATCH bump (без изменения поведения)
- `perf:` → PATCH bump (оптимизация)
- `feat!:` или `BREAKING:` → MINOR bump (ломающее изменение)

### CHANGELOG

**ОБЯЗАТЕЛЬНО** обновляй `CHANGELOG.md` при каждом коммите:
- Добавляй запись в секцию `[Unreleased]` (или создавай новую версию если bump)
- Формат: [Keep a Changelog](https://keepachangelog.com/)
- Категории: Added, Changed, Fixed, Removed, Security

## ⚠️ CRITICAL: Telegram Bot API State в Docker

### НЕ ТРОГАЙ init-data очистку `/data` без крайней необходимости

**Инцидент (2026-03-09):** Попытка сохранить binlog для быстрого рестарта уронила прод на ~1 час.

**Что случилось:**
1. Попытались сохранять binlog между рестартами (ради ~10с старта вместо ~150с)
2. `chown -R 1000:2000 /data` забирал ownership binlog у `telegram-bot-api` → Permission denied crash loop
3. Crash loop повредил поддиректории Bot API → Bot API застрял в вечном "restart"
4. Удаление только `*.binlog` файлов недостаточно — нужно чистить и директории с внутренним state

**Правила:**
- **Bot API хранит state НЕ ТОЛЬКО в `*.binlog`** — есть поддиректории в `/data/*/` с внутренней БД. Частичная очистка = corrupted state
- **Init скрипт ДОЛЖЕН удалять ВСЕ Bot API директории** (содержащие binlog) + все `*.binlog` файлы
- **Изменения в init скриптах контейнера ОСОБЕННО ОПАСНЫ** — каждый фикс = 8-20 мин Docker build. Тестируй на staging
- **Permissions в `/data`**: sqlite → `botuser:shareddata`, всё остальное → `telegram-bot-api:shareddata`
- **Если хочешь ускорить старт** — это отдельная задача, не hotfix. Нужен гранулярный chown и staging-тест

## YouTube Downloads on Railway

### CRITICAL: Proxy is REQUIRED
- **YouTube downloads on Railway DO NOT work without proxy**
- Railway IPs are flagged by YouTube bot detection
- MUST use proxy (WARP or Tailscale) for downloads to work
- DO NOT suggest removing WARP_PROXY - it's essential
