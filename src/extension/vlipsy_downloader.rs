use super::{BotExtension, Capability, ExtensionCategory};

pub struct VlipsyExtension;

impl BotExtension for VlipsyExtension {
    fn id(&self) -> &str {
        "vlipsy"
    }

    fn locale_key(&self) -> &str {
        "ext_vlipsy"
    }

    fn icon(&self) -> &str {
        "\u{1F3AC}" // 🎬
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![
            Capability {
                name: "Search".into(),
                description: "Find video reactions by keyword".into(),
            },
            Capability {
                name: "Trending".into(),
                description: "Browse trending clips".into(),
            },
            Capability {
                name: "Reactions".into(),
                description: "Movie & TV show reactions".into(),
            },
            Capability {
                name: "Download".into(),
                description: "Download clips as MP4".into(),
            },
        ]
    }

    fn is_available(&self) -> bool {
        std::env::var("VLIPSY_API_KEY").ok().filter(|s| !s.is_empty()).is_some()
    }

    fn category(&self) -> ExtensionCategory {
        ExtensionCategory::Downloader
    }
}
