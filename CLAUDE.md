# Claude Code Instructions for doradura

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
