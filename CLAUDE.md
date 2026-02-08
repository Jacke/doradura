# Claude Code Instructions for doradura

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

## ⚠️ CRITICAL: Railway Commands

**`railway run` выполняется ЛОКАЛЬНО на Mac, НЕ в Railway контейнере!**

- ❌ `railway run --service doradura <command>` - запускает команду на ЛОКАЛЬНОЙ машине
- ✅ `railway ssh --service doradura` - заходит В Railway контейнер
- ✅ После `railway ssh` можно выполнять команды внутри контейнера

**Всегда помни:** если нужно проверить что-то ВНУТРИ Railway контейнера - используй `railway ssh`, НЕ `railway run`!

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

## YouTube Downloads on Railway

### CRITICAL: Proxy is REQUIRED
- **YouTube downloads on Railway DO NOT work without proxy**
- Railway IPs are flagged by YouTube bot detection
- MUST use proxy (WARP or Tailscale) for downloads to work
- DO NOT suggest removing WARP_PROXY - it's essential
