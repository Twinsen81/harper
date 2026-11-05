//! Checks the sentences given as arguments with the LLM and prints the raw answer and the outcomes.
//!
//! `cargo run -p harper-llm --example check -- "Yesterday we go to the office."`

use harper_core::Dialect;
use harper_llm::{LlmChecker, LlmConfig, interpret, prepare};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let sentences: Vec<String> = std::env::args().skip(1).collect();
    let config = LlmConfig {
        enabled: true,
        ..LlmConfig::default()
    };
    let checker = LlmChecker::default();

    let prepared = prepare(&config, Dialect::American, sentences.clone(), None);
    let started = std::time::Instant::now();
    let body = checker.complete(&config, &prepared.body).await?;
    println!("{:.1} s", started.elapsed().as_secs_f64());
    let response: serde_json::Value = serde_json::from_str(&body)?;
    println!("answer: {}", response["choices"][0]["message"]["content"]);
    println!("usage: {}", response["usage"]);
    println!("outcomes: {:#?}", interpret(&prepared, &config, &body)?);

    // The full path with the echo retry, as the integrations use it.
    let started = std::time::Instant::now();
    let outcomes = checker
        .check_sentences(&config, Dialect::American, sentences)
        .await?;
    println!(
        "with retries, {:.1} s: {outcomes:#?}",
        started.elapsed().as_secs_f64()
    );

    Ok(())
}
