use super::{BotExtension, Capability, ExtensionCategory};

pub struct YtDlpExtension;

impl BotExtension for YtDlpExtension {
    fn id(&self) -> &str {
        "ytdlp"
    }

    fn locale_key(&self) -> &str {
        "ext_ytdlp"
    }

    fn icon(&self) -> &str {
        "\u{1F310}" // globe
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![
            Capability {
                name: "YouTube".into(),
                description: "Video and audio".into(),
            },
            Capability {
                name: "TikTok".into(),
                description: "Video downloads".into(),
            },
            Capability {
                name: "Instagram".into(),
                description: "Reels, stories, posts".into(),
            },
            Capability {
                name: "SoundCloud".into(),
                description: "Audio tracks".into(),
            },
            Capability {
                name: "1000+ sites".into(),
                description: "Via yt-dlp".into(),
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
