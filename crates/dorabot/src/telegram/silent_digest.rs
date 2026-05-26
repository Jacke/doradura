//! Silent-downloads MOTD digest (V49).
//!
//! When a user has silent downloads enabled, finished (and failed) downloads
//! are recorded in the `silent_digest` table instead of pinging the user. On
//! the user's next interaction with the bot we recap them once, message-of-the-
//! day style, then mark the rows shown.
//!
//! `maybe_show_silent_digest` is called at the top of the message and callback
//! endpoints. It is cheap when there's nothing pending (a single indexed
//! `UPDATE … RETURNING` that touches no rows) and idempotent (a concurrent
//! second call gets an empty result, so the recap never doubles).

use std::sync::Arc;

use teloxide::prelude::*;

use crate::storage::SharedStorage;
use crate::telegram::Bot;
use crate::telegram::BotExt;

/// Show the pending silent-download recap to `user_id`, if any. Best-effort:
/// storage or send errors are logged and swallowed so this never blocks the
/// normal update handling it precedes.
pub async fn maybe_show_silent_digest(bot: &Bot, shared_storage: &Arc<SharedStorage>, user_id: i64) {
    let entries = match shared_storage.take_unshown_silent_digest(user_id).await {
        Ok(entries) if !entries.is_empty() => entries,
        Ok(_) => return,
        Err(e) => {
            log::warn!("silent_digest: take_unshown failed for {}: {}", user_id, e);
            return;
        }
    };

    let lang = crate::i18n::user_lang_from_storage(shared_storage, user_id).await;
    let text = build_digest_message(&entries, &lang);
    if let Err(e) = bot.send_md(ChatId(user_id), text).await {
        log::warn!("silent_digest: failed to send recap to {}: {}", user_id, e);
    }
}

/// Format the MOTD recap from pending digest rows. Public for unit testing.
pub fn build_digest_message(
    entries: &[doracore::storage::db::SilentDigestEntry],
    lang: &unic_langid::LanguageIdentifier,
) -> String {
    let done = entries.iter().filter(|e| e.status != "failed").count();
    let failed = entries.iter().filter(|e| e.status == "failed").count();

    let mut lines = Vec::with_capacity(entries.len() + 1);
    let header = if failed == 0 {
        crate::i18n::t_args(
            lang,
            "silent-motd-header-done",
            &doracore::fluent_args!("count" => done as i64),
        )
    } else if done == 0 {
        crate::i18n::t_args(
            lang,
            "silent-motd-header-failed",
            &doracore::fluent_args!("count" => failed as i64),
        )
    } else {
        crate::i18n::t_args(
            lang,
            "silent-motd-header-mixed",
            &doracore::fluent_args!("done" => done as i64, "failed" => failed as i64),
        )
    };
    lines.push(header);

    let untitled = crate::i18n::t(lang, "silent-motd-untitled");
    let failed_suffix = crate::i18n::t(lang, "silent-motd-failed-suffix");
    for entry in entries {
        let icon = if entry.status == "failed" {
            "❌"
        } else {
            match entry.format.as_deref() {
                Some("mp3") => "🎵",
                Some("mp4") => "🎬",
                _ => "📄",
            }
        };
        let title = entry.title.as_deref().unwrap_or(untitled.as_str());
        let suffix = if entry.status == "failed" {
            failed_suffix.as_str()
        } else {
            ""
        };
        lines.push(format!("{} {}{}", icon, crate::core::escape_markdown(title), suffix));
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use doracore::storage::db::SilentDigestEntry;

    /// Fluent wraps interpolated args in bidi isolation marks
    /// (U+2068 FSI / U+2069 PDI). Strip them so substring assertions that span
    /// literal text and an interpolated value match.
    fn strip_iso(s: &str) -> String {
        s.chars().filter(|c| *c != '\u{2068}' && *c != '\u{2069}').collect()
    }

    fn entry(title: &str, format: &str, status: &str) -> SilentDigestEntry {
        SilentDigestEntry {
            title: Some(title.to_string()),
            format: Some(format.to_string()),
            status: status.to_string(),
        }
    }

    #[test]
    fn all_done_header_and_icons() {
        let entries = vec![entry("Дора - Дорадура", "mp3", "done"), entry("Клип", "mp4", "done")];
        let msg = strip_iso(&build_digest_message(&entries, &crate::i18n::lang_from_code("ru")));
        assert!(msg.contains("готово тихих загрузок: 2"));
        assert!(msg.contains("🎵"));
        assert!(msg.contains("🎬"));
        assert!(!msg.contains("не удалось"));
    }

    #[test]
    fn mixed_done_and_failed() {
        let entries = vec![entry("OK", "mp3", "done"), entry("Broken", "mp4", "failed")];
        let msg = strip_iso(&build_digest_message(&entries, &crate::i18n::lang_from_code("ru")));
        assert!(msg.contains("готово: 1, не удалось: 1"));
        assert!(msg.contains("❌"));
    }

    #[test]
    fn only_failed_header() {
        let entries = vec![entry("Broken", "mp4", "failed")];
        let msg = strip_iso(&build_digest_message(&entries, &crate::i18n::lang_from_code("ru")));
        assert!(msg.contains("не удалось загрузок: 1"));
    }

    #[test]
    fn missing_title_falls_back() {
        let mut e = entry("x", "mp3", "failed");
        e.title = None;
        let msg = build_digest_message(&[e], &crate::i18n::lang_from_code("ru"));
        assert!(msg.contains("без названия"));
    }
}
