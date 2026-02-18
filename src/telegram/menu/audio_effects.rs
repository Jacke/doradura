use crate::core::escape_markdown;
use crate::storage::db::{self, DbPool};
use crate::telegram::Bot;
use std::sync::Arc;
use teloxide::prelude::*;
use teloxide::types::{InlineKeyboardMarkup, MessageId};
use teloxide::RequestError;
use uuid::Uuid;

use super::helpers::edit_caption_or_text;

// ==================== Audio Cut ====================

pub(crate) async fn handle_audio_cut_callback(bot: Bot, q: CallbackQuery, db_pool: Arc<DbPool>) -> ResponseResult<()> {
    let callback_id = q.id.clone();
    let data = q.data.clone().unwrap_or_default();
    let chat_id = q.message.as_ref().map(|m| m.chat().id);
    let message_id = q.message.as_ref().map(|m| m.id());

    if let (Some(chat_id), Some(message_id)) = (chat_id, message_id) {
        let parts: Vec<&str> = data.split(':').collect();
        if parts.len() < 2 {
            bot.answer_callback_query(callback_id).await?;
            return Ok(());
        }

        let action = parts[1];
        let conn = db::get_connection(&db_pool)
            .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
        if !db::is_premium_or_vip(&conn, chat_id.0)
            .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
        {
            bot.answer_callback_query(callback_id)
                .text("⭐ This feature is available in Premium for ~$6/month → /plan")
                .show_alert(true)
                .await?;
            return Ok(());
        }

        match action {
            "open" => {
                let session_id = if let Some(session_id) = parts.get(2) {
                    *session_id
                } else {
                    bot.answer_callback_query(callback_id)
                        .text("❌ Invalid request")
                        .await?;
                    return Ok(());
                };
                let session = match db::get_audio_effect_session(&conn, session_id)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?
                {
                    Some(session) => session,
                    None => {
                        bot.answer_callback_query(callback_id)
                            .text("❌ Session not found")
                            .show_alert(true)
                            .await?;
                        return Ok(());
                    }
                };

                if session.is_expired() {
                    bot.answer_callback_query(callback_id)
                        .text("❌ Session expired (24 hours). Please download the track again.")
                        .show_alert(true)
                        .await?;
                    return Ok(());
                }

                let now = chrono::Utc::now();
                let cut_session = db::AudioCutSession {
                    id: Uuid::new_v4().to_string(),
                    user_id: chat_id.0,
                    audio_session_id: session_id.to_string(),
                    created_at: now,
                    expires_at: now + chrono::Duration::minutes(10),
                };
                db::upsert_audio_cut_session(&conn, &cut_session)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;

                bot.answer_callback_query(callback_id).await?;

                if let Err(e) = bot.edit_message_reply_markup(chat_id, message_id).await {
                    log::warn!("Failed to remove buttons from audio message: {}", e);
                }

                let keyboard = InlineKeyboardMarkup::new(vec![vec![crate::telegram::cb(
                    "❌ Cancel".to_string(),
                    "ac:cancel".to_string(),
                )]]);

                crate::telegram::send_message_markdown_v2(
                    &bot,
                    chat_id,
                    "✂️ Send time intervals for audio trimming in `mm:ss-mm:ss` or `hh:mm:ss-hh:mm:ss` format\\.\nMultiple intervals separated by commas\\.\n\nExample: `00:10-00:25, 01:00-01:10`\n\nOr type `cancel`\\.",
                    Some(keyboard),
                )
                .await?;
            }
            "cancel" => {
                db::delete_audio_cut_session_by_user(&conn, chat_id.0)
                    .map_err(|e| RequestError::from(std::sync::Arc::new(std::io::Error::other(e.to_string()))))?;
                bot.answer_callback_query(callback_id).await?;
                let _ = bot.delete_message(chat_id, message_id).await;
            }
            _ => {
                bot.answer_callback_query(callback_id).await?;
            }
        }
    }

    Ok(())
}

// ==================== Audio Effects UI ====================

/// Create audio effects keyboard with pitch and tempo controls
pub(crate) fn create_audio_effects_keyboard(
    session_id: &str,
    current_pitch: i8,
    current_tempo: f32,
    current_bass: i8,
    current_morph: crate::download::audio_effects::MorphProfile,
) -> InlineKeyboardMarkup {
    use teloxide::types::InlineKeyboardButton;

    let build_pitch_row = |values: &[i8]| -> Vec<InlineKeyboardButton> {
        values
            .iter()
            .map(|&value| {
                let marker = if current_pitch == value { " ✓" } else { "" };
                let prefix = if value >= 0 { "P+" } else { "P" };
                let label = format!("{}{}{}", prefix, value.abs(), marker);
                crate::telegram::cb(label, format!("ae:pitch:{}:{}", session_id, value))
            })
            .collect()
    };

    let pitch_rows = vec![build_pitch_row(&[-3, -2, -1]), build_pitch_row(&[0, 1, 2, 3])];

    let build_tempo_row = |values: &[f32]| -> Vec<InlineKeyboardButton> {
        values
            .iter()
            .map(|&value| {
                let marker = if (current_tempo - value).abs() < 0.01 {
                    " ✓"
                } else {
                    ""
                };
                crate::telegram::cb(
                    format!("T{}x{}", value, marker),
                    format!("ae:tempo:{}:{}", session_id, value),
                )
            })
            .collect()
    };

    let tempo_rows = vec![build_tempo_row(&[0.5, 0.75]), build_tempo_row(&[1.0, 1.25, 1.5, 2.0])];

    let build_bass_row = |values: &[i8]| -> Vec<InlineKeyboardButton> {
        values
            .iter()
            .map(|&value| {
                let marker = if current_bass == value { " ✓" } else { "" };
                crate::telegram::cb(
                    format!("B{:+}{}", value, marker),
                    format!("ae:bass:{}:{:+}", session_id, value),
                )
            })
            .collect()
    };

    let bass_rows = vec![build_bass_row(&[-6, -3, 0]), build_bass_row(&[3, 6])];

    let action_row = vec![
        crate::telegram::cb("✅ Apply Changes", format!("ae:apply:{}", session_id)),
        crate::telegram::cb("🔄 Reset", format!("ae:reset:{}", session_id)),
    ];

    let skip_row = vec![crate::telegram::cb("⏭️ Skip", format!("ae:skip:{}", session_id))];

    let morph_row = vec![crate::telegram::cb(
        format!(
            "🤖 M: {}",
            match current_morph {
                crate::download::audio_effects::MorphProfile::None => "Off",
                crate::download::audio_effects::MorphProfile::Soft => "Soft",
                crate::download::audio_effects::MorphProfile::Aggressive => "Aggro",
                crate::download::audio_effects::MorphProfile::Lofi => "LoFi",
                crate::download::audio_effects::MorphProfile::Wide => "Wide",
            }
        ),
        format!("ae:morph:{}", session_id),
    )];

    let mut rows: Vec<Vec<InlineKeyboardButton>> = Vec::new();
    rows.extend(pitch_rows);
    rows.extend(tempo_rows);
    rows.extend(bass_rows);
    rows.push(morph_row);
    rows.push(action_row);
    rows.push(skip_row);

    InlineKeyboardMarkup::new(rows)
}

/// Show audio effects editor by sending a new message
pub(crate) async fn show_audio_effects_editor(
    bot: &Bot,
    chat_id: ChatId,
    session: &crate::download::audio_effects::AudioEffectSession,
) -> ResponseResult<()> {
    let pitch_str = escape_markdown(&format!("{:+}", session.pitch_semitones));
    let tempo_str = escape_markdown(&format!("{}", session.tempo_factor));

    let bass_str = escape_markdown(&format!("{:+} dB", session.bass_gain_db));
    let morph_str = match session.morph_profile {
        crate::download::audio_effects::MorphProfile::None => "Off",
        crate::download::audio_effects::MorphProfile::Soft => "Soft",
        crate::download::audio_effects::MorphProfile::Aggressive => "Aggro",
        crate::download::audio_effects::MorphProfile::Lofi => "LoFi",
        crate::download::audio_effects::MorphProfile::Wide => "Wide",
    };

    let text = format!(
        "🎵 *Audio Effects Editor*\n\
        Title: {}\n\
        Current: P {} \\| T {}x \\| B {} \\| M {}\n\n\
        Adjust pitch, tempo, bass, morph preset, then press Apply\\.",
        escape_markdown(&session.title),
        pitch_str,
        tempo_str,
        bass_str,
        escape_markdown(morph_str),
    );

    let keyboard = create_audio_effects_keyboard(
        &session.id,
        session.pitch_semitones,
        session.tempo_factor,
        session.bass_gain_db,
        session.morph_profile,
    );

    bot.send_message(chat_id, "P = Pitch • T = Tempo • B = Bass").await?;

    bot.send_message(chat_id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await?;

    Ok(())
}

/// Update existing audio effects editor message
pub(crate) async fn update_audio_effects_editor(
    bot: &Bot,
    chat_id: ChatId,
    message_id: MessageId,
    session: &crate::download::audio_effects::AudioEffectSession,
) -> ResponseResult<()> {
    let pitch_str = escape_markdown(&format!("{:+}", session.pitch_semitones));
    let tempo_str = escape_markdown(&format!("{}", session.tempo_factor));

    let bass_str = escape_markdown(&format!("{:+} dB", session.bass_gain_db));
    let morph_str = match session.morph_profile {
        crate::download::audio_effects::MorphProfile::None => "Off",
        crate::download::audio_effects::MorphProfile::Soft => "Soft",
        crate::download::audio_effects::MorphProfile::Aggressive => "Aggro",
        crate::download::audio_effects::MorphProfile::Lofi => "LoFi",
        crate::download::audio_effects::MorphProfile::Wide => "Wide",
    };

    let text = format!(
        "🎵 *Audio Effects Editor*\n\
        Title: {}\n\
        Current: P {} \\| T {}x \\| B {} \\| M {}\n\n\
        Adjust pitch, tempo, bass, morph preset, then press Apply\\.",
        escape_markdown(&session.title),
        pitch_str,
        tempo_str,
        bass_str,
        escape_markdown(morph_str),
    );

    let keyboard = create_audio_effects_keyboard(
        &session.id,
        session.pitch_semitones,
        session.tempo_factor,
        session.bass_gain_db,
        session.morph_profile,
    );

    bot.edit_message_text(chat_id, message_id, text)
        .parse_mode(teloxide::types::ParseMode::MarkdownV2)
        .reply_markup(keyboard)
        .await?;

    Ok(())
}

/// Handle audio effects callbacks
pub async fn handle_audio_effects_callback(
    bot: Bot,
    q: CallbackQuery,
    db_pool: Arc<crate::storage::db::DbPool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use crate::storage::db;

    let callback_id = q.id.clone();
    let data = q.data.clone().ok_or("No callback data")?;

    let message = q.message.ok_or("No message in callback")?;
    let chat_id = message.chat().id;
    let message_id = message.id();

    // Parse callback data
    let parts: Vec<&str> = data.split(':').collect();
    if parts.len() < 2 {
        bot.answer_callback_query(callback_id).await?;
        return Ok(());
    }

    let action = parts[1];

    // Check Premium/VIP access
    let conn = db::get_connection(&db_pool)?;
    if !db::is_premium_or_vip(&conn, chat_id.0)? {
        bot.answer_callback_query(callback_id)
            .text("⭐ This feature is available in Premium for ~$6/month → /plan")
            .show_alert(true)
            .await?;
        return Ok(());
    }

    match action {
        "open" => {
            let session_id = parts.get(2).ok_or("Missing session_id")?;

            let session = db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

            if session.is_expired() {
                bot.answer_callback_query(callback_id)
                    .text("❌ Session expired (24 hours). Please download the track again.")
                    .show_alert(true)
                    .await?;
                return Ok(());
            }

            bot.answer_callback_query(callback_id).await?;

            // Remove the "Edit Audio" button from the audio message
            if let Err(e) = bot.edit_message_reply_markup(chat_id, message_id).await {
                log::warn!("Failed to remove button from audio message: {}", e);
            }

            // Send a new editor message
            show_audio_effects_editor(&bot, chat_id, &session).await?;
        }

        "pitch" => {
            let session_id = parts.get(2).ok_or("Missing session_id")?;
            let pitch_str = parts.get(3).ok_or("Missing pitch value")?;
            let pitch: i8 = pitch_str.parse().map_err(|_| "Invalid pitch")?;

            let mut session = db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

            if session.processing {
                bot.answer_callback_query(callback_id)
                    .text("⏳ Please wait, processing...")
                    .await?;
                return Ok(());
            }

            session.pitch_semitones = pitch;
            db::update_audio_effect_session(
                &conn,
                session_id,
                pitch,
                session.tempo_factor,
                session.bass_gain_db,
                session.morph_profile.as_str(),
                &session.current_file_path,
                session.version,
            )?;

            bot.answer_callback_query(callback_id).await?;
            update_audio_effects_editor(&bot, chat_id, message_id, &session).await?;
        }

        "tempo" => {
            let session_id = parts.get(2).ok_or("Missing session_id")?;
            let tempo_str = parts.get(3).ok_or("Missing tempo value")?;
            let tempo: f32 = tempo_str.parse().map_err(|_| "Invalid tempo")?;

            let mut session = db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

            if session.processing {
                bot.answer_callback_query(callback_id)
                    .text("⏳ Please wait, processing...")
                    .await?;
                return Ok(());
            }

            session.tempo_factor = tempo;
            db::update_audio_effect_session(
                &conn,
                session_id,
                session.pitch_semitones,
                tempo,
                session.bass_gain_db,
                session.morph_profile.as_str(),
                &session.current_file_path,
                session.version,
            )?;

            bot.answer_callback_query(callback_id).await?;
            update_audio_effects_editor(&bot, chat_id, message_id, &session).await?;
        }

        "bass" => {
            let session_id = parts.get(2).ok_or("Missing session_id")?;
            let bass_str = parts.get(3).ok_or("Missing bass value")?;
            let bass: i8 = bass_str.parse().map_err(|_| "Invalid bass")?;

            let mut session = db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

            if session.processing {
                bot.answer_callback_query(callback_id)
                    .text("⏳ Please wait, processing...")
                    .await?;
                return Ok(());
            }

            session.bass_gain_db = bass;
            db::update_audio_effect_session(
                &conn,
                session_id,
                session.pitch_semitones,
                session.tempo_factor,
                bass,
                session.morph_profile.as_str(),
                &session.current_file_path,
                session.version,
            )?;

            bot.answer_callback_query(callback_id).await?;
            update_audio_effects_editor(&bot, chat_id, message_id, &session).await?;
        }

        "morph" => {
            let session_id = parts.get(2).ok_or("Missing session_id")?;

            let mut session = db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

            if session.processing {
                bot.answer_callback_query(callback_id)
                    .text("⏳ Please wait, processing...")
                    .await?;
                return Ok(());
            }

            // Cycle morph profiles
            session.morph_profile = match session.morph_profile {
                crate::download::audio_effects::MorphProfile::None => {
                    crate::download::audio_effects::MorphProfile::Soft
                }
                crate::download::audio_effects::MorphProfile::Soft => {
                    crate::download::audio_effects::MorphProfile::Aggressive
                }
                crate::download::audio_effects::MorphProfile::Aggressive => {
                    crate::download::audio_effects::MorphProfile::Lofi
                }
                crate::download::audio_effects::MorphProfile::Lofi => {
                    crate::download::audio_effects::MorphProfile::Wide
                }
                crate::download::audio_effects::MorphProfile::Wide => {
                    crate::download::audio_effects::MorphProfile::None
                }
            };

            db::update_audio_effect_session(
                &conn,
                session_id,
                session.pitch_semitones,
                session.tempo_factor,
                session.bass_gain_db,
                session.morph_profile.as_str(),
                &session.current_file_path,
                session.version,
            )?;

            bot.answer_callback_query(callback_id).await?;
            update_audio_effects_editor(&bot, chat_id, message_id, &session).await?;
        }

        "apply" => {
            let session_id = parts.get(2).ok_or("Missing session_id")?;

            let session = db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

            if session.processing {
                bot.answer_callback_query(callback_id)
                    .text("⏳ Please wait, processing...")
                    .await?;
                return Ok(());
            }

            bot.answer_callback_query(callback_id).await?;

            // Set processing flag
            db::set_session_processing(&conn, session_id, true)?;

            // Show processing message
            edit_caption_or_text(
                &bot,
                chat_id,
                message_id,
                format!(
                    "⏳ *Processing audio\\.\\.\\.*\n\n\
                    Pitch: {}\n\
                    Tempo: {}x\n\
                    Bass: {}\n\
                    Morph: {}\n\n\
                    {}",
                    escape_markdown(&format!("{:+}", session.pitch_semitones)),
                    escape_markdown(&format!("{}", session.tempo_factor)),
                    escape_markdown(&format!("{:+} dB", session.bass_gain_db)),
                    escape_markdown(match session.morph_profile {
                        crate::download::audio_effects::MorphProfile::None => "Off",
                        crate::download::audio_effects::MorphProfile::Soft => "Soft",
                        crate::download::audio_effects::MorphProfile::Aggressive => "Aggro",
                        crate::download::audio_effects::MorphProfile::Lofi => "LoFi",
                        crate::download::audio_effects::MorphProfile::Wide => "Wide",
                    }),
                    if session.duration > 300 {
                        "This may take up to 30 seconds\\.\\.\\."
                    } else {
                        "Please wait a few seconds\\.\\.\\."
                    }
                ),
                None,
            )
            .await?;

            // Spawn processing task
            let bot_clone = bot.clone();
            let db_pool_clone = Arc::clone(&db_pool);
            let session_clone = session.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    process_audio_effects(bot_clone, chat_id, message_id, session_clone, db_pool_clone).await
                {
                    log::error!("Failed to process audio effects: {}", e);
                }
            });
        }

        "reset" => {
            let session_id = parts.get(2).ok_or("Missing session_id")?;

            let mut session = db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

            session.pitch_semitones = 0;
            session.tempo_factor = 1.0;
            session.bass_gain_db = 0;
            session.morph_profile = crate::download::audio_effects::MorphProfile::None;
            db::update_audio_effect_session(
                &conn,
                session_id,
                0,
                1.0,
                0,
                crate::download::audio_effects::MorphProfile::None.as_str(),
                &session.current_file_path,
                session.version,
            )?;

            bot.answer_callback_query(callback_id).await?;
            update_audio_effects_editor(&bot, chat_id, message_id, &session).await?;
        }

        "cancel" => {
            bot.answer_callback_query(callback_id).await?;
            bot.delete_message(chat_id, message_id).await?;
        }

        "skip" => {
            bot.answer_callback_query(callback_id).await?;
            bot.delete_message(chat_id, message_id).await?;
        }

        "again" => {
            let session_id = parts.get(2).ok_or("Missing session_id")?;

            let session = db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

            if session.is_expired() {
                bot.answer_callback_query(callback_id)
                    .text("❌ Session expired (24 hours). Please download the track again.")
                    .show_alert(true)
                    .await?;
                return Ok(());
            }

            bot.answer_callback_query(callback_id).await?;

            // Send new editor message
            let pitch_str = escape_markdown(&format!("{:+}", session.pitch_semitones));
            let tempo_str = escape_markdown(&format!("{}", session.tempo_factor));

            let text = format!(
                "🎵 *Audio Effects Editor*\n\
                Title: {}\n\
                Current: Pitch {} \\| Tempo {}x \\| Bass {} \\| Morph {}\n\n\
                Adjust pitch, tempo, bass, morph preset, then press Apply\\.",
                escape_markdown(&session.title),
                pitch_str,
                tempo_str,
                escape_markdown(&format!("{:+} dB", session.bass_gain_db)),
                escape_markdown(match session.morph_profile {
                    crate::download::audio_effects::MorphProfile::None => "Off",
                    crate::download::audio_effects::MorphProfile::Soft => "Soft",
                    crate::download::audio_effects::MorphProfile::Aggressive => "Aggro",
                    crate::download::audio_effects::MorphProfile::Lofi => "LoFi",
                    crate::download::audio_effects::MorphProfile::Wide => "Wide",
                })
            );

            let keyboard = create_audio_effects_keyboard(
                &session.id,
                session.pitch_semitones,
                session.tempo_factor,
                session.bass_gain_db,
                session.morph_profile,
            );

            // New editor message after applying again (plain text message)
            bot.send_message(chat_id, text)
                .parse_mode(teloxide::types::ParseMode::MarkdownV2)
                .reply_markup(keyboard)
                .await?;
        }

        "original" => {
            let session_id = parts.get(2).ok_or("Missing session_id")?;

            let session = db::get_audio_effect_session(&conn, session_id)?.ok_or("Session not found")?;

            bot.answer_callback_query(callback_id).await?;

            // Send original file
            if std::path::Path::new(&session.original_file_path).exists() {
                let file = teloxide::types::InputFile::file(&session.original_file_path);
                bot.send_audio(chat_id, file)
                    .title(format!("{} (Original)", session.title))
                    .duration(session.duration)
                    .await?;
            } else {
                bot.send_message(chat_id, "❌ Original file not found. The session may have expired.")
                    .await?;
            }
        }

        _ => {
            bot.answer_callback_query(callback_id).await?;
        }
    }

    Ok(())
}

/// Process audio effects and send modified file
pub(crate) async fn process_audio_effects(
    bot: Bot,
    chat_id: ChatId,
    editor_message_id: MessageId,
    session: crate::download::audio_effects::AudioEffectSession,
    db_pool: Arc<crate::storage::db::DbPool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use crate::core::config;
    use crate::storage::db;
    use std::path::Path;

    let session_id = session.id.clone();
    let new_version = session.version + 1;

    // Generate output path
    let output_path_raw =
        crate::download::audio_effects::get_modified_file_path(&session_id, new_version, &config::DOWNLOAD_FOLDER);
    let output_path = shellexpand::tilde(&output_path_raw).into_owned();
    if let Some(parent) = Path::new(&output_path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Apply effects
    let settings = session.settings();
    let result =
        crate::download::audio_effects::apply_audio_effects(&session.original_file_path, &output_path, &settings).await;

    // Clear processing flag
    let conn = db::get_connection(&db_pool)?;
    db::set_session_processing(&conn, &session_id, false)?;

    match result {
        Ok(_) => {
            // Send modified audio
            let file = teloxide::types::InputFile::file(&output_path);
            let title = format!(
                "{} (Pitch {:+}, Tempo {}x, Bass {:+} dB, Morph {})",
                session.title,
                session.pitch_semitones,
                session.tempo_factor,
                session.bass_gain_db,
                match session.morph_profile {
                    crate::download::audio_effects::MorphProfile::None => "Off",
                    crate::download::audio_effects::MorphProfile::Soft => "Soft",
                    crate::download::audio_effects::MorphProfile::Aggressive => "Aggro",
                    crate::download::audio_effects::MorphProfile::Lofi => "LoFi",
                    crate::download::audio_effects::MorphProfile::Wide => "Wide",
                }
            );

            let sent_message = bot
                .send_audio(chat_id, file)
                .title(&title)
                .duration(session.duration)
                .await?;

            // Add "Edit Again" and "Get Original" buttons
            let keyboard = InlineKeyboardMarkup::new(vec![vec![
                crate::telegram::cb("🎛️ Edit Again", format!("ae:again:{}", session_id)),
                crate::telegram::cb("📥 Get Original", format!("ae:original:{}", session_id)),
            ]]);

            // Replace the sent audio message caption with the new buttons (no text change)
            bot.edit_message_reply_markup(chat_id, sent_message.id)
                .reply_markup(keyboard)
                .await?;

            // Update session in DB
            db::update_audio_effect_session(
                &conn,
                &session_id,
                session.pitch_semitones,
                session.tempo_factor,
                session.bass_gain_db,
                session.morph_profile.as_str(),
                &output_path,
                new_version,
            )?;

            // Delete old version file if exists
            if session.version > 0 && session.current_file_path != session.original_file_path {
                let _ = tokio::fs::remove_file(&session.current_file_path).await;
            }

            // Delete editor message
            bot.delete_message(chat_id, editor_message_id).await?;

            log::info!(
                "Audio effects applied for session {}: pitch {:+}, tempo {}x",
                session_id,
                session.pitch_semitones,
                session.tempo_factor
            );
        }
        Err(e) => {
            log::error!("Failed to apply audio effects: {}", e);

            let mut error_msg = e.to_string();
            if error_msg.chars().count() > 900 {
                let trimmed: String = error_msg.chars().take(900).collect();
                error_msg = format!("{} …", trimmed);
            }

            let error_text = format!("❌ *Processing error*\n\n{}", escape_markdown(&error_msg));

            edit_caption_or_text(&bot, chat_id, editor_message_id, error_text, None).await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_audio_effects_keyboard_default_values() {
        use crate::download::audio_effects::MorphProfile;
        let keyboard = create_audio_effects_keyboard("session123", 0, 1.0, 0, MorphProfile::None);

        // Keyboard should have 9 rows (2 pitch + 2 tempo + 2 bass + 1 morph + 1 action + 1 skip)
        assert_eq!(keyboard.inline_keyboard.len(), 9);
    }

    #[test]
    fn test_create_audio_effects_keyboard_with_changes() {
        use crate::download::audio_effects::MorphProfile;
        let keyboard = create_audio_effects_keyboard("session456", 2, 1.5, 3, MorphProfile::Lofi);

        // Verify the keyboard is created correctly
        assert!(!keyboard.inline_keyboard.is_empty());

        // Find the morph row (row 6, 0-indexed)
        let morph_row = &keyboard.inline_keyboard[6];
        let morph_button = &morph_row[0];
        // LoFi profile should show "LoFi" in the button text
        assert!(
            morph_button.text.contains("LoFi"),
            "Morph button: {}",
            morph_button.text
        );
    }

    #[test]
    fn test_create_audio_effects_keyboard_action_row() {
        use crate::download::audio_effects::MorphProfile;
        let keyboard = create_audio_effects_keyboard("test_id", 0, 1.0, 0, MorphProfile::None);

        // Action row (row 7, 0-indexed) should have Apply and Reset buttons
        let action_row = &keyboard.inline_keyboard[7];
        assert!(action_row[0].text.contains("Apply"), "Button: {}", action_row[0].text);
    }

    #[test]
    fn test_create_audio_effects_keyboard_skip_row() {
        use crate::download::audio_effects::MorphProfile;
        let keyboard = create_audio_effects_keyboard("test_id", 0, 1.0, 0, MorphProfile::None);

        // Skip row should be the last row (row 8, 0-indexed)
        let skip_row = &keyboard.inline_keyboard[8];
        assert!(skip_row[0].text.contains("Skip"), "Button: {}", skip_row[0].text);
    }
}
