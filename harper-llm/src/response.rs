//! Parses the model's answer.

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use serde_json::Value;

/// One changed sentence, as the model returns it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Fix {
    /// The 1-based sentence id from the user prompt.
    pub id: usize,
    /// The full corrected sentence (still masked).
    pub text: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub why: String,
}

/// Reads the fixes from a chat completions response body.
///
/// Models do not always answer with only one JSON object, even without a schema:
/// - The JSON can be inside a code fence or after a "thinking" block.
/// - Gemma 4 sometimes corrects itself in the answer ("Wait, I need to correct the error.")
///   and writes a second JSON object. The last object with a `fixes` list is the final answer.
/// - `{}` means that nothing needs fixing.
///
/// An empty answer is an error, not "no fixes": Margin found that "thinking" tokens can
/// use the whole output budget, and the empty answer then looked like a correct text.
pub fn parse_fixes(body: &str) -> Result<Vec<Fix>> {
    let response: Value = serde_json::from_str(body).context("The LLM response is not JSON.")?;
    let content = response["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| anyhow!("The LLM response has no message content."))?;

    if content.trim().is_empty() {
        bail!("The LLM answer is empty. Turn off \"thinking\" or increase the output limit.");
    }

    let objects = json_objects(content);

    if let Some(fixes) = objects.iter().rev().find_map(|object| object.get("fixes")) {
        return serde_json::from_value(fixes.clone())
            .with_context(|| format!("The LLM answer has an unexpected form: {content}"));
    }
    if objects
        .iter()
        .any(|object| object.as_object().is_some_and(|map| map.is_empty()))
    {
        return Ok(Vec::new());
    }

    bail!("The LLM answer has no JSON object with fixes: {content}")
}

/// Finds the top-level JSON objects in free text, in order.
fn json_objects(content: &str) -> Vec<Value> {
    let mut objects = Vec::new();
    let mut position = 0;

    while let Some(offset) = content[position..].find('{') {
        let start = position + offset;
        let mut stream = serde_json::Deserializer::from_str(&content[start..]).into_iter::<Value>();

        match stream.next() {
            Some(Ok(value)) => {
                position = start + stream.byte_offset();
                objects.push(value);
            }
            _ => position = start + 1,
        }
    }

    objects
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn completion(content: &str) -> String {
        json!({ "choices": [{ "message": { "role": "assistant", "content": content } }] })
            .to_string()
    }

    #[test]
    fn parses_fixes() {
        let fixes = parse_fixes(&completion(
            r#"{"fixes":[{"id":2,"text":"We were late.","kind":"grammar","why":"Agreement."}]}"#,
        ))
        .unwrap();

        assert_eq!(
            fixes,
            vec![Fix {
                id: 2,
                text: "We were late.".to_owned(),
                kind: "grammar".to_owned(),
                why: "Agreement.".to_owned(),
            }]
        );
    }

    #[test]
    fn parses_an_answer_inside_other_text() {
        let fixes = parse_fixes(&completion(
            "<think>ok</think>\n```json\n{\"fixes\":[{\"id\":1,\"text\":\"A.\"}]}\n```",
        ))
        .unwrap();

        assert_eq!(fixes[0].id, 1);
        assert_eq!(fixes[0].kind, "");
    }

    #[test]
    fn parses_no_fixes() {
        assert!(
            parse_fixes(&completion(r#"{"fixes":[]}"#))
                .unwrap()
                .is_empty()
        );
        assert!(
            parse_fixes(&completion("```json\n{}\n```"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn uses_the_last_answer_when_the_model_corrects_itself() {
        let content = "```json\n{\"fixes\":[{\"id\":1,\"text\":\"faster then\"}]}\n```\n\n\
                       Wait, I need to correct the error. Use {braces} carefully.\n\n\
                       ```json\n{\"fixes\":[{\"id\":1,\"text\":\"faster than\"}]}\n```";

        let fixes = parse_fixes(&completion(content)).unwrap();

        assert_eq!(fixes.len(), 1);
        assert_eq!(fixes[0].text, "faster than");
    }

    #[test]
    fn empty_or_broken_answers_are_errors() {
        assert!(parse_fixes(&completion("")).is_err());
        assert!(parse_fixes(&completion("All sentences are fine.")).is_err());
        assert!(parse_fixes("not json").is_err());
    }
}
