use super::{BotExtension, Capability, ExtensionCategory};

pub struct ConverterExtension;

impl BotExtension for ConverterExtension {
    fn id(&self) -> &str {
        "converter"
    }

    fn locale_key(&self) -> &str {
        "ext_converter"
    }

    fn icon(&self) -> &str {
        "\u{1F504}" // counterclockwise arrows
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![
            Capability {
                name: "Video Note".into(),
                description: "Video to circle".into(),
            },
            Capability {
                name: "GIF".into(),
                description: "Video to GIF".into(),
            },
            Capability {
                name: "MP3 Extract".into(),
                description: "Audio from video".into(),
            },
            Capability {
                name: "Compress".into(),
                description: "Video compression".into(),
            },
            Capability {
                name: "Documents".into(),
                description: "DOCX/ODT to PDF".into(),
            },
        ]
    }

    fn is_available(&self) -> bool {
        true
    }

    fn category(&self) -> ExtensionCategory {
        ExtensionCategory::Converter
    }
}
