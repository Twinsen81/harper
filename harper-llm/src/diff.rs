//! A word-level diff between a sentence and its corrected version.
//!
//! The model returns the full corrected sentence. The diff finds the exact changed
//! ranges, so the model never has to give character positions, and a repeated word
//! cannot move the underline to the wrong place.
//!
//! The hunk grouping is ported from Margin's `diff.ts` (MIT, see `THIRD_PARTY_NOTICES.md`).
//! All offsets are in `char`s, the same unit as Harper's `Span<char>`.

/// One change: replace `start..end` of the original with `replacement`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub start: usize,
    pub end: usize,
    pub replacement: String,
}

#[derive(Debug, PartialEq, Eq)]
enum Op {
    Equal(String),
    Delete(String),
    Insert(String),
}

/// Above this number of cells, the middle part is treated as one replacement.
const MAX_LCS_CELLS: usize = 250_000;

/// Splits text into word, whitespace and punctuation tokens.
/// An apostrophe between two letters is part of the word ("don't", "you’re").
fn tokenize(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let is_inner_apostrophe = |i: usize| {
        matches!(chars[i], '\'' | '’')
            && i > 0
            && i + 1 < chars.len()
            && is_word(chars[i - 1])
            && is_word(chars[i + 1])
    };

    let mut tokens: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let start = i;
        if is_word(chars[i]) {
            while i < chars.len() && (is_word(chars[i]) || is_inner_apostrophe(i)) {
                i += 1;
            }
        } else if chars[i].is_whitespace() {
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
        } else {
            i += 1;
        }
        tokens.push(chars[start..i].iter().collect());
    }
    tokens
}

/// A token-level diff. Deletions come before insertions inside each changed block.
fn diff_tokens(old: &[String], new: &[String]) -> Vec<Op> {
    let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();

    let old_mid = &old[prefix..old.len() - suffix];
    let new_mid = &new[prefix..new.len() - suffix];

    let mut ops: Vec<Op> = old[..prefix].iter().cloned().map(Op::Equal).collect();
    ops.extend(diff_middle(old_mid, new_mid));
    ops.extend(old[old.len() - suffix..].iter().cloned().map(Op::Equal));
    ops
}

fn diff_middle(old: &[String], new: &[String]) -> Vec<Op> {
    if (old.len() + 1) * (new.len() + 1) > MAX_LCS_CELLS {
        return old
            .iter()
            .cloned()
            .map(Op::Delete)
            .chain(new.iter().cloned().map(Op::Insert))
            .collect();
    }

    // lcs[i][j] = the LCS length of old[i..] and new[j..].
    let mut lcs = vec![vec![0usize; new.len() + 1]; old.len() + 1];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            lcs[i][j] = if old[i] == new[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut ops = Vec::new();
    let mut deleted = Vec::new();
    let mut inserted = Vec::new();
    let flush = |ops: &mut Vec<Op>, deleted: &mut Vec<String>, inserted: &mut Vec<String>| {
        ops.extend(deleted.drain(..).map(Op::Delete));
        ops.extend(inserted.drain(..).map(Op::Insert));
    };

    let (mut i, mut j) = (0, 0);
    while i < old.len() || j < new.len() {
        if i < old.len() && j < new.len() && old[i] == new[j] {
            flush(&mut ops, &mut deleted, &mut inserted);
            ops.push(Op::Equal(old[i].clone()));
            i += 1;
            j += 1;
        } else if j == new.len() || (i < old.len() && lcs[i + 1][j] >= lcs[i][j + 1]) {
            deleted.push(old[i].clone());
            i += 1;
        } else {
            inserted.push(new[j].clone());
            j += 1;
        }
    }
    flush(&mut ops, &mut deleted, &mut inserted);
    ops
}

/// Word-diffs a sentence against its corrected version.
///
/// Changes separated only by whitespace are grouped into one hunk.
/// Whitespace-only changes are dropped: a model that normalizes spaces does not fix anything.
pub fn to_hunks(original: &str, fixed: &str) -> Vec<Hunk> {
    let original_chars: Vec<char> = original.chars().collect();
    let ops = diff_tokens(&tokenize(original), &tokenize(fixed));

    let mut hunks = Vec::new();
    let mut pos = 0;
    let mut open: Option<Hunk> = None;
    let mut gap = String::new();

    let close = |open: &mut Option<Hunk>, gap: &mut String, hunks: &mut Vec<Hunk>| {
        if let Some(hunk) = open.take() {
            let hunk = shrink_shared_whitespace(&original_chars, hunk);
            let removed: String = original_chars[hunk.start..hunk.end].iter().collect();
            if !removed.trim().is_empty() || !hunk.replacement.trim().is_empty() {
                hunks.push(hunk);
            }
        }
        gap.clear();
    };

    for op in ops {
        match op {
            Op::Equal(value) => {
                if open.is_some() && value.trim().is_empty() {
                    gap.push_str(&value);
                } else {
                    close(&mut open, &mut gap, &mut hunks);
                }
                pos += value.chars().count();
            }
            change => {
                if let Some(hunk) = open.as_mut().filter(|_| !gap.is_empty()) {
                    // Bridge a whitespace-only gap, so "was  late" -> "were late" stays one hunk.
                    hunk.end = pos;
                    hunk.replacement.push_str(&gap);
                    gap.clear();
                }
                let hunk = open.get_or_insert(Hunk {
                    start: pos,
                    end: pos,
                    replacement: String::new(),
                });
                match change {
                    Op::Delete(value) => {
                        pos += value.chars().count();
                        hunk.end = pos;
                    }
                    Op::Insert(value) => hunk.replacement.push_str(&value),
                    Op::Equal(_) => unreachable!(),
                }
            }
        }
    }
    close(&mut open, &mut gap, &mut hunks);

    hunks
}

/// Removes whitespace that both sides of a hunk share at the start or at the end,
/// so a hunk never rewrites whitespace that it does not need to.
fn shrink_shared_whitespace(original: &[char], mut hunk: Hunk) -> Hunk {
    loop {
        let first = hunk.replacement.chars().next();
        match first {
            Some(c) if hunk.start < hunk.end && c.is_whitespace() && original[hunk.start] == c => {
                hunk.start += 1;
                hunk.replacement.remove(0);
            }
            _ => break,
        }
    }
    loop {
        let last = hunk.replacement.chars().last();
        match last {
            Some(c)
                if hunk.start < hunk.end && c.is_whitespace() && original[hunk.end - 1] == c =>
            {
                hunk.end -= 1;
                hunk.replacement.pop();
            }
            _ => break,
        }
    }
    hunk
}

/// Harper cannot show a zero-width lint, so a pure insertion is attached to a word:
/// to the word that starts at the insertion point if there is one
/// ("need book" -> underline "book", "a book"), otherwise to the word before it.
/// If there is no word on either side (an insertion between two punctuation marks),
/// the hunk keeps its zero width.
pub fn anchor_insertion(original: &str, hunk: Hunk) -> Hunk {
    if hunk.start != hunk.end {
        return hunk;
    }

    let chars: Vec<char> = original.chars().collect();
    let is_word = |c: char| c.is_alphanumeric() || matches!(c, '_' | '\'' | '’');
    let next_end = (hunk.end..chars.len())
        .find(|&i| !is_word(chars[i]))
        .unwrap_or(chars.len());

    if next_end > hunk.end {
        let next: String = chars[hunk.end..next_end].iter().collect();
        return Hunk {
            start: hunk.start,
            end: next_end,
            replacement: hunk.replacement + &next,
        };
    }

    let previous_start = (0..hunk.start)
        .rev()
        .find(|&i| !is_word(chars[i]))
        .map_or(0, |i| i + 1);
    let previous: String = chars[previous_start..hunk.start].iter().collect();

    Hunk {
        start: previous_start,
        end: hunk.end,
        replacement: previous + &hunk.replacement,
    }
}

/// Applies hunks (with offsets into `text`) to `text`.
pub fn apply_hunks(text: &str, hunks: &[Hunk]) -> String {
    let mut chars: Vec<char> = text.chars().collect();
    let mut sorted: Vec<&Hunk> = hunks.iter().collect();
    sorted.sort_by_key(|hunk| std::cmp::Reverse(hunk.start));

    for hunk in sorted {
        chars.splice(hunk.start..hunk.end, hunk.replacement.chars());
    }
    chars.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunk(start: usize, end: usize, replacement: &str) -> Hunk {
        Hunk {
            start,
            end,
            replacement: replacement.to_owned(),
        }
    }

    fn round_trip(original: &str, fixed: &str) {
        assert_eq!(apply_hunks(original, &to_hunks(original, fixed)), fixed);
    }

    #[test]
    fn single_word_change() {
        assert_eq!(
            to_hunks("We was late.", "We were late."),
            vec![hunk(3, 6, "were")]
        );
    }

    #[test]
    fn whitespace_gap_is_bridged() {
        assert_eq!(
            to_hunks("I has went home.", "I have gone home."),
            vec![hunk(2, 10, "have gone")]
        );
    }

    #[test]
    fn repeated_word_marks_the_right_one() {
        // The first "go" is correct. Only the second one changes.
        assert_eq!(
            to_hunks(
                "I go to work and yesterday I go home late.",
                "I go to work and yesterday I went home late."
            ),
            vec![hunk(29, 31, "went")]
        );
    }

    #[test]
    fn whitespace_only_changes_are_dropped() {
        assert!(to_hunks("We  were late.", "We were late.").is_empty());
        assert!(to_hunks("Same text.", "Same text.").is_empty());
    }

    #[test]
    fn insertion_and_deletion() {
        assert_eq!(
            to_hunks("I need book.", "I need a book."),
            vec![hunk(7, 7, "a ")]
        );
        // Both "is " are equal, so removing the second one is as correct as removing the first.
        assert_eq!(
            to_hunks("It is is fine.", "It is fine."),
            vec![hunk(6, 9, "")]
        );
    }

    #[test]
    fn contraction_is_one_token() {
        assert_eq!(
            to_hunks("He don't like it.", "He doesn't like it."),
            vec![hunk(3, 8, "doesn't")]
        );
    }

    #[test]
    fn offsets_are_chars_after_emoji() {
        assert_eq!(
            to_hunks("😀 The grammar do.", "😀 The grammar does."),
            vec![hunk(14, 16, "does")]
        );
    }

    #[test]
    fn round_trips() {
        round_trip(
            "We was ready, the bus was late.",
            "We were ready, but the bus was late.",
        );
        round_trip("The dogs barks at night.", "The dogs bark at night.");
        round_trip("a b c d e", "e d c b a");
        round_trip("", "New text.");
        round_trip("Old text.", "");
    }

    #[test]
    fn anchors_insertion_to_next_or_previous_word() {
        assert_eq!(
            anchor_insertion("I need book.", hunk(7, 7, "a ")),
            hunk(7, 11, "a book")
        );
        assert_eq!(
            anchor_insertion("I am done", hunk(9, 9, " now")),
            hunk(5, 9, "done now")
        );
        assert_eq!(
            anchor_insertion("x", hunk(0, 1, "y")),
            hunk(0, 1, "y"),
            "a hunk with a width is not changed"
        );
    }
}
