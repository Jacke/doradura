# 🔍 Debugging Railway Environment Variables

## Problem
Vars added in Railway Dashboard are not visible to the bot:
```
⚠️  YTDL_COOKIES_FILE: not set
⚠️  YTDL_COOKIES_BROWSER: not set
```

## ✅ Checklist

### 1) Correct service
A project can have multiple services.
- In Dashboard, ensure vars are in the **same service** that runs the bot (usually matches the GitHub repo name).
- Only one active service should contain these vars.

### 2) Correct environment
Projects may have multiple environments (prod/staging/etc.).
- Top-right: select the active environment (default "production").
- Add vars to that environment.

### 3) Correct spelling
Vars must be exact, without quotes or spaces:
| Variable Name | Value | Notes |
|--------------|-------|-------|
| `TELOXIDE_TOKEN` | `your_bot_token` | |
| `YTDL_COOKIES_FILE` | `youtube_cookies.txt` | |

Do **not** use:
- Quoted names/values like `"TELOXIDE_TOKEN"` or `"youtube_cookies.txt"`
- Leading/trailing spaces or newlines

### 4) Deployment succeeded
- Deployments → Latest → status must be **Success**.
- If Failed/Crashed, inspect logs.

### 5) Service restarted
Railway should restart after var changes, but if not:
- Open Logs → click "Restart" or redeploy.

### 6) Logs confirm vars
Check logs for entries indicating env vars were read (or missing). Add temporary logging if needed.

### 7) Shell inspection
Open a Shell from the deployment:
```bash
env | grep YTDL
```
If empty, vars were not applied to this service/environment.

### 8) Secret value length
Ensure the full token/cookie path is pasted; avoid clipped values.

### 9) Redeploy after changes
If vars were added after initial deploy, trigger a redeploy so the new env is applied.

If all checks pass and vars are still missing, recreate them carefully and redeploy. DOC