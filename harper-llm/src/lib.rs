//! PROTOTYPE: an optional grammar check with a local large language model (LLM).
//!
//! Harper's rules stay in charge. This crate sends sentences to an OpenAI-compatible
//! chat completions endpoint (LM Studio, Ollama, the llama.cpp server) and converts
//! the answers to [`harper_core::linting::Lint`]s, for errors that rules cannot find,
//! such as a wrong tense.
//!
//! The pipeline for one batch of sentences:
//!
//! 1. [`mask`] replaces protected spans (code, URLs, mentions) with placeholders.
//! 2. [`prompt`] builds the request. The model returns the full corrected sentence
//!    for each sentence that it changed.
//! 3. [`response`] parses the answer.
//! 4. [`diff`] computes word-level hunks between the original and the corrected sentence,
//!    so the model never has to give character positions.
//! 5. [`guards`] rejects fixes that change negation, numbers, quotes or too many words.
//!
//! [`check`] connects these steps without I/O. [`checker`] adds the HTTP call, a cache
//! and the conversion to lints for a [`harper_core::Document`].
//!
//! Several parts are ported from Margin (MIT). See `THIRD_PARTY_NOTICES.md`.

pub mod check;
pub mod checker;
pub mod config;
pub mod diff;
pub mod guards;
pub mod mask;
pub mod prompt;
pub mod response;

pub use check::{Outcome, Prepared, SentenceFix, interpret, prepare};
pub use checker::{LLM_ORIGIN_TAG, LlmChecker, Lookup, Sentence, batches, sentences};
pub use config::LlmConfig;
