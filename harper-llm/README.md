# harper-llm (prototype)

An optional grammar check with a local large language model (LLM), for Harper integrations.
Harper's rules stay in charge. The LLM looks only for errors that rules cannot find,
for example a wrong tense ("Yesterday I go to the store.").

The check sends sentences to an OpenAI-compatible chat completions endpoint
(LM Studio, Ollama or the llama.cpp server). Nothing leaves the computer when the endpoint is local.

Several parts are ported from [Margin](https://github.com/phoenixak/margin) (MIT).
See `THIRD_PARTY_NOTICES.md`.

## How it works

1. Protected spans (code, URLs, emails, `@mentions`, `#channels`, `:emoji:`) are replaced with placeholders.
2. Up to 8 sentences go to the model in one request. The model returns the full corrected sentence
   for each sentence that it changed.
3. A word-level diff finds the exact changed words.
4. Guards reject fixes that change negation, numbers, quoted text or Markdown, or more than 35% of the words.
5. A sentence that the model flagged but returned unchanged is sent once more, alone.
6. The outcomes are cached by sentence. Integrations read the cache on each lint run and check
   missing sentences in the background.

## Configuration

`LlmConfig` is deserialized from camelCase JSON. All fields are optional:

```json
{
  "enabled": true,
  "endpoint": "http://localhost:1234/v1/chat/completions",
  "model": "google/gemma-4-26b-a4b",
  "reasoningEffort": "none",
  "batchSize": 8,
  "completeSentencesOnly": true,
  "structuredOutput": false
}
```

- Keep `reasoningEffort` at `"none"`. "Thinking" made requests about 5 times slower and caused timeouts.
- Keep `structuredOutput` off with LM Studio. The strict JSON schema lowered recall from 71% to 16%.

## Evaluation

The `eval` example runs Margin's synthetic fixtures (63 errors and 71 correct sentences,
without debatable items) through the real pipeline:

```sh
cargo run --release -p harper-llm --example eval -- --model google/gemma-4-26b-a4b
```

Results on an Apple M5 Max with LM Studio (batch size 8, thinking off, no schema),
with prompt version 3 (the model also fixes spelling):

| Model | Precision | Recall | False positives | Median time per batch |
|---|---|---|---|---|
| Gemma 4 26B-A4B | 100% | 90.5% | 0 of 71 | 1.7 s |

Harper's own rules flag 21 of the 63 errors. Harper and Gemma 4 26B-A4B together catch 93.7%.
The result was the same in two runs.

With prompt version 2 (Margin's rule "do not fix spelling"):

| Model | Precision | Recall | False positives | Median time per batch |
|---|---|---|---|---|
| Gemma 4 26B-A4B | 100% | 88.9% | 0 of 71 | 2.1 s |
| Qwen 3.8 27B | 95.1% | 92.1% | 1 of 71 | 7.3 s |
| Gemma 4 E4B | 90.7% | 77.8% | 2 of 71 | 2.7 s |

The `check` example sends the sentences given as arguments and prints the raw answer:

```sh
cargo run -p harper-llm --example check -- "Ystrday we go to the office and fix the build"
```
