# Harper with a local LLM (unofficial fork)

This fork of [Harper](https://github.com/Automattic/harper) adds an optional grammar check with
a language model that runs on your own Mac. Harper's rules stay in charge. The model finds
errors that rules cannot find, for example a wrong tense ("Yesterday we go to the office"),
and Harper shows them as normal suggestions with an "AI" label. Your text does not leave
your Mac.

It works in Harper Desktop (Slack, Messages, Mail and the other apps you turn on in Harper)
and in `harper-ls` (editors).

This is not an official Harper build. The Harper maintainers are building their own small
on-device models. Until those are ready, this fork uses a general model in LM Studio.

The branch is `llm-prototype`. It is regularly rebased on Harper's `master`.

## Requirements

- A Mac with Apple Silicon and 24 GB of RAM or more. 32 GB or more is better.
- About 25 GB of free disk space (build tools, the build, and the model).
- Homebrew and the Xcode Command Line Tools.
- [LM Studio](https://lmstudio.ai) to run the model. The install script can install it.

## Install

```sh
git clone -b llm-prototype https://github.com/Twinsen81/harper.git harper-llm
cd harper-llm
scripts/install-llm-harper.sh --setup-model --enable-llm
```

The script:

1. Installs missing build tools (Rust, Node.js, pnpm, just, wasm-pack).
2. Builds Harper Desktop. The first build takes 10-20 minutes.
3. With `--setup-model`: installs LM Studio if necessary, downloads the recommended model,
   starts LM Studio's server and loads the model.
4. Quits Harper, replaces `/Applications/Harper.app` with this build and starts it.
   The old app goes to a temporary folder.
5. With `--enable-llm`: turns on the AI check in Harper's settings.

Then:

1. Give Harper the Accessibility permission when macOS asks: System Settings > Privacy &
   Security > Accessibility. After a new build you may need to turn the switch off and on.
2. Open Harper's Settings > General > "AI check (prototype)" and click
   "Test the connection". You should see the corrected test sentence in about 1-5 seconds.

Options of the script: `--build-only`, `--with-harper-ls` (also installs the language
server for editors), `--help`.

## The model

| Your Mac | Model in LM Studio | Download |
|---|---|---|
| 32 GB of RAM or more | Gemma 4 12B, MLX 6-bit (`lmstudio-community/gemma-4-12B-it-MLX-6bit`) | 9.8 GB |
| 24 GB of RAM | Gemma 4 12B QAT, GGUF (`lmstudio-community/gemma-4-12B-it-QAT-GGUF`) | 7.2 GB |

The script chooses one from the RAM of your Mac. Results on a test set of 63 errors and 71
correct sentences: the 6-bit model found 95% of the errors and changed no correct sentence.
About 1 second per sentence on an M5 Max. On a base M3, expect several seconds per sentence.

LM Studio settings: context length 8192. Harper turns off the model's "thinking" in each
request, so you do not need to change it in LM Studio.

## Settings in Harper

Settings > General > "AI check (prototype)":

- **Check with a local language model**: on or off.
- **Endpoint**: `http://localhost:1234/v1/chat/completions` (LM Studio's default). Any server
  with the OpenAI chat completions API works, for example Ollama or llama.cpp's
  `llama-server`, but only LM Studio was tested.
- **Model**: the model name in LM Studio, for example `google/gemma-4-12b`. If it is set,
  LM Studio loads the model on the first request. If it is empty, Harper uses the model that
  LM Studio has loaded.
- **Wait after typing**: the AI check starts after this time without changes. 600 ms is good.

To turn off the AI check from a suggestion, use "Turn off AI check" in its popup.

## Update

```sh
cd harper-llm
git pull --rebase
scripts/install-llm-harper.sh
```

This build never updates itself. An official Harper update would replace it, so the updater
is turned off. "Check for updates" in the settings only shows how to update.

## Uninstall

Delete `/Applications/Harper.app` and install the official Harper from
https://writewithharper.com. Your Harper settings and dictionary stay.

## Troubleshooting

- **No AI suggestions at all.** Check that LM Studio's server runs (`lms server status`) and
  that "Test the connection" works. Check that the app is turned on in Harper's integrations.
- **The first check after a pause takes very long.** The model was moved out of memory.
  Close memory-heavy apps, or use the smaller model.
- **Highlights do not appear in any app.** The Accessibility permission is missing, or it
  belongs to an older build. Turn Harper's switch off and on in System Settings.
- **The build fails.** Run `scripts/install-llm-harper.sh --build-only` and read the first error.

## For AI agents: setting this up for a user

Follow these steps exactly. Ask the user before each step that needs their action (marked
USER). Do not change other settings on the user's Mac.

1. Check the Mac:
   - `uname -m` must print `arm64`.
   - `sysctl -n hw.memsize`: 24 GB (25769803776) or more. With less, tell the user that the AI
     check will be slow or will not work, and stop unless they want to continue.
   - `xcode-select -p` must succeed. If not, USER: run `xcode-select --install` and finish
     the dialog.
   - `brew --version` must succeed. If not, USER: install Homebrew from https://brew.sh.
2. Clone and run the script (it takes 10-30 minutes; run it with a long timeout):

   ```sh
   git clone -b llm-prototype https://github.com/Twinsen81/harper.git harper-llm
   cd harper-llm
   scripts/install-llm-harper.sh --setup-model --enable-llm
   ```

   - If the script stops with "Open LM Studio once": USER: open LM Studio, close its
     welcome screens, then run the script again.
   - If it prints "No Harper settings yet": USER: open Harper and finish its setup, then run
     `scripts/install-llm-harper.sh --enable-llm` with `HARPER_LLM_MODEL` set to the model
     name that the first run printed (`google/gemma-4-12b` or `google/gemma-4-12b-qat`).
3. Check the model server:

   ```sh
   ~/.lmstudio/bin/lms ps
   curl -s http://localhost:1234/v1/chat/completions -H 'Content-Type: application/json' \
     -d '{"messages":[{"role":"user","content":"Fix: Yesterday we go to the office."}],"reasoning_effort":"none","max_tokens":40}'
   ```

   `lms ps` must list the model. The answer must contain "went".
4. Check the whole pipeline (optional, about 1 minute):

   ```sh
   cargo run --release -p harper-llm --example check -- "Yesterday we go to the office and fix the build."
   ```

   The output must show fixes "went" and "fixed".
5. USER: give Harper the Accessibility permission (System Settings > Privacy & Security >
   Accessibility), then type "Yesterday we go to the office" in an app that Harper checks,
   for example Notes or Slack, and wait about 2 seconds. "go" must get a suggestion with the
   label "AI".
6. Tell the user:
   - how to update (the "Update" section);
   - that this build does not update itself;
   - that LM Studio must run, with its server started, for the AI check. LM Studio has
     options to start at login and to start its server automatically; show the user where
     they are in the installed LM Studio version.

Do not push to this repository, and do not open pull requests against
`Automattic/harper` from this branch.

## How it works

See `harper-llm/README.md`. In short: sentences go to the model in batches. The model returns
corrected sentences, and a word diff finds the changed words. Guards reject changes to
negation, numbers, quotes and Markdown, and changes that rewrite too much. A suggestion that
overlaps one of Harper's own suggestions is dropped. The design is based on
[Margin](https://github.com/phoenixak/margin) (MIT).
