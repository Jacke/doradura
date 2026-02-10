use super::{BotExtension, Capability, ExtensionCategory};

pub struct AudioEffectsExtension;

impl BotExtension for AudioEffectsExtension {
    fn id(&self) -> &str {
        "audio_effects"
    }

    fn locale_key(&self) -> &str {
        "ext_audio_effects"
    }

    fn icon(&self) -> &str {
        "\u{1F39B}\u{FE0F}" // control knobs
    }

    fn capabilities(&self) -> Vec<Capability> {
        vec![
            Capability {
                name: "Pitch".into(),
                description: "Change pitch".into(),
            },
            Capability {
                name: "Tempo".into(),
                description: "Change speed".into(),
            },
            Capability {
                name: "Bass Boost".into(),
                description: "Enhance bass".into(),
            },
            Capability {
                name: "Morph".into(),
                description: "Voice presets".into(),
            },
        ]
    }

    fn is_available(&self) -> bool {
        true
    }

    fn category(&self) -> ExtensionCategory {
        ExtensionCategory::AudioProcessor
    }
}
