use super::{BotExtension, Capability, ExtensionCategory};

pub struct HttpExtension;

impl BotExtension for HttpExtension {
    fn id(&self) -> &str {
        "http"
    }

    fn locale_key(&self) -> &str {
        "ext_http"
    }

    fn icon(&self) -> &str {
        "\u{1F4E5}" // inbox tray
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![
            Capability {
                name: "MP3/MP4".into(),
                description: "Direct file URLs".into(),
            },
            Capability {
                name: "WAV/FLAC".into(),
                description: "Lossless audio".into(),
            },
            Capability {
                name: "Resume".into(),
                description: "Chunked download with resume".into(),
            },
        ]
    }

    fn is_available(&self) -> bool {
        true
    }

    fn category(&self) -> ExtensionCategory {
        ExtensionCategory::Downloader
    }
}
