//! MTProto Download CLI
//!
//! Independent CLI tool for downloading Telegram files using MTProto.
//! This is an experimental tool for testing the MTProto integration.
//!
//! Usage:
//!   mtproto-download list                     - List files from database
//!   mtproto-download by-file-id -f <ID> -o <PATH>  - Download by file_id
//!   mtproto-download by-db-id -i <ID> -o <DIR>     - Download by DB entry ID
//!   mtproto-download decode -f <ID>           - Decode file_id without downloading

use clap::{Parser, Subcommand};
use doradura::experimental::mtproto::{DecodedFileId, MtProtoClient, MtProtoDownloader};
use doradura::storage::db;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "mtproto-download")]
#[command(about = "Download Telegram files using MTProto protocol")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Download a file by its Bot API file_id
    ByFileId {
        /// The file_id to download
        #[arg(short = 'f', long)]
        file_id: String,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Download a file by database entry ID
    ByDbId {
        /// Download history entry ID
        #[arg(short, long)]
        id: i64,

        /// Output directory (filename will be generated)
        #[arg(short, long, default_value = ".")]
        output_dir: PathBuf,
    },

    /// List files available for download from database
    List {
        /// Maximum number of files to show
        #[arg(short, long, default_value = "20")]
        limit: i32,

        /// Filter by format (mp3, mp4)
        #[arg(short, long)]
        format: Option<String>,
    },

    /// Decode a file_id without downloading (for debugging)
    Decode {
        /// The file_id to decode
        #[arg(short = 'f', long)]
        file_id: String,
    },

    /// Download by chat_id and message_id (alternative method)
    ByMessage {
        /// Chat/User ID
        #[arg(short, long)]
        chat_id: i64,

        /// Message ID
        #[arg(short, long)]
        message_id: i32,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,
    },

    /// Show bot information (test connection)
    Whoami,

    /// Get fresh media info from a message by ID
    GetMessage {
        /// Message ID containing the media
        #[arg(short, long)]
        message_id: i32,
    },

    /// Download media from a message with fresh file_reference
    FromMessage {
        /// Message ID containing the media
        #[arg(short, long)]
        message_id: i32,

        /// Output file path
        #[arg(short, long)]
        output: PathBuf,
    },
}

fn get_env(name: &str) -> anyhow::Result<String> {
    std::env::var(name).map_err(|_| anyhow::anyhow!("Environment variable {} not set", name))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load environment
    dotenvy::dotenv().ok();
    pretty_env_logger::init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Decode { file_id } => {
            // Decode without needing client
            println!("Decoding file_id: {}", file_id);
            match DecodedFileId::decode(&file_id) {
                Ok(decoded) => {
                    println!("\n=== Decoded File ID ===");
                    println!("Version: {}.{}", decoded.version, decoded.sub_version);
                    println!("DC ID: {}", decoded.dc_id);
                    println!("File Type: {:?}", decoded.file_type);
                    println!("ID: {}", decoded.id);
                    println!("Access Hash: {}", decoded.access_hash);
                    println!("File Reference: {} bytes", decoded.file_reference.len());
                    if let Some(c) = decoded.photo_size_type {
                        println!("Photo Size Type: {}", c);
                    }
                    if let Some(v) = decoded.volume_id {
                        println!("Volume ID: {}", v);
                    }
                    if let Some(l) = decoded.local_id {
                        println!("Local ID: {}", l);
                    }
                    println!("Is Document: {}", decoded.file_type.is_document());
                    println!("Is Photo: {}", decoded.file_type.is_photo());
                }
                Err(e) => {
                    eprintln!("Failed to decode: {}", e);
                    std::process::exit(1);
                }
            }
            return Ok(());
        }

        Commands::List { limit, format } => {
            // List files from database
            let db_path = get_env("DATABASE_URL")
                .or_else(|_| get_env("DATABASE_PATH"))
                .unwrap_or_else(|_| "database.sqlite".to_string());

            println!("Loading database from: {}", db_path);

            let pool = doradura::storage::create_pool(&db_path)?;
            let conn = doradura::storage::get_connection(&pool)?;

            let files = db::get_sent_files(&conn, Some(limit))?;

            println!("\n=== Files available for download ===\n");

            let mut count = 0;
            for file in files {
                // Filter by format if specified
                if let Some(ref fmt) = format {
                    if &file.format != fmt {
                        continue;
                    }
                }

                count += 1;
                println!("[{}] {} ({})", file.id, file.title, file.format);
                println!("    User: {}", file.username.as_deref().unwrap_or("-"));
                println!("    file_id: {}", file.file_id);
                println!();
            }

            println!("Total: {} files", count);
            return Ok(());
        }

        _ => {}
    }

    // Commands that require MTProto client
    let api_id: i32 = get_env("TELEGRAM_API_ID")?.parse()?;
    let api_hash = get_env("TELEGRAM_API_HASH")?;
    let bot_token = get_env("BOT_TOKEN")?;
    let session_path = std::env::var("MTPROTO_SESSION_PATH").unwrap_or_else(|_| "mtproto_session.bin".to_string());

    println!("Initializing MTProto client...");
    let client = MtProtoClient::new_bot(api_id, &api_hash, &bot_token, std::path::Path::new(&session_path)).await?;

    let downloader = MtProtoDownloader::with_bot_token(client, bot_token);

    match cli.command {
        Commands::Whoami => {
            println!("Fetching bot info...");
            let _ = downloader.decode_file_id("test"); // Verify downloader is usable
            println!("MTProto client initialized successfully!");
            println!("Session saved to: {}", session_path);
        }

        Commands::ByFileId { file_id, output } => {
            println!("Downloading file_id: {}...", &file_id[..20.min(file_id.len())]);
            let size = downloader.download_by_file_id(&file_id, &output).await?;
            println!("Downloaded {} bytes to {:?}", size, output);
        }

        Commands::ByDbId { id, output_dir } => {
            let db_path = get_env("DATABASE_URL")
                .or_else(|_| get_env("DATABASE_PATH"))
                .unwrap_or_else(|_| "database.sqlite".to_string());

            let pool = doradura::storage::create_pool(&db_path)?;
            let conn = doradura::storage::get_connection(&pool)?;

            // Try download_history first
            if let Some(entry) = db::get_download_history_entry(&conn, id, id)? {
                if let Some(file_id) = entry.file_id {
                    // Sanitize filename
                    let safe_title: String = entry
                        .title
                        .chars()
                        .map(|c| {
                            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                                c
                            } else {
                                '_'
                            }
                        })
                        .collect();
                    let filename = format!("{}.{}", safe_title, entry.format);
                    let output = output_dir.join(&filename);

                    println!("Downloading '{}' (ID: {})...", entry.title, id);
                    let size = downloader.download_by_file_id(&file_id, &output).await?;
                    println!("Downloaded {} bytes to {:?}", size, output);
                } else {
                    eprintln!("Entry {} has no file_id", id);
                    std::process::exit(1);
                }
            } else {
                // Try cuts table
                if let Some(cut) = db::get_cut_entry(&conn, id, id)? {
                    if let Some(file_id) = cut.file_id {
                        let safe_title: String = cut
                            .title
                            .chars()
                            .map(|c| {
                                if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                                    c
                                } else {
                                    '_'
                                }
                            })
                            .collect();
                        let filename = format!("{}_cut.mp4", safe_title);
                        let output = output_dir.join(&filename);

                        println!("Downloading cut '{}' (ID: {})...", cut.title, id);
                        let size = downloader.download_by_file_id(&file_id, &output).await?;
                        println!("Downloaded {} bytes to {:?}", size, output);
                    } else {
                        eprintln!("Cut entry {} has no file_id", id);
                        std::process::exit(1);
                    }
                } else {
                    eprintln!("Entry {} not found in download_history or cuts", id);
                    std::process::exit(1);
                }
            }
        }

        Commands::ByMessage {
            chat_id,
            message_id,
            output,
        } => {
            println!("Downloading from chat {} message {}...", chat_id, message_id);
            let size = downloader.download_by_message(chat_id, message_id, &output).await?;
            println!("Downloaded {} bytes to {:?}", size, output);
        }

        Commands::GetMessage { message_id } => {
            println!("Getting media info for message {}...", message_id);

            match downloader.get_fresh_media_info(message_id).await {
                Ok(media) => {
                    let date = chrono::DateTime::from_timestamp(media.date as i64, 0)
                        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                        .unwrap_or_else(|| media.date.to_string());

                    println!("\n=== Media Info ===");
                    println!("Message ID: {}", media.message_id);
                    println!("Type: {:?}", media.media_type);
                    println!("Size: {} bytes", media.size);
                    println!("Date: {}", date);
                    if let Some(ref name) = media.filename {
                        println!("Filename: {}", name);
                    }
                    if let Some(ref mime) = media.mime_type {
                        println!("MIME: {}", mime);
                    }
                    if let Some(dur) = media.duration {
                        println!("Duration: {}s", dur);
                    }
                    println!("DC ID: {}", media.dc_id);
                    println!("file_reference: {} bytes (fresh!)", media.file_reference.len());
                    println!("\nUse 'from-message -m {} -o <output>' to download", message_id);
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    eprintln!("Note: Bots can only fetch messages they sent. Make sure the message_id is correct.");
                }
            }
        }

        Commands::FromMessage { message_id, output } => {
            println!("Getting fresh media info for message {}...", message_id);

            let media = downloader.get_fresh_media_info(message_id).await?;

            println!("Found: {:?}, {} bytes", media.media_type, media.size);
            if let Some(ref name) = media.filename {
                println!("Filename: {}", name);
            }

            println!("Downloading with fresh file_reference...");
            let size = downloader.download_media(&media, &output).await?;
            println!("Downloaded {} bytes to {:?}", size, output);
        }

        // Already handled above
        Commands::Decode { .. } | Commands::List { .. } => unreachable!(),
    }

    Ok(())
}
