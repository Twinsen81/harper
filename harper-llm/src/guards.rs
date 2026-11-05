//! Guards reject fixes that are probably harmful, even when the model is sure.
//!
//! Ported from Margin's `guards.ts` (MIT, see `THIRD_PARTY_NOTICES.md`).

use std::sync::LazyLock;

use regex::Regex;

use crate::diff::Hunk;

/// Why a fix was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Rejection {
    /// The model removed, repeated or reordered a protected-span placeholder.
    Placeholders,
    /// The model's own reason says that nothing was wrong.
    SaysNoError,
    /// The number of negation words changed.
    Negation,
    /// The numbers changed.
    Numbers,
    /// The quoted text changed.
    Quotes,
    /// The number of Markdown characters changed.
    Markdown,
    /// The fix only adds or removes a serial comma, a style choice that Harper owns.
    HarperStyle,
    /// The fix changes too many words.
    EditRatio,
}

static NEGATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:not|no|never|none|nobody|nothing|nowhere|neither|nor|cannot)\b|n['’]t\b")
        .unwrap()
});
static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+(?:[.,:/]\d+)*").unwrap());
static QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""[^"\n]*"|“[^”\n]*”"#).unwrap());
// Markup characters that a grammar fix never needs to add or remove.
static MARKDOWN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[*_~`\[\]<>|#]").unwrap());
static WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\p{L}\p{N}]+(?:['’]\p{L}+)*").unwrap());
static SAYS_NO_ERROR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\bno (?:changes?|corrections?|errors?|fix(?:es)?) (?:are |is )?(?:needed|required|necessary|found)\b|\b(?:sentence|it|this) is (?:already )?(?:grammatically )?correct\b",
    )
    .unwrap()
});
static SERIAL_COMMA_CONTEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^,?\s+(?:and|or)\b").unwrap());

fn all<'a>(regex: &Regex, text: &'a str) -> Vec<&'a str> {
    regex.find_iter(text).map(|m| m.as_str()).collect()
}

fn same_multiset(mut a: Vec<&str>, mut b: Vec<&str>) -> bool {
    a.sort_unstable();
    b.sort_unstable();
    a == b
}

/// The number of words in `text`.
pub fn count_words(text: &str) -> usize {
    WORD.find_iter(text).count()
}

/// Checks a fix. `hunks` are relative to `original` (in `char`s).
/// Returns why the fix is rejected, or `None` if the fix may be shown.
pub fn guard_fix(
    original: &str,
    fixed: &str,
    hunks: &[Hunk],
    max_edit_ratio: f64,
) -> Option<Rejection> {
    if all(&NEGATION, original).len() != all(&NEGATION, fixed).len() {
        return Some(Rejection::Negation);
    }
    if !same_multiset(all(&NUMBER, original), all(&NUMBER, fixed)) {
        return Some(Rejection::Numbers);
    }
    if all(&QUOTED, original) != all(&QUOTED, fixed) {
        return Some(Rejection::Quotes);
    }
    if all(&MARKDOWN, original).len() != all(&MARKDOWN, fixed).len() {
        return Some(Rejection::Markdown);
    }

    let chars: Vec<char> = original.chars().collect();
    let slice = |start: usize, end: usize| chars[start..end].iter().collect::<String>();

    if !hunks.is_empty() && hunks.iter().all(|hunk| is_serial_comma(&chars, hunk)) {
        return Some(Rejection::HarperStyle);
    }

    let changed: usize = hunks
        .iter()
        .map(|hunk| count_words(&slice(hunk.start, hunk.end)).max(count_words(&hunk.replacement)))
        .sum();
    let total = count_words(original).max(1);

    if changed > 1 && changed as f64 / total as f64 > max_edit_ratio {
        return Some(Rejection::EditRatio);
    }

    None
}

/// True when the model's reason says that nothing was wrong ("No changes needed."),
/// although it changed the sentence. Such a change is a polish, not a fix.
pub fn says_no_error(why: &str) -> bool {
    SAYS_NO_ERROR.is_match(why)
}

/// A hunk that only adds or removes the comma before "and" or "or".
fn is_serial_comma(original: &[char], hunk: &Hunk) -> bool {
    let removed: String = original[hunk.start..hunk.end].iter().collect();
    let removed = removed.trim();
    let added = hunk.replacement.trim();
    let comma_only = (removed.is_empty() && added == ",") || (removed == "," && added.is_empty());
    let after: String = original[hunk.end..].iter().collect();

    comma_only && SERIAL_COMMA_CONTEXT.is_match(&after)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::to_hunks;

    fn check(original: &str, fixed: &str) -> Option<Rejection> {
        guard_fix(original, fixed, &to_hunks(original, fixed), 0.35)
    }

    #[test]
    fn accepts_a_normal_fix() {
        assert_eq!(
            check("She have three children.", "She has three children."),
            None
        );
    }

    #[test]
    fn rejects_a_negation_change() {
        assert_eq!(
            check("I don't like the new design.", "I like the new design."),
            Some(Rejection::Negation)
        );
    }

    #[test]
    fn accepts_a_contraction_that_keeps_the_negation() {
        assert_eq!(
            check("He don't like coffee.", "He doesn't like coffee."),
            None
        );
    }

    #[test]
    fn rejects_changed_numbers() {
        assert_eq!(
            check("We was there 3 times.", "We were there 4 times."),
            Some(Rejection::Numbers)
        );
    }

    #[test]
    fn rejects_changed_quotes() {
        assert_eq!(
            check(
                r#"He said "we was late" again."#,
                r#"He said "we were late" again."#
            ),
            Some(Rejection::Quotes)
        );
    }

    #[test]
    fn rejects_leaked_markdown() {
        assert_eq!(
            check("This are important.", "This is **important**."),
            Some(Rejection::Markdown)
        );
    }

    #[test]
    fn rejects_serial_comma_only() {
        assert_eq!(
            check(
                "We need eggs, milk and bread.",
                "We need eggs, milk, and bread."
            ),
            Some(Rejection::HarperStyle)
        );
    }

    #[test]
    fn rejects_a_rewrite() {
        assert_eq!(
            check(
                "The meeting was long and boring.",
                "The lengthy meeting bored everyone."
            ),
            Some(Rejection::EditRatio)
        );
    }

    #[test]
    fn counts_words_like_margin() {
        assert_eq!(count_words("I don't know, it's 2026!"), 5);
    }

    #[test]
    fn detects_no_error_reasons() {
        assert!(says_no_error("No changes needed."));
        assert!(says_no_error("The sentence is already correct."));
        assert!(!says_no_error("Plural subject takes a plural verb."));
    }
}
