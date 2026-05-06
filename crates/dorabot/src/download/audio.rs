//! Audio download and processing module
//!
//! Thin wrapper around the unified download pipeline, adding audio-specific
//! post-processing (audio effects button).

use crate::core::config;
use crate::core::error::AppError;
use crate::core::metrics;
use crate::core::types::Plan;
use crate::download::context::DownloadContext;
use crate::download::error::DownloadError;
use crate::download::pipeline::{self, PipelineFormat, PipelineResult};
use crate::download::progress::{ProgressBarStyle, ProgressMessage};
use crate::download::source::bot_global;
use crate::storage::SharedStorage;
use crate::telegram::Bot;
use crate::telegram::ext::BotExt;
use std::sync::Arc;
use teloxide::prelude::*;
use tokio::time::timeout;
use tracing::Instrument;

/// Download audio file and send it to user
///
/// Downloads audio from URL using the unified download pipeline, shows progress updates,
/// validates file size, and sends the file to the user via Telegram.
/// After successful send, adds audio effects buttons (Edit/Cut).
pub async fn download_and_send_audio(
    ctx: DownloadContext,
    audio_bitrate: Option<String>,
    time_range: Option<(String, String)>,
    with_lyrics: bool,
) -> ResponseResult<()> {
    let DownloadContext {
        bot,
        chat_id,
        url,
        rate_limiter: _rate_limiter,
        db_pool,
        shared_storage,
        message_id,
        alert_manager,
        created_timestamp: _created_timestamp,
    } = ctx;
    log::info!(
        "Starting download_and_send_audio for chat {} with URL: {}",
        chat_id,
        url
    );
    let bot_clone = bot.clone();
    let db_pool_clone = db_pool.clone();
    let shared_storage_clone = shared_storage.clone();

    // Inherit the parent span (from queue_processor) so all audio logs carry op=...
    // Run inline (awaited) instead of spawning: queue_processor relies on this
    // function returning AFTER the download+upload is actually complete. A prior
    // fire-and-forget spawn caused queue_processor to release the permit 50ms
    // into the job, letting multiple downloads run in parallel despite
    // max_concurrent=1 and showing "hanging" downloads to users.
    let span = tracing::Span::current();
    async move {
        // Get user plan for metrics
        let user_plan = if let Some(ref storage) = shared_storage_clone {
            storage
                .get_user(chat_id.0)
                .await
                .ok()
                .flatten()
                .map(|u| u.plan)
                .unwrap_or_default()
        } else {
            Plan::default()
        };

        metrics::record_format_request("mp3", user_plan.as_str());
        metrics::record_platform_download(metrics::extract_platform(url.as_str()));

        let quality = audio_bitrate.as_deref().unwrap_or("default");
        let timer = metrics::DOWNLOAD_DURATION_SECONDS
            .with_label_values(&["mp3", quality])
            .start_timer();

        let format = PipelineFormat::Audio {
            bitrate: audio_bitrate.clone(),
            time_range,
        };
        let registry = bot_global();

        // Create progress_msg BEFORE timeout so we can clean it up if timeout fires
        let lang = if let Some(ref storage) = shared_storage_clone {
            crate::i18n::user_lang_from_storage(storage, chat_id.0).await
        } else {
            crate::i18n::lang_from_code("ru")
        };
        let mut progress_msg = ProgressMessage::new(chat_id, lang.clone());
        if let Some(ref storage) = shared_storage_clone
            && let Ok(style_str) = storage.get_user_progress_bar_style(chat_id.0).await
        {
            progress_msg.style = ProgressBarStyle::parse(&style_str);
        }

        // Global timeout for entire download operation
        let result: Result<(), AppError> = match timeout(config::download::global_timeout(), async {
            let pipeline_result = pipeline::execute(
                &bot_clone,
                chat_id,
                &url,
                &format,
                db_pool_clone.as_ref(),
                shared_storage_clone.as_ref(),
                message_id,
                alert_manager.as_ref(),
                registry,
                &mut progress_msg,
            )
            .await
            .map_err(|e| e.into_app_error())?;

            metrics::record_file_size("mp3", pipeline_result.file_size);

            // Audio-specific: add effects button
            add_audio_effects_button(&bot_clone, chat_id, &pipeline_result, shared_storage_clone.as_ref()).await;

            // Lyrics: fetch and show section picker — user picks → caption is edited on audio msg
            log::info!(
                "audio: with_lyrics={} title='{}' artist='{}' shared_storage_some={}",
                with_lyrics,
                pipeline_result.title,
                pipeline_result.artist,
                shared_storage_clone.is_some()
            );
            if with_lyrics {
                let bot_lyr = bot_clone.clone();
                let title_lyr = pipeline_result.title.clone();
                let artist_lyr = pipeline_result.artist.clone();
                let audio_msg_id = pipeline_result.sent_message.id;
                if let Some(ref ss) = shared_storage_clone {
                    let ss_clone = std::sync::Arc::clone(ss);
                    tokio::spawn(async move {
                        show_lyrics_picker_for_audio(
                            &bot_lyr,
                            chat_id,
                            audio_msg_id,
                            &artist_lyr,
                            &title_lyr,
                            &ss_clone,
                        )
                        .await;
                    });
                } else {
                    log::warn!("audio: with_lyrics=true but shared_storage is None — skipping lyrics");
                }
            }

            // Share page: create after successful audio send (YouTube only, fire-and-forget)
            if crate::core::share::is_youtube_url(url.as_str())
                && let Some(ref storage) = shared_storage_clone
            {
                let storage_share = std::sync::Arc::clone(storage);
                let url_str = url.to_string();
                let title_share = pipeline_result.title.clone();
                let artist_share = pipeline_result.artist.clone();
                let duration_share = pipeline_result.duration;
                let bot_share = bot_clone.clone();
                tokio::spawn(async move {
                    let thumb = crate::core::share::youtube_thumbnail_url(&url_str);
                    let artist_opt = if artist_share.trim().is_empty() {
                        None
                    } else {
                        Some(artist_share.as_str())
                    };
                    if let Some((share_url, streaming_links)) = crate::core::share::create_share_page(
                        &storage_share,
                        &url_str,
                        &title_share,
                        artist_opt,
                        thumb.as_deref(),
                        Some(duration_share as u64),
                    )
                    .await
                    {
                        send_share_message(&bot_share, chat_id, &title_share, &share_url, streaming_links.as_ref())
                            .await;
                    }
                });
            }

            // Schedule file cleanup (including any carousel extras)
            let extra_paths: Vec<String> = pipeline_result
                .output
                .additional_files
                .as_ref()
                .map(|files| files.iter().map(|f| f.file_path.clone()).collect())
                .unwrap_or_default();
            pipeline::schedule_cleanup_with_extras(pipeline_result.download_path.clone(), extra_paths);

            Ok(())
        })
        .await
        {
            Ok(inner) => inner,
            Err(_) => {
                log::error!(
                    "🚨 Audio download timed out after {} seconds",
                    config::download::GLOBAL_TIMEOUT_SECS
                );
                Err(AppError::Download(DownloadError::Timeout(format!(
                    "Download timed out (exceeded {} minutes)",
                    config::download::GLOBAL_TIMEOUT_SECS / 60
                ))))
            }
        };

        match result {
            Ok(()) => {
                log::info!("Audio download completed successfully for chat {}", chat_id);
                timer.observe_duration();
                metrics::record_download_success("mp3", quality);
                let signoff = crate::i18n::random_signoff(&lang);
                let _ = bot_clone
                    .send_message(chat_id, signoff)
                    .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                    .await;
            }
            Err(e) => {
                e.track_with_operation("audio_download");
                log::error!(
                    "An error occurred during audio download for chat {} ({}): {:?}",
                    chat_id,
                    url,
                    e
                );
                timer.observe_duration();

                // Delete hanging ⏳ progress message so it doesn't stay on screen forever
                if let Some(msg_id) = progress_msg.message_id {
                    bot_clone.try_delete(chat_id, msg_id).await;
                }

                let pipeline_error = pipeline::PipelineError::Operational(e);
                pipeline::handle_pipeline_error(
                    &bot_clone,
                    chat_id,
                    &url,
                    &pipeline_error,
                    &format,
                    alert_manager.as_ref(),
                    message_id,
                )
                .await;
            }
        }
    }
    .instrument(span)
    .await;

    Ok(())
}

/// Add audio effects button (Edit/Cut) for eligible users.
///
/// Creates an AudioEffectSession, copies the downloaded file for effects processing,
/// and adds inline keyboard buttons to the sent message.
async fn add_audio_effects_button(
    bot: &Bot,
    chat_id: ChatId,
    result: &PipelineResult,
    shared_storage: Option<&Arc<SharedStorage>>,
) {
    let Some(storage) = shared_storage else {
        return;
    };

    use crate::download::audio_effects::{self, AudioEffectSession};

    let session_id = uuid::Uuid::new_v4().to_string();
    let session_file_path_raw = audio_effects::get_original_file_path(&session_id, &config::DOWNLOAD_FOLDER);
    let session_file_path = shellexpand::tilde(&session_file_path_raw).into_owned();

    // Copy file before it gets deleted
    match fs_err::tokio::copy(&result.download_path, &session_file_path).await {
        Ok(bytes) => {
            log::info!("Audio effects: copied {} bytes to {}", bytes, session_file_path);
            let session = AudioEffectSession::new(
                session_id.clone(),
                chat_id.0,
                session_file_path,
                result.sent_message.id.0,
                result.display_title.as_ref().to_string(),
                result.duration,
            );

            match storage.create_audio_effect_session(&session).await {
                Ok(_) => {
                    log::info!("Audio effects: session created with id {}", session_id);
                    let bot_for_button = bot.clone();
                    let sent_message_id = result.sent_message.id;
                    let session_id_clone = session_id.clone();
                    tokio::spawn(async move {
                        use teloxide::types::InlineKeyboardMarkup;

                        let keyboard = InlineKeyboardMarkup::new(vec![
                            vec![
                                crate::telegram::cb("Edit Audio", format!("ae:open:{}", session_id_clone)),
                                crate::telegram::cb("Cut Audio", format!("ac:open:{}", session_id_clone)),
                            ],
                            vec![crate::telegram::cb("🎵 Lyrics", format!("lyr:{}", session_id_clone))],
                        ]);

                        match bot_for_button
                            .edit_message_reply_markup(chat_id, sent_message_id)
                            .reply_markup(keyboard)
                            .await
                        {
                            Err(e) => {
                                log::warn!("Failed to add audio effects button: {}", e);
                            }
                            _ => {
                                log::info!(
                                    "Added audio effects button to message {} for session {}",
                                    sent_message_id.0,
                                    session_id_clone
                                );
                            }
                        }
                    });
                }
                Err(e) => {
                    log::warn!("Failed to create audio effect session in DB: {}", e);
                }
            }
        }
        Err(e) => {
            log::warn!(
                "Failed to copy file for audio effects: {} (src: {}, dst: {})",
                e,
                result.download_path,
                session_file_path
            );
        }
    }
}

/// Send a follow-up Telegram message with streaming service buttons after a successful download.
async fn send_share_message(
    bot: &Bot,
    chat_id: ChatId,
    title: &str,
    share_url: &str,
    streaming_links: Option<&crate::core::odesli::StreamingLinks>,
) {
    use teloxide::requests::Requester;
    use teloxide::types::{InlineKeyboardButton, InlineKeyboardMarkup};

    let mut row1: Vec<InlineKeyboardButton> = Vec::new();
    let mut has_links = false;

    if let Some(links) = streaming_links {
        if let Some(ref url) = links.spotify
            && let Ok(u) = url.parse()
        {
            row1.push(InlineKeyboardButton::url("💚 Spotify", u));
            has_links = true;
        }
        if let Some(ref url) = links.apple_music
            && let Ok(u) = url.parse()
        {
            row1.push(InlineKeyboardButton::url("🍎 Apple", u));
            has_links = true;
        }
        if let Some(ref url) = links.youtube_music
            && let Ok(u) = url.parse()
        {
            row1.push(InlineKeyboardButton::url("🔴 YT Music", u));
            has_links = true;
        }
    }

    let Ok(share_parsed) = share_url.parse() else {
        log::warn!("Invalid share URL: {}", share_url);
        return;
    };
    let row2 = vec![InlineKeyboardButton::url("🔗 All platforms", share_parsed)];

    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    if !row1.is_empty() {
        rows.push(row1);
    }
    rows.push(row2);

    let keyboard = InlineKeyboardMarkup::new(rows);

    let text = if has_links {
        format!("🎧 \"{}\" — listen legally:", title)
    } else {
        format!("🔗 \"{}\":", title)
    };

    if let Err(e) = bot.send_message(chat_id, text).reply_markup(keyboard).await {
        log::warn!("Failed to send share message: {}", e);
    }
}

/// Fetch lyrics, show section picker. When user picks a section, the audio caption is edited.
async fn show_lyrics_picker_for_audio(
    bot: &Bot,
    chat_id: ChatId,
    audio_msg_id: teloxide::types::MessageId,
    artist: &str,
    title: &str,
    shared_storage: &std::sync::Arc<crate::storage::SharedStorage>,
) {
    log::info!(
        "show_lyrics_picker_for_audio: invoked artist='{}' title='{}'",
        artist,
        title
    );
    if title.trim().is_empty() {
        log::warn!("show_lyrics_picker_for_audio: bailing on empty title");
        return;
    }

    // Smart cascade: title-parser produces multiple `(artist, track)`
    // candidates from the raw video title (forward split, reverse split,
    // feat-stripped, title-only). The first candidate that resolves wins;
    // catches re-upload channels where the channel name isn't the actual
    // performer (e.g. "musiko lyriko" → real artist comes from the title).
    let lyrics = match crate::lyrics::fetch_lyrics_smart(artist, title, None).await {
        Some(lyr) => lyr,
        None => {
            log::info!("with_lyrics: smart cascade exhausted for '{} - {}'", artist, title);
            let display = if artist.trim().is_empty() {
                title.to_string()
            } else {
                format!("{} – {}", artist, title)
            };
            let msg = format!(
                "📝 Не удалось найти текст для «{}».\n\nGenius/LRCLIB не вернули совпадений. Попробуй другую ссылку с явным «исполнитель – трек» в названии.",
                display
            );
            if let Err(e) = bot.send_message(chat_id, msg).await {
                log::warn!("Failed to send 'no lyrics found' notice: {}", e);
            }
            return;
        }
    };

    // Auto-apply when full lyrics fit comfortably in a Telegram caption (1024
    // char hard limit; 900 keeps headroom for emojis/wide chars). Skips the
    // picker entirely — one-tap UX for short tracks. The user opted in via
    // "📝 Lyrics ☑", so applying without prompting is the expected outcome.
    let all_text = lyrics.all_text();
    if all_text.chars().count() <= 900 && !all_text.trim().is_empty() {
        if let Err(e) = bot.edit_message_caption(chat_id, audio_msg_id).caption(all_text).await {
            log::warn!("Auto-apply lyrics caption failed: {}", e);
        }
        return;
    }

    // For unstructured lyrics (no [Verse]/[Chorus] markers) the parser
    // returns a single mega-section — useless for a picker. Re-segment into
    // ~8-line chunks so the user gets meaningful choices.
    let working_sections: Vec<crate::lyrics::LyricsSection> = if !lyrics.has_structure
        && let Some(only) = lyrics.sections.first()
        && only.lines.len() > 8
    {
        crate::lyrics::auto_segment_unstructured(&only.lines)
    } else {
        lyrics.sections.clone()
    };

    let session_id = uuid::Uuid::new_v4().to_string()[..8].to_string();
    let sections_json = serde_json::to_string(&working_sections).unwrap_or_default();
    let _ = shared_storage
        .create_lyrics_session(
            &session_id,
            chat_id.0,
            &lyrics.artist,
            &lyrics.title,
            &sections_json,
            lyrics.has_structure,
        )
        .await;

    // Build picker with callbacks: downloads:lyr_cap:{audio_msg_id}:{session_id}:{idx}
    use std::collections::HashMap;
    let mut total: HashMap<String, usize> = HashMap::new();
    for s in &working_sections {
        *total.entry(s.name.clone()).or_insert(0) += 1;
    }
    let mut seen: HashMap<String, usize> = HashMap::new();

    // Single-section pickers stay roomy (one button per row, longer preview).
    // Multi-section grids stay narrow so 3-per-row chunks remain legible.
    let label_max = if working_sections.len() == 1 { 36 } else { 28 };
    let buttons: Vec<teloxide::types::InlineKeyboardButton> = working_sections
        .iter()
        .enumerate()
        .map(|(idx, s)| {
            let occ = seen.entry(s.name.clone()).or_insert(0);
            *occ += 1;
            let mut display = s.clone();
            if total.get(&s.name).copied().unwrap_or(1) > 1 {
                display.name = format!("{} ({})", s.name, occ);
            }
            let label = crate::lyrics::section_button_label(&display, label_max);
            crate::telegram::cb(
                label,
                format!("downloads:lyr_cap:{}:{}:{}", audio_msg_id.0, session_id, idx),
            )
        })
        .collect();

    let chunk_size = if working_sections.len() <= 3 { 1 } else { 2 };
    let mut rows: Vec<Vec<teloxide::types::InlineKeyboardButton>> =
        buttons.chunks(chunk_size).map(|c| c.to_vec()).collect();
    rows.push(vec![crate::telegram::cb(
        "📄 All Lyrics".to_string(),
        format!("downloads:lyr_cap:{}:{}:all", audio_msg_id.0, session_id),
    )]);
    rows.push(vec![crate::telegram::cb(
        "❌ Skip".to_string(),
        "downloads:cancel".to_string(),
    )]);

    let display = format!("{} – {}", lyrics.artist, lyrics.title);
    let msg = if working_sections.len() > 1 {
        format!("🎵 {}\nChoose lyrics to add as caption:", display)
    } else {
        format!("🎵 {}\nAdd lyrics as caption?", display)
    };

    let keyboard = teloxide::types::InlineKeyboardMarkup::new(rows);
    if let Err(e) = bot.send_message(chat_id, msg).reply_markup(keyboard).await {
        log::warn!("Failed to send lyrics picker for with_lyrics: {}", e);
    }
}
