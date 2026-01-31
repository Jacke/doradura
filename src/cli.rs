use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "doradura")]
#[command(author, version, about = "High-performance Telegram bot for downloading music and videos", long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Run the bot in normal mode
    Run {
        /// Use webhook mode instead of long polling
        #[arg(long)]
        webhook: bool,
    },

    /// Run the bot in staging mode (uses staging environment variables)
    RunStaging {
        /// Use webhook mode instead of long polling
        #[arg(long)]
        webhook: bool,
    },

    /// Run the bot with cookies refresh/update
    RunWithCookies {
        /// Path to cookies file
        #[arg(short, long)]
        cookies: Option<String>,

        /// Use webhook mode instead of long polling
        #[arg(long)]
        webhook: bool,
    },

    /// Refresh missing metadata in download history
    RefreshMetadata {
        /// Limit the number of entries to process
        #[arg(short, long)]
        limit: Option<usize>,

        /// Only show what would be refreshed without actually doing it
        #[arg(long)]
        dry_run: bool,

        /// Verbosely log each entry processed
        #[arg(short, long)]
        verbose: bool,
    },

    /// Update yt-dlp to the latest version
    UpdateYtdlp {
        /// Force update even if already up to date
        #[arg(long)]
        force: bool,

        /// Check version without updating
        #[arg(long)]
        check: bool,
    },

    /// Download media from URL (for testing)
    Download {
        /// URL to download from (YouTube, SoundCloud, etc.)
        url: String,

        /// Output format: mp3, mp4 (default: mp4)
        #[arg(short, long, default_value = "mp4")]
        format: String,

        /// Video quality: best, 1080p, 720p, 480p, 360p (default: best)
        #[arg(short, long, default_value = "best")]
        quality: String,

        /// Audio bitrate: 128k, 192k, 256k, 320k (default: 320k)
        #[arg(short, long, default_value = "320k")]
        bitrate: String,

        /// Output directory (default: current directory)
        #[arg(short, long)]
        output: Option<String>,

        /// Show verbose progress
        #[arg(short, long)]
        verbose: bool,
    },

    /// Get video info/metadata without downloading
    Info {
        /// URL to get info from
        url: String,

        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

impl Cli {
    pub fn parse_args() -> Self {
        Self::parse()
    }
}
