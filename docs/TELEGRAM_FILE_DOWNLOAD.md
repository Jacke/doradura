# Downloading Files from Telegram

## Overview

The bot supports downloading files from Telegram by their `file_id`. This functionality is useful for recovering files that were sent by the bot but are no longer present on the local server.

## Features

- Download any files from Telegram by `file_id`
- Automatic saving to the `./downloads/` directory
- Preserves the original filename from Telegram
- Supports all file types (documents, photos, videos, audio)
- Admin-only access
- Logging of all operations

## Admin Command

### `/download_tg <file_id>`

Downloads a file from Telegram and saves it locally.

**Access**: Admin only

**Syntax**:
```
/download_tg <file_id>
```

**Usage example**:
```
/download_tg BQACAgIAAxkBAAIBCGXxxx...
```

**Bot response on success**:
```
File downloaded successfully!

Path: ./downloads/document.pdf
Name: document.pdf
Size: 2.45 MB
File ID: BQACAgIAAxkBAAIBCGXxxx...
```

**Bot response on error**:
```
Error downloading file:

[Error description]

Possible causes:
- Invalid file_id
- File was deleted from Telegram
- File is too old (>1 hour for non-documents)
- No access rights to the file
```

## How to get a file_id?

### Method 1: From the bot's database

If the file was sent by the bot and saved in history:

```sql
SELECT file_id, title FROM download_history
WHERE user_id = 123456789
ORDER BY downloaded_at DESC;
```

### Method 2: Via Telegram Bot API

1. Send a file to the bot (or forward an existing message with a file)
2. Use the `getUpdates` method or webhook to get the `file_id`
3. For documents: `message.document.file_id`
4. For photos: `message.photo[last].file_id`
5. For videos: `message.video.file_id`
6. For audio: `message.audio.file_id`

### Method 3: Bot logs

When sending files, the bot logs the `file_id`:
```
[INFO] Sent audio file: file_id = BQACAgIAAxkBAAIBCGXxxx...
```

## Programmatic Usage

### Function `download_file_from_telegram`

The main function for downloading files from Telegram.

**Signature**:
```rust
pub async fn download_file_from_telegram(
    bot: &Bot,
    file_id: &str,
    destination_path: Option<PathBuf>,
) -> Result<PathBuf>
```

**Parameters**:
- `bot` - Telegram bot instance
- `file_id` - Telegram file_id of the file to download
- `destination_path` - Optional path to save the file. If `None`, the file is saved to `./downloads/`

**Returns**:
- `Ok(PathBuf)` - Path to the downloaded file
- `Err(anyhow::Error)` - Download error

**Code usage example**:
```rust
use doradura::telegram::download_file_from_telegram;
use std::path::PathBuf;

// Download to default directory (./downloads/)
let path = download_file_from_telegram(&bot, "BQACAgIAAxkBAAIBCGXxxx...", None).await?;
println!("File saved: {:?}", path);

// Download to a specific location
let custom_path = PathBuf::from("./backups/my_file.pdf");
let path = download_file_from_telegram(&bot, "BQACAgIAAxkBAAIBCGXxxx...", Some(custom_path)).await?;
println!("File saved: {:?}", path);
```

## Telegram API Limitations

### File retention periods

Telegram stores files on its servers with different retention periods depending on the type:

| File type | Retention period |
|-----------|-----------------|
| Documents | Indefinitely (until deleted by the user) |
| Photos | 1 hour after upload |
| Video/Audio | 1 hour after upload |
| Voice/Video messages | 1 hour after upload |

**Important**: For reliable file recovery, use the "document" type when sending via the bot.

### File sizes

- The Telegram Bot API supports downloading files up to **20 MB**
- When using a local Telegram Bot API Server, the limit depends on the startup mode. If the server is launched without `--local`, it generally inherits the official Bot API limits.

The current bot configuration uses a local Bot API Server (if `BOT_API_URL` is set), so larger files are supported.

## Download Directory

By default all files are saved to:
```
./downloads/
```

The directory is created automatically on the first download.

**Structure**:
```
./downloads/
├── document_123.pdf
├── audio_456.mp3
├── video_789.mp4
└── file_ABC.bin
```

The filename is taken from the Telegram path (`file.path`). If the path contains no filename, a name is generated based on the `file_id`.

## Logging

All download operations are logged:

```log
[INFO] Starting download for file_id: BQACAgIAAxkBAAIBCGXxxx...
[INFO] File info retrieved: path = documents/file_123.pdf, size = 2567890 bytes
[INFO] Destination path: "./downloads/file_123.pdf"
[INFO] File downloaded successfully to: "./downloads/file_123.pdf"
[INFO] File size: 2567890 bytes (2.45 MB)
```

## Security

### Access Control

- The `/download_tg` command is available **to admins only**
- The check is performed via the `is_admin(username)` function
- The admin is determined by the `ADMIN_USERNAME` environment variable

### Validation

- Checks that `file_id` is present in the command
- Handles invalid `file_id` values
- Graceful handling of network and filesystem errors

## Usage Scenario Examples

### Scenario 1: Recovering a lost file

1. A user reports that a downloaded file is corrupted
2. The admin finds the `file_id` in the database:
   ```sql
   SELECT file_id FROM download_history WHERE user_id = 123456789 AND title LIKE '%song%';
   ```
3. The admin downloads the original:
   ```
   /download_tg BQACAgIAAxkBAAIBCGXxxx...
   ```
4. Verifies the file and resends it to the user

### Scenario 2: Backing up important files

Script for backing up all files from history:

```rust
use doradura::telegram::download_file_from_telegram;
use doradura::storage::db;

async fn backup_all_files(bot: &Bot, db_pool: &DbPool) -> Result<()> {
    let conn = db::get_connection(db_pool)?;
    let history = db::get_all_download_history(&conn)?;

    for record in history {
        if let Some(file_id) = record.file_id {
            let backup_path = PathBuf::from(format!("./backups/{}", record.id));
            match download_file_from_telegram(bot, &file_id, Some(backup_path)).await {
                Ok(path) => println!("Backed up: {:?}", path),
                Err(e) => eprintln!("Failed to backup {}: {}", file_id, e),
            }
        }
    }

    Ok(())
}
```

### Scenario 3: Migrating between servers

1. On the old server, export all `file_id` values:
   ```sql
   SELECT file_id FROM download_history WHERE file_id IS NOT NULL;
   ```
2. On the new server, download the files:
   ```bash
   while read file_id; do
       echo "/download_tg $file_id" | send-to-bot
   done < file_ids.txt
   ```

## Integration with Existing Code

### Saving file_id on send

Make sure that when sending files to users, you save the `file_id` to the database:

```rust
// After a successful send
let sent_message = bot.send_document(chat_id, document).await?;
let file_id = sent_message.document().map(|d| d.file.id.clone());

// Save to history
save_download_history(
    &conn,
    user_id,
    url,
    title,
    format,
    file_id.as_deref(), // Pass file_id
)?;
```

## Troubleshooting

### Error: "Failed to download file"

**Causes**:
1. File deleted from Telegram (>1 hour has passed for photos/videos)
2. Invalid `file_id`
3. Network issues

**Solution**:
- Verify that the `file_id` is correct
- Make sure the file is still available in Telegram
- Check the bot logs for error details

### Error: "Permission denied"

**Causes**:
1. No write permissions on the `./downloads/` directory
2. Directory is write-protected

**Solution**:
```bash
mkdir -p ./downloads
chmod 755 ./downloads
```

### Error: "File too large"

**Causes**:
1. File exceeds the Bot API limit (20 MB)
2. Local Bot API Server is not configured

**Solution**:
- Configure a local Telegram Bot API Server
- Set `BOT_API_URL` in the environment variables

## FAQ

**Q: Can I download a file that was sent more than a year ago?**
A: Yes, if it is a document (`document`). Telegram stores documents indefinitely. Photos and videos are only kept for 1 hour.

**Q: Will downloading work after a bot restart?**
A: Yes, `file_id` remains valid regardless of bot restarts.

**Q: Can I download a file from another bot?**
A: No, `file_id` is tied to a specific bot. Files from other bots are not accessible.

**Q: Where is information about downloaded files stored?**
A: In the SQLite database in the `download_history` table (column `file_id`).

## See Also

- [Telegram Bot API - File](https://core.telegram.org/bots/api#file)
- [Telegram Bot API - getFile](https://core.telegram.org/bots/api#getfile)
- [Local Bot API Server](https://github.com/tdlib/telegram-bot-api)
