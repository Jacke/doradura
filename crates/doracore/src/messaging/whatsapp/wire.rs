//! Pure builders for WhatsApp Cloud API outbound payloads.
//!
//! Every function here returns a `serde_json::Value` ready to POST to
//! `/{phone_id}/messages` — **no network, no side effects** — so the whole
//! translation layer is unit-testable. The async [`super::client`] just sends
//! whatever these produce.

use crate::messaging::capabilities::Capabilities;
use crate::messaging::types::{Keyboard, MediaKind};
use serde_json::{Value, json};

use super::format::{
    self, BUTTON_TITLE_MAX, INTERACTIVE_BODY_MAX, LIST_BUTTON_LABEL_MAX, LIST_ROW_DESC_MAX, LIST_ROW_TITLE_MAX,
    SECTION_TITLE_MAX,
};

/// WhatsApp message `type` string for a neutral [`MediaKind`].
///
/// WhatsApp has no video-note or animation primitive, so both ride on `video`
/// (the closest native carrier).
pub fn media_type_str(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Audio => "audio",
        MediaKind::Video | MediaKind::VideoNote | MediaKind::Animation => "video",
        MediaKind::Photo => "image",
        MediaKind::Document => "document",
    }
}

/// Whether the Cloud API accepts a caption for this media type. Audio and
/// stickers carry none; image/video/document do.
pub fn supports_caption(kind: MediaKind) -> bool {
    !matches!(kind, MediaKind::Audio)
}

/// Where the media bytes come from on the wire: a public URL WhatsApp fetches,
/// or a `media_id` we uploaded / cached earlier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaRef {
    Link(String),
    Id(String),
}

/// A plain text message (`type: text`). `preview_url` asks WhatsApp to render a
/// link preview for the first URL in the body.
pub fn text_payload(to: &str, body: &str, preview_url: bool) -> Value {
    json!({
        "messaging_product": "whatsapp",
        "recipient_type": "individual",
        "to": to,
        "type": "text",
        "text": { "preview_url": preview_url, "body": body },
    })
}

/// An interactive **reply-buttons** message (≤3 buttons). `buttons` are
/// `(action_id, label)` pairs; labels are clamped to 20 chars and ids deduped
/// implicitly by the caller's routing tokens.
pub fn buttons_payload(to: &str, body: &str, buttons: &[(String, String)]) -> Value {
    let btns: Vec<Value> = buttons
        .iter()
        .take(3)
        .map(|(id, label)| {
            json!({
                "type": "reply",
                "reply": { "id": id, "title": non_empty(&format::truncate(label, BUTTON_TITLE_MAX), "·") },
            })
        })
        .collect();
    json!({
        "messaging_product": "whatsapp",
        "recipient_type": "individual",
        "to": to,
        "type": "interactive",
        "interactive": {
            "type": "button",
            "body": { "text": non_empty(&format::truncate(body, INTERACTIVE_BODY_MAX), "·") },
            "action": { "buttons": btns },
        },
    })
}

/// An interactive **single-select list** message (≤10 rows). `button_label` is
/// the text on the list-opener button; `rows` are `(action_id, label)` pairs.
pub fn list_payload(to: &str, body: &str, button_label: &str, section_title: &str, rows: &[(String, String)]) -> Value {
    let rows_json: Vec<Value> = rows
        .iter()
        .take(10)
        .map(|(id, label)| {
            json!({
                "id": id,
                "title": non_empty(&format::truncate(label, LIST_ROW_TITLE_MAX), "·"),
            })
        })
        .collect();
    json!({
        "messaging_product": "whatsapp",
        "recipient_type": "individual",
        "to": to,
        "type": "interactive",
        "interactive": {
            "type": "list",
            "body": { "text": non_empty(&format::truncate(body, INTERACTIVE_BODY_MAX), "·") },
            "action": {
                "button": non_empty(&format::truncate(button_label, LIST_BUTTON_LABEL_MAX), "Menu"),
                "sections": [{
                    "title": format::truncate(section_title, SECTION_TITLE_MAX),
                    "rows": rows_json,
                }],
            },
        },
    })
}

/// A media message (`type: image|video|audio|document`). `caption` is dropped
/// for kinds that don't support it; `filename` is only attached to documents.
pub fn media_payload(
    to: &str,
    kind: MediaKind,
    media: &MediaRef,
    caption: Option<&str>,
    filename: Option<&str>,
) -> Value {
    let ty = media_type_str(kind);
    let mut obj = serde_json::Map::new();
    match media {
        MediaRef::Link(url) => {
            obj.insert("link".into(), json!(url));
        }
        MediaRef::Id(id) => {
            obj.insert("id".into(), json!(id));
        }
    }
    if supports_caption(kind)
        && let Some(c) = caption.filter(|c| !c.is_empty())
    {
        obj.insert("caption".into(), json!(c));
    }
    if matches!(kind, MediaKind::Document)
        && let Some(name) = filename
    {
        obj.insert("filename".into(), json!(name));
    }
    json!({
        "messaging_product": "whatsapp",
        "recipient_type": "individual",
        "to": to,
        "type": ty,
        ty: Value::Object(obj),
    })
}

/// How a neutral [`Keyboard`] should be rendered for WhatsApp, chosen from the
/// platform's [`Capabilities`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyboardPlan {
    /// No buttons at all.
    None,
    /// ≤3 → native reply buttons.
    Buttons(Vec<(String, String)>),
    /// ≤10 → single-select list.
    List(Vec<(String, String)>),
    /// More than 10 (or `inline_keyboard == false`) → append a numbered menu to
    /// the body; the inbound side resolves the user's number reply to an action.
    NumberedText(Vec<(String, String)>),
}

/// Decide how to render `kb` given platform `caps`. Pure — no truncation here;
/// the payload builders clamp field lengths.
pub fn plan_keyboard(kb: Option<&Keyboard>, caps: &Capabilities) -> KeyboardPlan {
    let Some(kb) = kb.filter(|k| !k.is_empty()) else {
        return KeyboardPlan::None;
    };
    let flat: Vec<(String, String)> = kb
        .flat()
        .into_iter()
        .map(|b| (b.action.clone(), b.label.clone()))
        .collect();
    let n = flat.len();
    if !caps.inline_keyboard {
        return KeyboardPlan::NumberedText(flat);
    }
    if n <= caps.max_buttons_total {
        KeyboardPlan::Buttons(flat)
    } else if n <= caps.list_menu_max {
        KeyboardPlan::List(flat)
    } else {
        KeyboardPlan::NumberedText(flat)
    }
}

/// Append a numbered menu to `body` for the overflow / no-buttons fallback:
/// `1. Label`, `2. Label`, … The inbound source maps a bare-number reply back
/// to the Nth action.
pub fn numbered_menu_text(body: &str, items: &[(String, String)]) -> String {
    let mut out = body.to_string();
    if !items.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        for (i, (_id, label)) in items.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&format!("{}. {}", i + 1, label));
        }
    }
    out
}

/// Clamp a list-row description to the Cloud API limit (exposed for callers that
/// build richer rows later).
pub fn clamp_row_description(desc: &str) -> String {
    format::truncate(desc, LIST_ROW_DESC_MAX)
}

/// WhatsApp rejects empty titles/bodies; substitute a placeholder.
fn non_empty(s: &str, fallback: &str) -> String {
    if s.trim().is_empty() {
        fallback.to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messaging::types::Button;

    fn caps() -> Capabilities {
        Capabilities::WHATSAPP
    }

    #[test]
    fn media_type_mapping() {
        assert_eq!(media_type_str(MediaKind::Audio), "audio");
        assert_eq!(media_type_str(MediaKind::Video), "video");
        assert_eq!(media_type_str(MediaKind::VideoNote), "video");
        assert_eq!(media_type_str(MediaKind::Animation), "video");
        assert_eq!(media_type_str(MediaKind::Photo), "image");
        assert_eq!(media_type_str(MediaKind::Document), "document");
    }

    #[test]
    fn text_payload_shape() {
        let v = text_payload("4915112345678", "hello", true);
        assert_eq!(v["messaging_product"], "whatsapp");
        assert_eq!(v["to"], "4915112345678");
        assert_eq!(v["type"], "text");
        assert_eq!(v["text"]["body"], "hello");
        assert_eq!(v["text"]["preview_url"], true);
    }

    #[test]
    fn buttons_payload_clamps_to_three_and_truncates_titles() {
        let btns = vec![
            ("a".into(), "First button label that is way too long".into()),
            ("b".into(), "Second".into()),
            ("c".into(), "Third".into()),
            ("d".into(), "Fourth (dropped)".into()),
        ];
        let v = buttons_payload("123", "pick one", &btns);
        let arr = v["interactive"]["action"]["buttons"].as_array().unwrap();
        assert_eq!(arr.len(), 3, "max 3 reply buttons");
        let title = arr[0]["reply"]["title"].as_str().unwrap();
        assert!(title.chars().count() <= BUTTON_TITLE_MAX);
        assert!(title.ends_with('…'));
        assert_eq!(arr[0]["reply"]["id"], "a");
        assert_eq!(v["interactive"]["type"], "button");
    }

    #[test]
    fn list_payload_shape() {
        let rows: Vec<(String, String)> = (0..12).map(|i| (format!("act{i}"), format!("Row {i}"))).collect();
        let v = list_payload("123", "menu body", "Open", "Choices", &rows);
        let sect = &v["interactive"]["action"]["sections"][0];
        assert_eq!(v["interactive"]["type"], "list");
        assert_eq!(sect["rows"].as_array().unwrap().len(), 10, "max 10 list rows");
        assert_eq!(v["interactive"]["action"]["button"], "Open");
    }

    #[test]
    fn media_payload_link_with_caption() {
        let v = media_payload(
            "123",
            MediaKind::Video,
            &MediaRef::Link("https://x/y.mp4".into()),
            Some("cap"),
            None,
        );
        assert_eq!(v["type"], "video");
        assert_eq!(v["video"]["link"], "https://x/y.mp4");
        assert_eq!(v["video"]["caption"], "cap");
    }

    #[test]
    fn media_payload_audio_drops_caption() {
        let v = media_payload("123", MediaKind::Audio, &MediaRef::Id("MID".into()), Some("cap"), None);
        assert_eq!(v["audio"]["id"], "MID");
        assert!(v["audio"].get("caption").is_none(), "audio carries no caption");
    }

    #[test]
    fn media_payload_document_filename() {
        let v = media_payload(
            "123",
            MediaKind::Document,
            &MediaRef::Id("MID".into()),
            Some("cap"),
            Some("song.mp3"),
        );
        assert_eq!(v["document"]["filename"], "song.mp3");
        assert_eq!(v["document"]["caption"], "cap");
    }

    #[test]
    fn plan_keyboard_thresholds() {
        let none = plan_keyboard(None, &caps());
        assert_eq!(none, KeyboardPlan::None);

        let three = Keyboard::new(vec![
            (0..3).map(|i| Button::new(format!("L{i}"), format!("a{i}"))).collect(),
        ]);
        assert!(matches!(plan_keyboard(Some(&three), &caps()), KeyboardPlan::Buttons(_)));

        let seven = Keyboard::new(vec![
            (0..7).map(|i| Button::new(format!("L{i}"), format!("a{i}"))).collect(),
        ]);
        assert!(matches!(plan_keyboard(Some(&seven), &caps()), KeyboardPlan::List(_)));

        let many = Keyboard::new(vec![
            (0..15).map(|i| Button::new(format!("L{i}"), format!("a{i}"))).collect(),
        ]);
        assert!(matches!(
            plan_keyboard(Some(&many), &caps()),
            KeyboardPlan::NumberedText(_)
        ));
    }

    #[test]
    fn plan_keyboard_no_inline_always_numbered() {
        let one = Keyboard::new(vec![vec![Button::new("L", "a")]]);
        assert!(matches!(
            plan_keyboard(Some(&one), &Capabilities::IMESSAGE),
            KeyboardPlan::NumberedText(_)
        ));
    }

    #[test]
    fn numbered_menu_text_format() {
        let items = vec![("a".into(), "Audio".into()), ("v".into(), "Video".into())];
        let out = numbered_menu_text("Choose:", &items);
        assert_eq!(out, "Choose:\n\n1. Audio\n2. Video");
    }

    #[test]
    fn empty_title_gets_placeholder() {
        let v = buttons_payload("1", "", &[("a".into(), "".into())]);
        assert_eq!(v["interactive"]["body"]["text"], "·");
        assert_eq!(v["interactive"]["action"]["buttons"][0]["reply"]["title"], "·");
    }
}
