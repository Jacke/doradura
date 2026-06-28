//! End-to-end integration test for the WhatsApp adapter against a local mock of
//! the Graph API. Exercises the real reqwest I/O path in
//! `doracore::messaging::whatsapp::client` — send, media upload, media download
//! — plus the `Messenger` trait surface, without hitting Meta.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::{Arc, Mutex};

use axum::{
    Router,
    extract::{Path, State},
    routing::{get, post},
};
use doracore::messaging::Messenger;
use doracore::messaging::types::{ChatRef, MediaKind, MediaSource, OutboundMessage, Platform, TextStyle};
use doracore::messaging::whatsapp::{WhatsAppClient, WhatsAppConfig, WhatsAppMessenger};
use serde_json::{Value, json};

/// Captured requests the mock saw, for assertions.
#[derive(Default)]
struct Captured {
    messages: Vec<Value>,
    upload_hits: usize,
}

type Shared = Arc<Mutex<Captured>>;

async fn messages_handler(State(state): State<Shared>, body: String) -> axum::Json<Value> {
    let v: Value = serde_json::from_str(&body).expect("messages body is JSON");
    state.lock().unwrap().messages.push(v);
    axum::Json(json!({ "messaging_product": "whatsapp", "messages": [{ "id": "wamid.MOCK123" }] }))
}

async fn media_upload_handler(State(state): State<Shared>, _body: axum::body::Bytes) -> axum::Json<Value> {
    state.lock().unwrap().upload_hits += 1;
    axum::Json(json!({ "id": "MOCKMEDIA42" }))
}

async fn download_handler() -> Vec<u8> {
    b"FAKE-MEDIA-BYTES".to_vec()
}

/// Spin the mock on an ephemeral port; returns (base_url, captured-state).
async fn spawn_mock() -> (String, Shared) {
    let state: Shared = Arc::new(Mutex::new(Captured::default()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");

    // The media-lookup route must return a download URL on *this* server, so it
    // closes over the freshly-bound base address.
    let base_for_lookup = base.clone();
    let app = Router::new()
        .route("/v21.0/{phone}/messages", post(messages_handler))
        .route("/v21.0/{phone}/media", post(media_upload_handler))
        .route(
            "/v21.0/{id}",
            get(move |path: Path<String>| {
                let base = base_for_lookup.clone();
                async move {
                    let Path(id) = path;
                    axum::Json(json!({ "url": format!("{base}/dl/{id}"), "id": id }))
                }
            }),
        )
        .route("/dl/{id}", get(download_handler))
        .with_state(state.clone());

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    // tiny yield so the listener is accepting before the test connects
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    (base, state)
}

fn cfg_for(base: &str) -> WhatsAppConfig {
    let mut cfg = WhatsAppConfig::new("tok", "PID", "vt", "sec");
    cfg.graph_base = base.to_string();
    cfg.graph_version = "v21.0".to_string();
    cfg
}

#[tokio::test]
async fn send_text_posts_payload_and_parses_wamid() {
    let (base, state) = spawn_mock().await;
    let messenger = WhatsAppMessenger::from_config(cfg_for(&base));

    let chat = ChatRef {
        platform: Platform::WhatsApp,
        chat_id: "4915112345678".into(),
    };
    let handle = messenger
        .send(
            &chat,
            OutboundMessage::Text {
                body: "<b>hi</b> there".into(),
                style: TextStyle::Html,
                keyboard: None,
            },
        )
        .await
        .expect("send ok");

    assert_eq!(handle.platform, Platform::WhatsApp);
    assert_eq!(handle.message_id, "wamid.MOCK123");

    let captured = state.lock().unwrap();
    assert_eq!(captured.messages.len(), 1);
    let m = &captured.messages[0];
    assert_eq!(m["type"], "text");
    assert_eq!(m["to"], "4915112345678");
    // HTML was converted to WhatsApp markup.
    assert_eq!(m["text"]["body"], "*hi* there");
}

#[tokio::test]
async fn send_media_local_uploads_then_sends_by_id() {
    let (base, state) = spawn_mock().await;
    let messenger = WhatsAppMessenger::from_config(cfg_for(&base));

    // Create a small local file to "upload".
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("song.mp3");
    std::fs::write(&path, b"ID3-fake-audio").unwrap();

    let chat = ChatRef {
        platform: Platform::WhatsApp,
        chat_id: "49151".into(),
    };
    let handle = messenger
        .send(
            &chat,
            OutboundMessage::Media {
                kind: MediaKind::Audio,
                source: MediaSource::LocalPath(path.to_string_lossy().to_string()),
                caption: Some("My Song".into()),
                style: TextStyle::Plain,
                keyboard: None,
            },
        )
        .await
        .expect("media send ok");

    assert_eq!(handle.message_id, "wamid.MOCK123");
    let captured = state.lock().unwrap();
    assert_eq!(captured.upload_hits, 1, "local file was uploaded");
    let m = &captured.messages[0];
    assert_eq!(m["type"], "audio");
    assert_eq!(m["audio"]["id"], "MOCKMEDIA42", "sent by uploaded media_id");
    // audio carries no caption per Cloud API.
    assert!(m["audio"].get("caption").is_none());
}

#[tokio::test]
async fn send_media_url_with_keyboard_sends_file_then_actions() {
    let (base, state) = spawn_mock().await;
    let messenger = WhatsAppMessenger::from_config(cfg_for(&base));

    let chat = ChatRef {
        platform: Platform::WhatsApp,
        chat_id: "49151".into(),
    };
    let kb = doracore::messaging::types::Keyboard::new(vec![vec![
        doracore::messaging::types::Button::new("More like this", "rec:more"),
        doracore::messaging::types::Button::new("Lyrics", "lyr:show"),
    ]]);
    messenger
        .send(
            &chat,
            OutboundMessage::Media {
                kind: MediaKind::Video,
                source: MediaSource::Url("https://cdn/x.mp4".into()),
                caption: Some("Clip".into()),
                style: TextStyle::Plain,
                keyboard: Some(kb),
            },
        )
        .await
        .expect("media+keyboard send ok");

    let captured = state.lock().unwrap();
    assert_eq!(captured.upload_hits, 0, "URL source needs no upload");
    assert_eq!(captured.messages.len(), 2, "file message + actions message");
    // 1) the media, sent by link, WITHOUT caption (caption moves to actions msg).
    assert_eq!(captured.messages[0]["type"], "video");
    assert_eq!(captured.messages[0]["video"]["link"], "https://cdn/x.mp4");
    assert!(captured.messages[0]["video"].get("caption").is_none());
    // 2) the actions message: interactive reply buttons carrying the caption.
    assert_eq!(captured.messages[1]["type"], "interactive");
    assert_eq!(captured.messages[1]["interactive"]["type"], "button");
    assert_eq!(captured.messages[1]["interactive"]["body"]["text"], "Clip");
    let btns = captured.messages[1]["interactive"]["action"]["buttons"]
        .as_array()
        .unwrap();
    assert_eq!(btns.len(), 2);
    assert_eq!(btns[0]["reply"]["id"], "rec:more");
}

#[tokio::test]
async fn edit_text_sends_a_new_message() {
    let (base, state) = spawn_mock().await;
    let messenger = WhatsAppMessenger::from_config(cfg_for(&base));
    let handle = doracore::messaging::types::MessageHandle {
        platform: Platform::WhatsApp,
        chat_id: "49151".into(),
        message_id: "wamid.OLD".into(),
    };
    let new = messenger
        .edit_text(&handle, "updated".into(), TextStyle::Plain, None)
        .await
        .expect("edit ok");
    // edit_in_place == false → a fresh message with the mock's new id.
    assert_eq!(new.message_id, "wamid.MOCK123");
    assert_eq!(state.lock().unwrap().messages.len(), 1);
}

#[tokio::test]
async fn client_downloads_media_to_file() {
    let (base, _state) = spawn_mock().await;
    let client = WhatsAppClient::new(cfg_for(&base));

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("incoming.bin");
    client
        .download_media("MEDIA999", dest.to_str().unwrap())
        .await
        .expect("download ok");
    let bytes = std::fs::read(&dest).unwrap();
    assert_eq!(bytes, b"FAKE-MEDIA-BYTES");
}

#[tokio::test]
async fn delete_is_noop_ok() {
    let (base, _state) = spawn_mock().await;
    let messenger = WhatsAppMessenger::from_config(cfg_for(&base));
    let handle = doracore::messaging::types::MessageHandle {
        platform: Platform::WhatsApp,
        chat_id: "49151".into(),
        message_id: "wamid.X".into(),
    };
    assert!(messenger.delete(&handle).await.is_ok());
}
