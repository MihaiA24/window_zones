#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
BIN_PATH="${WINDOW_ZONES_BINARY:-}"
TARGET_DIR=''
# Keep the build toolchain available after gates isolate the application's HOME.
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"

KEEP_LOGS=0
COMMAND=""
for arg in "$@"; do
    case "$arg" in
        --keep-logs) KEEP_LOGS=1 ;;
        gnome|gnome-live|kde-live|x11|all)
            if [[ -n "$COMMAND" ]]; then
                printf 'Usage: %s [--keep-logs] {gnome|gnome-live|kde-live|x11|all}\n' "${BASH_SOURCE[0]}" >&2
                exit 2
            fi
            COMMAND="$arg"
            ;;
        -h|--help)
            printf 'Usage: %s [--keep-logs] {gnome|gnome-live|kde-live|x11|all}\n' "${BASH_SOURCE[0]}"
            exit 0
            ;;
        *)
            printf 'Unknown argument: %s\nUsage: %s [--keep-logs] {gnome|gnome-live|kde-live|x11|all}\n' "$arg" "${BASH_SOURCE[0]}" >&2
            exit 2
            ;;
    esac
done
if [[ -z "$COMMAND" ]]; then
    printf 'Usage: %s [--keep-logs] {gnome|gnome-live|kde-live|x11|all}\n' "${BASH_SOURCE[0]}" >&2
    exit 2
fi

if [[ -z "$BIN_PATH" ]]; then
    # Let Cargo resolve relative environment/config paths from the same cwd as the build.
    if ! TARGET_DIR=$(cargo metadata --locked --no-deps --format-version 1 \
        --manifest-path "$PROJECT_ROOT/Cargo.toml" \
        | /usr/bin/python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])'); then
        printf 'Failed to resolve Cargo target directory.\n' >&2
        exit 1
    fi
    BIN_PATH="$TARGET_DIR/debug/window_zones"
fi

umask 077
TMP_DIR="${WINDOW_ZONES_SMOKE_ROOT:-/tmp/window_zones-smoke.$$}"
if [[ "$TMP_DIR" != /* ]] || ! mkdir -m 700 -- "$TMP_DIR"; then
    printf 'Smoke root must be a new absolute directory: %s\n' "$TMP_DIR" >&2
    exit 1
fi
ASSERT_LOG="$TMP_DIR/assertions.log"
: >"$ASSERT_LOG"
PID_FILE="$TMP_DIR/pids"
: >"$PID_FILE"
export WINDOW_ZONES_SMOKE_OWNER="$TMP_DIR"
FAILURES=0
ASSERTIONS=0
SKIPPED=0
CURRENT_GATE=""
LAST_SHOT=""
GNOME_LIVE_ACTIVE=0
GNOME_LIVE_RESTORE_FAILURE=0

# Modules declare functions only; this entry owns initialization, shell options and traps.
source "$SCRIPT_DIR/smoke/assertions.sh"
source "$SCRIPT_DIR/smoke/waits.sh"
source "$SCRIPT_DIR/smoke/processes.sh"
source "$SCRIPT_DIR/smoke/geometry.sh"
source "$SCRIPT_DIR/smoke/config.sh"
source "$SCRIPT_DIR/smoke/tui.sh"
source "$SCRIPT_DIR/smoke/seam.sh"
source "$SCRIPT_DIR/smoke/gnome-native.sh"
source "$SCRIPT_DIR/smoke/gnome-live/observations.sh"
source "$SCRIPT_DIR/smoke/gnome-live/hotkeys.sh"
source "$SCRIPT_DIR/smoke/gnome-live/lifecycle.sh"
source "$SCRIPT_DIR/smoke/gnome-live/placement.sh"
source "$SCRIPT_DIR/smoke/gnome-live/app.sh"
source "$SCRIPT_DIR/smoke/kwin/session.sh"
source "$SCRIPT_DIR/smoke/kwin/observations.sh"
source "$SCRIPT_DIR/smoke/kwin/placement.sh"
source "$SCRIPT_DIR/smoke/kwin/hotkeys.sh"
source "$SCRIPT_DIR/smoke/kwin/runner.sh"
source "$SCRIPT_DIR/smoke/x11/observations.sh"
source "$SCRIPT_DIR/smoke/x11/placement.sh"
source "$SCRIPT_DIR/smoke/x11/runner.sh"
install_smoke_traps
owned_process require

main() {
    if [[ "$COMMAND" == kde-live ]]; then
        run_kde_live || true
    fi
    if [[ "$COMMAND" == gnome-live ]]; then
        if build_binaries 'GNOME live'; then
            run_gnome_live || true
        fi
    fi
    if [[ "$COMMAND" == gnome || "$COMMAND" == all ]]; then
        if build_binaries GNOME; then
            run_gnome || true
        fi
    fi
    if [[ "$COMMAND" == x11 || "$COMMAND" == all ]]; then
        if build_binaries X11; then
            run_x11 || true
        fi
    fi
    log "Smoke assertions ($COMMAND): $ASSERTIONS; passed: $((ASSERTIONS - FAILURES - SKIPPED)); failed: $FAILURES; skipped: $SKIPPED"
    log "Assertion log: $ASSERT_LOG"
    if [[ "$KEEP_LOGS" -eq 0 ]]; then
        log 'Logs removed (rerun with --keep-logs to retain temporary evidence)'
    fi
    [[ "$FAILURES" -eq 0 ]]
}

main
