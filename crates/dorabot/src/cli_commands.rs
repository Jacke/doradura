//! CLI command handlers extracted from main.rs
//!
//! Contains implementations for:
//! - `download` — CLI download with proxy chain
//! - `info` — media info lookup
//! - `refresh-metadata` — metadata refresh
//! - `update-ytdlp` — yt-dlp management

use anyhow::Result;
use std::sync::Arc;

use crate::core::config;
use crate::download::ytdlp;
use crate::metadata_refresh;
use crate::storage::create_pool;

/// Run the metadata refresh command
pub async fn run_metadata_refresh(limit: Option<usize>, dry_run: bool, verbose: bool) -> Result<()> {
    let db_pool = Arc::new(
        create_pool(&config::DATABASE_PATH).map_err(|e| anyhow::anyhow!("Failed to create database pool: {}", e))?,
    );

    let bot_token = config::BOT_TOKEN.to_string();
    if bot_token.is_empty() {
        return Err(anyhow::anyhow!("BOT_TOKEN environment variable not set"));
    }

    metadata_refresh::refresh_missing_metadata(db_pool, bot_token, limit, dry_run, verbose).await?;
    Ok(())
}

/// Run yt-dlp update command
pub async fn run_ytdlp_update(force: bool, check: bool) -> Result<()> {
    if check {
        ytdlp::print_ytdlp_version().await?;
    } else if force {
        ytdlp::force_update_ytdlp().await?;
    } else {
        ytdlp::check_and_update_ytdlp().await?;
    }
    Ok(())
}

/// Run CLI download command
#[allow(clippy::too_many_arguments)]
pub async fn run_cli_download(
    url: String,
    format: String,
    quality: String,
    bitrate: String,
    output: Option<String>,
    verbose: bool,
) -> Result<()> {
    use crate::download::metadata::{get_proxy_chain, is_proxy_related_error};
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    println!("🎬 Doradura CLI Download");
    println!("========================");
    println!("URL: {}", url);
    println!("Format: {}", format);

    let output_dir = output.unwrap_or_else(|| ".".to_string());
    let ytdl_bin = config::YTDL_BIN.clone();

    // Build format string based on format type and quality/bitrate
    let format_arg = match format.as_str() {
        "mp3" => {
            println!("Audio bitrate: {}", bitrate);
            "bestaudio[ext=m4a]/bestaudio/best".to_string()
        }
        "mp4" => {
            let quality_format = match quality.as_str() {
                "1080p" => "bestvideo[height<=1080][ext=mp4]+bestaudio[ext=m4a]/bestvideo[height<=1080]+bestaudio/best[height<=1080]",
                "720p" => "bestvideo[height<=720][ext=mp4]+bestaudio[ext=m4a]/bestvideo[height<=720]+bestaudio/best[height<=720]",
                "480p" => "bestvideo[height<=480][ext=mp4]+bestaudio[ext=m4a]/bestvideo[height<=480]+bestaudio/best[height<=480]",
                "360p" => "bestvideo[height<=360][ext=mp4]+bestaudio[ext=m4a]/bestvideo[height<=360]+bestaudio/best[height<=360]",
                _ => "bestvideo[ext=mp4]+bestaudio[ext=m4a]/bestvideo+bestaudio/best",
            };
            println!("Video quality: {}", quality);
            quality_format.to_string()
        }
        _ => {
            return Err(anyhow::anyhow!("Unsupported format: {}. Use mp3 or mp4.", format));
        }
    };

    let output_template = format!("{}/%(title)s.%(ext)s", output_dir);

    let proxy_chain = get_proxy_chain();
    let total_proxies = proxy_chain.len();
    let mut last_error: Option<String> = None;

    if total_proxies == 1 && proxy_chain[0].is_none() {
        println!("⚠️ No proxy configured. For YouTube downloads, consider setting:");
        println!("   • WARP_PROXY=socks5://127.0.0.1:40000 (Cloudflare WARP)");
        println!();
    }

    // Check if PO Token server is running (for YouTube)
    if url.contains("youtube.com") || url.contains("youtu.be") {
        let po_token_check = std::process::Command::new("curl")
            .args([
                "-s",
                "-o",
                "/dev/null",
                "-w",
                "%{http_code}",
                "http://127.0.0.1:4416/health",
            ])
            .output();
        let server_running = po_token_check
            .map(|o| String::from_utf8_lossy(&o.stdout).contains("200"))
            .unwrap_or(false);

        if !server_running && verbose {
            println!("💡 PO Token server not detected at http://127.0.0.1:4416");
            println!("   For YouTube, run: bgutil-ytdlp-pot-provider");
            println!();
        }
    }

    // Try each proxy in the chain
    for (attempt, proxy_option) in proxy_chain.into_iter().enumerate() {
        let proxy_name = proxy_option
            .as_ref()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Direct (no proxy)".to_string());

        println!(
            "\n📡 Download attempt {}/{} using [{}]",
            attempt + 1,
            total_proxies,
            proxy_name
        );

        let mut args: Vec<String> = vec![
            "-o".to_string(),
            output_template.clone(),
            "--format".to_string(),
            format_arg.clone(),
            "--no-check-certificate".to_string(),
            "--newline".to_string(),
        ];

        if format == "mp3" {
            args.extend_from_slice(&[
                "-x".to_string(),
                "--audio-format".to_string(),
                "mp3".to_string(),
                "--audio-quality".to_string(),
                match bitrate.as_str() {
                    "128k" => "128K",
                    "192k" => "192K",
                    "256k" => "256K",
                    _ => "320K",
                }
                .to_string(),
            ]);
        } else {
            args.extend_from_slice(&["--merge-output-format".to_string(), "mp4".to_string()]);
        }

        if let Some(ref proxy_config) = proxy_option {
            args.extend_from_slice(&["--proxy".to_string(), proxy_config.url.clone()]);
        }

        if let Some(ref cookies_file) = *config::YTDL_COOKIES_FILE {
            if !cookies_file.is_empty() && std::path::Path::new(cookies_file).exists() {
                args.extend_from_slice(&["--cookies".to_string(), cookies_file.clone()]);
                if verbose && attempt == 0 {
                    println!("Using cookies from: {}", cookies_file);
                }
            }
        }

        // Use android + web_music clients (minimal BotGuard checks with WARP)
        args.extend_from_slice(&[
            "--extractor-args".to_string(),
            "youtube:player_client=android,web_music;formats=missing_pot".to_string(),
            "--js-runtimes".to_string(),
            "deno".to_string(),
            "--impersonate".to_string(),
            "Chrome-131:Android-14".to_string(),
        ]);

        args.push(url.clone());

        if verbose {
            println!("📥 Starting download...");
            println!("Command: {} {}", ytdl_bin, args.join(" "));
            println!();
        } else {
            println!("📥 Downloading...");
        }

        let mut child = match Command::new(&ytdl_bin)
            .args(&args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                last_error = Some(format!("Failed to spawn yt-dlp: {}", e));
                continue;
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        if let Some(stdout_stream) = stdout {
            let reader = BufReader::new(stdout_stream);
            for line in reader.lines().map_while(Result::ok) {
                if verbose {
                    println!("{}", line);
                } else if line.contains("[download]") && line.contains('%') {
                    print!("\r{}", line);
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                }
            }
        }

        let mut stderr_output = String::new();
        if let Some(stderr_stream) = stderr {
            let reader = BufReader::new(stderr_stream);
            for line in reader.lines().map_while(Result::ok) {
                if verbose {
                    eprintln!("{}", line);
                }
                stderr_output.push_str(&line);
                stderr_output.push('\n');
            }
        }

        let status = match child.wait() {
            Ok(s) => s,
            Err(e) => {
                last_error = Some(format!("Failed to wait for yt-dlp: {}", e));
                continue;
            }
        };

        if status.success() {
            println!("\n\n✅ Download completed successfully!");
            println!("📂 Output directory: {}", output_dir);
            return Ok(());
        }

        if is_proxy_related_error(&stderr_output) && attempt + 1 < total_proxies {
            println!(
                "\n⚠️ Proxy-related error, trying next proxy... (error: {})",
                stderr_output.lines().next().unwrap_or("unknown")
            );
            last_error = Some(stderr_output);
            continue;
        }

        last_error = Some(stderr_output);
        break;
    }

    eprintln!("\n\n❌ Download failed!");
    if let Some(error) = last_error {
        eprintln!("Error output:\n{}", error);
    }
    Err(anyhow::anyhow!("Download failed after trying all proxies"))
}

/// Run CLI info command
pub async fn run_cli_info(url: String, json: bool) -> Result<()> {
    use std::process::Command;

    let ytdl_bin = config::YTDL_BIN.clone();

    if json {
        let mut args: Vec<String> = vec![
            "--dump-json".to_string(),
            "--no-download".to_string(),
            "--no-check-certificate".to_string(),
        ];

        if let Some(ref cookies_file) = *config::YTDL_COOKIES_FILE {
            if !cookies_file.is_empty() && std::path::Path::new(cookies_file).exists() {
                args.insert(0, "--cookies".to_string());
                args.insert(1, cookies_file.clone());
            }
        }

        args.extend_from_slice(&[
            "--extractor-args".to_string(),
            "youtubepot-bgutilhttp:base_url=http://127.0.0.1:4416".to_string(),
        ]);

        args.push(url.clone());

        let output = Command::new(&ytdl_bin)
            .args(&args)
            .output()
            .map_err(|e| anyhow::anyhow!("Failed to run yt-dlp: {}", e))?;

        if output.status.success() {
            let json_str = String::from_utf8_lossy(&output.stdout);
            println!("{}", json_str);
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("Failed to get info: {}", stderr));
        }
    } else {
        let print_format = "Title: %(title)s\nChannel: %(channel)s\nDuration: %(duration_string)s\nView count: %(view_count)s\nUpload date: %(upload_date)s\nDescription: %(description).200s...";

        let mut args: Vec<String> = vec![
            "--print".to_string(),
            print_format.to_string(),
            "--no-download".to_string(),
            "--no-check-certificate".to_string(),
        ];

        if let Some(ref cookies_file) = *config::YTDL_COOKIES_FILE {
            if !cookies_file.is_empty() && std::path::Path::new(cookies_file).exists() {
                args.insert(0, "--cookies".to_string());
                args.insert(1, cookies_file.clone());
            }
        }

        args.extend_from_slice(&[
            "--extractor-args".to_string(),
            "youtubepot-bgutilhttp:base_url=http://127.0.0.1:4416".to_string(),
        ]);

        args.push(url.clone());

        println!("🎬 Video Information");
        println!("====================");
        println!("URL: {}\n", url);

        let output = Command::new(&ytdl_bin)
            .args(&args)
            .output()
            .map_err(|e| anyhow::anyhow!("Failed to run yt-dlp: {}", e))?;

        if output.status.success() {
            let info = String::from_utf8_lossy(&output.stdout);
            println!("{}", info);

            println!("\n📋 Available Formats:");
            println!("---------------------");

            let mut format_args: Vec<String> = vec!["--list-formats".to_string(), "--no-check-certificate".to_string()];

            format_args.extend_from_slice(&[
                "--extractor-args".to_string(),
                "youtubepot-bgutilhttp:base_url=http://127.0.0.1:4416".to_string(),
            ]);

            format_args.push(url);

            let format_output = Command::new(&ytdl_bin)
                .args(&format_args)
                .output()
                .map_err(|e| anyhow::anyhow!("Failed to get formats: {}", e))?;

            if format_output.status.success() {
                let formats = String::from_utf8_lossy(&format_output.stdout);
                for line in formats.lines() {
                    if line.contains("mp4")
                        || line.contains("m4a")
                        || line.contains("webm")
                        || line.starts_with("ID")
                        || line.starts_with("--")
                    {
                        println!("{}", line);
                    }
                }
            }
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow::anyhow!("Failed to get info: {}", stderr));
        }
    }

    Ok(())
}
