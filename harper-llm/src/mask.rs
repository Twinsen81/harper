//! Protected spans are replaced with `⟦n⟧` before a sentence goes to the model,
//! and restored afterwards. The model cannot damage them, and it does not see
//! code or URLs that could confuse it.
//!
//! Ported from Margin's `mask.ts` (MIT, see `THIRD_PARTY_NOTICES.md`), with two changes:
//! - The Rust `regex` crate has no lookaround or backreferences, so the inline code,
//!   math and link patterns are written differently.
//! - Chat syntax (`@mention`, `#channel`, `:emoji:`) is also protected.

use std::sync::LazyLock;

use regex::Regex;

/// A sentence with protected spans replaced by placeholders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Masked {
    pub text: String,
    /// The original text of each placeholder, by index.
    pub originals: Vec<String>,
}

static PROTECTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        &[
            // Markdown link or image. Only `[` and `](target)` are masked: the link text stays editable.
            r#"(?P<open>!?\[)(?P<label>[^\]\[]*)(?P<close>\]\([^)\s]+(?:\s+"[^"]*")?\))"#,
            r"```[^`]*```|``[^`]*``|`[^`]+`",     // inline code
            r"\[\[[^\]]*\]\]",                    // wiki link
            r"\[\^[^\]]+\]",                      // footnote reference
            r"\$[^\s$](?:[^$\n]*[^\s$])?\$",      // inline math
            r"<https?://[^>\s]+>",                // autolink
            r"</?[a-zA-Z][\w-]*(?:\s[^<>]*)?/?>", // HTML tag
            r#"\bhttps?://[^\s<>()]*[^\s<>().,;:!?'"]"#, // bare URL, without trailing punctuation
            r"\b[\w.+-]+@[\w-]+(?:\.[\w-]+)+\b",  // email (before mentions, so it wins)
            r"@\w(?:[\w.-]*\w)?",                 // chat mention
            r"#\w(?:[\w-]*\w)?",                  // chat channel
            r":[a-z][a-z0-9_+-]*:",               // chat emoji code
        ]
        .join("|"),
    )
    .unwrap()
});

static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"⟦(\d+)⟧").unwrap());

/// Replaces protected spans with numbered placeholders.
pub fn mask(text: &str) -> Masked {
    let mut originals = Vec::new();
    let mut masked = String::with_capacity(text.len());
    let mut last = 0;

    let mut push_placeholder = |masked: &mut String, original: &str| {
        masked.push_str(&format!("⟦{}⟧", originals.len()));
        originals.push(original.to_owned());
    };

    for captures in PROTECTED.captures_iter(text) {
        let whole = captures.get(0).unwrap();
        masked.push_str(&text[last..whole.start()]);

        match (
            captures.name("open"),
            captures.name("label"),
            captures.name("close"),
        ) {
            (Some(open), Some(label), Some(close)) => {
                push_placeholder(&mut masked, open.as_str());
                masked.push_str(label.as_str());
                push_placeholder(&mut masked, close.as_str());
            }
            _ => push_placeholder(&mut masked, whole.as_str()),
        }

        last = whole.end();
    }

    masked.push_str(&text[last..]);

    Masked {
        text: masked,
        originals,
    }
}

/// Restores the placeholders. Every placeholder must appear exactly once and in its
/// original order. Otherwise the model damaged protected content, and this returns `None`.
pub fn unmask(text: &str, originals: &[String]) -> Option<String> {
    let seen: Vec<usize> = PLACEHOLDER
        .captures_iter(text)
        .map(|captures| captures[1].parse().unwrap_or(usize::MAX))
        .collect();

    if seen.len() != originals.len() || seen.iter().enumerate().any(|(i, n)| *n != i) {
        return None;
    }

    Some(
        PLACEHOLDER
            .replace_all(text, |captures: &regex::Captures| {
                originals[captures[1].parse::<usize>().unwrap()].clone()
            })
            .into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn masked_text(text: &str) -> String {
        mask(text).text
    }

    #[test]
    fn masks_code_urls_and_emails() {
        assert_eq!(
            masked_text("Run `pnpm test` and email ops@example.com or see https://x.dev/a."),
            "Run ⟦0⟧ and email ⟦1⟧ or see ⟦2⟧."
        );
    }

    #[test]
    fn keeps_link_text_editable() {
        let masked = mask("See [the guide](https://example.com/guide) when your ready.");

        assert_eq!(masked.text, "See ⟦0⟧the guide⟦1⟧ when your ready.");
        assert_eq!(masked.originals, vec!["[", "](https://example.com/guide)"]);
    }

    #[test]
    fn masks_chat_syntax() {
        assert_eq!(
            masked_text("@anna.k posted in #dev-team :tada: at 10:30."),
            "⟦0⟧ posted in ⟦1⟧ ⟦2⟧ at 10:30."
        );
    }

    #[test]
    fn does_not_mask_prices_or_plain_text() {
        assert_eq!(
            masked_text("It costs $5 and $10 in C#."),
            "It costs $5 and $10 in C#."
        );
    }

    #[test]
    fn masks_math_wiki_links_and_html() {
        assert_eq!(
            masked_text("The formula $E = mc^2$ is in [[Notes]] and <kbd>Ctrl</kbd>."),
            "The formula ⟦0⟧ is in ⟦1⟧ and ⟦2⟧Ctrl⟦3⟧."
        );
    }

    #[test]
    fn round_trip_restores_the_original() {
        let text = "Use the [settings page](https://example.com/a_b-c?x=1&y=2) to change `you're` password.";
        let masked = mask(text);

        assert_eq!(unmask(&masked.text, &masked.originals).unwrap(), text);
    }

    #[test]
    fn unmask_rejects_missing_repeated_or_reordered_placeholders() {
        let originals = vec!["a".to_owned(), "b".to_owned()];

        assert!(unmask("only ⟦0⟧", &originals).is_none());
        assert!(unmask("⟦0⟧ ⟦0⟧ ⟦1⟧", &originals).is_none());
        assert!(unmask("⟦1⟧ ⟦0⟧", &originals).is_none());
        assert_eq!(unmask("⟦0⟧ and ⟦1⟧", &originals).unwrap(), "a and b");
    }
}
