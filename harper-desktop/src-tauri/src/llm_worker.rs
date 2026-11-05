//! PROTOTYPE: runs the local LLM check (see [`harper_llm`]) for the highlighter.
//!
//! The highlighter calls [`LlmWorker::lints`] for each text on each frame. That call never
//! waits for the LLM:
//! - It returns the lints of the sentences that are already in the checker's cache.
//! - When the text has not changed for [`LlmSettings::idle_ms`], it sends the other sentences
//!   to the LLM on a background thread. The results show on a later frame.
//!
//! The lookup result is kept per text until the text, the settings, the cache or the ignored
//! lints change, so most frames do not build a [`Document`].

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use harper_core::linting::Lint;
use harper_core::{Dialect, Document, IgnoredLints};
use harper_llm::{LLM_ORIGIN_TAG, LlmChecker, LlmConfig, batches};
use tokio::runtime::{Builder, Runtime};

use crate::config::LlmSettings;

/// Shorter sentences are not sent: in chat apps they are mostly greetings and short answers.
const MIN_SENTENCE_CHARS: usize = 15;

/// After a failed request, no new requests are sent for this time.
const RETRY_DELAY: Duration = Duration::from_secs(10);

/// When more texts than this are kept, the old ones are removed.
const MAX_TEXTS: usize = 64;

pub struct LlmWorker {
    checker: LlmChecker,
    runtime: Runtime,
    texts: HashMap<String, TextState>,
    /// Increases when the ignored lints may have changed.
    ignored_epoch: u64,
    /// No requests are sent before this time.
    retry_after: Arc<Mutex<Option<Instant>>>,
}

struct TextState {
    /// When the highlighter first saw this exact text. The text is "idle" after `idle_ms`.
    first_seen: Instant,
    key: Option<LookupKey>,
    lints: Vec<Lint>,
    /// Sentences that are not in the cache and were not sent yet.
    missing: Vec<String>,
}

/// Everything that can change the lookup result for one text.
#[derive(PartialEq)]
struct LookupKey {
    generation: u64,
    ignored_epoch: u64,
    config: LlmConfig,
    dialect: Dialect,
}

impl LlmWorker {
    pub fn new() -> Self {
        Self {
            checker: LlmChecker::default(),
            runtime: Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("harper-llm")
                .enable_all()
                .build()
                .expect("failed to build the LLM runtime"),
            texts: HashMap::new(),
            ignored_epoch: 0,
            retry_after: Default::default(),
        }
    }

    /// Call this when the ignored lints change.
    pub fn invalidate(&mut self) {
        self.ignored_epoch += 1;
    }

    /// Returns the LLM lints for `text` that are ready now, and starts the check of
    /// the other sentences when the text is idle. `make_document` is called only when
    /// the lookup must run again.
    pub fn lints(
        &mut self,
        settings: &LlmSettings,
        dialect: Dialect,
        text: &str,
        ignored_lints: &IgnoredLints,
        make_document: impl FnOnce() -> Document,
    ) -> Vec<Lint> {
        if !settings.check.enabled {
            return Vec::new();
        }

        if self.texts.len() >= MAX_TEXTS && !self.texts.contains_key(text) {
            self.texts.clear();
        }

        let key = LookupKey {
            generation: self.checker.generation(),
            ignored_epoch: self.ignored_epoch,
            config: settings.check.clone(),
            dialect,
        };

        let state = match self.texts.get_mut(text) {
            Some(state) => state,
            None => self
                .texts
                .entry(text.to_owned())
                .or_insert_with(|| TextState {
                    first_seen: Instant::now(),
                    key: None,
                    lints: Vec::new(),
                    missing: Vec::new(),
                }),
        };

        if state.key.as_ref() != Some(&key) {
            let document = make_document();
            let lookup = self.checker.lookup(&settings.check, dialect, &document);
            let mut lints = lookup.lints;
            ignored_lints.remove_ignored(&mut lints, &document);

            state.lints = lints;
            state.missing = lookup
                .missing
                .into_iter()
                .filter(|sentence| sentence.chars().count() >= MIN_SENTENCE_CHARS)
                .collect();
            state.key = Some(key);
        }

        let is_idle = state.first_seen.elapsed() >= Duration::from_millis(settings.idle_ms);
        let may_send = self
            .retry_after
            .lock()
            .unwrap()
            .is_none_or(|time| Instant::now() >= time);

        if is_idle && may_send && !state.missing.is_empty() {
            // The sentences come back as `missing` only after a failed request and a new lookup.
            let missing = std::mem::take(&mut state.missing);
            send(
                &self.runtime,
                &self.checker,
                &self.retry_after,
                &settings.check,
                dialect,
                missing,
            );
        }

        state.lints.clone()
    }
}

/// Checks the sentences on the background runtime. The results go to the checker's cache.
fn send(
    runtime: &Runtime,
    checker: &LlmChecker,
    retry_after: &Arc<Mutex<Option<Instant>>>,
    config: &LlmConfig,
    dialect: Dialect,
    sentences: Vec<String>,
) {
    let claimed = checker.claim(config, dialect, sentences);
    if claimed.is_empty() {
        return;
    }

    let checker = checker.clone();
    let retry_after = retry_after.clone();
    let config = config.clone();

    runtime.spawn(async move {
        for batch in batches(&config, claimed) {
            // Each failed batch is removed from the pending sentences, so it can be sent again.
            if let Err(error) = checker.check_batch(&config, dialect, batch).await {
                eprintln!("LLM check failed: {error:#}");
                *retry_after.lock().unwrap() = Some(Instant::now() + RETRY_DELAY);
            }
        }
    });
}

/// Adds the LLM lints to Harper's lints. An LLM lint that overlaps a Harper lint is dropped:
/// Harper's rules are more reliable, and two highlights on one word are hard to use.
pub fn merge_llm_lints(organized_lints: &mut BTreeMap<String, Vec<Lint>>, llm_lints: Vec<Lint>) {
    let kept: Vec<Lint> = llm_lints
        .into_iter()
        .filter(|llm_lint| {
            !organized_lints
                .values()
                .flatten()
                .any(|lint| lint.span.overlaps_with(llm_lint.span))
        })
        .collect();

    if !kept.is_empty() {
        organized_lints.insert(LLM_ORIGIN_TAG.to_owned(), kept);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use harper_core::Span;

    use super::*;

    fn lint(start: usize, end: usize) -> Lint {
        Lint {
            span: Span::new(start, end),
            ..Lint::default()
        }
    }

    fn enabled_settings() -> LlmSettings {
        let mut settings = LlmSettings::default();
        settings.check.enabled = true;
        // Nothing listens on port 9 (discard) on a normal machine.
        settings.check.endpoint = "http://127.0.0.1:9/v1/chat/completions".to_owned();
        settings.idle_ms = 60_000;
        settings
    }

    #[test]
    fn merge_drops_llm_lints_that_overlap_harper_lints() {
        let mut organized = BTreeMap::from([("SpellCheck".to_owned(), vec![lint(0, 4)])]);

        merge_llm_lints(&mut organized, vec![lint(2, 6), lint(10, 12)]);

        assert_eq!(organized[LLM_ORIGIN_TAG], vec![lint(10, 12)]);
        assert_eq!(organized["SpellCheck"], vec![lint(0, 4)]);
    }

    #[test]
    fn merge_adds_nothing_when_all_llm_lints_overlap() {
        let mut organized = BTreeMap::from([("SpellCheck".to_owned(), vec![lint(0, 4)])]);

        merge_llm_lints(&mut organized, vec![lint(1, 2)]);

        assert!(!organized.contains_key(LLM_ORIGIN_TAG));
    }

    #[test]
    fn disabled_check_does_not_build_a_document() {
        let mut worker = LlmWorker::new();

        let lints = worker.lints(
            &LlmSettings::default(),
            Dialect::American,
            "Yesterday we go to the office.",
            &IgnoredLints::new(),
            || panic!("the document must not be built"),
        );

        assert!(lints.is_empty());
    }

    #[test]
    fn lookup_runs_again_only_when_something_changes() {
        let mut worker = LlmWorker::new();
        let settings = enabled_settings();
        let text = "Yesterday we go to the office and fix the build.";
        let builds = Cell::new(0);
        let run = |worker: &mut LlmWorker| {
            worker.lints(
                &settings,
                Dialect::American,
                text,
                &IgnoredLints::new(),
                || {
                    builds.set(builds.get() + 1);
                    Document::new_markdown_default_curated(text)
                },
            )
        };

        run(&mut worker);
        run(&mut worker);
        assert_eq!(builds.get(), 1);

        worker.invalidate();
        run(&mut worker);
        assert_eq!(builds.get(), 2);
    }

    #[test]
    fn text_that_is_not_idle_is_not_sent() {
        let mut worker = LlmWorker::new();
        let text = "Yesterday we go to the office and fix the build.";

        worker.lints(
            &enabled_settings(),
            Dialect::American,
            text,
            &IgnoredLints::new(),
            || Document::new_markdown_default_curated(text),
        );

        assert_eq!(worker.texts[text].missing, vec![text.to_owned()]);
    }

    #[test]
    fn short_sentences_are_not_sent() {
        let mut worker = LlmWorker::new();
        let text = "Yes we go now.";

        worker.lints(
            &enabled_settings(),
            Dialect::American,
            text,
            &IgnoredLints::new(),
            || Document::new_markdown_default_curated(text),
        );

        assert!(worker.texts[text].missing.is_empty());
    }
}
