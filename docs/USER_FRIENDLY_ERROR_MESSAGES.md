# User-Friendly Error Messages

## Problem

Users were receiving overly technical and frustrating error messages:

**Before:**
```
Cookies for YouTube are expired or invalid.

Please update cookies.
Contact @stansob.
```

**Issues:**
1. Mentioning the admin in every error message is annoying to the user
2. Technical details about "cookies" are not understood by regular users
3. Message is too long
4. Gives no actionable advice ("update cookies" - how?)

## Solution

All error messages have been rewritten into user-friendly versions:

### 1. Invalid Cookies

**Before:**
```
Cookies for YouTube are expired or invalid.

Please update cookies.
Contact @stansob.
```

**After:**
```
Temporary issue with YouTube.

Try a different video or retry later.
```

**Why it's better:**
- Does not mention technical details
- Does not alarm the user
- Gives simple advice: try a different video
- No admin mention

### 2. Bot Detection

**Before:**
```
YouTube blocked the request (bot detected).

The issue is resolved by updating cookies.
Contact @stansob.
```

**After:**
```
YouTube blocked the request.

Try a different video or retry later.
```

**Why it's better:**
- Removed "(bot detected)" - does not alarm the user
- Does not mention cookies
- Simple advice

### 3. Video Unavailable

**Before:**
```
Video unavailable.

Possible reasons:
• Video is private or deleted
• Regional restrictions
• Video blocked by the author
```

**After:**
```
Video unavailable.

It may be private, deleted, or blocked in your region.
```

**Why it's better:**
- Shorter
- Same reasons but in one line
- Friendly tone

### 4. Network Error

**Before:**
```
Network issue while retrieving data.
```

**After:**
```
Network issue.

Try again in a minute.
```

**Why it's better:**
- Shorter
- Specific advice: wait a minute

### 5. Unknown Error

**Before:**
```
Could not retrieve video data.

Check that the link is correct and the video is available.
Contact @stansob.
```

**After:**
```
Could not download the video.

Check that the link is correct.
```

**Why it's better:**
- Shorter
- No admin mention
- Simple advice

## Code Changes

[src/download/ytdlp_errors.rs:94-112](src/download/ytdlp_errors.rs#L94-L112)

```rust
pub fn get_error_message(error_type: &YtDlpErrorType) -> String {
    match error_type {
        YtDlpErrorType::InvalidCookies => {
            "Temporary issue with YouTube.\n\nTry a different video or retry later.".to_string()
        }
        YtDlpErrorType::BotDetection => {
            "YouTube blocked the request.\n\nTry a different video or retry later.".to_string()
        }
        YtDlpErrorType::VideoUnavailable => {
            "Video unavailable.\n\nIt may be private, deleted, or blocked in your region.".to_string()
        }
        YtDlpErrorType::NetworkError => {
            "Network issue.\n\nTry again in a minute.".to_string()
        }
        YtDlpErrorType::Unknown => {
            "Could not download the video.\n\nCheck that the link is correct.".to_string()
        }
    }
}
```

### Removed admin_contact_line() Function

The function is no longer needed since we no longer mention the admin in user-facing messages.

**Removed:**
```rust
fn admin_contact_line() -> Option<String> {
    let admin_username = crate::core::config::admin::ADMIN_USERNAME.as_str();
    if admin_username.is_empty() {
        None
    } else {
        Some(format!("\nContact @{}.", admin_username))
    }
}
```

## Where the Admin Still Receives Notifications

Important: **The admin still receives detailed notifications** about critical errors!

[src/download/ytdlp_errors.rs:141-149](src/download/ytdlp_errors.rs#L141-L149)

```rust
pub fn should_notify_admin(error_type: &YtDlpErrorType) -> bool {
    match error_type {
        YtDlpErrorType::InvalidCookies => true,   // Admin will be notified
        YtDlpErrorType::BotDetection => true,     // Admin will be notified
        YtDlpErrorType::VideoUnavailable => false,
        YtDlpErrorType::NetworkError => false,
        YtDlpErrorType::Unknown => true,          // Admin will be notified
    }
}
```

**What the admin receives:**
```
YTDLP ERROR (video download)
user_chat_id: 53170594
url: https://www.youtube.com/watch?v=...
error_type: InvalidCookies

command:
/opt/homebrew/bin/yt-dlp -o /Users/stan/downloads/...

stdout (tail):
[youtube] Extracting URL: https://www.youtube.com/...

stderr (tail):
WARNING: [youtube] Cookies are no longer valid. Re-extracting...
ERROR: [youtube] Sign in to confirm you're not a bot.

recommendations:
RECOMMENDATIONS FOR FIXING:
• Cookies are expired or were updated in the browser
...
```

## Principles of User-Friendly Messages

### 1. Do Not Alarm the User

Bad:
- "Cookies are invalid"
- "Bot detected"
- "Extraction signature failed"

Good:
- "Temporary issue"
- "Try again later"
- "Video unavailable"

### 2. Give Actionable Advice

Bad:
- "Update cookies" (how?)
- "Check the configuration" (which one?)

Good:
- "Try a different video"
- "Retry later"
- "Check that the link is correct"

### 3. Be Brief

Bad:
```
Video unavailable.

Possible reasons:
• Video is private or deleted
• Regional restrictions
• Video blocked by the author
• Copyright issues
• Temporary service unavailability
```

Good:
```
Video unavailable.

It may be private, deleted, or blocked in your region.
```

### 4. Do Not Mention Technical Information

Bad:
- "HTTP Error 403"
- "Signature extraction failed"
- "Cookie rotation detected"

Good:
- "Access issue"
- "Temporary issue"
- "Try again later"

### 5. Do Not Mention the Admin Unnecessarily

Bad:
```
Error.

Contact @admin.
```

Good:
```
Error.

Try again.
```

**When to mention the admin:**
- Only if the problem requires manual intervention
- If it is a critical bug
- If the user has already tried multiple times

## Before/After Comparison

| Situation | Before | After |
|----------|-------|-------|
| **Invalid Cookies** | "Cookies are invalid. Update cookies. Contact @admin" (3 lines, admin mention) | "Temporary issue with YouTube. Try a different video" (2 lines, simple advice) |
| **Bot Detection** | "YouTube detected a bot. Update cookies. Contact @admin" (3 lines, scary) | "YouTube blocked the request. Try again later" (2 lines, calm) |
| **Video Unavailable** | List of 3 bullet points (verbose) | One line with possible reasons (concise) |
| **Network Error** | "Network issue while retrieving data" (unclear what to do) | "Network issue. Try again in a minute" (specific advice) |
| **Unknown** | "Could not retrieve data. Contact @admin" (admin mention) | "Could not download video. Check the link" (simple advice) |

## Real Scenario Examples

### Scenario 1: Expired Cookies

**User sends:**
```
https://www.youtube.com/watch?v=dQw4w9WgXcQ
```

**Bot replies:**
```
Temporary issue with YouTube.

Try a different video or retry later.
```

**User thinks:**
- "Ok, temporary issue, I'll try another video"
- Does not panic, does not write to admin

**Admin receives:**
```
YTDLP ERROR (video download)
user_chat_id: 12345
error_type: InvalidCookies
...
RECOMMENDATIONS: Update cookies
```

**Admin:**
- Sees the issue
- Updates cookies
- Problem resolved for all users

### Scenario 2: Video Unavailable

**User sends:**
```
https://www.youtube.com/watch?v=private_video
```

**Bot replies:**
```
Video unavailable.

It may be private, deleted, or blocked in your region.
```

**User thinks:**
- "Right, the video is private"
- Will try another video
- Does not write to admin (because it's their problem, not the bot's)

**Admin:**
- Does NOT receive a notification (this is not a bot bug)

### Scenario 3: Network Issue

**User sends:**
```
https://www.youtube.com/watch?v=some_video
```

**Bot replies:**
```
Network issue.

Try again in a minute.
```

**User:**
- Waits a minute
- Tries again
- Works!

**Admin:**
- Does NOT receive spam from temporary network issues

## Metrics Remain Detailed

Important: **Metrics are still detailed and show real error types!**

```bash
curl http://localhost:9094/metrics | grep errors_total
```

```
doradura_errors_total{error_type="invalid_cookies",operation="metadata"} 5
doradura_errors_total{error_type="bot_detection",operation="video_download"} 2
doradura_errors_total{error_type="video_unavailable",operation="audio_download"} 12
...
```

**In Grafana:**
- Admin sees exact error categories
- Can track trends
- Can configure alerts

## Best Practices

### For User Messages

1. **Be brief** - maximum 2 lines
2. **Use plain language** - no technical terms
3. **Give actionable advice** - what can the user do?
4. **Do not alarm** - avoid words like "error", "failure", "critical"
5. **Do not mention the admin** - only if truly critical

### For Logs and Admin Notifications

1. **Be detailed** - all technical information
2. **Include context** - URL, user_id, command, stderr
3. **Give recommendations** - how to fix
4. **Use structure** - easy to read and parse

### For Metrics

1. **Use detailed categories** - invalid_cookies, bot_detection, etc.
2. **Add labels** - operation (metadata, audio_download, video_download)
3. **Always increment** - even if not shown to the user

## Summary

**Users:**
- See short, understandable messages
- Are not alarmed by technical terms
- Do not spam the admin over trivial issues
- Receive actionable advice

**Admin:**
- Receives detailed notifications about critical errors
- Sees full logs and stderr
- Has recommendations for fixing
- Can track metrics in Grafana

**Metrics and Monitoring:**
- Remain detailed
- Show real error types
- Alerts can be configured
- History is saved for analysis

## Related Files

- [src/download/ytdlp_errors.rs](src/download/ytdlp_errors.rs) - Error messages
- [ERROR_METRICS_COMPREHENSIVE.md](ERROR_METRICS_COMPREHENSIVE.md) - Error metrics
- [HOW_TO_VIEW_METRICS.md](HOW_TO_VIEW_METRICS.md) - How to view metrics
