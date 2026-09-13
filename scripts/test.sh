#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"

cd "$PROJECT_ROOT"
bash -n "$PROJECT_ROOT"/scripts/*.sh
node --check "$PROJECT_ROOT/gnome-extension/extension.js"
node --check "$PROJECT_ROOT/kwin-script/contents/code/main.js"
# node accepts syntax QJSEngine rejects (object spread); qmllint is Qt's own parser.
if command -v qmllint >/dev/null 2>&1; then
  qmllint "$PROJECT_ROOT/kwin-script/contents/code/main.js"
fi

cargo fmt --all -- --check
cargo test --locked
cargo test --locked --doc

if command -v cargo-clippy >/dev/null 2>&1; then
  cargo clippy --locked --all-targets --all-features -- -D warnings
else
  echo "cargo-clippy not installed. Skipping clippy checks."
fi
