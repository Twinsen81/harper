#!/usr/bin/env bash
# Builds Harper Desktop with the local LLM check (branch llm-prototype) and installs it.
# See LLM_FORK.md. Run it from anywhere inside the repository.
#
# Usage: scripts/install-llm-harper.sh [options]
#   --build-only       Build the app, but do not install it.
#   --with-harper-ls   Also install the harper-ls language server (cargo install).
#   --setup-model      Also install LM Studio (if missing), download the recommended model,
#                      load it, and start the LM Studio server.
#   --enable-llm       Turn on the AI check in Harper's settings and set the model name.
#                      Needs --setup-model, or a model name in HARPER_LLM_MODEL.
#   -h, --help         Show this help.
set -euo pipefail

build_only=false
with_harper_ls=false
setup_model=false
enable_llm=false

for arg in "$@"; do
  case "$arg" in
    --build-only) build_only=true ;;
    --with-harper-ls) with_harper_ls=true ;;
    --setup-model) setup_model=true ;;
    --enable-llm) enable_llm=true ;;
    -h | --help) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "Unknown option: $arg" >&2; exit 2 ;;
  esac
done

step() { printf '\n==> %s\n' "$*"; }
fail() { printf '\nError: %s\n' "$*" >&2; exit 1; }
has() { command -v "$1" >/dev/null 2>&1; }

repo="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
cd "$repo"

[[ "$(uname -s)" == "Darwin" ]] || fail "This script supports macOS only."
[[ "$(uname -m)" == "arm64" ]] || fail "This script supports Apple Silicon Macs only."

ram_gb=$(( $(sysctl -n hw.memsize) / 1024 / 1024 / 1024 ))
if (( ram_gb >= 32 )); then
  model_url="https://huggingface.co/lmstudio-community/gemma-4-12B-it-MLX-6bit"
  model_key="google/gemma-4-12b"
  model_note="Gemma 4 12B, MLX 6-bit (9.8 GB)"
else
  model_url="https://huggingface.co/lmstudio-community/gemma-4-12B-it-QAT-GGUF"
  model_key="google/gemma-4-12b-qat"
  model_note="Gemma 4 12B QAT, GGUF (7.2 GB)"
fi
(( ram_gb >= 24 )) || echo "Warning: this Mac has ${ram_gb} GB of RAM. The AI check needs about 24 GB or more."

step "Checking the build tools"
xcode-select -p >/dev/null 2>&1 || fail "Install the Xcode Command Line Tools first: xcode-select --install"
has brew || fail "Install Homebrew first: https://brew.sh"

if ! has cargo; then
  [[ -x "$HOME/.cargo/bin/cargo" ]] || {
    echo "Installing Rust (rustup)..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path
  }
  export PATH="$HOME/.cargo/bin:$PATH"
fi
rustup target list --installed | grep -q '^wasm32-unknown-unknown$' || rustup target add wasm32-unknown-unknown

if ! has node; then
  echo "Installing Node.js..."
  brew install node
fi
node_major=$(node -p 'process.versions.node.split(".")[0]')
(( node_major >= 22 )) || fail "Node.js 22 or newer is required (found $(node -v))."

if ! has pnpm; then
  echo "Installing pnpm..."
  corepack enable 2>/dev/null || brew install pnpm
fi

for tool in just wasm-pack; do
  has "$tool" || { echo "Installing $tool..."; brew install "$tool"; }
done

step "Building the web packages (harper.js, editor, components)"
just build-harperjs build-lint-framework build-components build-harper-editor

step "Building Harper Desktop (takes 10-20 minutes the first time)"
(
  cd harper-desktop
  pnpm install
  # No updater files: they need the official signing key, and this build never updates itself.
  pnpm tauri build -b app --config '{"bundle":{"createUpdaterArtifacts":false}}' --target aarch64-apple-darwin
)
app="$repo/harper-desktop/src-tauri/target/aarch64-apple-darwin/release/bundle/macos/Harper.app"
[[ -d "$app" ]] || fail "The build finished, but $app does not exist."
echo "Built: $app"

if $with_harper_ls; then
  step "Installing harper-ls"
  cargo install --locked --path harper-ls
fi

if $setup_model; then
  step "Setting up LM Studio and the model: $model_note"
  lms="$HOME/.lmstudio/bin/lms"
  if [[ ! -x "$lms" ]]; then
    [[ -d "/Applications/LM Studio.app" ]] || brew install --cask lm-studio
    echo "Starting LM Studio once, so that it installs its command-line tool..."
    open -a "LM Studio"
    for _ in $(seq 1 60); do [[ -x "$lms" ]] && break; sleep 2; done
    [[ -x "$lms" ]] || fail "LM Studio did not install $lms. Open LM Studio once, then run this script again."
  fi
  "$lms" get "$model_url" --yes
  "$lms" server start
  "$lms" load "$model_key" --context-length 8192 --yes || echo "Could not load $model_key now. LM Studio loads it on the first request."
fi

if $build_only; then
  step "Done (build only). The app is at: $app"
  exit 0
fi

step "Installing to /Applications"
osascript -e 'quit app "Harper"' >/dev/null 2>&1 || true
for _ in $(seq 1 20); do pgrep -f "Harper.app/Contents/MacOS/harper-desktop" >/dev/null || break; sleep 0.5; done
pkill -f "Harper.app/Contents/MacOS/harper-desktop" 2>/dev/null || true

if [[ -d /Applications/Harper.app ]]; then
  backup="$(mktemp -d)/Harper.app"
  mv /Applications/Harper.app "$backup"
  echo "The previous Harper.app is at $backup (deleted when macOS cleans temporary files)."
fi
cp -R "$app" /Applications/Harper.app

if $enable_llm; then
  step "Turning on the AI check"
  model="${HARPER_LLM_MODEL:-}"
  $setup_model && model="$model_key"
  [[ -n "$model" ]] || fail "--enable-llm needs --setup-model or HARPER_LLM_MODEL."
  config="$HOME/Library/Application Support/harper-desktop/config.json"
  if [[ -f "$config" ]]; then
    python3 -I - "$config" "$model" <<'PY'
import json, sys
path, model = sys.argv[1], sys.argv[2]
with open(path) as file:
    config = json.load(file)
llm = config.setdefault("llm", {})
llm.update({"enabled": True, "model": model, "idleMs": 600, "completeSentencesOnly": False})
with open(path, "w") as file:
    json.dump(config, file)
print(f"AI check on, model {model}.")
PY
  else
    echo "No Harper settings yet. Open Harper once, finish the setup, then run: scripts/install-llm-harper.sh --enable-llm"
  fi
fi

open /Applications/Harper.app

step "Done"
cat <<EOF
Next steps:
  1. If macOS asks, give Harper the Accessibility permission
     (System Settings > Privacy & Security > Accessibility). After a rebuild you may need
     to turn it off and on again.
  2. Harper > Settings > General > "AI check (prototype)": turn it on, set the model to
     "$model_key", and click "Test the connection".
  3. Keep LM Studio running with its server started (LM Studio > Developer).
To update later: git pull, then run this script again.
EOF
