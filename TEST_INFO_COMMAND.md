# Testing the /info Command

## Run the bot
1. Stop any running bot instances:
```bash
pkill -f doradura
```
2. Start the bot (pick one):
   - **Option A: .env**
     ```bash
     source .env
     ./target/debug/doradura
     ```
   - **Option B: `run_with_cookies.sh`**
     ```bash
     ./run_with_cookies.sh
     ```
   - **Option C: Background**
     ```bash
     source .env
     nohup ./target/debug/doradura > bot.log 2>&1 &
     ```

## Test the command
Send the bot:
```
/info https://www.youtube.com/watch?v=dQw4w9WgXcQ
```
Or without URL (should show usage):
```
/info
```

## View logs
Logs are in `app.log`.
```bash
tail -f app.log
```
Or after the run:
```bash
tail -100 app.log | grep -A 30 "info command"
```

## What to look for
Expected log sequence:
1. `🎯 Received command: Info from chat XXX`
2. `⚡ Command::Info matched, calling handle_info_command`
3. `📋 /info command called`
4. `✅ Message text found: '/info ...'`
5. `📊 Parts count: 2 - Parts: ["/info", "URL"]`
6. `🔗 Extracted URL: ...`
7. `📤 Sending 'processing' message...`
8. `🔍 Fetching metadata from yt-dlp...`
9. `✅ Metadata fetched successfully`
10. `📤 Sending formatted response...`
11. `✅ Response sent successfully!`

## If it fails
Check logs for `❌` errors or `⚠️` warnings and where the sequence stops.

## Stop the bot
```bash
pkill -f doradura
```
