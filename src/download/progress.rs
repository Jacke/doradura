use crate::core::{escape_markdown_v2 as escape_markdown, extract_retry_after};
use crate::i18n;
use crate::telegram::Bot;
use fluent_templates::fluent_bundle::FluentArgs;
use teloxide::prelude::*;
use teloxide::types::MessageId;
use unic_langid::LanguageIdentifier;

// ======================== Progress Bar Styles ========================

/// Selectable progress bar visual style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProgressBarStyle {
    #[default]
    Classic, // [█████░░░░░]
    Gradient, // ▓▓▓▓▒▒░░░░
    Emoji,    // 🟩🟩🟩🟩⬜⬜⬜⬜
    Dots,     // ● ● ● ● ○ ○ ○ ○
    Runner,   // ━━━🏃░░░░░░
    Rpg,      // ❤️ BOSS ████░░ 50HP
    Fire,     // 🔥🔥🔥░░░░░░░
    Moon,     // 🌕🌕🌖🌑🌑
}

impl ProgressBarStyle {
    /// Parse from stored string value.
    pub fn parse(s: &str) -> Self {
        match s {
            "classic" => Self::Classic,
            "gradient" => Self::Gradient,
            "emoji" => Self::Emoji,
            "dots" => Self::Dots,
            "runner" => Self::Runner,
            "rpg" => Self::Rpg,
            "fire" => Self::Fire,
            "moon" => Self::Moon,
            _ => Self::Classic,
        }
    }

    /// Serialize to string for DB storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Classic => "classic",
            Self::Gradient => "gradient",
            Self::Emoji => "emoji",
            Self::Dots => "dots",
            Self::Runner => "runner",
            Self::Rpg => "rpg",
            Self::Fire => "fire",
            Self::Moon => "moon",
        }
    }

    /// Human-readable display name.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Classic => "Classic",
            Self::Gradient => "Gradient",
            Self::Emoji => "Emoji",
            Self::Dots => "Dots",
            Self::Runner => "Runner",
            Self::Rpg => "RPG Boss",
            Self::Fire => "Fire",
            Self::Moon => "Moon",
        }
    }

    /// Preview string (short example at 50%).
    pub fn preview(&self) -> &'static str {
        match self {
            Self::Classic => "[█████░░░░░]",
            Self::Gradient => "▓▓▓▓▓▒▒░░░",
            Self::Emoji => "🟩🟩🟩🟩🟩⬜⬜⬜⬜⬜",
            Self::Dots => "● ● ● ● ● ○ ○ ○ ○ ○",
            Self::Runner => "━━━━━🏃░░░░",
            Self::Rpg => "❤️ ████░░░░ 50HP",
            Self::Fire => "🔥🔥🔥🔥🔥░░░░░",
            Self::Moon => "🌕🌕🌕🌕🌕🌑🌑🌑🌑🌑",
        }
    }

    /// All available styles, in display order.
    pub fn all() -> &'static [Self] {
        &[
            Self::Classic,
            Self::Gradient,
            Self::Emoji,
            Self::Dots,
            Self::Runner,
            Self::Rpg,
            Self::Fire,
            Self::Moon,
        ]
    }
}

/// Returns a display name for the source (host) of a URL.
pub fn source_display_name(url: &url::Url) -> &'static str {
    match url.host_str().map(|h| h.to_lowercase()).as_deref() {
        Some(h) if h.contains("youtube") || h.contains("youtu.be") => "YouTube",
        Some(h) if h.contains("soundcloud") => "SoundCloud",
        Some(h) if h.contains("instagram") => "Instagram",
        Some(h) if h.contains("tiktok") => "TikTok",
        Some(h) if h.contains("twitter") || h.contains("x.com") => "X",
        Some(h) if h.contains("vimeo") => "Vimeo",
        Some(h) if h.contains("twitch") => "Twitch",
        Some(h) if h.contains("bandcamp") => "Bandcamp",
        Some(h) if h.contains("reddit") => "Reddit",
        Some(h) if h.contains("rutube") => "Rutube",
        Some(h) if h.contains("vk.com") => "VK",
        _ => "Web",
    }
}

/// Download state for displaying progress to the user.
///
/// Used to track the various stages of the download and file sending process.
#[derive(Debug, Clone)]
pub enum DownloadStatus {
    /// Download is starting
    Starting {
        /// File/track title
        title: String,
        /// File format for emoji selection: "mp3", "mp4", "srt", "txt" (optional)
        file_format: Option<String>,
        /// Artist name (optional)
        artist: Option<String>,
    },
    /// Download in progress with a progress bar
    Downloading {
        /// File/track title
        title: String,
        /// Download progress in percent (0-100)
        progress: u8,
        /// Download speed in MB/s (optional)
        speed_mbs: Option<f64>,
        /// Estimated time remaining in seconds (optional)
        eta_seconds: Option<u64>,
        /// Current size in bytes (optional)
        current_size: Option<u64>,
        /// Total size in bytes (optional)
        total_size: Option<u64>,
        /// File format for emoji selection: "mp3", "mp4", "srt", "txt" (optional)
        file_format: Option<String>,
        /// Update counter for emoji animation
        update_count: u32,
        /// Artist name (optional)
        artist: Option<String>,
    },
    /// Sending file to the Telegram server
    Uploading {
        /// File/track title
        title: String,
        /// Number of dots for animation (0-3)
        dots: u8,
        /// Approximate upload progress in percent (0-100, optional)
        progress: Option<u8>,
        /// Upload speed in MB/s (optional)
        speed_mbs: Option<f64>,
        /// Estimated time remaining in seconds (optional)
        eta_seconds: Option<u64>,
        /// Current size in bytes (optional)
        current_size: Option<u64>,
        /// Total size in bytes (optional)
        total_size: Option<u64>,
        /// File format for emoji selection: "mp3", "mp4", "srt", "txt" (optional)
        file_format: Option<String>,
        /// Update counter for emoji animation
        update_count: u32,
        /// Artist name (optional)
        artist: Option<String>,
    },
    /// Successful download with timing information
    Success {
        /// File/track title
        title: String,
        /// Elapsed time in seconds
        elapsed_secs: u64,
        /// File format for emoji selection: "mp3", "mp4", "srt", "txt" (optional)
        file_format: Option<String>,
    },
    /// Final state (title only, no additional information)
    Completed {
        /// File/track title
        title: String,
        /// File format for emoji selection: "mp3", "mp4", "srt", "txt" (optional)
        file_format: Option<String>,
    },
    /// Download error
    Error {
        /// File/track title
        title: String,
        /// Error description
        error: String,
        /// File format for emoji selection: "mp3", "mp4", "srt", "txt" (optional)
        file_format: Option<String>,
    },
}

impl DownloadStatus {
    /// Returns the emoji based on the file format (static, for Starting/Success/Completed/Error)
    fn get_emoji(file_format: Option<&String>) -> &'static str {
        match file_format {
            Some(format) => match format.as_str() {
                "mp4" | "mp4+mp3" => "🎬",
                "srt" => "📝",
                "txt" => "📄",
                _ => "🎵",
            },
            None => "🎵",
        }
    }

    /// Returns an animated emoji that alternates on each progress update
    fn get_animated_emoji(file_format: Option<&String>, update_count: u32) -> &'static str {
        let is_even = update_count.is_multiple_of(2);
        match file_format {
            Some(format) => match format.as_str() {
                "mp4" | "mp4+mp3" => {
                    if is_even {
                        "🎬"
                    } else {
                        "🎥"
                    }
                }
                "srt" => "📝",
                "txt" => "📄",
                _ => {
                    if is_even {
                        "🎵"
                    } else {
                        "🎶"
                    }
                }
            },
            None => {
                if is_even {
                    "🎵"
                } else {
                    "🎶"
                }
            }
        }
    }

    /// Returns a speed emoji based on MB/s value
    fn speed_emoji(speed_mbs: f64) -> &'static str {
        if speed_mbs < 1.0 {
            "🐌"
        } else if speed_mbs < 5.0 {
            "⚡"
        } else if speed_mbs < 20.0 {
            "🚀"
        } else {
            "💨"
        }
    }

    /// Generates the formatted status message text for the current state.
    ///
    /// Formats the message according to Telegram MarkdownV2 syntax,
    /// including a progress bar for the downloading state and escaping of special characters.
    ///
    /// # Returns
    ///
    /// A string with the formatted download status message.
    ///
    /// # Example
    ///
    /// ```
    /// use doradura::download::progress::{DownloadStatus, ProgressBarStyle};
    ///
    /// let status = DownloadStatus::Downloading {
    ///     title: "Test Song".to_string(),
    ///     progress: 50,
    ///     speed_mbs: None,
    ///     eta_seconds: None,
    ///     current_size: None,
    ///     total_size: None,
    ///     file_format: Some("mp3".to_string()),
    ///     update_count: 0,
    ///     artist: None,
    /// };
    /// let lang: unic_langid::LanguageIdentifier = "ru".parse().unwrap();
    /// let message = status.to_message(&lang, ProgressBarStyle::default(), None);
    /// ```
    pub fn to_message(&self, lang: &LanguageIdentifier, style: ProgressBarStyle, source_badge: Option<&str>) -> String {
        match self {
            DownloadStatus::Starting {
                title,
                file_format,
                artist,
            } => {
                let escaped = escape_markdown(title);
                let emoji = Self::get_emoji(file_format.as_ref());
                let starting_text = escape_markdown(&i18n::t(lang, "progress.starting"));
                let mut s = String::with_capacity(escaped.len() + starting_text.len() + 100);
                s.push_str(emoji);
                s.push_str(" *");
                s.push_str(&escaped);
                s.push('*');
                if let Some(a) = artist.as_deref().filter(|a| !a.is_empty()) {
                    s.push_str("\n👤 ");
                    s.push_str(&escape_markdown(a));
                }
                if let Some(badge) = source_badge.filter(|b| !b.is_empty()) {
                    s.push_str("\n📺 ");
                    s.push_str(&escape_markdown(badge));
                }
                s.push_str("\n\n⏳ ");
                s.push_str(&starting_text);
                s
            }
            DownloadStatus::Downloading {
                title,
                progress,
                speed_mbs,
                eta_seconds,
                current_size,
                total_size,
                file_format,
                update_count,
                artist,
            } => {
                let escaped = escape_markdown(title);
                let emoji = Self::get_animated_emoji(file_format.as_ref(), *update_count);
                let bar = create_progress_bar(*progress, style);
                let downloading_text = escape_markdown(&i18n::t(lang, "progress.downloading"));
                let mut s = String::with_capacity(escaped.len() + bar.len() + 200);
                s.push_str(emoji);
                s.push_str(" *");
                s.push_str(&escaped);
                s.push('*');
                if let Some(a) = artist.as_deref().filter(|a| !a.is_empty()) {
                    s.push_str("\n👤 ");
                    s.push_str(&escape_markdown(a));
                }
                if let Some(badge) = source_badge.filter(|b| !b.is_empty()) {
                    s.push_str("\n📺 ");
                    s.push_str(&escape_markdown(badge));
                }
                s.push_str("\n\n📥 ");
                s.push_str(&downloading_text);
                s.push_str(": ");
                s.push_str(&progress.to_string());
                s.push_str("%\n");
                s.push_str(&bar);

                if let Some(speed) = speed_mbs {
                    let speed_label = escape_markdown(&i18n::t(lang, "progress.speed"));
                    let spd_emoji = Self::speed_emoji(*speed);
                    s.push_str("\n\n");
                    s.push_str(spd_emoji);
                    s.push(' ');
                    s.push_str(&speed_label);
                    s.push_str(": ");
                    s.push_str(&format!("{:.1} MB/s", speed).replace('.', "\\."));
                }

                if let Some(eta) = eta_seconds {
                    let minutes = eta / 60;
                    let seconds = eta % 60;
                    let eta_label = escape_markdown(&i18n::t(lang, "progress.eta"));
                    let min_label = escape_markdown(&i18n::t(lang, "progress.min"));
                    let sec_label = escape_markdown(&i18n::t(lang, "progress.sec"));
                    s.push_str("\n⏱️ ");
                    s.push_str(&eta_label);
                    s.push_str(": ");
                    if minutes > 0 {
                        let escaped_min = escape_markdown(&minutes.to_string());
                        let escaped_sec = escape_markdown(&seconds.to_string());
                        s.push_str(&escaped_min);
                        s.push(' ');
                        s.push_str(&min_label);
                        s.push(' ');
                        s.push_str(&escaped_sec);
                        s.push(' ');
                        s.push_str(&sec_label);
                    } else {
                        let escaped_sec = escape_markdown(&seconds.to_string());
                        s.push_str(&escaped_sec);
                        s.push(' ');
                        s.push_str(&sec_label);
                    }
                }

                if let (Some(current), Some(total)) = (current_size, total_size) {
                    let current_mb = *current as f64 / (1024.0 * 1024.0);
                    let total_mb = *total as f64 / (1024.0 * 1024.0);
                    let size_label = escape_markdown(&i18n::t(lang, "progress.size"));
                    s.push_str("\n📦 ");
                    s.push_str(&size_label);
                    s.push_str(": ");
                    s.push_str(&format!("{:.1} / {:.1} MB", current_mb, total_mb).replace('.', "\\."));
                }

                s
            }
            DownloadStatus::Uploading {
                title,
                dots,
                progress,
                speed_mbs,
                eta_seconds,
                current_size,
                total_size,
                file_format,
                update_count,
                artist,
            } => {
                let escaped = escape_markdown(title);
                let emoji = Self::get_animated_emoji(file_format.as_ref(), *update_count);
                let uploading_text = escape_markdown(&i18n::t(lang, "progress.uploading"));
                let mut s = String::with_capacity(escaped.len() + 2000);
                s.push_str(emoji);
                s.push_str(" *");
                s.push_str(&escaped);
                s.push('*');
                if let Some(a) = artist.as_deref().filter(|a| !a.is_empty()) {
                    s.push_str("\n👤 ");
                    s.push_str(&escape_markdown(a));
                }
                if let Some(badge) = source_badge.filter(|b| !b.is_empty()) {
                    s.push_str("\n📺 ");
                    s.push_str(&escape_markdown(badge));
                }
                s.push_str("\n\n📤 ");
                s.push_str(&uploading_text);

                if let Some(p) = *progress {
                    let bar = create_progress_bar(p, style);
                    s.push_str(": ");
                    s.push_str(&p.to_string());
                    s.push_str("%\n");
                    s.push_str(&bar);
                } else {
                    // Sound wave animation for audio, dots for non-audio
                    let is_audio = matches!(file_format.as_deref(), Some("mp3"));
                    if is_audio {
                        const WAVE_FRAMES: &[&str] = &["▁▃▅▇▅▃▁▃", "▃▅▇▅▃▁▃▅", "▅▇▅▃▁▃▅▇", "▇▅▃▁▃▅▇▅"];
                        let frame = WAVE_FRAMES[(*dots as usize) % WAVE_FRAMES.len()];
                        s.push(' ');
                        s.push_str(frame);
                    } else {
                        let dots_count = (*dots % 4) as usize;
                        let dots_str = if dots_count == 0 {
                            String::new()
                        } else {
                            "\\.".repeat(dots_count)
                        };
                        s.push_str(&dots_str);
                    }
                }

                if let Some(speed) = speed_mbs {
                    let speed_label = escape_markdown(&i18n::t(lang, "progress.speed"));
                    let spd_emoji = Self::speed_emoji(*speed);
                    s.push_str("\n\n");
                    s.push_str(spd_emoji);
                    s.push(' ');
                    s.push_str(&speed_label);
                    s.push_str(": ");
                    s.push_str(&format!("{:.1} MB/s", speed).replace('.', "\\."));
                }

                if let Some(eta) = eta_seconds {
                    let minutes = eta / 60;
                    let seconds = eta % 60;
                    let eta_label = escape_markdown(&i18n::t(lang, "progress.eta"));
                    let min_label = escape_markdown(&i18n::t(lang, "progress.min"));
                    let sec_label = escape_markdown(&i18n::t(lang, "progress.sec"));
                    s.push_str("\n⏱️ ");
                    s.push_str(&eta_label);
                    s.push_str(": ");
                    if minutes > 0 {
                        let escaped_min = escape_markdown(&minutes.to_string());
                        let escaped_sec = escape_markdown(&seconds.to_string());
                        s.push_str(&escaped_min);
                        s.push(' ');
                        s.push_str(&min_label);
                        s.push(' ');
                        s.push_str(&escaped_sec);
                        s.push(' ');
                        s.push_str(&sec_label);
                    } else {
                        let escaped_sec = escape_markdown(&seconds.to_string());
                        s.push_str(&escaped_sec);
                        s.push(' ');
                        s.push_str(&sec_label);
                    }
                }

                if let (Some(current), Some(total)) = (current_size, total_size) {
                    let current_mb = *current as f64 / (1024.0 * 1024.0);
                    let total_mb = *total as f64 / (1024.0 * 1024.0);
                    let size_label = escape_markdown(&i18n::t(lang, "progress.size"));
                    s.push_str("\n📦 ");
                    s.push_str(&size_label);
                    s.push_str(": ");
                    s.push_str(&format!("{:.1} / {:.1} MB", current_mb, total_mb).replace('.', "\\."));
                }

                s
            }
            DownloadStatus::Success {
                title,
                elapsed_secs,
                file_format,
            } => {
                let escaped = escape_markdown(title);
                let emoji = Self::get_emoji(file_format.as_ref());
                let mut args = FluentArgs::new();
                args.set("elapsed", *elapsed_secs as i64);
                let success_text = escape_markdown(&i18n::t_args(lang, "progress.success", &args));
                let mut s = String::with_capacity(escaped.len() + success_text.len() + 20);
                s.push_str(emoji);
                s.push_str(" *");
                s.push_str(&escaped);
                s.push_str("*\n\n✅ ");
                s.push_str(&success_text);
                s
            }
            DownloadStatus::Completed { title, file_format } => {
                let escaped = escape_markdown(title);
                let emoji = Self::get_emoji(file_format.as_ref());
                let mut s = String::with_capacity(escaped.len() + 10);
                s.push_str(emoji);
                s.push_str(" *");
                s.push_str(&escaped);
                s.push('*');
                s
            }
            DownloadStatus::Error {
                title,
                error,
                file_format,
            } => {
                let escaped_title = escape_markdown(title);
                let escaped_error = escape_markdown(error);
                let emoji = Self::get_emoji(file_format.as_ref());
                let error_label = escape_markdown(&i18n::t(lang, "progress.error"));
                let mut s = String::with_capacity(escaped_title.len() + escaped_error.len() + error_label.len() + 30);
                s.push_str(emoji);
                s.push_str(" *");
                s.push_str(&escaped_title);
                s.push_str("*\n\n❌ ");
                s.push_str(&error_label);
                s.push_str(": ");
                s.push_str(&escaped_error);
                s
            }
        }
    }
}

/// Creates a visual progress bar in the selected style
fn create_progress_bar(progress: u8, style: ProgressBarStyle) -> String {
    let progress = progress.min(100);
    let filled = (progress / 10) as usize;
    let empty = 10 - filled;

    match style {
        ProgressBarStyle::Classic => {
            format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
        }
        ProgressBarStyle::Gradient => {
            // Smooth gradient: filled=▓, transition=▒, empty=░
            let transition = if filled < 10 && filled > 0 { 1 } else { 0 };
            let grad_filled = if transition > 0 { filled - 1 } else { filled };
            let grad_empty = empty.saturating_sub(transition);
            format!(
                "{}{}{}",
                "▓".repeat(grad_filled + transition.min(1)),
                "▒".repeat(transition),
                "░".repeat(grad_empty)
            )
        }
        ProgressBarStyle::Emoji => {
            format!("{}{}", "🟩".repeat(filled), "⬜".repeat(empty))
        }
        ProgressBarStyle::Dots => {
            let f: Vec<&str> = std::iter::repeat_n("●", filled).collect();
            let e: Vec<&str> = std::iter::repeat_n("○", empty).collect();
            format!("{} {}", f.join(" "), e.join(" "))
        }
        ProgressBarStyle::Runner => {
            if filled == 10 {
                format!("{}🏁", "━".repeat(9))
            } else {
                format!("{}🏃{}", "━".repeat(filled), "░".repeat(empty.saturating_sub(1).max(0)))
            }
        }
        ProgressBarStyle::Rpg => {
            let hp = progress;
            format!("❤️ {}{}  {}HP", "█".repeat(filled), "░".repeat(empty), hp)
        }
        ProgressBarStyle::Fire => {
            format!("{}{}", "🔥".repeat(filled), "░".repeat(empty))
        }
        ProgressBarStyle::Moon => {
            // 🌕 = full, 🌑 = empty, middle segment gets 🌖
            if filled == 0 {
                "🌑".repeat(10)
            } else if filled == 10 {
                "🌕".repeat(10)
            } else {
                format!("{}🌖{}", "🌕".repeat(filled.saturating_sub(1)), "🌑".repeat(empty))
            }
        }
    }
}

// escape_markdown and extract_retry_after are now imported from crate::core

/// Structure for managing the download progress message.
///
/// Tracks the progress message ID and allows updating it as the download proceeds.
pub struct ProgressMessage {
    /// User's chat ID
    pub chat_id: ChatId,
    /// Progress message ID (None if not yet sent)
    pub message_id: Option<MessageId>,
    /// User's language for localizing progress messages
    pub lang: LanguageIdentifier,
    /// Progress bar style chosen by the user
    pub style: ProgressBarStyle,
    /// Source and quality badge (e.g. "YouTube · MP3 320kbps")
    pub source_badge: Option<String>,
}

impl ProgressMessage {
    /// Creates a new progress message for the specified chat.
    ///
    /// # Arguments
    ///
    /// * `chat_id` - User's chat ID
    ///
    /// # Returns
    ///
    /// A new `ProgressMessage` instance with no sent message yet.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use teloxide::types::ChatId;
    /// use doradura::download::progress::ProgressMessage;
    /// use unic_langid::LanguageIdentifier;
    ///
    /// let lang: LanguageIdentifier = "ru".parse().unwrap();
    /// let mut progress = ProgressMessage::new(ChatId(123456789), lang);
    /// ```
    pub fn new(chat_id: ChatId, lang: LanguageIdentifier) -> Self {
        Self {
            chat_id,
            message_id: None,
            lang,
            style: ProgressBarStyle::default(),
            source_badge: None,
        }
    }

    /// Sends or updates the download progress message.
    ///
    /// If the message has not been sent yet, creates a new one. If it already exists,
    /// edits the existing message. On edit failure, sends a new message.
    ///
    /// # Arguments
    ///
    /// * `bot` - Telegram bot instance
    /// * `status` - Current download state
    ///
    /// # Returns
    ///
    /// Returns `ResponseResult<()>` or an error when sending/editing the message.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use doradura::telegram::Bot;
    /// use doradura::download::progress::{ProgressMessage, DownloadStatus};
    /// use teloxide::types::ChatId;
    ///
    /// # async fn example(bot: Bot, chat_id: ChatId) -> teloxide::RequestError {
    /// let lang: unic_langid::LanguageIdentifier = "ru".parse().unwrap();
    /// let mut progress = ProgressMessage::new(chat_id, lang);
    /// progress.update(&bot, DownloadStatus::Starting {
    ///     title: "Test Song".to_string(),
    ///     file_format: Some("mp3".to_string()),
    ///     artist: None,
    /// }).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn update(&mut self, bot: &Bot, status: DownloadStatus) -> ResponseResult<()> {
        let text = status.to_message(&self.lang, self.style, self.source_badge.as_deref());

        if let Some(msg_id) = self.message_id {
            // Update existing message
            match bot
                .edit_message_text(self.chat_id, msg_id, text.clone())
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .await
            {
                Ok(_) => Ok(()),
                Err(e) => {
                    let error_str = e.to_string();
                    // If the message hasn't changed - that's fine, no need to send a new one
                    if error_str.contains("message is not modified") {
                        // This is a normal situation - the message already contains this content
                        // Don't log as error and don't send a new message
                        return Ok(());
                    }

                    // Check rate limiting
                    if let Some(retry_after_secs) = extract_retry_after(&error_str) {
                        log::warn!(
                            "Rate limit hit when editing message: Retry after {}s. Waiting...",
                            retry_after_secs
                        );
                        // Wait the specified time + a small delay for reliability
                        tokio::time::sleep(tokio::time::Duration::from_secs(retry_after_secs + 1)).await;
                        // Try to edit again
                        match bot
                            .edit_message_text(self.chat_id, msg_id, text.clone())
                            .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                            .await
                        {
                            Ok(_) => return Ok(()),
                            Err(e2) => {
                                let error_str2 = e2.to_string();
                                // If rate limited again or another error - send a new message
                                if error_str2.contains("message is not modified") {
                                    return Ok(());
                                }
                                log::warn!(
                                    "Still failed to edit message after rate limit wait: {}. Trying to send new one.",
                                    e2
                                );
                            }
                        }
                    } else {
                        log::warn!("Failed to edit message: {}. Trying to send new one.", e);
                    }

                    // If editing failed for another reason, send a new message
                    let msg = bot
                        .send_message(self.chat_id, text)
                        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                        .await?;
                    self.message_id = Some(msg.id);
                    Ok(())
                }
            }
        } else {
            // Send a new message
            let msg = bot
                .send_message(self.chat_id, text)
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .await?;
            self.message_id = Some(msg.id);
            Ok(())
        }
    }

    /// Clears the message (leaves only the title) after the specified delay.
    ///
    /// Useful for clearing progress details after a successful download, leaving only the file title.
    ///
    /// # Arguments
    ///
    /// * `bot` - Telegram bot instance
    /// * `delay_secs` - Delay in seconds before clearing
    /// * `title` - File title for the final message
    ///
    /// # Returns
    ///
    /// Returns `ResponseResult<()>` or an error when updating the message.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use doradura::telegram::Bot;
    /// use doradura::download::progress::ProgressMessage;
    ///
    /// # async fn example(bot: Bot, mut progress: ProgressMessage) -> teloxide::RequestError {
    /// // Clear the message after 10 seconds
    /// progress.clear_after(&bot, 10, "Test Song".to_string(), Some("mp3".to_string())).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn clear_after(
        &mut self,
        bot: &Bot,
        delay_secs: u64,
        title: String,
        file_format: Option<String>,
    ) -> ResponseResult<()> {
        if self.message_id.is_some() {
            tokio::time::sleep(tokio::time::Duration::from_secs(delay_secs)).await;
            self.update(
                bot,
                DownloadStatus::Completed {
                    title: title.clone(),
                    file_format,
                },
            )
            .await?;
            log::info!(
                "Cleared progress message for chat {} after {} seconds",
                self.chat_id,
                delay_secs
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== create_progress_bar Tests ====================

    #[test]
    fn test_progress_bar() {
        let s = ProgressBarStyle::Classic;
        assert_eq!(create_progress_bar(0, s), "[░░░░░░░░░░]");
        assert_eq!(create_progress_bar(50, s), "[█████░░░░░]");
        assert_eq!(create_progress_bar(100, s), "[██████████]");
    }

    #[test]
    fn test_progress_bar_intermediate_values() {
        let s = ProgressBarStyle::Classic;
        assert_eq!(create_progress_bar(10, s), "[█░░░░░░░░░]");
        assert_eq!(create_progress_bar(25, s), "[██░░░░░░░░]");
        assert_eq!(create_progress_bar(75, s), "[███████░░░]");
        assert_eq!(create_progress_bar(90, s), "[█████████░]");
    }

    #[test]
    fn test_progress_bar_overflow() {
        let s = ProgressBarStyle::Classic;
        // Progress > 100 should be capped
        assert_eq!(create_progress_bar(150, s), "[██████████]");
        assert_eq!(create_progress_bar(255, s), "[██████████]");
    }

    #[test]
    fn test_progress_bar_styles() {
        // Emoji style
        let bar = create_progress_bar(50, ProgressBarStyle::Emoji);
        assert!(bar.contains("🟩"));
        assert!(bar.contains("⬜"));

        // Fire style
        let bar = create_progress_bar(30, ProgressBarStyle::Fire);
        assert!(bar.contains("🔥"));

        // Moon style
        let bar = create_progress_bar(50, ProgressBarStyle::Moon);
        assert!(bar.contains("🌕"));
        assert!(bar.contains("🌑"));

        // RPG style
        let bar = create_progress_bar(50, ProgressBarStyle::Rpg);
        assert!(bar.contains("❤️"));
        assert!(bar.contains("50HP"));
    }

    #[test]
    fn test_progress_bar_style_roundtrip() {
        for style in ProgressBarStyle::all() {
            let s = style.as_str();
            let parsed = ProgressBarStyle::parse(s);
            assert_eq!(*style, parsed, "Roundtrip failed for {}", s);
        }
    }

    #[test]
    fn test_source_display_name() {
        let yt = url::Url::parse("https://www.youtube.com/watch?v=abc").unwrap();
        assert_eq!(source_display_name(&yt), "YouTube");

        let sc = url::Url::parse("https://soundcloud.com/artist/track").unwrap();
        assert_eq!(source_display_name(&sc), "SoundCloud");

        let other = url::Url::parse("https://example.com/file.mp3").unwrap();
        assert_eq!(source_display_name(&other), "Web");
    }

    // ==================== escape_markdown Tests ====================

    #[test]
    fn test_escape_markdown() {
        assert_eq!(escape_markdown("Hello World"), "Hello World");
        assert_eq!(escape_markdown("Test_file.mp3"), "Test\\_file\\.mp3");
        assert_eq!(escape_markdown("Song [2024]"), "Song \\[2024\\]");
    }

    #[test]
    fn test_escape_markdown_all_special() {
        let input = r"_*[]()~`>#+-=|{}.!";
        let expected = r"\_\*\[\]\(\)\~\`\>\#\+\-\=\|\{\}\.\!";
        assert_eq!(escape_markdown(input), expected);
    }

    #[test]
    fn test_escape_markdown_backslash() {
        assert_eq!(escape_markdown("a\\b"), "a\\\\b");
    }

    #[test]
    fn test_escape_markdown_empty() {
        assert_eq!(escape_markdown(""), "");
    }

    // ==================== extract_retry_after Tests ====================

    #[test]
    fn test_extract_retry_after_standard() {
        assert_eq!(extract_retry_after("Retry after 30s"), Some(30));
        assert_eq!(extract_retry_after("retry after 60s"), Some(60));
    }

    #[test]
    fn test_extract_retry_after_colon_format() {
        assert_eq!(extract_retry_after("retry_after: 45"), Some(45));
        assert_eq!(extract_retry_after("retry_after:30"), Some(30));
    }

    #[test]
    fn test_extract_retry_after_no_match() {
        assert_eq!(extract_retry_after("No retry info"), None);
        assert_eq!(extract_retry_after(""), None);
    }

    // ==================== DownloadStatus::get_emoji Tests ====================

    #[test]
    fn test_get_emoji_mp3() {
        assert_eq!(DownloadStatus::get_emoji(Some(&"mp3".to_string())), "🎵");
    }

    #[test]
    fn test_get_emoji_mp4() {
        assert_eq!(DownloadStatus::get_emoji(Some(&"mp4".to_string())), "🎬");
        assert_eq!(DownloadStatus::get_emoji(Some(&"mp4+mp3".to_string())), "🎬");
    }

    #[test]
    fn test_get_emoji_srt() {
        assert_eq!(DownloadStatus::get_emoji(Some(&"srt".to_string())), "📝");
    }

    #[test]
    fn test_get_emoji_txt() {
        assert_eq!(DownloadStatus::get_emoji(Some(&"txt".to_string())), "📄");
    }

    #[test]
    fn test_get_emoji_default() {
        assert_eq!(DownloadStatus::get_emoji(None), "🎵");
        assert_eq!(DownloadStatus::get_emoji(Some(&"unknown".to_string())), "🎵");
    }

    // ==================== DownloadStatus::to_message Tests ====================

    fn test_lang() -> LanguageIdentifier {
        crate::i18n::lang_from_code("ru")
    }

    #[test]
    fn test_status_starting_message() {
        let lang = test_lang();
        let status = DownloadStatus::Starting {
            title: "Test Song".to_string(),
            file_format: Some("mp3".to_string()),
            artist: None,
        };
        let msg = status.to_message(&lang, ProgressBarStyle::default(), None);
        assert!(msg.contains("Test Song"));
        assert!(msg.contains("⏳"));
    }

    #[test]
    fn test_status_starting_with_artist() {
        let lang = test_lang();
        let status = DownloadStatus::Starting {
            title: "Test Song".to_string(),
            file_format: Some("mp3".to_string()),
            artist: Some("Rick Astley".to_string()),
        };
        let msg = status.to_message(&lang, ProgressBarStyle::default(), None);
        assert!(msg.contains("Test Song"));
        assert!(msg.contains("👤"));
        assert!(msg.contains("Rick Astley"));
    }

    #[test]
    fn test_status_downloading_message() {
        let lang = test_lang();
        let status = DownloadStatus::Downloading {
            title: "Test Song".to_string(),
            progress: 50,
            speed_mbs: Some(5.5),
            eta_seconds: Some(30),
            current_size: Some(50 * 1024 * 1024),
            total_size: Some(100 * 1024 * 1024),
            file_format: Some("mp3".to_string()),
            update_count: 0,
            artist: None,
        };
        let msg = status.to_message(&lang, ProgressBarStyle::default(), None);
        assert!(msg.contains("Test Song"));
        assert!(msg.contains("50%"));
        assert!(msg.contains("📥"));
    }

    #[test]
    fn test_status_downloading_speed_emoji() {
        let lang = test_lang();
        // Slow speed
        let status = DownloadStatus::Downloading {
            title: "Test".to_string(),
            progress: 50,
            speed_mbs: Some(0.5),
            eta_seconds: None,
            current_size: None,
            total_size: None,
            file_format: Some("mp3".to_string()),
            update_count: 0,
            artist: None,
        };
        let msg = status.to_message(&lang, ProgressBarStyle::default(), None);
        assert!(msg.contains("🐌"));

        // Fast speed
        let status = DownloadStatus::Downloading {
            title: "Test".to_string(),
            progress: 50,
            speed_mbs: Some(25.0),
            eta_seconds: None,
            current_size: None,
            total_size: None,
            file_format: Some("mp3".to_string()),
            update_count: 0,
            artist: None,
        };
        let msg = status.to_message(&lang, ProgressBarStyle::default(), None);
        assert!(msg.contains("💨"));
    }

    #[test]
    fn test_status_downloading_animated_emoji() {
        let lang = test_lang();
        let status_even = DownloadStatus::Downloading {
            title: "Test".to_string(),
            progress: 50,
            speed_mbs: None,
            eta_seconds: None,
            current_size: None,
            total_size: None,
            file_format: Some("mp3".to_string()),
            update_count: 0,
            artist: None,
        };
        let status_odd = DownloadStatus::Downloading {
            title: "Test".to_string(),
            progress: 50,
            speed_mbs: None,
            eta_seconds: None,
            current_size: None,
            total_size: None,
            file_format: Some("mp3".to_string()),
            update_count: 1,
            artist: None,
        };
        let msg_even = status_even.to_message(&lang, ProgressBarStyle::default(), None);
        let msg_odd = status_odd.to_message(&lang, ProgressBarStyle::default(), None);
        assert!(msg_even.contains("🎵"));
        assert!(msg_odd.contains("🎶"));
    }

    #[test]
    fn test_status_uploading_message() {
        let lang = test_lang();
        let status = DownloadStatus::Uploading {
            title: "Test Song".to_string(),
            dots: 2,
            progress: None,
            speed_mbs: None,
            eta_seconds: None,
            current_size: None,
            total_size: None,
            file_format: None,
            update_count: 0,
            artist: None,
        };
        let msg = status.to_message(&lang, ProgressBarStyle::default(), None);
        assert!(msg.contains("Test Song"));
        assert!(msg.contains("📤"));
    }

    #[test]
    fn test_status_uploading_with_progress() {
        let lang = test_lang();
        let status = DownloadStatus::Uploading {
            title: "Test Song".to_string(),
            dots: 0,
            progress: Some(75),
            speed_mbs: Some(10.0),
            eta_seconds: Some(15),
            current_size: None,
            total_size: None,
            file_format: Some("mp4".to_string()),
            update_count: 0,
            artist: None,
        };
        let msg = status.to_message(&lang, ProgressBarStyle::default(), None);
        assert!(msg.contains("75%"));
    }

    #[test]
    fn test_status_success_message() {
        let lang = test_lang();
        let status = DownloadStatus::Success {
            title: "Test Song".to_string(),
            elapsed_secs: 5,
            file_format: Some("mp3".to_string()),
        };
        let msg = status.to_message(&lang, ProgressBarStyle::default(), None);
        assert!(msg.contains("Test Song"));
        assert!(msg.contains("✅"));
        assert!(msg.contains("5"));
    }

    #[test]
    fn test_status_completed_message() {
        let lang = test_lang();
        let status = DownloadStatus::Completed {
            title: "Test Song".to_string(),
            file_format: Some("mp3".to_string()),
        };
        let msg = status.to_message(&lang, ProgressBarStyle::default(), None);
        assert!(msg.contains("Test Song"));
        assert!(msg.contains("🎵"));
    }

    #[test]
    fn test_status_error_message() {
        let lang = test_lang();
        let status = DownloadStatus::Error {
            title: "Test Song".to_string(),
            error: "Network error".to_string(),
            file_format: Some("mp3".to_string()),
        };
        let msg = status.to_message(&lang, ProgressBarStyle::default(), None);
        assert!(msg.contains("Test Song"));
        assert!(msg.contains("❌"));
        assert!(msg.contains("Network error"));
    }

    #[test]
    fn test_status_message_english() {
        let lang = crate::i18n::lang_from_code("en");
        let status = DownloadStatus::Starting {
            title: "Test Song".to_string(),
            file_format: Some("mp3".to_string()),
            artist: None,
        };
        let msg = status.to_message(&lang, ProgressBarStyle::default(), None);
        assert!(msg.contains("Starting download"));
    }

    // ==================== ProgressMessage Tests ====================

    #[test]
    fn test_progress_message_new() {
        let lang = test_lang();
        let pm = ProgressMessage::new(ChatId(12345), lang);
        assert_eq!(pm.chat_id, ChatId(12345));
        assert!(pm.message_id.is_none());
    }
}
