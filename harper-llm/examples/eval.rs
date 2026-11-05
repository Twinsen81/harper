//! Evaluates the LLM check against Margin's synthetic fixtures (MIT, see `THIRD_PARTY_NOTICES.md`).
//!
//! It uses the real pipeline: the fixtures become one Markdown document (one paragraph per item),
//! Harper splits it into sentences, and the sentences go to the model in batches.
//! The scoring follows Margin's `score.ts`.
//!
//! Usage:
//!
//! ```sh
//! cargo run --release -p harper-llm --example eval -- \
//!     --model google/gemma-4-26b-a4b [--endpoint URL] [--batch 8] [--complete-only] [--structured-output] [--verbose]
//! ```

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use harper_core::linting::{LintGroup, Linter, Suggestion};
use harper_core::spell::FstDictionary;
use harper_core::{Dialect, Document, Span};
use harper_llm::checker::outcome_to_lints;
use harper_llm::diff::{Hunk, apply_hunks};
use harper_llm::{LlmChecker, LlmConfig, Outcome, batches, sentences};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Fixtures {
    items: Vec<Item>,
}

#[derive(Debug, Clone, Deserialize)]
struct Item {
    id: String,
    text: String,
    #[serde(default)]
    accept: Vec<String>,
    #[serde(default)]
    debatable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Score {
    Correct,
    Wrong,
    Missed,
    Untouched,
    FalsePositive,
}

struct Args {
    config: LlmConfig,
    verbose: bool,
}

fn parse_args() -> Result<Args> {
    let mut config = LlmConfig {
        enabled: true,
        timeout_ms: 300_000,
        complete_sentences_only: false,
        ..LlmConfig::default()
    };
    let mut verbose = false;
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        let mut value = || args.next().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--model" => config.model = value()?,
            "--endpoint" => config.endpoint = value()?,
            "--batch" => config.batch_size = value()?.parse()?,
            "--reasoning-effort" => config.reasoning_effort = value()?,
            "--complete-only" => config.complete_sentences_only = true,
            "--verbose" => verbose = true,
            "--structured-output" => config.structured_output = true,
            other => bail!("Unknown argument: {other}"),
        }
    }

    Ok(Args { config, verbose })
}

fn load(name: &str) -> Result<Vec<Item>> {
    let path = format!("{}/eval/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).with_context(|| format!("Unable to read {path}"))?;
    Ok(serde_json::from_str::<Fixtures>(&text)?.items)
}

fn normalize(text: &str) -> String {
    text.replace(['‘', '’'], "'")
        .replace(['“', '”'], "\"")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn score(item: &Item, output: &str) -> Score {
    let changed = normalize(output) != normalize(&item.text);
    match (item.accept.is_empty(), changed) {
        (true, true) => Score::FalsePositive,
        (true, false) => Score::Untouched,
        (false, false) => Score::Missed,
        (false, true)
            if item
                .accept
                .iter()
                .any(|a| normalize(a) == normalize(output)) =>
        {
            Score::Correct
        }
        (false, true) => Score::Wrong,
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let Args { config, verbose } = parse_args()?;
    let dialect = Dialect::American;

    let items: Vec<Item> = ["errors", "clean", "markdown"]
        .into_iter()
        .map(load)
        .collect::<Result<Vec<_>>>()?
        .concat();

    // One Markdown paragraph per item, as in Margin's `layout`.
    let mut text = String::new();
    let mut ranges: Vec<Span<char>> = Vec::new();
    for item in &items {
        let start = text.chars().count();
        text.push_str(&item.text);
        ranges.push(Span::new(start, start + item.text.chars().count()));
        text.push_str("\n\n");
    }

    let document = Document::new_markdown_default_curated(&text);
    let found = sentences(&document, &config);

    let mut harper = LintGroup::new_curated(FstDictionary::curated(), dialect);
    let harper_lints = harper.lint(&document);

    let checker = LlmChecker::default();
    let mut lints = Vec::new();
    let mut rejected: BTreeMap<String, usize> = BTreeMap::new();
    let mut request_times: Vec<Duration> = Vec::new();
    let mut failed_batches = 0;

    let texts: Vec<String> = found.iter().map(|s| s.text.clone()).collect();
    let started = Instant::now();
    let mut offset = 0;

    for batch in batches(&config, texts) {
        let batch_sentences = &found[offset..offset + batch.len()];
        offset += batch.len();

        let request_started = Instant::now();
        let result = checker.check_sentences(&config, dialect, batch).await;
        request_times.push(request_started.elapsed());

        match result {
            Ok(outcomes) => {
                for (sentence, outcome) in batch_sentences.iter().zip(&outcomes) {
                    if let Outcome::Rejected(reason) = outcome {
                        *rejected.entry(format!("{reason:?}")).or_default() += 1;
                    }
                    lints.extend(outcome_to_lints(sentence, outcome));
                }
            }
            Err(error) => {
                failed_batches += 1;
                eprintln!("Batch failed: {error:#}");
            }
        }
        eprint!("\r{offset}/{} sentences", found.len());
    }
    eprintln!();
    let total_time = started.elapsed();

    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let (mut errors, mut clean, mut harper_flagged, mut combined) = (0, 0, 0, 0);
    let mut report = Vec::new();

    for (item, range) in items.iter().zip(&ranges) {
        let hunks: Vec<Hunk> = lints
            .iter()
            .filter(|lint| lint.span.start >= range.start && lint.span.end <= range.end)
            .map(|lint| Hunk {
                start: lint.span.start - range.start,
                end: lint.span.end - range.start,
                replacement: match &lint.suggestions[0] {
                    Suggestion::ReplaceWith(chars) => chars.iter().collect(),
                    _ => String::new(),
                },
            })
            .collect();
        let output = apply_hunks(&item.text, &hunks);
        let result = score(item, &output);
        let flagged_by_harper = harper_lints
            .iter()
            .any(|lint| lint.span.start >= range.start && lint.span.end <= range.end);

        if !matches!(result, Score::Correct | Score::Untouched) || verbose {
            report.push(format!(
                "{:<14} {:<5} {}{}\n               -> {}",
                format!("{result:?}"),
                item.id,
                if item.debatable { "(debatable) " } else { "" },
                item.text,
                output
            ));
        }

        if item.debatable {
            continue;
        }
        *counts.entry(score_name(result)).or_default() += 1;
        if item.accept.is_empty() {
            clean += 1;
        } else {
            errors += 1;
            harper_flagged += usize::from(flagged_by_harper);
            combined += usize::from(flagged_by_harper || result == Score::Correct);
        }
    }

    let count = |name: &str| counts.get(name).copied().unwrap_or(0);
    let (correct, wrong, false_positives) =
        (count("correct"), count("wrong"), count("false-positive"));
    let changes = correct + wrong + false_positives;
    let precision = if changes > 0 {
        correct as f64 / changes as f64
    } else {
        1.0
    };
    let recall = correct as f64 / errors.max(1) as f64;
    let f05 = if precision + recall > 0.0 {
        1.25 * precision * recall / (0.25 * precision + recall)
    } else {
        0.0
    };

    request_times.sort();
    let median = request_times
        .get(request_times.len() / 2)
        .copied()
        .unwrap_or_default();

    println!("{}", report.join("\n"));
    println!();
    println!(
        "model:            {}",
        if config.model.is_empty() {
            "(loaded model)"
        } else {
            &config.model
        }
    );
    println!("batch size:       {}", config.batch_size);
    println!("schema:           {}", config.structured_output);
    println!(
        "items:            {errors} with an error, {clean} correct (debatable items excluded)"
    );
    println!(
        "sentences sent:   {} in {} requests ({failed_batches} failed)",
        found.len(),
        request_times.len()
    );
    println!("correct fixes:    {correct}");
    println!("wrong fixes:      {wrong}");
    println!("missed:           {}", count("missed"));
    println!("false positives:  {false_positives} of {clean} correct items");
    println!("precision:        {:.1}%", precision * 100.0);
    println!("recall:           {:.1}%", recall * 100.0);
    println!("F0.5:             {:.3}", f05);
    println!(
        "Harper flags:     {harper_flagged} of {errors} errors; Harper or LLM: {:.1}%",
        combined as f64 / errors.max(1) as f64 * 100.0
    );
    println!("rejected fixes:   {rejected:?}");
    println!(
        "time:             {:.1} s total, {:.2} s median per batch (with echo retries)",
        total_time.as_secs_f64(),
        median.as_secs_f64()
    );

    Ok(())
}

fn score_name(score: Score) -> &'static str {
    match score {
        Score::Correct => "correct",
        Score::Wrong => "wrong",
        Score::Missed => "missed",
        Score::Untouched => "untouched",
        Score::FalsePositive => "false-positive",
    }
}
