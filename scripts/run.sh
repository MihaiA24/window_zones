#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"

RUN_ARGS=("$@")
if [[ ${#RUN_ARGS[@]} -eq 0 ]]; then
  RUN_ARGS=(--backend dry-run status)
fi

if [[ -n "${WINDOW_ZONES_BINARY:-}" ]]; then
  BIN_PATH="${WINDOW_ZONES_BINARY}"
elif [[ -x "$PROJECT_ROOT/target/release/window_zones" ]]; then
  BIN_PATH="$PROJECT_ROOT/target/release/window_zones"
elif [[ -x "$PROJECT_ROOT/target/debug/window_zones" ]]; then
  BIN_PATH="$PROJECT_ROOT/target/debug/window_zones"
elif command -v cargo >/dev/null 2>&1; then
  cd "$PROJECT_ROOT"
  exec cargo run --locked --release -- "${RUN_ARGS[@]}"
else
  echo "No binary available and cargo is not installed." >&2
  exit 1
fi

if [[ ! -x "$BIN_PATH" ]]; then
  echo "No usable window_zones binary found at $BIN_PATH" >&2
  exit 1
fi

exec "$BIN_PATH" "${RUN_ARGS[@]}"
