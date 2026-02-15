# Admin Commands

All admin commands require the user's Telegram ID to be in `ADMIN_IDS` or match `ADMIN_USER_ID`.

## Public Commands (in /start menu)

| Command | Description |
|---------|-------------|
| `/admin` | Show admin dashboard |
| `/users` | List users with stats |
| `/setplan <user_id> <plan> [days]` | Set user plan (free/premium/vip) |
| `/charges [N]` | Show recent Telegram Star charges |
| `/transactions` | Show payment transactions |
| `/backup` | Create and send database backup |
| `/health` | System health check |
| `/analytics` | Usage analytics |
| `/metrics [period]` | Download/upload metrics |
| `/revenue` | Revenue report |
| `/downloads` | Active download queue |
| `/uploads` | Upload statistics |
| `/cuts` | Cut/trim statistics |
| `/version` | Bot version info |
| `/botapi_speed` | Bot API server speed test |
| `/sent_files [user_id]` | Show sent files for a user |
| `/download_tg <file_id>` | Download a file by Telegram file_id |
| `/downsub_health` | Downsub service health check |

## Hidden Commands (not in menu)

These commands are not registered in the Telegram command menu and won't appear in autocomplete.

| Command | Description |
|---------|-------------|
| `/update_cookies` | Update YouTube cookies from file |
| `/diagnose_cookies` | Diagnose cookie issues |
| `/update_ytdlp` | Update yt-dlp binary |
| `/browser_login` | Trigger browser-based YouTube login |
| `/browser_status` | Check browser login status |
| `/send <user_id> <message>` | Send a message to a user |
| `/broadcast <message>` | Broadcast a message to all users |

## Usage Examples

### /send — Direct Message

Send a message to a specific user on behalf of the bot:

```
/send 123456789 Hello! Your issue has been resolved.
```

- The target user must exist in the database
- If the user has blocked the bot, you'll get a notification
- The message is sent as plain text

### /broadcast — Mass Message

Send a message to all registered users:

```
/broadcast We've added a new feature! Try /settings to check it out.
```

- Rate-limited to ~28 messages/second (Telegram allows max 30/sec)
- The admin who sends the broadcast is skipped (you already see the message)
- After completion, you get a summary: sent / blocked / failed counts
- Blocked users (who stopped the bot) are counted separately from errors
