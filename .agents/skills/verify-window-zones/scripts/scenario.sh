#!/usr/bin/env bash
# Replays the recorded verification scenario for window_zones through wzv (see ../SKILL.md).
set -euo pipefail

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
WZV="$HERE/wzv"
ROOT="${WZV_ROOT:-$(cd -- "$HERE/../../../.." && pwd)/target/verify-window-zones}"

usage() {
    cat <<'EOF'
Usage: scenario.sh [--run-id PREFIX]

Replays the recorded scenario against owned dry-run instances (default PREFIX: scenario-<UTC time>):
  PREFIX-s1       starter config -> `run` session -> Ctrl+Alt+Left three times (half, two-thirds,
                  third) -> Ctrl+Alt+Backspace restore -> Ctrl+Alt+Shift+Right to the second
                  display -> save a broken config (reported, previous bindings stay active and
                  still move the window) -> save the fixed config (reloads) -> one-shot dispatch of
                  an unbound hotkey exits 1 -> quit exits 0 with no live-backend calls and no lock
                  -> cleanup --dry-run changes nothing -> cleanup keeps the evidence.
  PREFIX-failed   failed attempt: TUI dispatch, cleanup --dry-run on a live run kills nothing, a
                  wrong expectation exits nonzero, cleanup after the failure stops only owned
                  processes and keeps evidence, a second cleanup is a no-op, and a recorded pid that
                  now belongs to another process is refused.
Writes PASS/FAIL per step to stdout and to $WZV_ROOT/PREFIX.log; exits 0 only if every step passes.
Evidence: $WZV_ROOT/PREFIX-s1/evidence/ and $WZV_ROOT/PREFIX-failed/evidence/.
EOF
}

PREFIX="scenario-$(date -u +%Y%m%d-%H%M%S)"
while (($#)); do
    case $1 in
        -h | --help) usage; exit 0 ;;
        --run-id)
            [[ $# -ge 2 ]] || { echo 'scenario.sh: --run-id needs a value' >&2; exit 2; }
            PREFIX=$2
            shift 2
            ;;
        *) echo "scenario.sh: unknown argument '$1' (see --help)" >&2; exit 2 ;;
    esac
done
S1=$PREFIX-s1
F=$PREFIX-failed
FAKE=$PREFIX-foreign
STARTED=''
FOREIGN_PID=''
FAKE_CREATED=0

field() { sed -n "s/^$1=//p"; }

step() {
    local name=$1
    shift
    printf '\n== %s\n' "$name"
    if "$@"; then
        printf 'PASS %s\n' "$name"
    else
        printf 'FAIL %s\n' "$name"
        exit 1
    fi
}

on_exit() {
    local rc=$?
    trap - EXIT
    if ((rc != 0)); then
        printf '\nRESULT: FAIL (exit %s); cleaning up the runs this scenario started, keeping evidence\n' "$rc"
        for run in $STARTED; do "$WZV" cleanup "$run" || true; done
    fi
    [[ -z $FOREIGN_PID ]] || kill "$FOREIGN_PID" 2>/dev/null || true
    ((FAKE_CREATED == 0)) || rm -rf -- "${ROOT:?}/$FAKE"
    exit "$rc"
}

doctor_to() { "$WZV" doctor "$1" | tee "$2"; }

stop_clean() {
    local out
    out=$("$WZV" stop "$1") || { printf '%s\n' "$out"; return 1; }
    printf '%s\n' "$out"
    [[ $(field live_backend_tool_calls <<<"$out") == 0 &&
        $(field instance_locks <<<"$out") == 0 &&
        $(field home_files <<<"$out") == 1 ]]
}

dry_run_noop() {
    local out scratch=$ROOT/$1/scratch
    out=$("$WZV" cleanup --dry-run "$1")
    printf '%s\n' "$out"
    [[ $out == *"would-remove $scratch"* && -d $scratch ]]
}

cleaned_keeps_evidence() {
    local run=$1 file
    shift
    "$WZV" cleanup "$run"
    [[ ! -e $ROOT/$run/scratch ]] || { echo "scratch still present"; return 1; }
    for file in "$@"; do
        [[ -s $ROOT/$run/evidence/$file ]] || { echo "missing evidence: $file"; return 1; }
    done
    echo "evidence kept: $*"
}

live_dry_run() {
    local out
    out=$("$WZV" cleanup --dry-run "$1")
    printf '%s\n' "$out"
    [[ $out == *"would-kill app pid=$2"* && $out == *"would-remove "* ]] || return 1
    kill -0 "$2" || { echo "app pid $2 died during a dry run"; return 1; }
    [[ -d $ROOT/$1/scratch ]] || { echo "scratch removed during a dry run"; return 1; }
    "$WZV" doctor "$1" >/dev/null || { echo "doctor fails after a dry run"; return 1; }
    echo "after --dry-run: app pid $2 alive, scratch present, doctor ok"
}

wrong_expectation() {
    local err
    if err=$("$WZV" send "$1" 'dispatch ctrl+alt+right' --expect 'Last action: alt+ctrl+up' --timeout 2 2>&1 >/dev/null); then
        echo 'a wrong expectation passed'
        return 1
    fi
    printf '%s\n' "$err"
    [[ $err == *'expected text not seen within 2s: Last action: alt+ctrl+up'* ]]
}

cleanup_after_failure() {
    local out evidence=$ROOT/$1/evidence
    out=$("$WZV" cleanup "$1")
    printf '%s\n' "$out"
    [[ $out == *'killed holder'* ]] || return 1
    if kill -0 "$2" 2>/dev/null; then echo "app pid $2 still alive"; return 1; fi
    [[ ! -e $ROOT/$1/scratch ]] || { echo 'scratch still present'; return 1; }
    [[ -s $evidence/app.log && -f $evidence/exit-code ]] || { echo 'evidence missing'; return 1; }
    grep -q '^\[expect FAIL\]' "$evidence/transcript.log" || { echo 'failed expectation not in transcript'; return 1; }
    echo "app pid $2 gone (exit code $(cat "$evidence/exit-code")), scratch removed, evidence kept"
}

second_cleanup_noop() {
    local out
    out=$("$WZV" cleanup "$1")
    printf '%s\n' "$out"
    [[ $out == *already-removed* && $out != *killed* ]]
}

foreign_pid_refused() {
    local out
    [[ ! -e $ROOT/$FAKE ]] || { echo "$ROOT/$FAKE already exists"; return 1; }
    sleep 600 </dev/null >/dev/null 2>&1 &
    FOREIGN_PID=$!
    mkdir -p "$ROOT/$FAKE/evidence"
    FAKE_CREATED=1
    printf 'RUN_ID=%s\n' "$FAKE" >"$ROOT/$FAKE/meta.env"
    printf 'app %s Thu Jan 1 00:00:00 1970\n' "$FOREIGN_PID" >"$ROOT/$FAKE/pids"
    out=$("$WZV" cleanup "$FAKE")
    printf '%s\n' "$out"
    [[ $out == *"skip-not-owned app pid=$FOREIGN_PID"* ]] || return 1
    kill -0 "$FOREIGN_PID" || { echo "foreign pid $FOREIGN_PID was killed"; return 1; }
    echo "foreign pid $FOREIGN_PID untouched; the scenario now stops it and removes the fake run"
    kill "$FOREIGN_PID"
    wait "$FOREIGN_PID" 2>/dev/null || true
    FOREIGN_PID=''
    rm -rf -- "${ROOT:?}/$FAKE"
    FAKE_CREATED=0
}

main() {
    local out ev1 pid
    printf 'scenario %s on %s, commit %s\n' "$PREFIX" "$(uname -sm)" "$(git -C "$HERE" rev-parse HEAD 2>/dev/null || echo none)"

    printf '\n== start %s (run mode, starter config)\n' "$S1"
    STARTED="$STARTED $S1"
    out=$("$WZV" start --run-id "$S1")
    printf '%s\n' "$out"
    ev1=$(field evidence_dir <<<"$out")
    step 'doctor: owned dry-run instance is ready' doctor_to "$S1" "$ev1/doctor-start.txt"
    step 'status lists the 18 starter bindings' \
        "$WZV" send "$S1" status --expect 'binding count: 18' --expect 'config state: Loaded'
    step 'Ctrl+Alt+Left moves the focused window to the left half' \
        "$WZV" send "$S1" 'dispatch ctrl+alt+left' --expect 'last move: x=0 y=0 w=960 h=1080'
    step 'Ctrl+Alt+Left again cycles to left two-thirds' \
        "$WZV" send "$S1" 'dispatch ctrl+alt+left' --expect 'last move: x=0 y=0 w=1280 h=1080'
    step 'Ctrl+Alt+Left again cycles to left third' \
        "$WZV" send "$S1" 'dispatch ctrl+alt+left' --expect 'last move: x=0 y=0 w=640 h=1080'
    step 'Ctrl+Alt+Backspace restores the geometry before the first App move' \
        "$WZV" send "$S1" 'dispatch ctrl+alt+backspace' --expect 'last move: x=40 y=40 w=640 h=480'
    step 'Ctrl+Alt+Shift+Right moves it, scaled, to the second display' \
        "$WZV" send "$S1" 'dispatch ctrl+alt+shift+right' --expect 'last move: x=1947 y=38 w=427 h=455'
    { cat "$ev1/config-00-init.toml"; echo 'this is not valid toml'; } >"$ROOT/$S1/scratch/broken.toml"
    step 'saving a broken config is reported and refused' \
        "$WZV" config "$S1" "$ROOT/$S1/scratch/broken.toml" \
        --expect 'Config reload error' --expect 'the previous bindings remain active'
    step 'the previous bindings still move the window (right half of the second display)' \
        "$WZV" send "$S1" 'dispatch ctrl+alt+right' --expect 'last move: x=2560 y=0 w=640 h=1024'
    step 'saving the fixed config reloads it' \
        "$WZV" config "$S1" "$ev1/config-00-init.toml" --expect 'Config state: Loaded (18 bindings)'
    step 'one-shot dispatch of an unbound hotkey exits 1' \
        "$WZV" exec "$S1" --expect-exit 1 --expect 'no binding configured for hotkey' -- dispatch ctrl+alt+f9
    step 'quit exits 0; no live-backend call, no lock, only the config file in HOME' stop_clean "$S1"
    step 'cleanup --dry-run plans the removal and changes nothing' dry_run_noop "$S1"
    step 'cleanup removes scratch and keeps the evidence' cleaned_keeps_evidence "$S1" \
        app.log transcript.log side-effects.txt exit-code doctor-start.txt \
        config-00-init.toml config-01.toml config-02.toml

    printf '\n== start %s (tui mode) for the failed attempt\n' "$F"
    STARTED="$STARTED $F"
    out=$("$WZV" start --run-id "$F" --mode tui --no-build)
    printf '%s\n' "$out"
    pid=$(field pid <<<"$out")
    step 'TUI dispatch updates the dashboard' \
        "$WZV" send "$F" 'dispatch ctrl+alt+left' --expect 'Last action: alt+ctrl+left'
    step 'cleanup --dry-run on a live run kills and removes nothing' live_dry_run "$F" "$pid"
    step 'a wrong expectation exits nonzero and names the missing text' wrong_expectation "$F"
    step 'the failed attempt left an owned live instance' "$WZV" doctor "$F"
    step 'cleanup after the failure stops only owned processes and keeps evidence' \
        cleanup_after_failure "$F" "$pid"
    step 'a second cleanup is a no-op' second_cleanup_noop "$F"
    step 'cleanup refuses a recorded pid that now belongs to another process' foreign_pid_refused

    printf '\nRESULT: PASS\nevidence: %s\nevidence: %s\nlog: %s\n' \
        "$ROOT/$S1/evidence" "$ROOT/$F/evidence" "$ROOT/$PREFIX.log"
}

mkdir -p "$ROOT"
[[ ! -e $ROOT/$PREFIX.log ]] || { echo "scenario.sh: $ROOT/$PREFIX.log exists; pick another --run-id" >&2; exit 2; }
exec > >(tee "$ROOT/$PREFIX.log") 2>&1
trap on_exit EXIT
main
