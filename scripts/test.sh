#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"

cd "$PROJECT_ROOT"
find "$SCRIPT_DIR" -type f -name '*.sh' -print0 |
  while IFS= read -r -d '' script; do
    bash -n "$script"
  done
if [[ "$(uname -s)" == Linux ]]; then
  bash "$PROJECT_ROOT/scripts/test-install.sh"
else
  echo "Skipping Linux-host installer fixtures on $(uname -s)."
fi
node --check "$PROJECT_ROOT/gnome-extension/extension.js"
if command -v gjs >/dev/null 2>&1; then
  gjs -m "$PROJECT_ROOT/gnome-extension/tests/run.js"
fi
node "$PROJECT_ROOT/kwin-script/tests/run.js"
find "$SCRIPT_DIR" "$PROJECT_ROOT/src" -type f -name '*.js' -print0 |
  while IFS= read -r -d '' fixture; do
    node --input-type=module --check < "$fixture"
  done
python3 - <<'PY'
import ast
from pathlib import Path

for path in Path("scripts").rglob("*.py"):
    ast.parse(path.read_text(), filename=str(path))
PY
# node accepts syntax QJSEngine rejects (object spread); qmllint is Qt's own parser.
if command -v qmllint >/dev/null 2>&1; then
  qmllint "$PROJECT_ROOT/kwin-script/contents/code/main.js"
fi
if [[ "$(uname -s)" == Linux ]]; then
  python3 "$PROJECT_ROOT/scripts/smoke/test_helpers.py"
  python3 "$PROJECT_ROOT/.agents/skills/verify-window-zones/test_verify.py"
else
  echo "Skipping Linux-only smoke/verifier safety checks on $(uname -s)."
fi

cargo fmt --all -- --check
cargo test --locked

if command -v cargo-clippy >/dev/null 2>&1; then
  cargo clippy --locked --all-targets --all-features -- -D warnings
else
  echo "cargo-clippy not installed. Skipping clippy checks."
fi
