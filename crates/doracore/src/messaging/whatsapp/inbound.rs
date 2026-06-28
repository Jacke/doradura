//! Inbound side: WhatsApp webhook JSON → neutral [`InboundMessage`]s.
//!
//! A single webhook `POST` is a batch:
//! `entry[].changes[].value.messages[]`. We walk every message, skip delivery
//! `statuses` (no `messages` key), and map each to one [`InboundMessage`].
//! Navigation is done over `serde_json::Value` directly so unknown fields the
//! API adds later never break parsing.

use serde_json::Value;

use crate::messaging::InboundSource;
use crate::messaging::types::{ChatRef, InboundEvent, InboundMessage, Platform, UserRef};

/// WhatsApp Cloud API inbound decoder. Stateless.
#[derive(Debug, Default, Clone, Copy)]
pub struct WhatsAppInbound;

impl WhatsAppInbound {
    pub fn new() -> Self {
        Self
    }
}

impl InboundSource for WhatsAppInbound {
    fn platform(&self) -> Platform {
        Platform::WhatsApp
    }

    fn normalize(&self, raw: &Value) -> Vec<InboundMessage> {
        let mut out = Vec::new();
        let Some(entries) = raw.get("entry").and_then(Value::as_array) else {
            return out;
        };
        for entry in entries {
            let Some(changes) = entry.get("changes").and_then(Value::as_array) else {
                continue;
            };
            for change in changes {
                let Some(messages) = change.pointer("/value/messages").and_then(Value::as_array) else {
                    continue; // statuses-only delivery → nothing to route
                };
                for msg in messages {
                    if let Some(im) = decode_message(msg) {
                        out.push(im);
                    }
                }
            }
        }
        out
    }
}

/// Decode one `messages[]` entry. Returns `None` for unsupported types.
fn decode_message(msg: &Value) -> Option<InboundMessage> {
    let from = msg.get("from").and_then(Value::as_str)?.to_string();
    let event = decode_event(msg)?;
    let chat = ChatRef {
        platform: Platform::WhatsApp,
        chat_id: from.clone(),
    };
    let user = UserRef {
        platform: Platform::WhatsApp,
        user_id: from,
    };
    Some(InboundMessage { chat, user, event })
}

/// Map a WhatsApp message to a neutral [`InboundEvent`].
fn decode_event(msg: &Value) -> Option<InboundEvent> {
    match msg.get("type").and_then(Value::as_str)? {
        "text" => {
            let body = msg.pointer("/text/body").and_then(Value::as_str)?.to_string();
            Some(InboundEvent::Text { body })
        }
        "interactive" => {
            let inter = msg.get("interactive")?;
            match inter.get("type").and_then(Value::as_str)? {
                "button_reply" => {
                    let id = inter.pointer("/button_reply/id").and_then(Value::as_str)?.to_string();
                    Some(InboundEvent::Action { id })
                }
                "list_reply" => {
                    let id = inter.pointer("/list_reply/id").and_then(Value::as_str)?.to_string();
                    Some(InboundEvent::Action { id })
                }
                _ => None,
            }
        }
        // Quick-reply buttons on template messages arrive as `type: button`.
        "button" => {
            let id = msg.pointer("/button/payload").and_then(Value::as_str)?.to_string();
            Some(InboundEvent::Action { id })
        }
        "document" => Some(InboundEvent::Document {
            file_ref: msg.pointer("/document/id").and_then(Value::as_str)?.to_string(),
            file_name: msg
                .pointer("/document/filename")
                .and_then(Value::as_str)
                .map(str::to_string),
            mime: msg
                .pointer("/document/mime_type")
                .and_then(Value::as_str)
                .map(str::to_string),
        }),
        // audio / image / video / sticker / location / contacts: not routed yet.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn wrap(message: Value) -> Value {
        json!({
            "object": "whatsapp_business_account",
            "entry": [{
                "id": "WABA_ID",
                "changes": [{
                    "field": "messages",
                    "value": {
                        "messaging_product": "whatsapp",
                        "metadata": { "display_phone_number": "1555", "phone_number_id": "PID" },
                        "contacts": [{ "profile": { "name": "Дора" }, "wa_id": "4915112345678" }],
                        "messages": [message]
                    }
                }]
            }]
        })
    }

    #[test]
    fn decodes_text_message() {
        let raw = wrap(json!({
            "from": "4915112345678",
            "id": "wamid.TEXT",
            "timestamp": "1700000000",
            "type": "text",
            "text": { "body": "https://youtu.be/jNQXAC9IVRw" }
        }));
        let msgs = WhatsAppInbound::new().normalize(&raw);
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].chat.platform, Platform::WhatsApp);
        assert_eq!(msgs[0].chat.chat_id, "4915112345678");
        assert_eq!(msgs[0].user.user_id, "4915112345678");
        assert_eq!(
            msgs[0].event,
            InboundEvent::Text {
                body: "https://youtu.be/jNQXAC9IVRw".into()
            }
        );
    }

    #[test]
    fn decodes_button_reply_to_action() {
        let raw = wrap(json!({
            "from": "49151",
            "id": "wamid.BTN",
            "type": "interactive",
            "interactive": {
                "type": "button_reply",
                "button_reply": { "id": "dl:audio", "title": "Audio" }
            }
        }));
        let msgs = WhatsAppInbound::new().normalize(&raw);
        assert_eq!(msgs[0].event, InboundEvent::Action { id: "dl:audio".into() });
    }

    #[test]
    fn decodes_list_reply_to_action() {
        let raw = wrap(json!({
            "from": "49151",
            "id": "wamid.LIST",
            "type": "interactive",
            "interactive": {
                "type": "list_reply",
                "list_reply": { "id": "exp:rs:42", "title": "Track" }
            }
        }));
        let msgs = WhatsAppInbound::new().normalize(&raw);
        assert_eq!(msgs[0].event, InboundEvent::Action { id: "exp:rs:42".into() });
    }

    #[test]
    fn decodes_template_quick_reply_button() {
        let raw = wrap(json!({
            "from": "49151",
            "id": "wamid.QR",
            "type": "button",
            "button": { "payload": "motd:open", "text": "Open" }
        }));
        let msgs = WhatsAppInbound::new().normalize(&raw);
        assert_eq!(msgs[0].event, InboundEvent::Action { id: "motd:open".into() });
    }

    #[test]
    fn decodes_document_upload() {
        let raw = wrap(json!({
            "from": "49151",
            "id": "wamid.DOC",
            "type": "document",
            "document": { "id": "MEDIA123", "filename": "cookies.txt", "mime_type": "text/plain" }
        }));
        let msgs = WhatsAppInbound::new().normalize(&raw);
        assert_eq!(
            msgs[0].event,
            InboundEvent::Document {
                file_ref: "MEDIA123".into(),
                file_name: Some("cookies.txt".into()),
                mime: Some("text/plain".into()),
            }
        );
    }

    #[test]
    fn skips_status_only_delivery() {
        let raw = json!({
            "object": "whatsapp_business_account",
            "entry": [{
                "id": "WABA",
                "changes": [{
                    "field": "messages",
                    "value": {
                        "messaging_product": "whatsapp",
                        "metadata": { "phone_number_id": "PID" },
                        "statuses": [{ "id": "wamid.X", "status": "delivered", "recipient_id": "49151" }]
                    }
                }]
            }]
        });
        assert!(WhatsAppInbound::new().normalize(&raw).is_empty());
    }

    #[test]
    fn batches_multiple_messages() {
        let raw = json!({
            "object": "whatsapp_business_account",
            "entry": [{
                "changes": [{
                    "value": {
                        "messages": [
                            { "from": "1", "type": "text", "text": { "body": "a" } },
                            { "from": "2", "type": "text", "text": { "body": "b" } }
                        ]
                    }
                }]
            }]
        });
        let msgs = WhatsAppInbound::new().normalize(&raw);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1].chat.chat_id, "2");
    }

    #[test]
    fn ignores_unknown_type_and_garbage() {
        assert!(WhatsAppInbound::new().normalize(&json!({})).is_empty());
        let raw = wrap(json!({ "from": "1", "type": "location", "location": { "latitude": 0 } }));
        assert!(WhatsAppInbound::new().normalize(&raw).is_empty());
    }
}
