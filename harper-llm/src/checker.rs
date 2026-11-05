//! Sends sentences of a [`Document`] to the LLM, caches the outcomes,
//! and converts them to Harper lints.
//!
//! The LLM is slow compared to Harper, so integrations call [`LlmChecker::lookup`]
//! on each lint run (it only reads the cache) and check the missing sentences
//! in the background with [`LlmChecker::check_batch`].

use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use harper_core::linting::{Lint, LintKind, Suggestion};
use harper_core::{Dialect, Document, Span, Token, TokenStringExt};
use lru::LruCache;
use serde_json::Value;
use tokio::sync::Semaphore;

use crate::LlmConfig;
use crate::check::{Outcome, interpret, prepare};
use crate::prompt::{ECHO_NOTE, PROMPT_VERSION};

/// The maximum number of echoed sentences that are sent again, per batch.
const MAX_ECHO_RETRIES: usize = 4;

/// The origin tag (rule name) of LLM lints.
pub const LLM_ORIGIN_TAG: &str = "LocalLlm";

/// Lower values are more important. Harper's own lints use 127 by default,
/// so Harper wins when an LLM lint overlaps with one of them.
const LLM_LINT_PRIORITY: u8 = 200;

/// The number of cached sentence outcomes.
const CACHE_CAPACITY: usize = 5000;

/// A sentence of a document and its location.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sentence {
    pub span: Span<char>,
    pub text: String,
}

/// The cache key. Each part can change the answer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    prompt_version: u32,
    endpoint: String,
    model: String,
    reasoning_effort: String,
    dialect: Dialect,
    sentence: String,
}

impl CacheKey {
    fn new(config: &LlmConfig, dialect: Dialect, sentence: &str) -> Self {
        Self {
            prompt_version: PROMPT_VERSION,
            endpoint: config.endpoint.clone(),
            model: config.model.clone(),
            reasoning_effort: config.reasoning_effort.clone(),
            dialect,
            sentence: sentence.to_owned(),
        }
    }
}

/// The result of a cache lookup for a document.
#[derive(Debug, Default)]
pub struct Lookup {
    /// Lints for the sentences that are in the cache.
    pub lints: Vec<Lint>,
    /// The text of the sentences that are not in the cache yet.
    pub missing: Vec<String>,
}

/// Sends sentences to the LLM and caches the outcomes. Clones share the same cache.
#[derive(Clone)]
pub struct LlmChecker {
    client: reqwest::Client,
    cache: Arc<Mutex<LruCache<CacheKey, Arc<Outcome>>>>,
    /// Sentences that a background task is checking now.
    pending: Arc<Mutex<HashSet<CacheKey>>>,
    /// A local model usually handles one request at a time, so we send them one by one.
    request_slot: Arc<Semaphore>,
    /// Increases each time new outcomes are stored in the cache.
    generation: Arc<AtomicU64>,
}

impl Default for LlmChecker {
    fn default() -> Self {
        Self {
            client: reqwest::Client::new(),
            cache: Arc::new(Mutex::new(LruCache::new(
                NonZeroUsize::new(CACHE_CAPACITY).unwrap(),
            ))),
            pending: Default::default(),
            request_slot: Arc::new(Semaphore::new(1)),
            generation: Default::default(),
        }
    }
}

impl LlmChecker {
    /// A number that increases each time [`Self::check_batch`] stores new outcomes.
    /// Callers can keep the result of [`Self::lookup`] until this number changes.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Gets the cached lints for `document` and the sentences that are not checked yet.
    /// This function does not send requests.
    pub fn lookup(&self, config: &LlmConfig, dialect: Dialect, document: &Document) -> Lookup {
        let mut cache = self.cache.lock().unwrap();

        sentences(document, config)
            .into_iter()
            .fold(Lookup::default(), |mut lookup, sentence| {
                match cache.get(&CacheKey::new(config, dialect, &sentence.text)) {
                    Some(outcome) => lookup.lints.extend(outcome_to_lints(&sentence, outcome)),
                    None => lookup.missing.push(sentence.text),
                }
                lookup
            })
    }

    /// Marks the sentences as pending and returns only the sentences
    /// that no other task is checking now.
    pub fn claim(
        &self,
        config: &LlmConfig,
        dialect: Dialect,
        sentences: Vec<String>,
    ) -> Vec<String> {
        let mut pending = self.pending.lock().unwrap();

        sentences
            .into_iter()
            .filter(|sentence| pending.insert(CacheKey::new(config, dialect, sentence)))
            .collect()
    }

    /// Checks one batch (see [`batches`]) and stores the outcomes in the cache.
    /// The sentences are not pending anymore after this call, also when it fails.
    pub async fn check_batch(
        &self,
        config: &LlmConfig,
        dialect: Dialect,
        batch: Vec<String>,
    ) -> Result<()> {
        let keys: Vec<CacheKey> = batch
            .iter()
            .map(|sentence| CacheKey::new(config, dialect, sentence))
            .collect();

        let result = self.check_sentences(config, dialect, batch).await;

        {
            let mut pending = self.pending.lock().unwrap();
            for key in &keys {
                pending.remove(key);
            }
        }

        let outcomes = result?;
        let mut cache = self.cache.lock().unwrap();
        for (key, outcome) in keys.into_iter().zip(outcomes) {
            cache.put(key, Arc::new(outcome));
        }
        self.generation.fetch_add(1, Ordering::Release);

        Ok(())
    }

    /// Checks one batch without the cache and returns one outcome per sentence.
    ///
    /// Margin's echo retry: a sentence that the model flagged but returned unchanged is sent
    /// again, alone, with [`ECHO_NOTE`] (at most [`MAX_ECHO_RETRIES`] per batch). A second echo
    /// counts as correct. The returned outcomes never contain [`Outcome::Echo`].
    pub async fn check_sentences(
        &self,
        config: &LlmConfig,
        dialect: Dialect,
        batch: Vec<String>,
    ) -> Result<Vec<Outcome>> {
        let prepared = prepare(config, dialect, batch, None);
        let body = self.complete(config, &prepared.body).await?;
        let mut outcomes = interpret(&prepared, config, &body)?;

        let echoes: Vec<usize> = (0..outcomes.len())
            .filter(|&i| outcomes[i] == Outcome::Echo)
            .collect();

        for (retry, index) in echoes.into_iter().enumerate() {
            outcomes[index] = if retry < MAX_ECHO_RETRIES {
                self.retry_echo(config, dialect, &prepared.sentences[index])
                    .await?
            } else {
                Outcome::Correct
            };
        }

        Ok(outcomes)
    }

    async fn retry_echo(
        &self,
        config: &LlmConfig,
        dialect: Dialect,
        sentence: &str,
    ) -> Result<Outcome> {
        let prepared = prepare(config, dialect, vec![sentence.to_owned()], Some(ECHO_NOTE));
        let body = self.complete(config, &prepared.body).await?;
        let outcome = interpret(&prepared, config, &body)?
            .pop()
            .unwrap_or(Outcome::Correct);

        Ok(match outcome {
            Outcome::Echo => Outcome::Correct,
            other => other,
        })
    }

    /// Sends one chat completions request and returns the response body.
    pub async fn complete(&self, config: &LlmConfig, body: &Value) -> Result<String> {
        let _slot = self.request_slot.acquire().await?;

        let response = self
            .client
            .post(&config.endpoint)
            .timeout(Duration::from_millis(config.timeout_ms))
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .with_context(|| format!("Unable to send the LLM request to {}", config.endpoint))?;

        let status = response.status();
        let text = response.text().await?;

        if !status.is_success() {
            return Err(anyhow!("The LLM server returned {status}: {text}"));
        }

        Ok(text)
    }
}

/// Splits sentences into request batches of at most [`LlmConfig::batch_size`] sentences
/// and [`LlmConfig::max_batch_chars`] characters. A longer sentence gets its own batch.
pub fn batches(config: &LlmConfig, sentences: Vec<String>) -> Vec<Vec<String>> {
    let batch_size = config.batch_size.max(1);

    sentences
        .into_iter()
        .fold(Vec::new(), |mut batches, sentence| {
            let fits = batches.last().is_some_and(|batch: &Vec<String>| {
                let chars: usize = batch.iter().map(|s| s.chars().count()).sum();
                batch.len() < batch_size
                    && chars + sentence.chars().count() <= config.max_batch_chars
            });

            if fits {
                batches.last_mut().unwrap().push(sentence);
            } else {
                batches.push(vec![sentence]);
            }
            batches
        })
}

/// Gets the sentences of a document that are worth an LLM request.
///
/// A sentence is skipped when it has fewer than [`LlmConfig::min_words`] words or contains
/// a code block. When [`LlmConfig::complete_sentences_only`] is set, a sentence must also
/// end with `.`, `!` or `?`.
pub fn sentences(document: &Document, config: &LlmConfig) -> Vec<Sentence> {
    let source = document.get_source();

    document
        .iter_sentences()
        .filter_map(|tokens| {
            let content: Vec<&Token> = tokens
                .iter()
                .filter(|token| !token.kind.is_whitespace() && !token.kind.is_paragraph_break())
                .collect();

            let is_complete = content
                .last()
                .is_some_and(|token| token.kind.is_sentence_terminator());
            // Inline code is unlintable too, but `mask` protects it. Block-level unlintable
            // content (a code block) has line breaks, and the whole sentence is skipped.
            let has_block_unlintable = content.iter().any(|token| {
                token.kind.is_unlintable() && token.span.get_content(source).contains(&'\n')
            });
            let word_count = content.iter().filter(|token| token.kind.is_word()).count();

            if (config.complete_sentences_only && !is_complete)
                || has_block_unlintable
                || word_count < config.min_words
            {
                return None;
            }

            let span = Span::new(content.first()?.span.start, content.last()?.span.end);

            Some(Sentence {
                span,
                text: span.get_content_string(source),
            })
        })
        .collect()
}

/// Converts the outcome for one sentence to lints with document positions.
pub fn outcome_to_lints(sentence: &Sentence, outcome: &Outcome) -> Vec<Lint> {
    let Outcome::Fixed(fixes) = outcome else {
        return Vec::new();
    };

    fixes
        .iter()
        .map(|fix| {
            let message = if fix.why.is_empty() {
                format!("The AI check suggests \"{}\".", fix.replacement)
            } else {
                fix.why.clone()
            };

            Lint {
                span: Span::new(
                    sentence.span.start + fix.start,
                    sentence.span.start + fix.end,
                ),
                lint_kind: lint_kind(&fix.kind),
                suggestions: vec![if fix.replacement.is_empty() {
                    Suggestion::Remove
                } else {
                    Suggestion::ReplaceWith(fix.replacement.chars().collect())
                }],
                message: format!("[LLM] {message}"),
                priority: LLM_LINT_PRIORITY,
            }
        })
        .collect()
}

fn lint_kind(kind: &str) -> LintKind {
    match kind {
        "word" => LintKind::WordChoice,
        "punctuation" => LintKind::Punctuation,
        "clarity" => LintKind::Readability,
        _ => LintKind::Grammar,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::*;

    fn completion(fixes: Value) -> String {
        json!({ "choices": [{ "message": { "content": json!({ "fixes": fixes }).to_string() } }] })
            .to_string()
    }

    fn texts(found: &[Sentence]) -> Vec<&str> {
        found.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn complete_sentences_only_skips_the_last_unfinished_sentence() {
        let document = Document::new_plain_english_curated(
            "Yesterday I go to the store. Hi. I am still typing this",
        );

        let config = LlmConfig::default();
        assert_eq!(
            texts(&sentences(&document, &config)),
            vec!["Yesterday I go to the store."]
        );

        let chat = LlmConfig {
            complete_sentences_only: false,
            ..LlmConfig::default()
        };
        assert_eq!(
            texts(&sentences(&document, &chat)),
            vec!["Yesterday I go to the store.", "I am still typing this"]
        );
    }

    #[test]
    fn inline_code_is_kept_and_code_blocks_are_skipped() {
        let document = Document::new_markdown_default_curated(
            "Run `pnpm test` before you open a pull request.\n\n```\nlet x = 1;\nfoo();\nbar();\n```\n",
        );

        assert_eq!(
            texts(&sentences(&document, &LlmConfig::default())),
            vec!["Run `pnpm test` before you open a pull request."]
        );
    }

    #[test]
    fn sentence_spans_point_into_the_source() {
        let source = "Café 😀 is nice.  Yesterday I go to the store.";
        let document = Document::new_plain_english_curated(source);
        let chars: Vec<char> = source.chars().collect();

        for sentence in sentences(&document, &LlmConfig::default()) {
            assert_eq!(sentence.span.get_content_string(&chars), sentence.text);
        }
    }

    #[test]
    fn batches_respect_size_and_char_limits() {
        let config = LlmConfig {
            batch_size: 2,
            max_batch_chars: 10,
            ..LlmConfig::default()
        };
        let input = ["aaa", "bbb", "ccc", "dddddddddddd", "e"]
            .map(str::to_owned)
            .to_vec();

        assert_eq!(
            batches(&config, input),
            vec![
                vec!["aaa", "bbb"],
                vec!["ccc"],
                vec!["dddddddddddd"],
                vec!["e"]
            ]
        );
    }

    #[test]
    fn claim_returns_each_sentence_only_once() {
        let checker = LlmChecker::default();
        let config = LlmConfig::default();
        let claim = |s: &[&str]| {
            checker.claim(
                &config,
                Dialect::American,
                s.iter().map(|s| s.to_string()).collect(),
            )
        };

        assert_eq!(claim(&["A b c.", "D e f."]), vec!["A b c.", "D e f."]);
        assert_eq!(claim(&["A b c.", "G h i."]), vec!["G h i."]);
    }

    /// Reads one HTTP request (headers and body) and returns it as text.
    async fn read_request(stream: &mut tokio::net::TcpStream) -> String {
        let mut data = Vec::new();
        let mut buffer = [0; 4096];

        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            data.extend_from_slice(&buffer[..read]);
            let text = String::from_utf8_lossy(&data).into_owned();

            if let Some(header_end) = text.find("\r\n\r\n") {
                let length: usize = text[..header_end]
                    .lines()
                    .find_map(|line| {
                        line.to_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse().unwrap())
                    })
                    .unwrap_or(0);
                if data.len() >= header_end + 4 + length || read == 0 {
                    return text;
                }
            }
        }
    }

    /// Starts an HTTP server that answers with `bodies`, one per request, in order.
    /// Returns its URL and the received requests.
    async fn mock_server(bodies: Vec<String>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let received = requests.clone();

        tokio::spawn(async move {
            for body in bodies {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_request(&mut stream).await;
                received.lock().unwrap().push(request);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });

        (format!("http://{address}/v1/chat/completions"), requests)
    }

    #[tokio::test]
    async fn echoed_sentence_is_sent_again_with_the_note() {
        let (endpoint, requests) = mock_server(vec![
            // The model flags sentence 1 but returns it unchanged.
            completion(json!([{ "id": 1, "text": "She go home.", "kind": "grammar", "why": "Agreement." }])),
            completion(json!([{ "id": 1, "text": "She goes home.", "kind": "grammar", "why": "Agreement." }])),
        ])
        .await;
        let config = LlmConfig {
            endpoint,
            ..LlmConfig::default()
        };

        let outcomes = LlmChecker::default()
            .check_sentences(&config, Dialect::American, vec!["She go home.".to_owned()])
            .await
            .unwrap();

        assert!(matches!(&outcomes[0], Outcome::Fixed(fixes) if fixes[0].replacement == "goes"));
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(!requests[0].contains("returned it unchanged"));
        assert!(requests[1].contains("returned it unchanged"));
    }

    #[tokio::test]
    async fn second_echo_counts_as_correct() {
        let echo = completion(
            json!([{ "id": 1, "text": "She goes home.", "kind": "grammar", "why": "Agreement." }]),
        );
        let (endpoint, _) = mock_server(vec![echo.clone(), echo]).await;
        let config = LlmConfig {
            endpoint,
            ..LlmConfig::default()
        };

        let outcomes = LlmChecker::default()
            .check_sentences(
                &config,
                Dialect::American,
                vec!["She goes home.".to_owned()],
            )
            .await
            .unwrap();

        assert_eq!(outcomes, vec![Outcome::Correct]);
    }

    #[tokio::test]
    async fn check_batch_fills_the_cache_and_lookup_returns_lints() {
        let (endpoint, _) = mock_server(vec![completion(json!([
            { "id": 1, "text": "I went to work and yesterday I went home.", "kind": "grammar", "why": "Past tense." }
        ]))])
        .await;
        let config = LlmConfig {
            enabled: true,
            endpoint,
            ..LlmConfig::default()
        };
        let checker = LlmChecker::default();
        let source = "I went to work and yesterday I go home. Another sentence is here.";
        let document = Document::new_plain_english_curated(source);

        let before = checker.lookup(&config, Dialect::American, &document);
        assert!(before.lints.is_empty());
        assert_eq!(before.missing.len(), 2);

        assert_eq!(checker.generation(), 0);
        let claimed = checker.claim(&config, Dialect::American, before.missing);
        for batch in batches(&config, claimed) {
            checker
                .check_batch(&config, Dialect::American, batch)
                .await
                .unwrap();
        }

        let after = checker.lookup(&config, Dialect::American, &document);
        let chars: Vec<char> = source.chars().collect();
        assert_eq!(checker.generation(), 1);
        assert!(after.missing.is_empty());
        assert_eq!(after.lints.len(), 1);
        assert_eq!(after.lints[0].span.get_content_string(&chars), "go");
        assert_eq!(after.lints[0].message, "[LLM] Past tense.");
        assert!(checker.pending.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn failed_check_is_not_cached_and_not_pending() {
        let config = LlmConfig {
            enabled: true,
            // Nothing listens on port 9 (discard) on a normal machine.
            endpoint: "http://127.0.0.1:9/v1/chat/completions".to_owned(),
            timeout_ms: 2_000,
            ..LlmConfig::default()
        };
        let checker = LlmChecker::default();
        let claimed = checker.claim(
            &config,
            Dialect::American,
            vec!["Yesterday I go home.".to_owned()],
        );

        assert!(
            checker
                .check_batch(&config, Dialect::American, claimed)
                .await
                .is_err()
        );
        assert!(checker.pending.lock().unwrap().is_empty());
        assert!(checker.cache.lock().unwrap().is_empty());
        assert_eq!(checker.generation(), 0);
    }
}
