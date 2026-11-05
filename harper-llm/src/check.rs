//! The check pipeline without I/O: prepare a request for a batch of sentences,
//! then turn the answer into one [`Outcome`] per sentence.

use std::collections::HashSet;

use anyhow::Result;
use harper_core::Dialect;
use serde_json::Value;

use crate::LlmConfig;
use crate::diff::{anchor_insertion, to_hunks};
use crate::guards::{Rejection, guard_fix, says_no_error};
use crate::mask::{Masked, mask, unmask};
use crate::prompt::request_body;
use crate::response::{Fix, parse_fixes};

/// One suggested change inside a sentence. Offsets are `char`s, relative to the sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentenceFix {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
    /// "grammar", "word", "punctuation" or "clarity".
    pub kind: String,
    /// The model's short reason.
    pub why: String,
}

/// The result of the check for one sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The model did not change the sentence.
    Correct,
    /// The model flagged the sentence but returned it unchanged (or changed only whitespace).
    /// The caller should ask once more for this sentence alone, with
    /// [`crate::prompt::ECHO_NOTE`], and treat a second echo as [`Outcome::Correct`].
    Echo,
    /// The model changed the sentence and the change passed all guards.
    Fixed(Vec<SentenceFix>),
    /// The model changed the sentence, but a guard rejected the change.
    Rejected(Rejection),
}

/// A request for one batch, and what is needed to read its answer.
#[derive(Debug, Clone)]
pub struct Prepared {
    pub sentences: Vec<String>,
    pub masked: Vec<Masked>,
    pub body: Value,
}

/// Masks the sentences and builds the request body.
/// `note` is added after the sentences (see [`crate::prompt::ECHO_NOTE`]).
pub fn prepare(
    config: &LlmConfig,
    dialect: Dialect,
    sentences: Vec<String>,
    note: Option<&str>,
) -> Prepared {
    let masked: Vec<Masked> = sentences.iter().map(|sentence| mask(sentence)).collect();
    let masked_texts: Vec<String> = masked.iter().map(|m| m.text.clone()).collect();
    let body = request_body(config, dialect, &masked_texts, note);

    Prepared {
        sentences,
        masked,
        body,
    }
}

/// Reads a chat completions response body and returns one outcome per sentence,
/// in the order of [`Prepared::sentences`].
///
/// Unknown ids are ignored. If the model returns the same id twice, the first fix wins.
pub fn interpret(
    prepared: &Prepared,
    config: &LlmConfig,
    response_body: &str,
) -> Result<Vec<Outcome>> {
    let fixes = parse_fixes(response_body)?;
    let mut outcomes = vec![Outcome::Correct; prepared.sentences.len()];
    let mut seen = HashSet::new();

    for fix in fixes {
        let Some(index) = fix.id.checked_sub(1).filter(|&i| i < outcomes.len()) else {
            continue;
        };
        if !seen.insert(index) {
            continue;
        }

        outcomes[index] = evaluate(
            &prepared.sentences[index],
            &prepared.masked[index],
            &fix,
            config.max_edit_ratio,
        );
    }

    Ok(outcomes)
}

/// Checks one fix against its original sentence.
pub fn evaluate(original: &str, masked: &Masked, fix: &Fix, max_edit_ratio: f64) -> Outcome {
    let Some(fixed) = unmask(&fix.text, &masked.originals) else {
        return Outcome::Rejected(Rejection::Placeholders);
    };

    let hunks = to_hunks(original, &fixed);
    if hunks.is_empty() {
        return if says_no_error(&fix.why) {
            Outcome::Correct
        } else {
            Outcome::Echo
        };
    }

    if says_no_error(&fix.why) {
        return Outcome::Rejected(Rejection::SaysNoError);
    }
    if let Some(rejection) = guard_fix(original, &fixed, &hunks, max_edit_ratio) {
        return Outcome::Rejected(rejection);
    }

    let fixes: Vec<SentenceFix> = hunks
        .into_iter()
        .map(|hunk| anchor_insertion(original, hunk))
        .filter(|hunk| hunk.start < hunk.end)
        .map(|hunk| SentenceFix {
            start: hunk.start,
            end: hunk.end,
            replacement: hunk.replacement,
            kind: fix.kind.clone(),
            why: fix.why.trim().to_owned(),
        })
        .collect();

    if fixes.is_empty() {
        Outcome::Correct
    } else {
        Outcome::Fixed(fixes)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::diff::{Hunk, apply_hunks};

    fn completion(fixes: Value) -> String {
        json!({ "choices": [{ "message": { "content": json!({ "fixes": fixes }).to_string() } }] })
            .to_string()
    }

    fn apply(sentence: &str, outcome: &Outcome) -> String {
        match outcome {
            Outcome::Fixed(fixes) => apply_hunks(
                sentence,
                &fixes
                    .iter()
                    .map(|f| Hunk {
                        start: f.start,
                        end: f.end,
                        replacement: f.replacement.clone(),
                    })
                    .collect::<Vec<_>>(),
            ),
            _ => sentence.to_owned(),
        }
    }

    #[test]
    fn prepare_masks_protected_spans() {
        let prepared = prepare(
            &LlmConfig::default(),
            Dialect::American,
            vec!["Run `cargo test` before you push.".to_owned()],
            None,
        );

        let user = prepared.body["messages"][1]["content"].as_str().unwrap();
        assert_eq!(user, "<s id=\"1\">Run ⟦0⟧ before you push.</s>");
    }

    #[test]
    fn interpret_maps_fixes_to_sentences() {
        let config = LlmConfig::default();
        let prepared = prepare(
            &config,
            Dialect::American,
            vec![
                "The site are down, see `status.md`.".to_owned(),
                "This one is fine.".to_owned(),
                "Yesterday I go home.".to_owned(),
            ],
            None,
        );
        let body = completion(json!([
            { "id": 3, "text": "Yesterday I went home.", "kind": "grammar", "why": "Past tense." },
            { "id": 1, "text": "The site is down, see ⟦0⟧.", "kind": "grammar", "why": "Agreement." },
            { "id": 1, "text": "Ignored second fix.", "kind": "grammar", "why": "" },
            { "id": 9, "text": "Unknown id.", "kind": "grammar", "why": "" },
        ]));

        let outcomes = interpret(&prepared, &config, &body).unwrap();

        assert_eq!(
            apply(&prepared.sentences[0], &outcomes[0]),
            "The site is down, see `status.md`."
        );
        assert_eq!(outcomes[1], Outcome::Correct);
        assert_eq!(
            apply(&prepared.sentences[2], &outcomes[2]),
            "Yesterday I went home."
        );
        assert_eq!(
            outcomes[2],
            Outcome::Fixed(vec![SentenceFix {
                start: 12,
                end: 14,
                replacement: "went".to_owned(),
                kind: "grammar".to_owned(),
                why: "Past tense.".to_owned(),
            }])
        );
    }

    #[test]
    fn damaged_placeholders_are_rejected() {
        let config = LlmConfig::default();
        let prepared = prepare(
            &config,
            Dialect::American,
            vec!["See https://x.dev now, it are good.".to_owned()],
            None,
        );
        let body = completion(
            json!([{ "id": 1, "text": "See the site now, it is good.", "kind": "grammar", "why": "" }]),
        );

        assert_eq!(
            interpret(&prepared, &config, &body).unwrap(),
            vec![Outcome::Rejected(Rejection::Placeholders)]
        );
    }

    #[test]
    fn echo_and_no_error_reasons() {
        let config = LlmConfig::default();
        let prepared = prepare(
            &config,
            Dialect::American,
            vec!["It is fine.".to_owned(), "It was good enough.".to_owned()],
            None,
        );
        let body = completion(json!([
            { "id": 1, "text": "It is  fine.", "kind": "grammar", "why": "Fixed." },
            { "id": 2, "text": "It was quite good.", "kind": "clarity", "why": "No changes needed." },
        ]));

        assert_eq!(
            interpret(&prepared, &config, &body).unwrap(),
            vec![Outcome::Echo, Outcome::Rejected(Rejection::SaysNoError)]
        );
    }

    #[test]
    fn unchanged_sentence_with_a_no_error_reason_is_correct() {
        let config = LlmConfig::default();
        let prepared = prepare(
            &config,
            Dialect::American,
            vec!["It is fine.".to_owned()],
            None,
        );
        let body = completion(
            json!([{ "id": 1, "text": "It is fine.", "kind": "grammar", "why": "The sentence is correct." }]),
        );

        assert_eq!(
            interpret(&prepared, &config, &body).unwrap(),
            vec![Outcome::Correct]
        );
    }
}
