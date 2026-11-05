use serde::{Deserialize, Serialize};

/// User settings for the LLM check.
///
/// The serialized form uses camelCase keys, so it can come directly from an editor's
/// JSON settings (for example the `harper-ls.llm` object).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LlmConfig {
    /// The LLM check runs only when this is `true`.
    pub enabled: bool,
    /// The full URL of an OpenAI-compatible chat completions endpoint.
    pub endpoint: String,
    /// The model name. An empty string means "the model that the server has loaded".
    pub model: String,
    /// The timeout for one request, in milliseconds.
    pub timeout_ms: u64,
    /// Sentences with fewer words than this are not sent to the LLM.
    pub min_words: usize,
    /// The `reasoning_effort` request field. The default `"none"` turns off "thinking".
    /// In tests with LM Studio, thinking made requests 5 times slower (or caused timeouts)
    /// and did not improve the results. An empty string omits the field.
    pub reasoning_effort: String,
    /// The maximum number of sentences in one request. Margin's evaluation found 8 to be
    /// the best value: 15 lowered recall, 4 was slower and not better.
    pub batch_size: usize,
    /// The maximum number of characters of sentence text in one request.
    pub max_batch_chars: usize,
    /// When `true`, only sentences that end with `.`, `!` or `?` are checked.
    /// This skips the sentence that is being typed in an editor. Chat apps should set it
    /// to `false`, because chat messages often have no final period.
    pub complete_sentences_only: bool,
    /// A fix is rejected when it changes more than this share of the sentence's words
    /// (and more than one word).
    pub max_edit_ratio: f64,
    /// Sends a strict JSON schema (`response_format`) with each request.
    /// Off by default: with LM Studio and Gemma 4 26B-A4B, the schema lowered recall
    /// on Margin's fixtures from 71% to 16%, because the model often answered with an
    /// empty list. The answer parser accepts JSON inside other text without the schema.
    pub structured_output: bool,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: "http://localhost:1234/v1/chat/completions".to_owned(),
            model: String::new(),
            timeout_ms: 60_000,
            min_words: 3,
            reasoning_effort: "none".to_owned(),
            batch_size: 8,
            max_batch_chars: 2400,
            complete_sentences_only: true,
            max_edit_ratio: 0.35,
            structured_output: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LlmConfig;

    #[test]
    fn partial_json_keeps_defaults() {
        let config: LlmConfig =
            serde_json::from_str(r#"{ "enabled": true, "model": "google/gemma-4-26b-a4b" }"#)
                .unwrap();

        assert!(config.enabled);
        assert_eq!(config.model, "google/gemma-4-26b-a4b");
        assert_eq!(config.batch_size, 8);
        assert_eq!(config.reasoning_effort, "none");
    }

    #[test]
    fn disabled_by_default() {
        assert!(!LlmConfig::default().enabled);
    }
}
