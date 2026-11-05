//! The check prompt and the request body.
//!
//! The rules and the example are Margin's check prompt, version 3, JSON format
//! (`packages/core/src/prompts/check.ts`, MIT, see `THIRD_PARTY_NOTICES.md`).
//!
//! One change: Margin tells the model not to fix spelling. With that rule, Gemma 4 often
//! did not see the tense error in "Ystrday we go to the office", because it did not read
//! "Ystrday" as "Yesterday". Now the model also fixes misspelled words. Its spelling fixes
//! are not shown twice: integrations drop LLM lints that overlap Harper's lints.

use harper_core::Dialect;
use serde_json::{Value, json};

use crate::LlmConfig;

/// Change this on any change to the prompt bytes or to the request parameters.
/// It is part of every cache key, so old results become invalid.
pub const PROMPT_VERSION: u32 = 3;

fn dialect_name(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::American => "American",
        Dialect::British => "British",
        Dialect::Australian => "Australian",
        Dialect::Canadian => "Canadian",
        Dialect::Indian => "Indian",
    }
}

/// The system prompt.
pub fn system_prompt(dialect: Dialect) -> String {
    let dialect = dialect_name(dialect);

    format!(
        r#"You are a copy editor doing a minimal-edit proofread of {dialect} English.

You receive sentences as <s id="N">…</s>. Some may be correct.
Fix only clear errors:
- misspelled words
- grammar and agreement (subject–verb, pronoun, tense consistency)
- a wrong word that is still a real word (their/there, affect/effect)
- missing, repeated or extra words
- run-on sentences and comma splices
- punctuation that changes or hides the meaning
Do not:
- change a sentence that is already correct
- change word choice, tone, style or dialect
- touch placeholders like ⟦0⟧; keep each one exactly once, in order
- add or remove negation, numbers, names or facts
- change product names, proper nouns or technical terms
- fix capitalization
- add optional punctuation: commas after introductory words or phrases, commas around "however" or "on the other hand", hyphens, or dashes in place of hyphens
- treat headings, list items, labels or short fragments as errors (don't add a final period or complete them)
Return only the sentences you changed, each as the full corrected sentence, with a kind and a reason of at most 12 words. If nothing needs fixing, return an empty list.

Answer with JSON only: {{"fixes":[{{"id":N,"text":"…","kind":"grammar|word|punctuation|clarity","why":"…"}}]}}

Example input:
<s id="1">The dogs barks at night.</s>
<s id="2">Open ⟦0⟧ to start.</s>
<s id="3">We was ready, the bus was late.</s>
Example output:
{{"fixes":[{{"id":1,"text":"The dogs bark at night.","kind":"grammar","why":"Plural subject takes a plural verb."}},{{"id":3,"text":"We were ready, but the bus was late.","kind":"grammar","why":"Subject–verb agreement and a comma splice."}}]}}"#
    )
}

/// The user prompt: one numbered sentence per line, with whitespace collapsed.
/// Ids start at 1.
pub fn user_prompt(masked_sentences: &[String]) -> String {
    masked_sentences
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
            format!("<s id=\"{}\">{collapsed}</s>", i + 1)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The JSON schema of the answer.
fn answer_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "fixes": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "integer" },
                        "text": { "type": "string" },
                        "kind": { "type": "string", "enum": ["grammar", "word", "punctuation", "clarity"] },
                        "why": { "type": "string" },
                    },
                    "required": ["id", "text", "kind", "why"],
                    "additionalProperties": false,
                },
            },
        },
        "required": ["fixes"],
        "additionalProperties": false,
    })
}

/// Added to the user prompt when a sentence is sent again, alone, because the model
/// flagged it but returned it unchanged. Margin's text.
pub const ECHO_NOTE: &str = "Note: you flagged this sentence as wrong earlier but returned it unchanged. If it has an error, return the corrected sentence with the error actually fixed. If it is correct, return no fixes.";

/// The body of a chat completions request for a batch of masked sentences.
/// `note` is added after the sentences (see [`ECHO_NOTE`]).
pub fn request_body(
    config: &LlmConfig,
    dialect: Dialect,
    masked_sentences: &[String],
    note: Option<&str>,
) -> Value {
    let sentences = user_prompt(masked_sentences);
    let user = match note {
        Some(note) => format!("{sentences}\n\n{note}"),
        None => sentences,
    };

    // Enough for every sentence to come back changed, with a reason and JSON overhead.
    // Margin's smaller budget (characters / 4 * 1.4 + 150) cut off answers for 8 sentences.
    let max_tokens = 512.max(user.chars().count() / 3 + 64 * masked_sentences.len() + 150);

    let mut body = json!({
        "temperature": 0,
        "stream": false,
        "max_tokens": max_tokens,
        "messages": [
            { "role": "system", "content": system_prompt(dialect) },
            { "role": "user", "content": user },
        ],
    });

    if config.structured_output {
        body["response_format"] = json!({
            "type": "json_schema",
            "json_schema": { "name": "fixes", "strict": true, "schema": answer_schema() },
        });
    }
    if !config.model.is_empty() {
        body["model"] = Value::String(config.model.clone());
    }
    if !config.reasoning_effort.is_empty() {
        body["reasoning_effort"] = Value::String(config.reasoning_effort.clone());
    }

    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_prompt_numbers_sentences_and_collapses_whitespace() {
        assert_eq!(
            user_prompt(&["One  two.".to_owned(), "Three\nfour.".to_owned()]),
            "<s id=\"1\">One two.</s>\n<s id=\"2\">Three four.</s>"
        );
    }

    #[test]
    fn system_prompt_names_the_dialect_and_has_the_example() {
        let prompt = system_prompt(Dialect::British);

        assert!(prompt.starts_with(
            "You are a copy editor doing a minimal-edit proofread of British English."
        ));
        assert!(prompt.contains(r#"{"fixes":[{"id":1,"text":"The dogs bark at night.""#));
    }

    #[test]
    fn request_body_sets_optional_fields() {
        let mut config = LlmConfig::default();
        let body = request_body(&config, Dialect::American, &["A b c.".to_owned()], None);
        assert!(body.get("model").is_none());
        assert!(body.get("response_format").is_none());
        assert_eq!(body["reasoning_effort"], "none");
        assert_eq!(body["temperature"], 0);
        assert_eq!(body["max_tokens"], 512);

        config.model = "google/gemma-4-26b-a4b".to_owned();
        config.reasoning_effort = String::new();
        config.structured_output = true;
        let body = request_body(&config, Dialect::American, &["A b c.".to_owned()], None);
        assert_eq!(body["model"], "google/gemma-4-26b-a4b");
        assert!(body.get("reasoning_effort").is_none());
        assert_eq!(body["response_format"]["type"], "json_schema");
    }

    #[test]
    fn note_follows_the_sentences() {
        let body = request_body(
            &LlmConfig::default(),
            Dialect::American,
            &["A b c.".to_owned()],
            Some(ECHO_NOTE),
        );

        assert_eq!(
            body["messages"][1]["content"],
            format!("<s id=\"1\">A b c.</s>\n\n{ECHO_NOTE}")
        );
    }
}
