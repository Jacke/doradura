# Snapshot Index

Complete list of all available snapshots for testing.

## 📊 Statistics

- **Total snapshots**: 7
- **Total API interactions**: 17
- **Coverage**: Commands, settings, downloads, errors
- **Tests**: 18 (11 in bot_commands_test + 7 in bot_snapshots_test)

## 📁 Snapshot List

| Snapshot | Type | Interactions | Description |
|----------|------|--------------|-------------|
| [start_command](#start_command) | Command | 1 | /start command with main menu |
| [info_command](#info_command) | Command | 1 | Format information |
| [settings_menu](#settings_menu) | Command | 1 | Settings menu |
| [language_selection](#language_selection) | Flow | 3 | Interface language selection |
| [youtube_processing](#youtube_processing) | Flow | 3 | YouTube URL processing |
| [audio_download_complete](#audio_download_complete) | Flow | 5 | Complete audio download cycle |
| [rate_limit_error](#rate_limit_error) | Error | 1 | Rate limit exceeded |

## 🔍 Detailed Description

### start_command
```
Type: Command
API Calls: 1
Methods: sendMessage
```
**Scenario**: User sends /start
**Response**: Welcome message with inline keyboard (Information, Settings, Downloads)
**Test**: `test_start_command_from_snapshot`

---

### info_command
```
Type: Command
API Calls: 1
Methods: sendMessage
```
**Scenario**: User requests /info
**Response**: Detailed information about:
- Video formats (2160p, 1440p, 1080p, 720p, 480p, 360p)
- Audio formats (320kbps, 192kbps, 128kbps)
- Supported services (YouTube, SoundCloud, Vimeo)

**Test**: `test_info_command_snapshot`

---

### settings_menu
```
Type: Command
API Calls: 1
Methods: sendMessage
```
**Scenario**: User opens /settings
**Response**: Settings menu with current parameters:
- Video quality: 1080p
- Audio bitrate: 192 kbps
- Default format: Audio
- Buttons to change each parameter

**Test**: `test_settings_menu_snapshot`

---

### language_selection
```
Type: Flow (Multi-step)
API Calls: 3
Methods: sendMessage → answerCallbackQuery → editMessageText
```
**Scenario**:
1. Show language selection menu (🇷🇺 Russian / 🇬🇧 English)
2. User selects Russian
3. Callback query confirmation
4. Update settings menu with new language

**Test**: `test_language_selection_flow`

---

### youtube_processing
```
Type: Flow (Multi-step)
API Calls: 3
Methods: sendMessage → sendPhoto → deleteMessage
```
**Scenario**:
1. Send "⏳ Processing link..." message
2. Send preview with thumbnail and quality options:
   - 🎵 Audio 320kbps / 192kbps
   - 📹 Video 1080p / 720p / 480p
3. Delete temporary message

**URL**: `https://www.youtube.com/watch?v=dQw4w9WgXcQ`
**Video**: Rick Astley - Never Gonna Give You Up
**Test**: `test_youtube_processing_flow`

---

### audio_download_complete
```
Type: Flow (Multi-step)
API Calls: 5
Methods: editMessageCaption (x3) → sendAudio → deleteMessage
```
**Scenario**:
1. Progress update: 0%
2. Progress update: 45%
3. Progress update: 100%
4. Send audio file:
   - Performer: Rick Astley
   - Title: Never Gonna Give You Up
   - Duration: 3:33 (213 sec)
   - Bitrate: 192 kbps
   - Size: 5 MB
5. Delete progress message

**Test**: `test_audio_download_complete_flow`

---

### rate_limit_error
```
Type: Error
API Calls: 1
Methods: sendMessage
```
**Scenario**: User exceeds the request rate limit
**Response**:
- Error message
- Remaining time: 45 seconds
- Suggestion to subscribe (/plan)

**Error Type**: rate_limit
**Test**: `test_rate_limit_error_snapshot`

---

## 🎯 API Method Coverage

| API Method | Snapshots | Uses |
|------------|-----------|------|
| sendMessage | start_command, info_command, settings_menu, language_selection, youtube_processing, rate_limit_error | 6 |
| sendPhoto | youtube_processing | 1 |
| sendAudio | audio_download_complete | 1 |
| deleteMessage | youtube_processing, audio_download_complete | 2 |
| editMessageCaption | audio_download_complete | 3 |
| editMessageText | language_selection | 1 |
| answerCallbackQuery | language_selection | 1 |

**Total**: 7 distinct API methods

## 🧪 How to use

### Load a snapshot
```rust
let snapshot = TelegramSnapshot::load_by_name("youtube_processing")?;
```

### Create a mock server
```rust
let mock = TelegramMock::from_snapshot("youtube_processing").await?;
let bot = mock.create_bot()?;
```

### Verify structure
```rust
assert_eq!(snapshot.interactions.len(), 3);
let (call, response) = &snapshot.interactions[0];
assert_eq!(call.path, "/sendMessage");
```

## 📝 Creating new snapshots

### Recommended scenarios to add:

1. **video_download_complete.json** - Complete video download cycle
2. **settings_change_quality.json** - Change video quality
3. **downloads_list.json** - View download history
4. **cuts_menu.json** - Cuts menu
5. **invalid_url_error.json** - Error for invalid URL
6. **subscription_info.json** - Subscription information
7. **admin_commands.json** - Admin commands

### Command to create
```bash
./tools/log_to_snapshot.py --interactive
```

## 🔗 See also

- [Full documentation](../../docs/SNAPSHOT_TESTING.md)
- [Quick start](../../docs/SNAPSHOT_TESTING_QUICKSTART.md)
- [Test examples](../bot_commands_test.rs)
