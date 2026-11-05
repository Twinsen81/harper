use harper_llm::LlmConfig;
use serde::{Deserialize, Serialize};

/// PROTOTYPE: settings of the local LLM check (see [`harper_llm`]).
///
/// The fields of [`LlmConfig`] are shared with `harper-ls`. The other fields are used only
/// by Harper Desktop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LlmSettings {
    #[serde(flatten)]
    pub check: LlmConfig,
    /// The LLM check starts only after the text has not changed for this time.
    /// Harper's own rules do not wait for it.
    pub idle_ms: u64,
}

impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            check: LlmConfig {
                // Chat messages (Slack, Messages) often have no final period.
                complete_sentences_only: false,
                ..LlmConfig::default()
            },
            idle_ms: 1500,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LlmSettings;

    #[test]
    fn serializes_flat_camel_case_keys() {
        let value = serde_json::to_value(LlmSettings::default()).unwrap();

        assert_eq!(value["enabled"], false);
        assert_eq!(value["idleMs"], 1500);
        assert_eq!(value["completeSentencesOnly"], false);
        assert!(value.get("check").is_none());
    }

    #[test]
    fn round_trips() {
        let mut settings = LlmSettings::default();
        settings.check.enabled = true;
        settings.check.model = "google/gemma-4-26b-a4b".to_owned();
        settings.idle_ms = 900;

        let serialized = serde_json::to_string(&settings).unwrap();

        assert_eq!(
            serde_json::from_str::<LlmSettings>(&serialized).unwrap(),
            settings
        );
    }
}
