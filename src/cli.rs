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
}

impl Cli {
    pub fn parse_args() -> Self {
        Self::parse()
    }
}
