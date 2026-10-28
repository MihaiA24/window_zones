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
GROUP_PID_FILE="$TMP_DIR/group-pids"
: >"$PID_FILE"
: >"$GROUP_PID_FILE"
FAILURES=0
ASSERTIONS=0
SKIPPED=0
CURRENT_GATE=""
LAST_SHOT=""
GNOME_LIVE_ACTIVE=0
GNOME_LIVE_RESTORE_FAILURE=0
log() {
    printf '%s\n' "$*" | tee -a "$ASSERT_LOG"
}

assert_text() {
    local name=$1 expected=$2 observed=$3
    ASSERTIONS=$((ASSERTIONS + 1))
    if [[ "$observed" == *"$expected"* ]]; then
        log "PASS $name: expected '$expected' observed '$observed'"
    else
        FAILURES=$((FAILURES + 1))
        log "FAIL $name: expected '$expected' observed '$observed'"
    fi
}

assert_eq() {
    local name=$1 expected=$2 observed=$3 screenshot=${4:-}
    ASSERTIONS=$((ASSERTIONS + 1))
    if [[ "$expected" == "$observed" ]]; then
        log "PASS $name: expected '$expected' observed '$observed'${screenshot:+ (screenshot $screenshot)}"
    else
        FAILURES=$((FAILURES + 1))
        log "FAIL $name: expected '$expected' observed '$observed'${screenshot:+ (screenshot $screenshot)}"
    fi
}

assert_rc_zero() {
    local name=$1 rc=$2 observed=$3
    ASSERTIONS=$((ASSERTIONS + 1))
    if [[ "$rc" -eq 0 ]]; then
        log "PASS $name: expected exit 0 observed exit $rc${observed:+ ($observed)}"
    else
        FAILURES=$((FAILURES + 1))
        log "FAIL $name: expected exit 0 observed exit $rc${observed:+ ($observed)}"
    fi
}

assert_rc_nonzero() {
    local name=$1 rc=$2 expected=$3 observed=$4
    ASSERTIONS=$((ASSERTIONS + 1))
    if [[ "$rc" -ne 0 ]]; then
        log "PASS $name: expected $expected observed exit $rc${observed:+ ($observed)}"
    else
        FAILURES=$((FAILURES + 1))
        log "FAIL $name: expected $expected observed exit $rc${observed:+ ($observed)}"
    fi
}

skip_assert() {
    local name=$1 reason=$2
    ASSERTIONS=$((ASSERTIONS + 1))
    SKIPPED=$((SKIPPED + 1))
    log "SKIP $name: $reason"
}

build_binaries() {
    local gate=$1
    shift
    if [[ -n "${WINDOW_ZONES_BINARY:-}" ]]; then
        assert_eq "$gate pinned binary is executable" executable \
            "$([[ -x "$BIN_PATH" ]] && printf executable || printf missing)"
        [[ -x "$BIN_PATH" ]]
        return
    fi
    local output='' rc=0
    if output=$(cargo build --locked --manifest-path "$PROJECT_ROOT/Cargo.toml" \
        --bin window_zones "$@" 2>&1); then
        rc=0
    else
        rc=$?
    fi
    printf '%s\n' "$output" >>"$ASSERT_LOG"
    assert_rc_zero "$gate cargo build" "$rc" "$BIN_PATH"
    return "$rc"
}

file_contains() {
    local file=$1 needle=$2
    [[ -f "$file" ]] && [[ "$(<"$file")" == *"$needle"* ]]
}

wait_for_text() {
    local file=$1 needle=$2 timeout_s=${3:-30}
    local deadline=$((SECONDS + timeout_s))
    while (( SECONDS < deadline )); do
        if file_contains "$file" "$needle"; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

count_text() {
    local file=$1 needle=$2 count=''
    count=$(grep -c -- "$needle" "$file" 2>/dev/null)
    printf '%s' "${count:-0}"
}

wait_for_new_text() {
    # A status line only proves anything if it was printed after the event under test, so wait for
    # an occurrence beyond the ones already in the log.
    local file=$1 needle=$2 before=$3 timeout_s=${4:-10}
    local deadline=$((SECONDS + timeout_s))
    while (( SECONDS < deadline )); do
        if (( $(count_text "$file" "$needle") > before )); then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

sorted_hotkey() {
    # Modifier order is the App's canonical choice, not the harness's business; compare token sets.
    printf '%s' "$1" | tr '+' '\n' | sed '/^$/d' | sort | tr '\n' '+' | sed 's/+$//'
}

wait_for_reported_action() {
    local file=$1 label=$2 timeout_s=${3:-10}
    local deadline=$((SECONDS + timeout_s)) value=''
    while (( SECONDS < deadline )); do
        value=$(sed -n "s/^ *$label//p" "$file" 2>/dev/null | grep -v '<none>' | tail -1)
        if [[ -n "$value" ]]; then
            break
        fi
        sleep 0.2
    done
    printf '%s' "$value"
}

wait_for_file() {
    local file=$1 timeout_s=${2:-30}
    local deadline=$((SECONDS + timeout_s))
    while (( SECONDS < deadline )); do
        if [[ -s "$file" ]]; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

wait_for_size_growth() {
    local file=$1 old_size=$2 timeout_s=${3:-10}
    local deadline=$((SECONDS + timeout_s)) size
    while (( SECONDS < deadline )); do
        size=$(stat -c '%s' "$file" 2>/dev/null || printf '0')
        if (( size > old_size )); then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

start_group() {
    local pid_variable=$1 log_file=$2
    shift 2
    # Assign in this shell: command substitution would orphan the child.
    setsid -- "$@" >"$log_file" 2>&1 &
    local child_pid=$!
    printf -v "$pid_variable" '%s' "$child_pid"
    printf '%s\n' "$child_pid" >>"$PID_FILE"
    printf '%s\n' "$child_pid" >>"$GROUP_PID_FILE"
}

start_plain() {
    local log_file=$1
    shift
    "$@" >"$log_file" 2>&1 &
    local pid=$!
    printf '%s\n' "$pid" >>"$PID_FILE"
    printf '%s\n' "$pid"
}

stop_pid() {
    local pid=$1
    [[ -n "$pid" ]] && kill "$pid" 2>/dev/null || true
}

wait_pid() {
    local pid=$1 timeout_s=${2:-10} state
    local deadline=$((SECONDS + timeout_s))
    while kill -0 "$pid" 2>/dev/null && (( SECONDS < deadline )); do
        state=$(ps -o stat= -p "$pid" 2>/dev/null || true)
        [[ "$state" == Z* ]] && break
        sleep 0.1
    done
    state=$(ps -o stat= -p "$pid" 2>/dev/null || true)
    if kill -0 "$pid" 2>/dev/null && [[ "$state" != Z* ]]; then
        return 124
    fi
    # The initial KWin service is an owned descendant of the launcher, not our child.
    [[ "${3:-child}" == external ]] && return 0
    if wait "$pid"; then
        return 0
    else
        return "$?"
    fi
}

cleanup() {
    local pid
    set +e
    while read -r pid; do
        [[ -n "$pid" ]] || continue
        kill -- -"$pid" 2>/dev/null || true
        kill "$pid" 2>/dev/null || true
    done <"$GROUP_PID_FILE"
    while read -r pid; do
        [[ -n "$pid" ]] || continue
        kill "$pid" 2>/dev/null || true
    done <"$PID_FILE"
    sleep 0.2
    while read -r pid; do
        [[ -n "$pid" ]] || continue
        kill -KILL "$pid" 2>/dev/null || true
    done <"$PID_FILE"
    if [[ "$KEEP_LOGS" -eq 0 ]]; then
        rm -rf "$TMP_DIR"
    else
        printf 'Logs retained in %s\n' "$TMP_DIR"
    fi
}

gnome_live_exit_trap() {
    local status=$?
    trap - EXIT INT TERM
    set +e
    if [[ "$GNOME_LIVE_ACTIVE" -eq 1 ]]; then
        restore_gnome_live || GNOME_LIVE_RESTORE_FAILURE=1
    fi
    cleanup
    if [[ "$GNOME_LIVE_RESTORE_FAILURE" -ne 0 ]]; then
        status=1
    fi
    exit "$status"
}
trap cleanup EXIT INT TERM

record_versions_common() {
    local distro='unknown'
    if [[ -r /etc/os-release ]]; then
        # shellcheck disable=SC1091
        . /etc/os-release
        distro=${PRETTY_NAME:-$NAME}
    fi
    log "VERSION kernel: $(uname -srmo)"
    log "VERSION distro: $distro"
}

record_versions_gnome() {
    record_versions_common
    log "VERSION gnome-shell: $(gnome-shell --version 2>&1)"
    log "VERSION gnome-text-editor: $(gnome-text-editor --version 2>&1 | sed -n '1p')"
}

record_versions_x11() {
    record_versions_common
    log "VERSION Xorg: $(/usr/lib/Xorg -version 2>&1 | sed -n '/X.Org X Server/p' | sed -n '1p')"
    log "VERSION Xvfb: $(pacman -Qi xorg-server 2>/dev/null | sed -n '/^Version/p' | sed 's/^ *//')"
    log "VERSION openbox: $(openbox --version 2>&1 | sed -n '1p')"
    log "VERSION xdotool: $(xdotool --version 2>&1 | sed -n '1p')"
}

write_valid_config() {
    cat >"$CONFIG_PATH" <<'EOF'
[[bindings]]
hotkey = "Alt+Ctrl+Left"
action = { type = "move-to-zone", zone = "left-half" }

[[bindings]]
hotkey = "Alt+Ctrl+Up"
action = { type = "move-to-zone", zone = "center-third" }

[[bindings]]
hotkey = "Alt+Ctrl+Right"
action = { type = "move-to-zone", zone = "left-two-thirds" }

[[bindings]]
hotkey = "Alt+Ctrl+Down"
action = { type = "move-to-next-display" }
EOF
}
write_five_bindings_config() {
    write_valid_config
    cat >>"$CONFIG_PATH" <<'EOF'

[[bindings]]
hotkey = "Alt+Ctrl+Shift+Right"
action = { type = "move-to-previous-display" }
EOF
}

write_invalid_config() {
    cat >"$CONFIG_PATH" <<'EOF'
[[bindings]]
hotkey = "Alt+Ctrl+Left"
action = { type = "move-to-zone", zone = "left-half" }
this is not valid toml
EOF
}

parse_gnome_displays() {
    local raw=$1
    python - "$raw" <<'PY'
import re, sys
s = sys.argv[1]
# gdbus prints `uint32` only on the first value of a type in a container, so later tuples omit it.
for item in re.findall(r"\('([^']+)',\s*(-?\d+),\s*(-?\d+),\s*(?:uint32\s+)?(\d+),\s*(?:uint32\s+)?(\d+)\)", s):
    print('|'.join(item))
PY
}

parse_gnome_focused() {
    local raw=$1
    python - "$raw" <<'PY'
import re, sys
s = sys.argv[1]
m = re.search(r"\((true|false),\s*(?:uint64\s+)?(\d+),\s*(-?\d+),\s*(-?\d+),\s*(?:uint32\s+)?(\d+),\s*(?:uint32\s+)?(\d+)\)", s)
if m:
    print('|'.join(m.groups()))
PY
}

find_last_window() {
    local id
    WINDOW_ID=''
    while read -r id; do
        [[ -n "$id" ]] && WINDOW_ID="$id"
    done < <(xdotool search --name 'Text Editor' 2>/dev/null || true)
    [[ -n "$WINDOW_ID" ]]
}

x11_geometry() {
    local geometry key value x='' y='' width='' height=''
    local extents left=0 right=0 top=0 bottom=0
    geometry=$(xdotool getwindowgeometry --shell "$WINDOW_ID" 2>/dev/null || true)
    while IFS='=' read -r key value; do
        case "$key" in
            X) x=$value ;;
            Y) y=$value ;;
            WIDTH) width=$value ;;
            HEIGHT) height=$value ;;
        esac
    done <<<"$geometry"
    extents=$(xprop -id "$WINDOW_ID" _NET_FRAME_EXTENTS 2>/dev/null || true)
    if [[ "$extents" =~ =[[:space:]]*([0-9]+),[[:space:]]*([0-9]+),[[:space:]]*([0-9]+),[[:space:]]*([0-9]+) ]]; then
        left=${BASH_REMATCH[1]}
        right=${BASH_REMATCH[2]}
        top=${BASH_REMATCH[3]}
        bottom=${BASH_REMATCH[4]}
    fi
    if [[ "$x" =~ ^-?[0-9]+$ && "$y" =~ ^-?[0-9]+$ \
        && "$width" =~ ^[0-9]+$ && "$height" =~ ^[0-9]+$ ]]; then
        # xdotool reports client root geometry; all gate targets describe the frame.
        printf '%s,%s,%s,%s' "$((x - left))" "$((y - top))" \
            "$((width + left + right))" "$((height + top + bottom))"
    else
        printf 'unavailable'
    fi
}
x11_geometry_stable() {
    local timeout_s=${1:-5} observed previous='' stable=0
    local deadline=$((SECONDS + timeout_s))
    while (( SECONDS < deadline )); do
        observed=$(x11_geometry)
        if [[ "$observed" != 'unavailable' && "$observed" == "$previous" ]]; then
            stable=$((stable + 1))
            if (( stable >= 5 )); then
                return 0
            fi
        else
            stable=0
        fi
        previous="$observed"
        sleep 0.2
    done
    return 1
}

x11_inject_hotkey() {
    local key=$1 output='' rc=0
    if output=$(DISPLAY="$X11_DISPLAY" xdotool key --clearmodifiers "alt+ctrl+$key" 2>&1); then
        rc=0
    else
        rc=$?
    fi
    if [[ -n "$output" ]]; then
        log "X11 XTEST hotkey alt+ctrl+$key: exit $rc output '$output'"
    else
        log "X11 XTEST hotkey alt+ctrl+$key: exit $rc"
    fi
    return "$rc"
}

x11_hotkey_diagnostics() {
    local run_log=$1 app_tail='' app_display='' line
    log 'X11 real-hotkey diagnostics:'
    app_display=$(tr '\0' '\n' <"/proc/$APP_PID/environ" 2>/dev/null | sed -n '/^DISPLAY=/p' | sed -n '1p' || true)
    log "  App environment: ${app_display:-DISPLAY unavailable}"
    app_tail=$(tail -n 80 "$run_log" 2>/dev/null || true)
    log '  App stdout/stderr tail:'
    while IFS= read -r line; do
        log "    $line"
    done <<<"$app_tail"
    if [[ "$app_tail" == *'Hotkey registration refused:'* ]]; then
        log '  XGrabKey registration was refused; check for another program owning a configured accelerator'
    elif [[ "$app_tail" == *'Hotkey registration failed:'* ]]; then
        log '  XGrabKey listener is unavailable; check the App DISPLAY and X server access'
    else
        log '  No XGrabKey registration error observed in App output'
    fi
    # A second App would contend for the instance lock and exclusive grabs, so
    # inspect the owning App's status/log rather than launching a diagnostic TUI.
}

wait_x11_geometry() {
    local expected=$1 timeout_s=${2:-8} observed
    local deadline=$((SECONDS + timeout_s))
    while (( SECONDS < deadline )); do
        observed=$(x11_geometry)
        if [[ "$observed" == "$expected" ]]; then
            printf '%s' "$observed"
            return 0
        fi
        sleep 0.1
    done
    printf '%s' "$(x11_geometry)"
    return 1
}

snapshot_x11() {
    local label=$1
    local path="$TMP_DIR/${CURRENT_GATE}-${label}.png"
    LAST_SHOT=''
    if timeout 5 import -display "$X11_DISPLAY" -window "$WINDOW_ID" "$path" >/dev/null 2>&1 && [[ -s "$path" ]]; then
        LAST_SHOT="$path"
    elif command -v ffmpeg >/dev/null 2>&1 \
        && timeout 5 env DISPLAY="$X11_DISPLAY" ffmpeg -loglevel error -f x11grab -video_size 3520x1080 -i "$X11_DISPLAY.0" -frames:v 1 "$path" >/dev/null 2>&1 \
        && [[ -s "$path" ]]; then
        LAST_SHOT="$path"
    fi
    if [[ -n "$LAST_SHOT" ]]; then
        log "SCREENSHOT $label: $LAST_SHOT"
    else
        log "SCREENSHOT $label: unavailable (import/ffmpeg failed)"
    fi
}

x11_assert_geometry() {
    local name=$1 expected=$2 observed
    observed=$(wait_x11_geometry "$expected" 8 || true)
    snapshot_x11 "$name"
    assert_eq "$name" "$expected" "$observed" "$LAST_SHOT"
    if [[ -z "$LAST_SHOT" ]]; then
        log "Harness limitation $name screenshot: no usable screenshot tool"
    fi
}

xon() {
    # Run an X client on the gate's display: $XON_DISPLAY plus $XON_XAUTHORITY when it has one.
    if [[ -n "${XON_XAUTHORITY:-}" ]]; then
        DISPLAY="$XON_DISPLAY" XAUTHORITY="$XON_XAUTHORITY" "$@"
    else
        DISPLAY="$XON_DISPLAY" "$@"
    fi
}

xwindow_for_pid() {
    # pid [timeout]: print the visible editor window that process mapped.
    local pid=$1 timeout_s=${2:-20} id='' owner=''
    local deadline=$((SECONDS + timeout_s))
    while (( SECONDS < deadline )); do
        while read -r id; do
            [[ -n "$id" ]] || continue
            owner=$(xon xdotool getwindowpid "$id" 2>/dev/null || true)
            if [[ "$owner" == "$pid" ]]; then
                printf '%s' "$id"
                return 0
            fi
        done < <(xon xdotool search --onlyvisible --name 'Text Editor' 2>/dev/null || true)
        sleep 0.2
    done
    return 1
}

wait_xwindow_gone() {
    # window [timeout]: the X window is destroyed and the window manager stopped listing it.
    local id=$1 timeout_s=${2:-10} clients='' hex
    local deadline=$((SECONDS + timeout_s))
    hex=$(printf '0x%x' "$id")
    while (( SECONDS < deadline )); do
        clients=$(xon xprop -root _NET_CLIENT_LIST 2>/dev/null || true)
        if ! xon xdotool getwindowname "$id" >/dev/null 2>&1 \
            && [[ ! "$clients" =~ (^|[^0-9a-fx])$hex([^0-9a-f]|$) ]]; then
            return 0
        fi
        sleep 0.2
    done
    return 1
}

assert_wm_state() {
    # name window needle want: want 1 = _NET_WM_STATE lists needle, 0 = it does not.
    local name=$1 window=$2 needle=$3 want=$4
    local deadline=$((SECONDS + 5)) state='' has=0
    while :; do
        state=$(xon xprop -id "$window" _NET_WM_STATE 2>&1 || true)
        [[ "$state" == *"$needle"* ]] && has=1 || has=0
        [[ "$has" == "$want" ]] && break
        (( SECONDS < deadline )) || break
        sleep 0.1
    done
    log "OBSERVED $name: $state"
    assert_eq "$name" "$want" "$has"
}

wait_observed() {
    # observer expected [timeout]: poll an observer function until it prints expected; print the last value.
    local observer=$1 expected=$2 timeout_s=${3:-8} observed=''
    local deadline=$((SECONDS + timeout_s))
    while :; do
        observed=$("$observer")
        [[ "$observed" == "$expected" ]] && break
        (( SECONDS < deadline )) || break
        sleep 0.2
    done
    printf '%s' "$observed"
}

start_fifo_app() {
    local mode=$1 backend=$2 log_file=$3
    local fifo="$TMP_DIR/${CURRENT_GATE}-${mode}.fifo"
    rm -f "$fifo"
    mkfifo "$fifo"
    env DISPLAY="${DISPLAY:-}" XDG_SESSION_TYPE="${XDG_SESSION_TYPE:-}" \
        XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-}" \
        DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-}" \
        GDK_BACKEND="${GDK_BACKEND:-}" \
        HOME="${HOME:-$TMP_DIR/home}" XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-$TMP_DIR/config}" \
        XDG_DATA_HOME="${XDG_DATA_HOME:-$TMP_DIR/data}" \
        "$BIN_PATH" --backend "$backend" --config "$CONFIG_PATH" "$mode" <"$fifo" >"$log_file" 2>&1 &
    APP_PID=$!
    printf '%s\n' "$APP_PID" >>"$PID_FILE"
    exec {APP_FD}>"$fifo"
}

send_app() {
    printf '%s\n' "$1" >&"$APP_FD"
}

close_app_input() {
    eval "exec ${APP_FD}>&-" || true
    APP_FD=''
}

stop_app_cleanly() {
    local rc=0
    if [[ -n "${APP_FD:-}" ]]; then
        send_app quit || true
        close_app_input
    fi
    if [[ -n "${APP_PID:-}" ]]; then
        wait_pid "$APP_PID" 12 || rc=$?
        APP_PID=''
    fi
    assert_rc_zero "$1" "$rc" "app log $2"
}

gnome_call() {
    DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" gdbus call --session \
        --dest org.window_zones.Gnome --object-path /org/window_zones/Gnome \
        --method "org.window_zones.Gnome2.$1" "${@:2}"
}

gnome_shell_call() {
    DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" gdbus call --session \
        --dest org.gnome.Shell --object-path "$1" --method "$2" "${@:3}"
}

gnome_live_enabled_extensions() {
    DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" gsettings get org.gnome.shell enabled-extensions 2>&1 || true
}

gnome_live_extension_info() {
    gnome_shell_call /org/gnome/Shell org.gnome.Shell.Extensions.GetExtensionInfo "$1" 2>&1 || true
}

gnome_live_wait_companion() {
    local timeout_s=${1:-20}
    local deadline=$((SECONDS + timeout_s))
    local service_list='' owner_line='' errors=''
    GNOME_LIVE_SERVICE_LIST=''
    GNOME_LIVE_EXTENSION_ERRORS=''
    while (( SECONDS < deadline )); do
        service_list=$(DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" busctl --user list 2>&1 || true)
        owner_line=$(printf '%s\n' "$service_list" | sed -n '/org.window_zones.Gnome/p' | sed -n '1p')
        if [[ -n "$owner_line" ]]; then
            errors=$(gnome_shell_call /org/gnome/Shell \
                org.gnome.Shell.Extensions.GetExtensionErrors "$GNOME_LIVE_EXT_UUID")
            if [[ "$errors" == *'[]'* ]]; then
                GNOME_LIVE_SERVICE_LIST="$owner_line"
                GNOME_LIVE_EXTENSION_ERRORS="$errors"
                return 0
            fi
        fi
        sleep 0.2
    done
    GNOME_LIVE_SERVICE_LIST="${owner_line:-<none>}"
    GNOME_LIVE_EXTENSION_ERRORS="$errors"
    return 1
}

gnome_live_wait_companion_gone() {
    local timeout_s=${1:-10}
    local deadline=$((SECONDS + timeout_s))
    while (( SECONDS < deadline )); do
        service_list=$(DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" busctl --user list 2>&1 || true)
        if [[ "$service_list" != *'org.window_zones.Gnome'* ]]; then
            return 0
        fi
        sleep 0.2
    done
    return 1
}

gtk_frame_geometry() {
    local display=$1 authority=$2
    local geometry='' key='' value='' x='' y='' width='' height=''
    if [[ -z "${WINDOW_ID:-}" ]]; then
        printf 'unavailable'
        return 0
    fi
    geometry=$(DISPLAY="$display" XAUTHORITY="$authority" \
        xdotool getwindowgeometry --shell "$WINDOW_ID" 2>/dev/null || true)
    while IFS='=' read -r key value; do
        case "$key" in
            X) x=$value ;;
            Y) y=$value ;;
            WIDTH) width=$value ;;
            HEIGHT) height=$value ;;
        esac
    done <<<"$geometry"
    # A GTK client under XWayland owns its shadow: the X window is _GTK_FRAME_EXTENTS larger on
    # every side than the frame the compositor places. Inset it so the observer and the Companion
    # describe the same rectangle.
    local extents left=0 right=0 top=0 bottom=0
    extents=$(DISPLAY="$display" XAUTHORITY="$authority" \
        xprop -id "$WINDOW_ID" _GTK_FRAME_EXTENTS 2>/dev/null | sed -n 's/.*= //p')
    if [[ "$extents" =~ ^([0-9]+),\ ([0-9]+),\ ([0-9]+),\ ([0-9]+)$ ]]; then
        left=${BASH_REMATCH[1]}
        right=${BASH_REMATCH[2]}
        top=${BASH_REMATCH[3]}
        bottom=${BASH_REMATCH[4]}
    fi
    if [[ "$x" =~ ^-?[0-9]+$ && "$y" =~ ^-?[0-9]+$ \
        && "$width" =~ ^[0-9]+$ && "$height" =~ ^[0-9]+$ ]]; then
        printf '%s,%s,%s,%s' "$((x + left))" "$((y + top))" \
            "$((width - left - right))" "$((height - top - bottom))"
    else
        printf 'unavailable'
    fi
}

gnome_live_geometry() {
    gtk_frame_geometry "$GNOME_XDISPLAY" "$GNOME_XAUTHORITY"
}

gnome_live_zone_targets() {
    # Derive the current display from the frame center; the wire id identifies the window.
    local focused present window_id x y w h id center_x center_y
    local area='' next_area='' display_lines found=-1 total=0 index=0
    focused=$(get_gnome_focused)
    IFS='|' read -r present window_id x y w h <<<"$focused"
    [[ "$present" == 'true' ]] || return 0
    center_x=$((x + w / 2))
    center_y=$((y + h / 2))
    # Match the executor's stable spatial ordering, including enumeration order on ties.
    display_lines=$(printf '%s\n' "$GNOME_LIVE_DISPLAY_LINES" | LC_ALL=C sort -s -t '|' -k2,2n -k3,3n)
    while IFS='|' read -r id x y w h; do
        [[ -n "$id" ]] && (( w > 0 && h > 0 )) || continue
        if (( found < 0 && center_x >= x && center_x < x + w \
            && center_y >= y && center_y < y + h )); then
            found=$total
            area="$x|$y|$w|$h"
            GNOME_LIVE_DISPLAY_ID="$id"
        fi
        total=$((total + 1))
    done <<<"$display_lines"
    (( found >= 0 )) || return 0
    while IFS='|' read -r id x y w h; do
        [[ -n "$id" ]] && (( w > 0 && h > 0 )) || continue
        if (( index == (found + 1) % total )); then
            next_area="$x|$y|$w|$h"
        fi
        index=$((index + 1))
    done <<<"$display_lines"
    IFS='|' read -r GNOME_LIVE_X GNOME_LIVE_Y GNOME_LIVE_W GNOME_LIVE_H <<<"$area"
    GNOME_LIVE_LEFT_HALF="$GNOME_LIVE_X,$GNOME_LIVE_Y,$((GNOME_LIVE_W / 2)),$GNOME_LIVE_H"
    GNOME_LIVE_CENTER_THIRD="$((GNOME_LIVE_X + GNOME_LIVE_W / 3)),$GNOME_LIVE_Y,$((GNOME_LIVE_W - 2 * (GNOME_LIVE_W / 3))),$GNOME_LIVE_H"
    GNOME_LIVE_TWO_THIRDS="$GNOME_LIVE_X,$GNOME_LIVE_Y,$((GNOME_LIVE_W - GNOME_LIVE_W / 3)),$GNOME_LIVE_H"
    GNOME_LIVE_NEXT_TWO_THIRDS=''
    GNOME_LIVE_DISPLAY_TOTAL=$total
    if (( total > 1 )) && [[ -n "$next_area" ]]; then
        local next_x next_y next_w next_h
        IFS='|' read -r next_x next_y next_w next_h <<<"$next_area"
        # A cross-display move keeps a recognised zone, so left-two-thirds stays left-two-thirds.
        GNOME_LIVE_NEXT_TWO_THIRDS="$next_x,$next_y,$((next_w - next_w / 3)),$next_h"
    fi
    log "GNOME live zones follow $GNOME_LIVE_DISPLAY_ID ($area); next-display two-thirds: ${GNOME_LIVE_NEXT_TWO_THIRDS:-none}"
}

gnome_live_active_window() {
    DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" \
        xdotool getactivewindow 2>/dev/null || printf 'unavailable'
}

gnome_live_activate_window() {
    [[ -n "${WINDOW_ID:-}" ]] || return 1
    DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" \
        xdotool windowactivate --sync "$WINDOW_ID" >/dev/null 2>&1 || true
    sleep 0.3
}

gnome_live_find_window() {
    WINDOW_ID=''
    local id pid last=''
    while read -r id; do
        [[ -n "$id" ]] || continue
        last=$id
        pid=$(DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" \
            xdotool getwindowpid "$id" 2>/dev/null || true)
        if [[ "$pid" == "$EDITOR_PID" ]]; then
            WINDOW_ID=$id
            return 0
        fi
    done < <(DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" \
        xdotool search --onlyvisible --name 'Text Editor' 2>/dev/null || true)
    WINDOW_ID=$last
    [[ -n "$WINDOW_ID" ]]
}

write_uinput_injector() {
    UINPUT_INJECTOR="$TMP_DIR/uinput_key.py"
    [[ -s "$UINPUT_INJECTOR" ]] && return 0
    cat >"$UINPUT_INJECTOR" <<'PY'
"""Press one accelerator on a kernel virtual keyboard.

Xwayland XTEST events never reach a compositor-level accelerator grab, so the only way to
evidence real hotkey capture on a seated GNOME session is a device libinput can see.
"""
import fcntl, os, struct, sys, time

UI_SET_EVBIT, UI_SET_KEYBIT = 0x40045564, 0x40045565
UI_DEV_CREATE, UI_DEV_DESTROY = 0x5501, 0x5502
EV_SYN, EV_KEY, SYN_REPORT = 0, 1, 0
KEYS = {
    'ctrl': 29, 'alt': 56, 'shift': 42, 'super': 125, 'cmd': 125,
    'left': 105, 'right': 106, 'up': 103, 'down': 108, 'pause': 119,
}

combo = [KEYS[name] for name in sys.argv[1].lower().split('+')]
fd = os.open('/dev/uinput', os.O_WRONLY | os.O_NONBLOCK)
try:
    fcntl.ioctl(fd, UI_SET_EVBIT, EV_KEY)
    for code in dict.fromkeys(combo):
        fcntl.ioctl(fd, UI_SET_KEYBIT, code)
    os.write(fd, struct.pack('=80sHHHHI' + 'i' * 256,
                             b'window-zones-smoke', 3, 0x6a6a, 0x6b6b, 1, 0, *([0] * 256)))
    fcntl.ioctl(fd, UI_DEV_CREATE)
    time.sleep(1.0)  # libinput has to notice the device before it forwards anything

    def emit(etype, code, value):
        # native 'l' matches the kernel's time fields: 24 bytes on LP64, 16 on 32-bit
        os.write(fd, struct.pack('llHHi', 0, 0, etype, code, value))

    for code in combo:
        emit(EV_KEY, code, 1)
        emit(EV_SYN, SYN_REPORT, 0)
    time.sleep(0.05)
    for code in reversed(combo):
        emit(EV_KEY, code, 0)
        emit(EV_SYN, SYN_REPORT, 0)
    time.sleep(0.3)
    fcntl.ioctl(fd, UI_DEV_DESTROY)
finally:
    os.close(fd)
PY
}

gnome_live_inject_hotkey() {
    local output='' rc=0
    write_uinput_injector
    if output=$(python3 "$UINPUT_INJECTOR" "$GNOME_SAFE_UINPUT_KEY" 2>&1); then
        rc=0
    else
        rc=$?
    fi
    log "GNOME live uinput hotkey $GNOME_SAFE_LEFT_HOTKEY: exit $rc${output:+ output '$output'}"
    return "$rc"
}

gnome_live_capture() {
    local label=$1
    local path="$TMP_DIR/${CURRENT_GATE}-${label}.png"
    local capture_log="$TMP_DIR/${CURRENT_GATE}-${label}-ffmpeg.log"
    local geometry width height
    LAST_SHOT=''
    geometry=$(DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" \
        xdotool getdisplaygeometry 2>/dev/null || true)
    if [[ "$geometry" =~ ^([0-9]+)[[:space:]]+([0-9]+)$ ]]; then
        width=${BASH_REMATCH[1]}
        height=${BASH_REMATCH[2]}
        if timeout 5 env DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" \
            ffmpeg -nostdin -loglevel error -f x11grab \
            -video_size "${width}x${height}" -i "${GNOME_XDISPLAY}.0+0,0" \
            -frames:v 1 -y "$path" >"$capture_log" 2>&1 \
            && [[ -s "$path" ]]; then
            LAST_SHOT="$path"
            log "SCREENSHOT $label: $path (ffmpeg x11grab root ${width}x${height})"
        else
            log "SCREENSHOT $label: unavailable (ffmpeg x11grab failed; log $capture_log: $(cat "$capture_log" 2>/dev/null || true))"
        fi
    else
        log "SCREENSHOT $label: unavailable (xdotool root geometry unavailable; ffmpeg not attempted)"
    fi
}

gnome_live_focus_baseline() {
    local timeout_s=${1:-10}
    local deadline=$((SECONDS + timeout_s))
    local focused='' observed='' active='' present='' window_id='' x='' y='' width='' height=''
    GNOME_LIVE_FOCUS_X11=''
    GNOME_LIVE_FOCUS_COMPANION=''
    GNOME_LIVE_FOCUS_ACTIVE=''
    while (( SECONDS < deadline )); do
        observed=$(gtk_frame_geometry "$GNOME_XDISPLAY" "$GNOME_XAUTHORITY")
        focused=$(get_gnome_focused)
        active=$(gnome_live_active_window)
        IFS='|' read -r present window_id x y width height <<<"$focused"
        if [[ "$observed" != 'unavailable' && "$present" == 'true' \
            && "$x,$y,$width,$height" == "$observed" && "$active" == "$WINDOW_ID" ]]; then
            GNOME_LIVE_FOCUS_X11="$observed"
            GNOME_LIVE_FOCUS_COMPANION="$focused"
            GNOME_LIVE_FOCUS_ACTIVE="$active"
            return 0
        fi
        sleep 0.2
    done
    GNOME_LIVE_FOCUS_X11="$observed"
    GNOME_LIVE_FOCUS_COMPANION="$focused"
    GNOME_LIVE_FOCUS_ACTIVE="$active"
    return 1
}

gnome_live_assert_focus() {
    local focused=$GNOME_LIVE_FOCUS_COMPANION observed=$GNOME_LIVE_FOCUS_X11
    local active=$GNOME_LIVE_FOCUS_ACTIVE
    local present='' window_id='' x='' y='' width='' height='' focused_geometry=''
    IFS='|' read -r present window_id x y width height <<<"$focused"
    focused_geometry="$x,$y,$width,$height"
    log "OBSERVED GNOME live focus: companion='$focused' xdotool='$observed' active='$active' window='$WINDOW_ID'"
    assert_eq 'GNOME live focused-window present' 'true' "$present"
    assert_eq 'GNOME live focus geometry agrees with xdotool' "$observed" "$focused_geometry"
    assert_eq 'GNOME live focus active X11 window' "$WINDOW_ID" "$active"
    gnome_live_capture focused-window
}

gnome_live_assert_geometry() {
    local name=$1 expected=$2
    local deadline=$((SECONDS + 10))
    local observed_x11='' observed_companion='' focused=''
    local present='' window_id='' x='' y='' width='' height=''
    gnome_live_activate_window || true
    while (( SECONDS < deadline )); do
        observed_x11=$(gtk_frame_geometry "$GNOME_XDISPLAY" "$GNOME_XAUTHORITY")
        focused=$(get_gnome_focused)
        IFS='|' read -r present window_id x y width height <<<"$focused"
        observed_companion="$focused"
        if [[ "$observed_x11" == "$expected" \
            && "$present" == 'true' \
            && "$x,$y,$width,$height" == "$expected" ]]; then
            break
        fi
        sleep 0.2
    done
    log "OBSERVED $name: companion='$observed_companion' xdotool='$observed_x11'"
    assert_eq "$name xdotool geometry" "$expected" "$observed_x11"
    assert_eq "$name companion geometry" "true|$expected" "$present|$x,$y,$width,$height"
    gnome_live_capture "$name"
}

write_gnome_live_valid_config() {
    cat >"$CONFIG_PATH" <<EOF
[[bindings]]
hotkey = "${GNOME_SAFE_LEFT_HOTKEY}"
action = { type = "move-to-zone", zone = "left-half" }

[[bindings]]
hotkey = "${GNOME_SAFE_CENTER_HOTKEY}"
action = { type = "move-to-zone", zone = "center-third" }

[[bindings]]
hotkey = "${GNOME_SAFE_RIGHT_HOTKEY}"
action = { type = "move-to-zone", zone = "left-two-thirds" }

[[bindings]]
hotkey = "${GNOME_SAFE_NEXT_HOTKEY}"
action = { type = "move-to-next-display" }
EOF
}

write_gnome_live_invalid_config() {
    cat >"$CONFIG_PATH" <<EOF
[[bindings]]
hotkey = "${GNOME_SAFE_LEFT_HOTKEY}"
action = { type = "move-to-zone", zone = "left-half" }
this is not valid toml
EOF
}

run_gnome_live_dispatch() {
    local label=$1 hotkey=$2 rc=0
    local out="$TMP_DIR/${CURRENT_GATE}-dispatch-${label}.log"
    gnome_live_activate_window || true
    if env XDG_SESSION_TYPE=wayland WAYLAND_DISPLAY="$WAYLAND_DISPLAY" \
        XDG_RUNTIME_DIR="$GNOME_RUNTIME" DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" \
        XDG_CONFIG_HOME="$GNOME_CONFIG" XDG_DATA_HOME="$GNOME_DATA" \
        HOME="$GNOME_HOME" XDG_CURRENT_DESKTOP=GNOME GNOME_DESKTOP_SESSION_ID=smoke \
        "$BIN_PATH" --backend wayland --config "$CONFIG_PATH" dispatch "$hotkey" >"$out" 2>&1; then
        rc=0
    else
        rc=$?
    fi
    cat "$out" >>"$ASSERT_LOG"
    assert_eq "$label dispatch exit" 0 "$rc"
    assert_text "$label dispatch state" 'Dispatch state: Succeeded' "$(<"$out")"
}

get_gnome_focused() {
    local raw
    raw=$(gnome_call GetFocusedWindow 2>&1 || true)
    parse_gnome_focused "$raw"
}


run_x11_dispatch() {
    local label=$1 hotkey=$2 rc=0
    local out="$TMP_DIR/${CURRENT_GATE}-dispatch-${label}.log"
    DISPLAY="$X11_DISPLAY" xdotool windowactivate --sync "$WINDOW_ID" >/dev/null 2>&1 || true
    log "X11 dispatch $label precondition: active=$(DISPLAY="$X11_DISPLAY" xdotool getactivewindow 2>/dev/null || printf unavailable) geometry=$(x11_geometry)"
    if env DISPLAY="$X11_DISPLAY" XDG_SESSION_TYPE=x11 XDG_CONFIG_HOME="$X11_CONFIG" \
        XDG_DATA_HOME="$X11_DATA" HOME="$X11_HOME" "$BIN_PATH" --backend x11 \
        --config "$CONFIG_PATH" dispatch "$hotkey" >"$out" 2>&1; then
        rc=0
    else
        rc=$?
    fi
    cat "$out" >>"$ASSERT_LOG"
    assert_eq "$label dispatch exit" 0 "$rc"
    assert_text "$label dispatch state" 'Dispatch state: Succeeded' "$(<"$out")"
}


tui_control_checks() {
    local tui_log=$1 label=$2 command expected before
    while IFS='|' read -r command expected; do
        before=$(stat -c '%s' "$tui_log" 2>/dev/null || printf '0')
        send_app "$command"
        if wait_for_size_growth "$tui_log" "$before" 8; then
            assert_text "$label TUI $command" "$expected" "$(<"$tui_log")"
        else
            assert_text "$label TUI $command" "$expected" ''
        fi
    done <<'EOF'
reload|Config:
restart|Hotkey state:
status|Commands: reload
EOF
}

tui_begin() {
    local backend=$1 label=$2 tui_log=$3
    start_fifo_app tui "$backend" "$tui_log"
    if wait_for_text "$tui_log" 'Window Zones TUI' 15; then
        assert_text "$label TUI starts" 'Window Zones TUI' "$(<"$tui_log")"
    else
        assert_text "$label TUI starts" 'Window Zones TUI' "$(cat "$tui_log" 2>/dev/null || true)"
    fi

    tui_control_checks "$tui_log" "$label"
}

tui_end() {
    local label=$1 tui_log=$2 rc=0
    send_app quit
    close_app_input
    if [[ -n "${APP_PID:-}" ]]; then
        wait_pid "$APP_PID" 12 || rc=$?
        APP_PID=''
    fi
    assert_rc_zero "$label TUI quit" "$rc" "tui log $tui_log"
}

run_tui_lifecycle() {
    local backend=$1 label=$2
    local tui_log="$TMP_DIR/${CURRENT_GATE}-${label}-tui.log"
    tui_begin "$backend" "$label" "$tui_log"

    if [[ "$backend" == x11 ]]; then
        run_x11_dispatch tui-baseline alt+ctrl+up
        x11_assert_geometry 'X11 TUI baseline' '640,0,640,1080'
    fi

    send_app 'dispatch alt+ctrl+left'
    if wait_for_text "$tui_log" 'Last action: alt+ctrl+left' 8; then
        assert_text "$label TUI dispatch" 'Last action: alt+ctrl+left' "$(<"$tui_log")"
    else
        assert_text "$label TUI dispatch" 'Last action: alt+ctrl+left' "$(cat "$tui_log" 2>/dev/null || true)"
    fi
    if [[ "$backend" == x11 ]]; then
        x11_assert_geometry 'X11 TUI dispatch moves window' '0,0,960,1080'
    fi

    tui_end "$label" "$tui_log"
}

gnome_live_choose_hotkeys() {
    local schema raw normalized candidate hotkey canonical reverse reserved conflict
    local -a candidates=(
        'ctrl+cmd+left|<super><control>left'
        'ctrl+cmd+up|<super><control>up'
        'ctrl+cmd+right|<super><control>right'
        'ctrl+cmd+down|<super><control>down'
        'pause|pause'
        'alt+pause|<alt>pause'
        'ctrl+pause|<control>pause'
    )
    GNOME_RESERVED_ACCELERATORS=()
    log 'GNOME live reserved accelerator enumeration:'
    for schema in org.gnome.desktop.wm.keybindings org.gnome.shell.keybindings \
        org.gnome.mutter.keybindings org.gnome.mutter.wayland.keybindings; do
        raw=$(DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" gsettings list-recursively "$schema" 2>&1 || true)
        normalized=$(printf '%s\n' "$raw" | tr '[:upper:]' '[:lower:]' | sed 's/<primary>/<control>/g')
        log "  $schema:"
        while IFS= read -r candidate; do
            [[ -n "$candidate" ]] && log "    $candidate"
        done <<<"$normalized"
        while IFS= read -r reserved; do
            GNOME_RESERVED_ACCELERATORS+=("$reserved")
        done < <(printf '%s\n' "$normalized" | grep -o "'[^']*'" | tr -d "'")
    done

    GNOME_SAFE_HOTKEYS=()
    for candidate in "${candidates[@]}"; do
        IFS='|' read -r hotkey canonical <<<"$candidate"
        reverse=${canonical/<super><control>/<control><super>}
        conflict=0
        for reserved in "${GNOME_RESERVED_ACCELERATORS[@]:-}"; do
            if [[ "$reserved" == "$canonical" || "$reserved" == "$reverse" ]]; then
                conflict=1
                break
            fi
        done
        if [[ "$conflict" -eq 0 ]]; then
            GNOME_SAFE_HOTKEYS+=("$hotkey")
        fi
    done
    log "GNOME live selected safe hotkeys: ${GNOME_SAFE_HOTKEYS[*]}"
    assert_eq 'GNOME live safe binding candidates' 1 \
        "$(( ${#GNOME_SAFE_HOTKEYS[@]} >= 4 ? 1 : 0 ))"
}

run_gnome_live_conflicting_binding() {
    local output='' rc=0
    if output=$(gnome_call RegisterHotkeys "['alt+ctrl+left']" 2>&1); then
        rc=0
    else
        rc=$?
    fi
    log "GNOME live conflicting RegisterHotkeys: exit $rc output '$output'"
    assert_rc_nonzero 'GNOME live conflicting binding is refused' "$rc" \
        'org.window_zones.Gnome.Error.Unsupported' "$output"
    assert_text 'GNOME live conflicting binding error name' \
        'org.window_zones.Gnome.Error.Unsupported' "$output"
    assert_text 'GNOME live conflicting binding names accelerator' \
        'alt+ctrl+left' "$output"
}

gnome_live_signal_count() {
    python - "$1" "$2" <<'PY'
import sys
path, needle = sys.argv[1:]
try:
    text = open(path, encoding="utf-8").read()
except OSError:
    text = ""
print(text.count(needle))
PY
}

run_gnome_live_atomic_registration() {
    local helper="$GNOME_ROOT/atomic-register.js"
    local monitor_log="$GNOME_ROOT/atomic-monitor.log"
    local helper_log="$GNOME_ROOT/atomic-helper.log"
    local monitor_pid='' helper_pid='' signal_count=0
    cat >"$helper" <<'EOF'
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

const valid = GLib.getenv('LIVE_VALID_HOTKEY');
const address = GLib.getenv('DBUS_SESSION_BUS_ADDRESS');
const connection = Gio.DBusConnection.new_for_address_sync(
    address,
    Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT |
    Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,
    null,
    null);
const proxy = Gio.DBusProxy.new_sync(
    connection,
    Gio.DBusProxyFlags.NONE,
    null,
    'org.window_zones.Gnome',
    '/org/window_zones/Gnome',
    'org.window_zones.Gnome2',
    null);
try {
    proxy.call_sync('RegisterHotkeys', new GLib.Variant('(as)', [[valid]]),
        Gio.DBusCallFlags.NONE, 5000, null);
    print('VALID_OK');
} catch (error) {
    print(`VALID_FAILED ${Gio.DBusError.get_remote_error(error)} ${error.message}`);
}
GLib.usleep(3000000);
try {
    proxy.call_sync('RegisterHotkeys', new GLib.Variant('(as)', [[valid, 'f25']]),
        Gio.DBusCallFlags.NONE, 5000, null);
    print('INVALID_ACCEPTED');
} catch (error) {
    print(`INVALID_REJECTED ${Gio.DBusError.get_remote_error(error)} ${error.message}`);
}
GLib.usleep(10000000);
try {
    proxy.call_sync('RegisterHotkeys', new GLib.Variant('(as)', [[]]),
        Gio.DBusCallFlags.NONE, 5000, null);
} catch (_) {
}
EOF
    : >"$monitor_log"
    : >"$helper_log"
    start_group monitor_pid "$monitor_log" env DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" \
        gdbus monitor --session --dest org.window_zones.Gnome \
        --object-path /org/window_zones/Gnome
    start_group helper_pid "$helper_log" env DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" \
        LIVE_VALID_HOTKEY="$GNOME_SAFE_LEFT_HOTKEY" gjs -m "$helper"
    if wait_for_text "$helper_log" 'VALID_OK' 8; then
        assert_text 'GNOME live atomic valid registration' 'VALID_OK' "$(<"$helper_log")"
    else
        assert_text 'GNOME live atomic valid registration' 'VALID_OK' \
            "$(cat "$helper_log" 2>/dev/null || true)"
    fi

    gnome_live_inject_hotkey || true
    if wait_for_text "$monitor_log" "$GNOME_SAFE_LEFT_HOTKEY" 5; then
        log "GNOME live atomic pre-error signal: $GNOME_SAFE_LEFT_HOTKEY"
    else
        log 'Harness limitation GNOME live atomic pre-error uinput signal was not observed'
    fi
    if wait_for_text "$helper_log" 'INVALID_REJECTED' 8; then
        assert_text 'GNOME live atomic invalid registration is refused' \
            'INVALID_REJECTED org.window_zones.Gnome.Error.Unsupported' "$(<"$helper_log")"
        assert_text 'GNOME live atomic invalid accelerator is named' 'f25' "$(<"$helper_log")"
    else
        assert_text 'GNOME live atomic invalid registration is refused' \
            'INVALID_REJECTED org.window_zones.Gnome.Error.Unsupported' \
            "$(cat "$helper_log" 2>/dev/null || true)"
        assert_text 'GNOME live atomic invalid accelerator is named' 'f25' \
            "$(cat "$helper_log" 2>/dev/null || true)"
    fi
    gnome_live_inject_hotkey || true
    sleep 0.5
    signal_count=$(gnome_live_signal_count "$monitor_log" "$GNOME_SAFE_LEFT_HOTKEY")
    log "GNOME live atomic signal count for $GNOME_SAFE_LEFT_HOTKEY: $signal_count"
    assert_eq 'GNOME live atomic registration preserves previous set' 1 \
        "$(( signal_count >= 2 ? 1 : 0 ))"
    stop_pid "$helper_pid"
    stop_pid "$monitor_pid"
    sleep 0.3
}

restore_gnome_live() {
    [[ "$GNOME_LIVE_ACTIVE" -eq 1 ]] || return 0
    local enabled_after='' after_info='' tiling_after_info='' failures_before=$FAILURES
    local window_after_enabled=0 tiling_after_enabled=0
    enabled_after=$(gnome_live_enabled_extensions)
    # Re-enabling appends the UUID. Restore order without clobbering changed membership.
    if [[ -n "${GNOME_LIVE_ENABLED_BEFORE:-}" && "$enabled_after" != "$GNOME_LIVE_ENABLED_BEFORE" ]] \
        && python3 - "$GNOME_LIVE_ENABLED_BEFORE" "$enabled_after" <<'PY'
import ast
import sys
sys.exit(sorted(ast.literal_eval(sys.argv[1])) != sorted(ast.literal_eval(sys.argv[2])))
PY
    then
        DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" gsettings set org.gnome.shell \
            enabled-extensions "$GNOME_LIVE_ENABLED_BEFORE" || true
        enabled_after=$(gnome_live_enabled_extensions)
    fi
    after_info=$(gnome_live_extension_info "$GNOME_LIVE_EXT_UUID")
    tiling_after_info=$(gnome_live_extension_info "$GNOME_LIVE_TILING_UUID")
    log "RESTORE enabled-extensions after: $enabled_after"
    log "RESTORE window-zones info after: $after_info"
    log "RESTORE tilingshell info after: $tiling_after_info"
    if [[ -n "${GNOME_LIVE_ENABLED_BEFORE:-}" ]]; then
        assert_eq 'GNOME live enabled-extensions unchanged' \
            "$GNOME_LIVE_ENABLED_BEFORE" "$enabled_after"
    fi
    if [[ "$after_info" == *"'enabled': <true>"* ]]; then
        window_after_enabled=1
    fi
    if [[ "$tiling_after_info" == *"'enabled': <true>"* ]]; then
        tiling_after_enabled=1
    fi
    if [[ -n "${GNOME_LIVE_WINDOW_ENABLED_BEFORE:-}" ]]; then
        assert_eq 'GNOME live window-zones enable state unchanged' \
            "$GNOME_LIVE_WINDOW_ENABLED_BEFORE" "$window_after_enabled"
    fi
    if [[ -n "${GNOME_LIVE_TILING_ENABLED_BEFORE:-}" ]]; then
        assert_eq 'GNOME live tilingshell enable state unchanged' \
            "$GNOME_LIVE_TILING_ENABLED_BEFORE" "$tiling_after_enabled"
    fi
    (( FAILURES == failures_before )) || return 1
    GNOME_LIVE_ACTIVE=0
    return 0
}

gnome_live_skip_after_companion() {
    local reason=$1 name
    local -a names=(
        'GNOME live focused-window present'
        'GNOME live focus geometry agrees with xdotool'
        'GNOME live focus active X11 window'
        'GNOME live test editor uses Xwayland backend'
        'left-half dispatch exit'
        'left-half dispatch state'
        'GNOME live left-half xdotool geometry'
        'GNOME live left-half companion geometry'
        'center-third dispatch exit'
        'center-third dispatch state'
        'GNOME live center-third xdotool geometry'
        'GNOME live center-third companion geometry'
        'left-two-thirds dispatch exit'
        'left-two-thirds dispatch state'
        'GNOME live left-two-thirds xdotool geometry'
        'GNOME live left-two-thirds companion geometry'
        'GNOME live seam helper builds against the public API'
        'GNOME live captures the test window id'
        'GNOME live second test window maps'
        'GNOME live focus moves to the second window'
        'GNOME live Companion reports a different focused id'
        'GNOME live captured id move after focus change succeeds'
        'GNOME live captured id moves the original window'
        'GNOME live captured id move leaves the focused window frame'
        'GNOME live captured id move leaves focus on the second window'
        'GNOME live closed target reports WindowGone'
        'GNOME live closed target leaves the other window unchanged'
        'GNOME live maximized precondition vertical'
        'GNOME live maximized precondition horizontal'
        'GNOME live maximized window move succeeds'
        'GNOME live maximized window is restored before the frame'
        'GNOME live maximized window gets the target frame'
        'GNOME live Super+Left edge-tiles the test window'
        'GNOME live edge-tiled precondition'
        'GNOME live edge-tiled window move succeeds'
        'GNOME live edge-tiled window is restored before the frame'
        'GNOME live edge-tiled window gets the target frame'
        'GNOME live fullscreen precondition'
        'GNOME live fullscreen window move is rejected'
        'GNOME live fullscreen rejection keeps fullscreen'
        'GNOME live fullscreen rejection keeps the frame'
        'GNOME live test window leaves fullscreen'
        'GNOME live conflicting binding is refused'
        'GNOME live conflicting binding error name'
        'GNOME live conflicting binding names accelerator'
        'GNOME live atomic valid registration'
        'GNOME live atomic invalid registration is refused'
        'GNOME live atomic invalid accelerator is named'
        'GNOME live atomic registration preserves previous set'
        'GNOME live App starts'
        'GNOME live safe hotkeys register'
        'GNOME live real hotkey moves window'
        'GNOME live real hotkey reports action'
        'GNOME live companion disable call'
        'GNOME live disconnect surfaces explicit unavailable state'
        'GNOME live disconnect keeps native backend'
        'GNOME live App survives companion disconnect'
        'GNOME live companion enable call'
        'GNOME live companion service recovery'
        'GNOME live extension errors empty after recovery'
        'GNOME live companion recovers without App restart'
        'GNOME live App PID unchanged after recovery'
        'config-baseline dispatch exit'
        'config-baseline dispatch state'
        'GNOME live config-baseline xdotool geometry'
        'GNOME live config-baseline companion geometry'
        'GNOME live invalid config exposes actionable error'
        'GNOME live invalid reload preserves previous bindings'
        'GNOME live invalid reload keeps hotkeys registered'
        'GNOME live invalid config keeps previous binding active'
        'GNOME live rejected replacement reports a refused reload'
        'GNOME live rejected replacement keeps the previous bindings'
        'GNOME live rejected replacement keeps hotkeys registered'
        'GNOME live rejected replacement keeps the previous grab working'
        'GNOME live valid config recovers'
        'GNOME live valid config restores bindings'
        'GNOME live App quits cleanly'
        'GNOME live TUI starts'
        'GNOME live TUI reload'
        'GNOME live TUI restart'
        'GNOME live TUI status'
        'GNOME live TUI dispatch'
        'GNOME live TUI quit'
    )
    for name in "${names[@]}"; do
        skip_assert "$name" "$reason"
    done
    log "SCREENSHOT GNOME live dependent assertions: unavailable ($reason)"
    log "SKIP GNOME live cross-display move: not applicable; companion was unavailable before the single-monitor check"
}

gnome_live_finish() {
    local rc=0
    restore_gnome_live || rc=$?
    if [[ "$rc" -eq 0 ]]; then
        trap cleanup EXIT INT TERM
    else
        GNOME_LIVE_RESTORE_FAILURE=1
        log 'FAIL GNOME live restoration did not complete; retaining strict EXIT trap'
    fi
}

gnome_live_move() {
    # window x,y,w,h: call the Companion's MoveWindow directly; print `ok` or the D-Bus error.
    # `--` keeps gdbus from reading a negative coordinate as an option.
    local out=''
    if out=$(gnome_call MoveWindow -- "$1" ${2//,/ } 2>&1); then
        printf 'ok'
    else
        printf '%s' "$out"
    fi
}

gnome_live_focused_id() {
    local present='' id='' rest=''
    IFS='|' read -r present id rest <<<"$(get_gnome_focused)"
    [[ "$present" == 'true' ]] && printf '%s' "$id"
}

run_gnome_live_adr9_window_checks() {
    # ADR 0009 on the seated compositor, observed through Xwayland's xdotool/xprop. Only the gate's
    # own editor windows are touched: a captured id outlives a focus change and reports closure;
    # maximized, edge-tiled (Super+Left, GNOME's default tiling binding) and fullscreen windows.
    local a="$WINDOW_ID" b='' b_pid='' a_id='' b_id='' out='' before='' b_before='' observed=''
    XON_DISPLAY="$GNOME_XDISPLAY"
    XON_XAUTHORITY="$GNOME_XAUTHORITY"
    write_uinput_injector
    build_seam_helper 'GNOME live'
    gnome_live_activate_window || true
    a_id=$(gnome_live_focused_id)
    assert_text 'GNOME live captures the test window id' 'captured' "${a_id:+captured}"
    gnome_live_move "$a_id" "$GNOME_LIVE_CENTER_THIRD" >/dev/null
    start_group b_pid "$GNOME_ROOT/editor-b.log" env \
        DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" GDK_BACKEND=x11 \
        XDG_RUNTIME_DIR="$GNOME_RUNTIME" DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" \
        XDG_CONFIG_HOME="$GNOME_CONFIG" XDG_DATA_HOME="$GNOME_DATA" HOME="$GNOME_HOME" \
        XDG_CURRENT_DESKTOP=GNOME XDG_SESSION_TYPE=wayland gnome-text-editor --standalone
    b=$(xwindow_for_pid "$b_pid" 20 || true)
    assert_text 'GNOME live second test window maps' 'mapped' "${b:+mapped}"
    if [[ -z "$b" || -z "$a_id" ]]; then
        stop_pid "$b_pid"
        return 0
    fi
    xon xdotool windowactivate --sync "$b" >/dev/null 2>&1 || true
    sleep 0.5
    b_id=$(gnome_live_focused_id)
    assert_eq 'GNOME live focus moves to the second window' "$b" "$(gnome_live_active_window)"
    assert_eq 'GNOME live Companion reports a different focused id' 1 \
        "$([[ -n "$b_id" && "$b_id" != "$a_id" ]] && printf 1 || printf 0)"
    WINDOW_ID="$b"
    sleep 0.5
    b_before=$(gnome_live_geometry)
    WINDOW_ID="$a"
    out=$(gnome_live_move "$a_id" "$GNOME_LIVE_LEFT_HALF")
    assert_eq 'GNOME live captured id move after focus change succeeds' ok "$out"
    assert_eq 'GNOME live captured id moves the original window' "$GNOME_LIVE_LEFT_HALF" \
        "$(wait_observed gnome_live_geometry "$GNOME_LIVE_LEFT_HALF")"
    WINDOW_ID="$b"
    assert_eq 'GNOME live captured id move leaves the focused window frame' "$b_before" \
        "$(gnome_live_geometry)"
    WINDOW_ID="$a"
    assert_eq 'GNOME live captured id move leaves focus on the second window' "$b" \
        "$(gnome_live_active_window)"
    gnome_live_capture captured-id-move

    stop_pid "$b_pid"
    wait_pid "$b_pid" 5 || true
    set +e
    wait_xwindow_gone "$b" 10 || true
    before=$(gnome_live_geometry)
    out=$(gnome_live_move "$b_id" "$GNOME_LIVE_CENTER_THIRD")
    assert_text 'GNOME live closed target reports WindowGone' \
        'org.window_zones.Gnome.Error.WindowGone' "$out"
    sleep 1
    assert_eq 'GNOME live closed target leaves the other window unchanged' "$before" \
        "$(gnome_live_geometry)"

    gnome_live_activate_window || true
    if [[ -n "$SEAM_HELPER" ]]; then
        seam state "$a" add _NET_WM_STATE_MAXIMIZED_VERT _NET_WM_STATE_MAXIMIZED_HORZ >/dev/null
    fi
    assert_wm_state 'GNOME live maximized precondition vertical' "$a" _NET_WM_STATE_MAXIMIZED_VERT 1
    assert_wm_state 'GNOME live maximized precondition horizontal' "$a" _NET_WM_STATE_MAXIMIZED_HORZ 1
    out=$(gnome_live_move "$a_id" "$GNOME_LIVE_CENTER_THIRD")
    assert_eq 'GNOME live maximized window move succeeds' ok "$out"
    assert_wm_state 'GNOME live maximized window is restored before the frame' "$a" \
        _NET_WM_STATE_MAXIMIZED 0
    assert_eq 'GNOME live maximized window gets the target frame' "$GNOME_LIVE_CENTER_THIRD" \
        "$(wait_observed gnome_live_geometry "$GNOME_LIVE_CENTER_THIRD")"

    gnome_live_activate_window || true
    out=$(python3 "$UINPUT_INJECTOR" super+left 2>&1) || log "GNOME live uinput super+left: $out"
    assert_eq 'GNOME live Super+Left edge-tiles the test window' "$GNOME_LIVE_LEFT_HALF" \
        "$(wait_observed gnome_live_geometry "$GNOME_LIVE_LEFT_HALF")"
    assert_wm_state 'GNOME live edge-tiled precondition' "$a" _NET_WM_STATE_MAXIMIZED_VERT 1
    out=$(gnome_live_move "$a_id" "$GNOME_LIVE_TWO_THIRDS")
    assert_eq 'GNOME live edge-tiled window move succeeds' ok "$out"
    assert_wm_state 'GNOME live edge-tiled window is restored before the frame' "$a" \
        _NET_WM_STATE_MAXIMIZED 0
    assert_eq 'GNOME live edge-tiled window gets the target frame' "$GNOME_LIVE_TWO_THIRDS" \
        "$(wait_observed gnome_live_geometry "$GNOME_LIVE_TWO_THIRDS")"

    if [[ -n "$SEAM_HELPER" ]]; then
        seam state "$a" add _NET_WM_STATE_FULLSCREEN >/dev/null
    fi
    assert_wm_state 'GNOME live fullscreen precondition' "$a" _NET_WM_STATE_FULLSCREEN 1
    sleep 1
    before=$(gnome_live_geometry)
    out=$(gnome_live_move "$a_id" "$GNOME_LIVE_CENTER_THIRD")
    assert_text 'GNOME live fullscreen window move is rejected' \
        'window is fullscreen; leave fullscreen first' "$out"
    assert_wm_state 'GNOME live fullscreen rejection keeps fullscreen' "$a" _NET_WM_STATE_FULLSCREEN 1
    sleep 1
    assert_eq 'GNOME live fullscreen rejection keeps the frame' "$before" "$(gnome_live_geometry)"
    gnome_live_capture fullscreen-rejected
    if [[ -n "$SEAM_HELPER" ]]; then
        seam state "$a" remove _NET_WM_STATE_FULLSCREEN >/dev/null
    fi
    assert_wm_state 'GNOME live test window leaves fullscreen' "$a" _NET_WM_STATE_FULLSCREEN 0
    gnome_live_move "$a_id" "$GNOME_LIVE_TWO_THIRDS" >/dev/null
    wait_observed gnome_live_geometry "$GNOME_LIVE_TWO_THIRDS" >/dev/null
    gnome_live_activate_window || true
}

run_gnome_live_app_checks() {
    local run_log="$GNOME_ROOT/live-run.log"
    local registered=0 real_geometry='' real_ok=0 real_path='uinput'
    local attempts=0 deadline=0 before_pid='' disable_out='' enable_out=''
    write_gnome_live_valid_config
    start_fifo_app run wayland "$run_log"
    if wait_for_text "$run_log" 'Interactive session started' 15; then
        assert_text 'GNOME live App starts' 'Interactive session started' "$(<"$run_log")"
    else
        assert_text 'GNOME live App starts' 'Interactive session started' \
            "$(cat "$run_log" 2>/dev/null || true)"
    fi

    if [[ -n "${APP_FD:-}" ]]; then
        send_app status
    fi
    if wait_for_text "$run_log" 'hotkey state: Registered' 10; then
        registered=1
        assert_text 'GNOME live safe hotkeys register' 'hotkey state: Registered' "$(<"$run_log")"
    else
        assert_text 'GNOME live safe hotkeys register' 'hotkey state: Registered' \
            "$(cat "$run_log" 2>/dev/null || true)"
    fi
    log "GNOME live safe binding registration verified: $registered ($GNOME_SAFE_LEFT_HOTKEY)"

    if [[ "$registered" -eq 1 ]]; then
        gnome_live_activate_window || true
        deadline=$((SECONDS + 10))
        while (( SECONDS < deadline )); do
            attempts=$((attempts + 1))
            gnome_live_inject_hotkey || true
            sleep 0.25
            real_geometry=$(gtk_frame_geometry "$GNOME_XDISPLAY" "$GNOME_XAUTHORITY")
            if [[ "$real_geometry" == "$GNOME_LIVE_LEFT_HALF" ]]; then
                real_ok=1
                break
            fi
        done
    else
        log 'GNOME live real-hotkey injection not attempted because the live grab did not register'
    fi
    if [[ "$real_ok" -eq 0 ]]; then
        log "Harness limitation GNOME live input did not move the window after $attempts attempts"
        log "MANUAL ACTION REQUIRED: press $GNOME_SAFE_LEFT_HOTKEY once in the focused Text Editor window"
        deadline=$((SECONDS + 15))
        while (( SECONDS < deadline )); do
            real_geometry=$(gtk_frame_geometry "$GNOME_XDISPLAY" "$GNOME_XAUTHORITY")
            if [[ "$real_geometry" == "$GNOME_LIVE_LEFT_HALF" ]]; then
                real_ok=1
                real_path='manual'
                break
            fi
            sleep 0.25
        done
    fi
    log "GNOME live real-hotkey evidence path: $real_path"
    assert_eq 'GNOME live real hotkey moves window' "$GNOME_LIVE_LEFT_HALF" "$real_geometry"
    gnome_live_capture real-hotkey
    if [[ -n "${APP_FD:-}" ]]; then
        send_app status
    fi
    # The App reports the canonical binding, which orders modifiers its own way, so compare the
    # token set rather than the spelling the config happens to use.
    assert_eq 'GNOME live real hotkey reports action' \
        "$(sorted_hotkey "$GNOME_SAFE_LEFT_HOTKEY")" \
        "$(sorted_hotkey "$(wait_for_reported_action "$run_log" 'last action: ' 10)")"

    # Disconnect and recovery on the seated session. The toggle is scoped and reverted: the
    # extension is disabled through GNOME's own D-Bus call and re-enabled before the gate ends,
    # and gnome_live_finish asserts enabled-extensions came back unchanged.
    local app_pid_before="${APP_PID:-}" disable_out enable_out errors_after
    local backend_after='' pid_alive=0 pid_same=0 companion_back=0
    disable_out=$(gnome_shell_call /org/gnome/Shell \
        org.gnome.Shell.Extensions.DisableExtension window-zones@mihai-a24 2>&1 || true)
    log "GNOME live companion disable: $disable_out"
    assert_text 'GNOME live companion disable call' 'true' "$disable_out"
    # The App notices the loss on its next poll of the companion, which is not instant.
    wait_for_text "$run_log" 'GNOME companion is unavailable' 45 || true
    assert_text 'GNOME live disconnect surfaces explicit unavailable state' \
        'GNOME companion is unavailable' "$(cat "$run_log" 2>/dev/null || true)"
    send_app status
    # Every backend line the App ever printed must still name the native path: no X11 fallback.
    backend_after=$(sed -n 's/^Window backend: //p' "$run_log" | sort -u | tr '\n' ',' | sed 's/,$//')
    assert_eq 'GNOME live disconnect keeps native backend' 'gnome-wayland' "$backend_after"
    kill -0 "$app_pid_before" 2>/dev/null && pid_alive=1
    assert_eq 'GNOME live App survives companion disconnect' 1 "$pid_alive"
    gnome_live_capture companion-disconnected

    enable_out=$(gnome_shell_call /org/gnome/Shell \
        org.gnome.Shell.Extensions.EnableExtension window-zones@mihai-a24 2>&1 || true)
    log "GNOME live companion enable: $enable_out"
    assert_text 'GNOME live companion enable call' 'true' "$enable_out"
    gnome_live_wait_companion 20 && companion_back=1
    assert_eq 'GNOME live companion service recovery' 1 "$companion_back"
    errors_after=$(gnome_shell_call /org/gnome/Shell \
        org.gnome.Shell.Extensions.GetExtensionErrors window-zones@mihai-a24 2>&1 || true)
    assert_text 'GNOME live extension errors empty after recovery' '[]' "$errors_after"
    wait_for_text "$run_log" 'Hotkey registration now recovered' 45 || true
    assert_text 'GNOME live companion recovers without App restart' \
        'Hotkey registration now recovered' "$(cat "$run_log" 2>/dev/null || true)"
    kill -0 "$app_pid_before" 2>/dev/null && pid_same=1
    assert_eq 'GNOME live App PID unchanged after recovery' 1 "$pid_same"
    gnome_live_capture companion-recovered

    run_gnome_live_dispatch config-baseline "$GNOME_SAFE_RIGHT_HOTKEY"
    gnome_live_assert_geometry config-baseline "$GNOME_LIVE_TWO_THIRDS"
    write_gnome_live_invalid_config
    sleep 0.4
    send_app reload
    sleep 0.3
    send_app reload
    if wait_for_text "$run_log" 'Config reload error' 10; then
        assert_text 'GNOME live invalid config exposes actionable error' \
            'Config reload error' "$(<"$run_log")"
    else
        assert_text 'GNOME live invalid config exposes actionable error' \
            'Config reload error' "$(cat "$run_log" 2>/dev/null || true)"
    fi
    local bindings_before hotkeys_before
    bindings_before=$(count_text "$run_log" 'binding count: 4')
    hotkeys_before=$(count_text "$run_log" 'hotkey state: Registered')
    send_app status
    wait_for_new_text "$run_log" 'binding count: 4' "$bindings_before" 10 || true
    assert_eq 'GNOME live invalid reload preserves previous bindings' 1 \
        "$(( $(count_text "$run_log" 'binding count: 4') > bindings_before ? 1 : 0 ))"
    assert_eq 'GNOME live invalid reload keeps hotkeys registered' 1 \
        "$(( $(count_text "$run_log" 'hotkey state: Registered') > hotkeys_before ? 1 : 0 ))"
    gnome_live_activate_window || true
    gnome_live_inject_hotkey || true
    deadline=$((SECONDS + 10))
    while (( SECONDS < deadline )); do
        real_geometry=$(gtk_frame_geometry "$GNOME_XDISPLAY" "$GNOME_XAUTHORITY")
        [[ "$real_geometry" == "$GNOME_LIVE_LEFT_HALF" ]] && break
        sleep 0.2
    done
    assert_eq 'GNOME live invalid config keeps previous binding active' \
        "$GNOME_LIVE_LEFT_HALF" "$real_geometry"
    gnome_live_capture invalid-config-binding

    # ADR 0009: a replacement the compositor refuses keeps the old config and its grabs. GNOME
    # reserves Alt+Ctrl+Left (the conflicting-binding check above proves it is refused).
    local refused_before
    refused_before=$(count_text "$run_log" 'were refused')
    write_gnome_live_valid_config
    cat >>"$CONFIG_PATH" <<'EOF'

[[bindings]]
hotkey = "Alt+Ctrl+Left"
action = { type = "move-to-zone", zone = "center-third" }
EOF
    sleep 0.4
    send_app reload
    sleep 0.3
    send_app reload
    wait_for_new_text "$run_log" 'were refused' "$refused_before" 10 || true
    assert_eq 'GNOME live rejected replacement reports a refused reload' 1 \
        "$(( $(count_text "$run_log" 'were refused') > refused_before ? 1 : 0 ))"
    bindings_before=$(count_text "$run_log" 'binding count: 4')
    hotkeys_before=$(count_text "$run_log" 'hotkey state: Registered')
    send_app status
    wait_for_new_text "$run_log" 'binding count: 4' "$bindings_before" 10 || true
    assert_eq 'GNOME live rejected replacement keeps the previous bindings' 1 \
        "$(( $(count_text "$run_log" 'binding count: 4') > bindings_before ? 1 : 0 ))"
    assert_eq 'GNOME live rejected replacement keeps hotkeys registered' 1 \
        "$(( $(count_text "$run_log" 'hotkey state: Registered') > hotkeys_before ? 1 : 0 ))"
    gnome_live_activate_window || true
    gnome_live_move "$(gnome_live_focused_id)" "$GNOME_LIVE_CENTER_THIRD" >/dev/null
    wait_observed gnome_live_geometry "$GNOME_LIVE_CENTER_THIRD" >/dev/null
    gnome_live_inject_hotkey || true
    assert_eq 'GNOME live rejected replacement keeps the previous grab working' \
        "$GNOME_LIVE_LEFT_HALF" "$(wait_observed gnome_live_geometry "$GNOME_LIVE_LEFT_HALF" 10)"
    gnome_live_capture rejected-replacement-binding

    write_gnome_live_valid_config
    sleep 0.4
    send_app reload
    sleep 0.3
    send_app reload
    if wait_for_text "$run_log" 'Config state: Loaded' 10; then
        assert_text 'GNOME live valid config recovers' 'Config state: Loaded' "$(<"$run_log")"
    else
        assert_text 'GNOME live valid config recovers' 'Config state: Loaded' \
            "$(cat "$run_log" 2>/dev/null || true)"
    fi
    bindings_before=$(count_text "$run_log" 'binding count: 4')
    send_app status
    wait_for_new_text "$run_log" 'binding count: 4' "$bindings_before" 10 || true
    assert_eq 'GNOME live valid config restores bindings' 1 \
        "$(( $(count_text "$run_log" 'binding count: 4') > bindings_before ? 1 : 0 ))"
    stop_app_cleanly 'GNOME live App quits cleanly' "$run_log"
}

run_gnome_live_tui_lifecycle() {
    local tui_log="$GNOME_ROOT/live-tui.log" rc=0
    write_gnome_live_valid_config
    start_fifo_app tui wayland "$tui_log"
    if wait_for_text "$tui_log" 'Window Zones TUI' 15; then
        assert_text 'GNOME live TUI starts' 'Window Zones TUI' "$(<"$tui_log")"
    else
        assert_text 'GNOME live TUI starts' 'Window Zones TUI' \
            "$(cat "$tui_log" 2>/dev/null || true)"
    fi

    tui_control_checks "$tui_log" 'GNOME live'

    gnome_live_activate_window || true
    send_app "dispatch $GNOME_SAFE_LEFT_HOTKEY"
    assert_eq 'GNOME live TUI dispatch' \
        "$(sorted_hotkey "$GNOME_SAFE_LEFT_HOTKEY")" \
        "$(sorted_hotkey "$(wait_for_reported_action "$tui_log" 'Last action: ' 10)")"

    tui_end 'GNOME live' "$tui_log"
}

run_gnome_live() {
    CURRENT_GATE='gnome-live'
    if [[ ! -w /dev/uinput ]]; then
        assert_eq 'GNOME live uinput preflight' writable '/dev/uinput is not writable'
        log 'BLOCKED GNOME live: uinput is required. With explicit administrator approval, load it with sudo modprobe uinput, save getfacl -p /dev/uinput, grant only this user rw via sudo setfacl -m "u:$(id -un):rw" /dev/uinput, and restore the saved ACL with sudo setfacl -P --restore on exit (see the GNOME runbook). Never make /dev/uinput world-writable.'
        return 1
    fi
    assert_eq 'GNOME live uinput preflight' writable writable
    GNOME_ROOT="$TMP_DIR/gnome-live"
    GNOME_CONFIG="$GNOME_ROOT/config"
    GNOME_DATA="$GNOME_ROOT/data"
    GNOME_RUNTIME="/run/user/$(id -u)"
    GNOME_HOME="$GNOME_ROOT/home"
    CONFIG_PATH="$GNOME_ROOT/config.toml"
    GNOME_LIVE_HOME="${HOME:-/home/$(id -un)}"
    GNOME_LIVE_EXT_UUID='window-zones@mihai-a24'
    GNOME_LIVE_TILING_UUID='tilingshell@ferrarodomenico.com'
    GNOME_LIVE_EXT_DIR="$GNOME_LIVE_HOME/.local/share/gnome-shell/extensions/$GNOME_LIVE_EXT_UUID"
    GNOME_LIVE_BACKUP_DIR="$GNOME_ROOT/installed-extension"
    GNOME_BUS="unix:path=$GNOME_RUNTIME/bus"
    WAYLAND_DISPLAY='wayland-0'
    GNOME_XDISPLAY=':0'
    GNOME_XAUTHORITY=''
    GNOME_LIVE_WINDOW_ENABLED_BEFORE=0
    GNOME_LIVE_TILING_ENABLED_BEFORE=0
    GNOME_LIVE_ACTIVE=1
    GNOME_LIVE_RESTORE_FAILURE=0
    APP_PID=''
    APP_FD=''
    WINDOW_ID=''
    export DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" \
        XDG_RUNTIME_DIR="$GNOME_RUNTIME" XDG_SESSION_TYPE=wayland \
        WAYLAND_DISPLAY DISPLAY="$GNOME_XDISPLAY" GDK_BACKEND=''
    mkdir -p "$GNOME_CONFIG" "$GNOME_DATA" "$GNOME_HOME"
    chmod 700 "$GNOME_HOME"
    trap gnome_live_exit_trap EXIT INT TERM
    set +e

    record_versions_gnome
    local enabled_before window_info_before tiling_info_before
    local snapshot_ok=1 companion_ready=0 extension_active=0
    enabled_before=$(gnome_live_enabled_extensions)
    window_info_before=$(gnome_live_extension_info "$GNOME_LIVE_EXT_UUID")
    tiling_info_before=$(gnome_live_extension_info "$GNOME_LIVE_TILING_UUID")
    GNOME_LIVE_ENABLED_BEFORE="$enabled_before"
    if [[ "$window_info_before" == *"'enabled': <true>"* ]]; then
        GNOME_LIVE_WINDOW_ENABLED_BEFORE=1
    fi
    if [[ "$tiling_info_before" == *"'enabled': <true>"* \
        || "$enabled_before" == *"$GNOME_LIVE_TILING_UUID"* ]]; then
        GNOME_LIVE_TILING_ENABLED_BEFORE=1
    fi
    log "SNAPSHOT enabled-extensions before: $enabled_before"
    log "SNAPSHOT window-zones info before: $window_info_before"
    log "SNAPSHOT tilingshell info before: $tiling_info_before"
    if [[ "$enabled_before" != \[* ]]; then
        snapshot_ok=0
    fi
    if [[ -d "$GNOME_LIVE_EXT_DIR" ]]; then
        if ! cp -a -- "$GNOME_LIVE_EXT_DIR" "$GNOME_LIVE_BACKUP_DIR"; then
            snapshot_ok=0
        fi
    else
        log "SNAPSHOT installed extension directory: absent ($GNOME_LIVE_EXT_DIR)"
    fi
    if [[ "$snapshot_ok" -ne 1 ]]; then
        assert_eq 'GNOME live snapshot complete' 1 0
    fi

    if gnome_live_wait_companion 5; then
        companion_ready=1
    fi
    assert_text 'GNOME live companion owns D-Bus name at start' \
        'org.window_zones.Gnome' "$GNOME_LIVE_SERVICE_LIST"
    if [[ "$companion_ready" -eq 1 || "$GNOME_LIVE_SERVICE_LIST" == *'org.window_zones.Gnome'* ]]; then
        assert_text 'GNOME live extension errors empty at start' '[]' \
            "$GNOME_LIVE_EXTENSION_ERRORS"
    else
        skip_assert 'GNOME live extension errors empty at start' \
            'companion name absent; GetExtensionErrors was not callable'
    fi
    if [[ "$window_info_before" == *"'state': <1.0>"* ]]; then
        extension_active=1
    fi
    assert_eq 'GNOME live companion extension is ACTIVE at start' 1 "$extension_active"
    if [[ "$companion_ready" -ne 1 ]]; then
        log 'BLOCKED GNOME live: companion is not active on the live session bus; log out and back in, then rerun gnome-live'
        gnome_live_skip_after_companion \
            'org.window_zones.Gnome absent or extension not ACTIVE; install the current companion and log out and back in'
        gnome_live_finish
        set -e
        return 0
    fi

    local repo_files_ok=1
    if ! cmp -s "$PROJECT_ROOT/gnome-extension/metadata.json" \
        "$GNOME_LIVE_EXT_DIR/metadata.json"; then
        repo_files_ok=0
    fi
    if ! cmp -s "$PROJECT_ROOT/gnome-extension/extension.js" \
        "$GNOME_LIVE_EXT_DIR/extension.js"; then
        repo_files_ok=0
    fi
    assert_eq 'GNOME live installed companion matches repository' 1 "$repo_files_ok"
    if [[ "$repo_files_ok" -ne 1 ]]; then
        log 'BLOCKED GNOME live: active companion is not the repository copy; install it and log out and back in before rerunning'
        gnome_live_skip_after_companion 'active companion files differ from repository copy'
        gnome_live_finish
        set -e
        return 0
    fi

    gnome_live_choose_hotkeys
    if [[ "${#GNOME_SAFE_HOTKEYS[@]}" -lt 4 ]]; then
        log 'GNOME live could not find four unreserved bindings; live checks stopped'
        gnome_live_skip_after_companion 'fewer than four unreserved live accelerator candidates'
        gnome_live_finish
        set -e
        return 0
    fi
    GNOME_SAFE_LEFT_HOTKEY="${GNOME_SAFE_HOTKEYS[0]}"
    GNOME_SAFE_CENTER_HOTKEY="${GNOME_SAFE_HOTKEYS[1]}"
    GNOME_SAFE_RIGHT_HOTKEY="${GNOME_SAFE_HOTKEYS[2]}"
    GNOME_SAFE_NEXT_HOTKEY="${GNOME_SAFE_HOTKEYS[3]}"
    GNOME_SAFE_HOTKEY="$GNOME_SAFE_LEFT_HOTKEY"
    GNOME_SAFE_UINPUT_KEY="$GNOME_SAFE_LEFT_HOTKEY"

    local line auth
    while IFS= read -r line; do
        if [[ "$line" =~ /Xwayland[[:space:]]+:([0-9]+).*-[aA]uth[[:space:]]([^[:space:]]+) ]]; then
            GNOME_XDISPLAY=":${BASH_REMATCH[1]}"
            GNOME_XAUTHORITY="${BASH_REMATCH[2]}"
            break
        fi
    done < <(ps -eo args=)
    if [[ ! -r "$GNOME_XAUTHORITY" ]]; then
        for auth in "$GNOME_RUNTIME"/.mutter-Xwaylandauth.*; do
            if [[ -r "$auth" ]]; then
                GNOME_XAUTHORITY="$auth"
                break
            fi
        done
    fi
    export DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY"
    log "GNOME live Xwayland observer: DISPLAY=$DISPLAY XAUTHORITY=$XAUTHORITY"
    assert_text 'GNOME live Xwayland auth discovered' '.mutter-Xwaylandauth.' "$GNOME_XAUTHORITY"

    local displays_raw display_lines display_count first_display second_display
    displays_raw=$(gnome_call GetDisplays 2>&1 || true)
    display_lines=$(parse_gnome_displays "$displays_raw" || true)
    display_count=$(printf '%s\n' "$display_lines" | sed '/^$/d' | wc -l | tr -d ' ')
    log "GNOME live GetDisplays raw: $displays_raw"
    log "GNOME live parsed displays: $display_lines"
    GNOME_LIVE_DISPLAY_LINES="$display_lines"
    assert_eq 'GNOME live reports at least one display' 1 "$(( display_count >= 1 ? 1 : 0 ))"
    first_display=$(printf '%s\n' "$display_lines" | sed -n '1p')
    if [[ ! "$first_display" =~ ^([^|]+)\|(-?[0-9]+)\|(-?[0-9]+)\|([0-9]+)\|([0-9]+)$ ]]; then
        assert_text 'GNOME live usable display geometry' '|' "$first_display"
        log 'GNOME live display geometry unavailable; remaining live checks stopped'
        gnome_live_finish
        set -e
        return 0
    fi
    IFS='|' read -r GNOME_LIVE_DISPLAY_ID GNOME_LIVE_X GNOME_LIVE_Y \
        GNOME_LIVE_W GNOME_LIVE_H <<<"$first_display"
    GNOME_LIVE_LEFT_HALF="$GNOME_LIVE_X,$GNOME_LIVE_Y,$((GNOME_LIVE_W / 2)),$GNOME_LIVE_H"
    GNOME_LIVE_CENTER_THIRD="$((GNOME_LIVE_X + GNOME_LIVE_W / 3)),$GNOME_LIVE_Y,$((GNOME_LIVE_W - 2 * (GNOME_LIVE_W / 3))),$GNOME_LIVE_H"
    GNOME_LIVE_TWO_THIRDS="$GNOME_LIVE_X,$GNOME_LIVE_Y,$((GNOME_LIVE_W - GNOME_LIVE_W / 3)),$GNOME_LIVE_H"
    log "GNOME live usable area from GetDisplays: id=$GNOME_LIVE_DISPLAY_ID x=$GNOME_LIVE_X y=$GNOME_LIVE_Y w=$GNOME_LIVE_W h=$GNOME_LIVE_H"
    GNOME_LIVE_NEXT_TWO_THIRDS=''
    GNOME_LIVE_DISPLAY_TOTAL="$display_count"
    write_gnome_live_valid_config

    local editor_log="$GNOME_ROOT/editor.log" editor_pid='' editor_deadline
    start_group editor_pid "$editor_log" env \
        DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" GDK_BACKEND=x11 \
        XDG_RUNTIME_DIR="$GNOME_RUNTIME" DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" \
        XDG_CONFIG_HOME="$GNOME_CONFIG" XDG_DATA_HOME="$GNOME_DATA" HOME="$GNOME_HOME" \
        XDG_CURRENT_DESKTOP=GNOME XDG_SESSION_TYPE=wayland gnome-text-editor --standalone
    EDITOR_PID="$editor_pid"
    log "OBSERVED GNOME live editor pid: $EDITOR_PID"
    editor_deadline=$((SECONDS + 20))
    while (( SECONDS < editor_deadline )); do
        gnome_live_find_window && break
        sleep 0.2
    done
    log "OBSERVED GNOME live editor X11 window: ${WINDOW_ID:-unavailable}"
    gnome_live_activate_window || true
    gnome_live_focus_baseline 10 || true
    gnome_live_assert_focus
    gnome_live_zone_targets
    local editor_backend=''
    editor_backend=$(tr '\0' '\n' <"/proc/$EDITOR_PID/environ" 2>/dev/null \
        | sed -n '/^GDK_BACKEND=/p' | sed -n '1p' || true)
    assert_eq 'GNOME live test editor uses Xwayland backend' 'GDK_BACKEND=x11' "$editor_backend"
    if [[ -z "${WINDOW_ID:-}" ]]; then
        log 'GNOME live test window was not observable through Xwayland; remaining checks stopped'
        gnome_live_finish
        set -e
        return 0
    fi

    run_gnome_live_dispatch left-half "$GNOME_SAFE_LEFT_HOTKEY"
    gnome_live_assert_geometry 'GNOME live left-half' "$GNOME_LIVE_LEFT_HALF"
    run_gnome_live_dispatch center-third "$GNOME_SAFE_CENTER_HOTKEY"
    gnome_live_assert_geometry 'GNOME live center-third' "$GNOME_LIVE_CENTER_THIRD"
    run_gnome_live_dispatch left-two-thirds "$GNOME_SAFE_RIGHT_HOTKEY"
    gnome_live_assert_geometry 'GNOME live left-two-thirds' "$GNOME_LIVE_TWO_THIRDS"
    if [[ -n "$GNOME_LIVE_NEXT_TWO_THIRDS" ]]; then
        run_gnome_live_dispatch next-display "$GNOME_SAFE_NEXT_HOTKEY"
        gnome_live_assert_geometry 'GNOME live cross-display move' \
            "$GNOME_LIVE_NEXT_TWO_THIRDS"
        if (( GNOME_LIVE_DISPLAY_TOTAL == 2 )); then
            # Two displays wrap, so a second move returns the window and leaves the later checks
            # on the display they were computed for.
            run_gnome_live_dispatch next-display-wrap "$GNOME_SAFE_NEXT_HOTKEY"
            gnome_live_assert_geometry 'GNOME live cross-display move wraps back' "$GNOME_LIVE_TWO_THIRDS"
        fi
    else
        skip_assert 'GNOME live cross-display move' \
            "not exercised: live session reports $display_count display"
    fi

    run_gnome_live_adr9_window_checks
    run_gnome_live_conflicting_binding
    run_gnome_live_atomic_registration
    run_gnome_live_app_checks
    run_gnome_live_tui_lifecycle
    stop_pid "$EDITOR_PID"
    gnome_live_finish
    set -e
}

kde_live_companion_call() {
    local method=$1
    shift
    busctl --address="$KDE_BUS" call \
        org.window_zones.KWin /org/window_zones/KWin org.window_zones.KWin3 "$method" "$@"
}

kde_live_kwin_call() {
    local method=$1
    shift
    busctl --address="$KDE_BUS" call \
        org.kde.KWin /Scripting org.kde.kwin.Scripting "$method" "$@"
}

kde_live_parse_capabilities() {
    python - "$1" <<'PY'
import re, sys
print("|".join(re.findall(r'"([^"]*)"', sys.argv[1])))
PY
}

kde_live_parse_displays() {
    python - "$1" <<'PY'
import re, sys
pattern = re.compile(
    r'"([^"]*)"\s+(?:int32\s+)?(-?\d+)\s+'
    r'(?:int32\s+)?(-?\d+)\s+(?:(?:uint32|int32)\s+)?(\d+)\s+'
    r'(?:(?:uint32|int32)\s+)?(\d+)'
)
for match in pattern.finditer(sys.argv[1]):
    print("|".join(match.groups()))
PY
}

kde_live_parse_focused() {
    python - "$1" <<'PY'
import re, sys
match = re.search(
    r'\b(true|false)\s+"([^"]*)"\s+'
    r'(?:int32\s+)?(-?\d+)\s+(?:int32\s+)?(-?\d+)\s+'
    r'(?:(?:uint32|int32)\s+)?(\d+)\s+(?:(?:uint32|int32)\s+)?(\d+)',
    sys.argv[1],
)
if match:
    print("|".join(match.groups()))
PY
}

kde_live_get_capabilities() {
    kde_live_parse_capabilities "$(kde_live_companion_call GetCapabilities 2>&1 || true)"
}


kde_live_get_focused() {
    local raw
    raw=$(kde_live_companion_call GetFocusedWindow 2>&1 || true)
    kde_live_parse_focused "$raw"
}

kde_live_wait_companion_ready() {
    local timeout_s=${1:-30}
    local deadline=$((SECONDS + timeout_s))
    local owners='' owner_line='' loaded='' capabilities='' displays='' count=0
    KDE_COMPANION_READY=0
    KDE_SCRIPT_LOADED_RAW=''
    KDE_CAPABILITIES_RAW=''
    KDE_DISPLAYS_RAW=''
    while (( SECONDS < deadline )); do
        owners=$(busctl --address="$KDE_BUS" list 2>&1 || true)
        owner_line=$(printf '%s\n' "$owners" | sed -n '/org.window_zones.KWin/p' | sed -n '1p')
        loaded=$(kde_live_kwin_call isScriptLoaded s window-zones 2>&1 || true)
        capabilities=$(kde_live_get_capabilities)
        displays=$(kde_live_companion_call GetDisplays 2>&1 || true)
        count=$(kde_live_parse_displays "$displays" | sed '/^$/d' | wc -l | tr -d ' ')
        if [[ -n "$owner_line" && "$loaded" == *'true'* \
            && "$capabilities" == 'focused-window|displays|move-resize|hotkeys' \
            && "$count" -eq 2 ]]; then
            KDE_COMPANION_READY=1
            KDE_SCRIPT_LOADED_RAW="$loaded"
            KDE_CAPABILITIES_RAW="$capabilities"
            KDE_DISPLAYS_RAW="$displays"
            KDE_KWIN_OWNER_LINE=$(printf '%s\n' "$owners" | sed -n '/org.kde.KWin/p' | sed -n '1p')
            KDE_COMPANION_OWNER_LINE="$owner_line"
            return 0
        fi
        sleep 0.2
    done
    KDE_SCRIPT_LOADED_RAW="$loaded"
    KDE_CAPABILITIES_RAW="$capabilities"
    KDE_DISPLAYS_RAW="$displays"
    KDE_KWIN_OWNER_LINE=$(printf '%s\n' "$owners" | sed -n '/org.kde.KWin/p' | sed -n '1p')
    KDE_COMPANION_OWNER_LINE="$owner_line"
    log "KDE companion readiness timed out after ${timeout_s}s: owner='$owner_line' isScriptLoaded='$loaded' capabilities='$capabilities' displays='$count'"
    return 1
}

kde_live_wait_script_loaded() {
    local expected=$1 timeout_s=${2:-10}
    local deadline=$((SECONDS + timeout_s))
    local observed=''
    while (( SECONDS < deadline )); do
        observed=$(kde_live_kwin_call isScriptLoaded s window-zones 2>&1 || true)
        if [[ "$expected" == true && "$observed" == *'true'* ]] \
            || [[ "$expected" == false && "$observed" == *'false'* ]]; then
            printf '%s' "$observed"
            return 0
        fi
        sleep 0.2
    done
    printf '%s' "$observed"
    return 1
}

kde_live_load_script() {
    local script=$1 plugin=$2 output='' id=''
    output=$(kde_live_kwin_call loadScript ss "$script" "$plugin" 2>&1 || true)
    id=$(printf '%s\n' "$output" | sed -n 's/^i //p' | sed -n '1p')
    if [[ "$id" =~ ^-?[0-9]+$ ]]; then
        KDE_LAST_SCRIPT_ID="$id"
    else
        KDE_LAST_SCRIPT_ID='-1'
    fi
    log "KDE loadScript $plugin: output='$output' id=$KDE_LAST_SCRIPT_ID"
}

kde_live_run_script() {
    # Script ids are positional (`const int id = scripts.size()` upstream) and are reused after an
    # unload, so /Scripting/Script<id> can address a different, already-running script. Scripting
    # start() runs every loaded script that is not running yet and needs no id.
    local id=${1:-} output=''
    output=$(busctl --address="$KDE_BUS" call org.kde.KWin /Scripting \
        org.kde.kwin.Scripting start 2>&1 || true)
    log "KDE Scripting start (after load id=${id:-none}): $output"
}

kde_live_unload_script() {
    local plugin=$1 output=''
    output=$(kde_live_kwin_call unloadScript s "$plugin" 2>&1 || true)
    log "KDE unloadScript $plugin: $output"
}

kde_live_observer_records() {
    python - "$1" <<'PY'
import json, sys
payload = None
try:
    with open(sys.argv[1], encoding="utf-8") as stream:
        for line in stream:
            marker = "KDE_OBSERVER "
            if marker in line:
                try:
                    payload = json.loads(line.split(marker, 1)[1].strip())
                except json.JSONDecodeError:
                    pass
except OSError:
    pass
if payload:
    for screen in payload.get("screens", []):
        geometry = screen.get("geometry", {})
        area = screen.get("maximizeArea", {})
        print("|".join(str(geometry.get(key, "")) for key in ("x", "y", "width", "height")))
        print("|".join([
            str(screen.get("name", "")),
            *(str(area.get(key, "")) for key in ("x", "y", "width", "height")),
        ]))
PY
}

kde_live_observer_layout_ok() {
    python - "$1" <<'PY'
import sys
records = []
try:
    with open(sys.argv[1], encoding="utf-8") as stream:
        for line in stream:
            fields = line.rstrip("\n").split("|")
            if len(fields) == 4:
                records.append(fields)
except OSError:
    pass
outputs = [tuple(map(int, row)) for row in records if all(item.lstrip("-").isdigit() for item in row)]
ok = len(outputs) == 2 and all(width > 0 and height > 0 for _, _, width, height in outputs)
if ok:
    (ax, ay, aw, ah), (bx, by, bw, bh) = outputs
    ok = not (
        ax < bx + bw and bx < ax + aw
        and ay < by + bh and by < ay + ah
    )
print("1" if ok else "0")
PY
}

kde_live_compare_displays() {
    local companion_file="$KDE_ROOT/companion-displays.raw"
    local observer_file="$KDE_ROOT/observer-records"
    printf '%s\n' "$KDE_DISPLAYS_RAW" >"$companion_file"
    kde_live_parse_displays "$KDE_DISPLAYS_RAW" >"$KDE_ROOT/companion-displays"
    kde_live_observer_records "$KDE_KWIN_LOG" >"$observer_file"
    python - "$KDE_ROOT/companion-displays" "$observer_file" <<'PY'
import sys
companion = {}
with open(sys.argv[1], encoding="utf-8") as stream:
    for line in stream:
        fields = line.rstrip("\n").split("|")
        if len(fields) == 5:
            companion[fields[0]] = tuple(map(int, fields[1:]))
observer = {}
with open(sys.argv[2], encoding="utf-8") as stream:
    for line in stream:
        fields = line.rstrip("\n").split("|")
        if len(fields) == 5 and not all(item.lstrip("-").isdigit() for item in fields):
            observer[fields[0]] = tuple(map(int, fields[1:]))
ok = len(companion) == 2 and len(observer) == 2
details = []
for name, rect in companion.items():
    expected = observer.get(name)
    details.append(f"{name}: companion={rect} observer-maximize={expected}")
    if expected is None or rect != expected:
        ok = False
print("; ".join(details))
print("1" if ok else "0")
PY
}

kde_live_observer_script() {
    cat >"$KDE_OBSERVER_JS" <<'EOF'
'use strict';
function rect(value) {
    return {
        x: Math.round(value.x),
        y: Math.round(value.y),
        width: Math.round(value.width),
        height: Math.round(value.height),
    };
}
function report() {
    const screens = [];
    workspace.screens.forEach((output, index) => {
        screens.push({
            name: output.name || `kwin-output-${index}`,
            geometry: rect(output.geometry),
            maximizeArea: rect(workspace.clientArea(
                KWin.MaximizeArea,
                output,
                workspace.currentDesktop)),
        });
    });
    // KWin 6 installs QJSEngine::ConsoleExtension and injects no print(); warnings are the one
    // level Qt logs without extra rules, so console.warn is what reaches the KWin log.
    console.warn(`KDE_OBSERVER ${JSON.stringify({screens})}`);
}
report();
const timer = new QTimer();
timer.interval = 200;
timer.timeout.connect(report);
timer.start();
EOF
}

kde_live_record_versions() {
    record_versions_common
    local outer='unknown' outer_version='unknown' xwayland_version='unavailable'
    local qt_version='unavailable' kf_version='unavailable'
    if [[ -n "${XDG_CURRENT_DESKTOP:-}" ]]; then
        outer="$XDG_CURRENT_DESKTOP"
    elif [[ -n "${XDG_SESSION_DESKTOP:-}" ]]; then
        outer="$XDG_SESSION_DESKTOP"
    fi
    if [[ "$outer" == *GNOME* || "$outer" == *gnome* ]] && which gnome-shell >/dev/null 2>&1; then
        outer_version=$(gnome-shell --version 2>&1 | sed -n '1p' || true)
    else
        outer_version=$(ps -eo args= 2>/dev/null \
            | sed -n '/^\(gnome-shell\|kwin_wayland\|sway\|Hyprland\)[[:space:]]/p' \
            | sed -n '1p' || true)
    fi
    [[ -n "$outer_version" ]] || outer_version='unavailable'
    log "VERSION outer compositor: $outer ($outer_version; XDG_SESSION_DESKTOP=${XDG_SESSION_DESKTOP:-unknown} WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-unknown})"
    log "VERSION kwin_wayland: $KDE_KWIN_VERSION"
    if which Xwayland >/dev/null 2>&1; then
        xwayland_version=$(Xwayland -version 2>&1 | sed -n '1p' || true)
    fi
    if which qtpaths6 >/dev/null 2>&1; then
        qt_version=$(qtpaths6 --qt-version 2>&1 | sed -n '1p' || true)
    elif which qmake6 >/dev/null 2>&1; then
        qt_version=$(qmake6 -query QT_VERSION 2>&1 | sed -n '1p' || true)
    fi
    if which kf6-config >/dev/null 2>&1; then

        kf_version=$(kf6-config --version 2>&1 | sed -n '1p' || true)
    fi
    log "VERSION Xwayland: $xwayland_version"
    log "VERSION Qt6: $qt_version"
    log "VERSION KDE Frameworks: $kf_version"
    log 'INPUT nested transport: Xwayland XTEST -> private KWin EIS (XwaylandEisNoPrompt=true)'
    log 'SESSION nested KWin is a Synthetic session; Release gate status remains deferred - unverified'
}

kde_live_preflight() {
    local missing='' tool='' version_ok=1 help_ok=1
    if ! which kwin_wayland >/dev/null 2>&1; then
        assert_eq 'KDE kwin_wayland installed' 1 0
        log 'BLOCKED KDE live: kwin_wayland is not installed; user must install kwin and its runtime dependencies; installation requires unavailable sudo credentials'
        return 1
    fi
    assert_eq 'KDE kwin_wayland installed' 1 1
    local -a required=(kpackagetool6 kwriteconfig6 busctl gjs xdotool xprop \
        dbus-run-session dbus-update-activation-environment python setsid gnome-text-editor)
    for tool in "${required[@]}"; do
        if which "$tool" >/dev/null 2>&1; then
            assert_eq "KDE preflight $tool installed" 1 1
        else
            missing="$missing $tool"
            assert_eq "KDE preflight $tool installed" 1 0
        fi
    done
    if [[ -n "$missing" ]]; then
        log "BLOCKED KDE live: missing required executables:$missing"
        return 1
    fi

    KDE_KWIN_VERSION=$(kwin_wayland --version 2>&1 || true)
    KDE_KWIN_HELP=$(kwin_wayland --help 2>&1 || true)
    [[ "$KDE_KWIN_VERSION" == *'6.'* ]] || version_ok=0
    assert_text 'KDE kwin_wayland is version 6' '6.' "$KDE_KWIN_VERSION"
    local -a flags=(--virtual --socket --output-count --xwayland --no-lockscreen --no-kactivities)
    for tool in "${flags[@]}"; do
        if [[ "$KDE_KWIN_HELP" != *"$tool"* ]]; then
            help_ok=0
        fi
        assert_text "KDE kwin_wayland supports $tool" "$tool" "$KDE_KWIN_HELP"
    done
    kde_live_record_versions
    if [[ "$version_ok" -ne 1 || "$help_ok" -ne 1 ]]; then
        log 'BLOCKED KDE live: installed kwin_wayland is not a supported KWin 6.x nested fixture'
        return 1
    fi
    return 0
}

kde_live_write_changed_config() {
    cat >"$CONFIG_PATH" <<'EOF'
[[bindings]]
hotkey = "Alt+Ctrl+Up"
action = { type = "move-to-zone", zone = "center-third" }
EOF
}

kde_live_write_publish_client_env() {
    cat >"$KDE_PUBLISH_CLIENT_ENV" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "${DISPLAY:-}" >"$KDE_XDISPLAY_FILE"
printf '%s\n' "${XAUTHORITY:-}" >"$KDE_XAUTHORITY_FILE"
printf '%s\n' "${DBUS_SESSION_BUS_ADDRESS:-}" >"$KDE_CLIENT_BUS_FILE"
EOF
    chmod 700 "$KDE_PUBLISH_CLIENT_ENV"
}

kde_live_start_fixture() {
    kde_live_write_publish_client_env
    cat >"$KDE_LAUNCHER" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$DBUS_SESSION_BUS_ADDRESS" >"$KDE_BUS_FILE"
# Xwayland can activate the EIS portal before the first client starts.
dbus-update-activation-environment "WAYLAND_DISPLAY=$KDE_SOCKET"
"$KDE_COMPANION_BIN" >"$KDE_SERVICE_LOG" 2>&1 &
printf '%s\n' "$!" >"$KDE_SERVICE_PID_FILE"
printf '%s\n' "$$" >"$KDE_KWIN_PID_FILE"
for ((attempt = 0; attempt < 100; attempt++)); do
    service_list=$(busctl --address="$DBUS_SESSION_BUS_ADDRESS" list 2>/dev/null || true)
    [[ "$service_list" == *'org.window_zones.KWin'* ]] && break
    sleep 0.1
done
# The virtual framebuffer backend keeps the two outputs at a fixed size. A nested windowed backend
# would make them windows of the outer compositor, which is free to resize them mid-run and did.
exec kwin_wayland \
    --virtual \
    --socket "$KDE_SOCKET" \
    --width 1200 --height 900 --scale 1 \
    --output-count 2 --xwayland \
    --no-lockscreen --no-kactivities \
    "$KDE_PUBLISH_CLIENT_ENV"
EOF
    chmod 700 "$KDE_LAUNCHER"
    start_group KDE_GROUP_PID "$KDE_KWIN_LOG" env \
        -u DISPLAY -u XAUTHORITY -u WAYLAND_DISPLAY -u WAYLAND_SOCKET -u KDEHOME \
        BUS_FILE="$KDE_BUS_FILE" KDE_BUS_FILE="$KDE_BUS_FILE" \
        KDE_COMPANION_BIN="$KDE_COMPANION_BIN" KDE_SERVICE_LOG="$KDE_SERVICE_LOG" \
        KDE_SERVICE_PID_FILE="$KDE_SERVICE_PID_FILE" KDE_KWIN_PID_FILE="$KDE_KWIN_PID_FILE" \
        KDE_SOCKET="$KDE_SOCKET" \
        KDE_PUBLISH_CLIENT_ENV="$KDE_PUBLISH_CLIENT_ENV" \
        KDE_XDISPLAY_FILE="$KDE_XDISPLAY_FILE" KDE_XAUTHORITY_FILE="$KDE_XAUTHORITY_FILE" \
        KDE_CLIENT_BUS_FILE="$KDE_CLIENT_BUS_FILE" \
        HOME="$KDE_HOME" XDG_CONFIG_HOME="$KDE_CONFIG" XDG_DATA_HOME="$KDE_DATA" \
        XDG_CACHE_HOME="$KDE_CACHE" XDG_STATE_HOME="$KDE_STATE" \
        XDG_RUNTIME_DIR="$KDE_RUNTIME" XDG_DATA_DIRS="$KDE_DATA_DIRS" \
        XDG_CURRENT_DESKTOP=KDE XDG_SESSION_DESKTOP=KDE DESKTOP_SESSION=plasma \
        KDE_SESSION_VERSION=6 XDG_SESSION_TYPE=wayland \
        QT_LOGGING_RULES='kwin_libeis.debug=true;kwin_scripting.debug=true' \
        QT_FORCE_STDERR_LOGGING=1 \
        dbus-run-session -- "$KDE_LAUNCHER"
    if wait_for_file "$KDE_BUS_FILE" 20; then
        KDE_BUS=$(cat "$KDE_BUS_FILE")
        export DBUS_SESSION_BUS_ADDRESS="$KDE_BUS"
    else
        assert_text 'KDE private D-Bus session starts' 'unix:' "$(cat "$KDE_KWIN_LOG" 2>/dev/null || true)"
        return 1
    fi
    if wait_for_file "$KDE_SERVICE_PID_FILE" 10; then
        KDE_SERVICE_PID=$(cat "$KDE_SERVICE_PID_FILE")
        printf '%s\n' "$KDE_SERVICE_PID" >>"$PID_FILE"
    fi
    if wait_for_file "$KDE_KWIN_PID_FILE" 10; then
        KDE_KWIN_PID=$(cat "$KDE_KWIN_PID_FILE")
        printf '%s\n' "$KDE_KWIN_PID" >>"$PID_FILE"
    fi
    if ! wait_for_text "$KDE_SERVICE_LOG" 'KWin companion service started' 15; then
        assert_text 'KDE companion service starts before KWin' \
            'KWin companion service started' "$(cat "$KDE_SERVICE_LOG" 2>/dev/null || true)"
    else
        assert_text 'KDE companion service starts before KWin' \
            'KWin companion service started' "$(<"$KDE_SERVICE_LOG")"
    fi
    if wait_for_file "$KDE_XDISPLAY_FILE" 30; then
        KDE_XDISPLAY=$(cat "$KDE_XDISPLAY_FILE")
    else
        KDE_XDISPLAY=''
    fi
    if wait_for_file "$KDE_XAUTHORITY_FILE" 30; then
        KDE_XAUTHORITY=$(cat "$KDE_XAUTHORITY_FILE")
    else
        KDE_XAUTHORITY=''
    fi
    KDE_CLIENT_BUS=$(cat "$KDE_CLIENT_BUS_FILE" 2>/dev/null || true)
    log "KDE nested client environment: DISPLAY=$KDE_XDISPLAY XAUTHORITY=$KDE_XAUTHORITY DBUS_SESSION_BUS_ADDRESS=$KDE_CLIENT_BUS"
    return 0
}

kde_live_prepare_environment() {
    export HOME="$KDE_HOME" XDG_CONFIG_HOME="$KDE_CONFIG" XDG_DATA_HOME="$KDE_DATA" \
        XDG_CACHE_HOME="$KDE_CACHE" XDG_STATE_HOME="$KDE_STATE" XDG_RUNTIME_DIR="$KDE_RUNTIME" \
        XDG_DATA_DIRS="$KDE_DATA_DIRS" XDG_SESSION_TYPE=wayland \
        XDG_CURRENT_DESKTOP=KDE XDG_SESSION_DESKTOP=KDE DESKTOP_SESSION=plasma \
        KDE_SESSION_VERSION=6 WAYLAND_DISPLAY="$KDE_SOCKET" \
        DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" GDK_BACKEND=''
    unset GNOME_DESKTOP_SESSION_ID SWAYSOCK HYPRLAND_INSTANCE_SIGNATURE WAYLAND_SOCKET KDEHOME
}

kde_live_x11_active_window() {
    DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" \
        xdotool getactivewindow 2>/dev/null || printf 'unavailable'
}

kde_live_activate_window() {
    [[ -n "${WINDOW_ID:-}" ]] || return 1
    DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" \
        xdotool windowactivate --sync "$WINDOW_ID" >/dev/null 2>&1 || true
    sleep 0.3
}

kde_live_find_window() {
    WINDOW_ID=''
    local id='' pid=''
    while read -r id; do
        [[ -n "$id" ]] || continue
        pid=$(DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" \
            xdotool getwindowpid "$id" 2>/dev/null || true)
        if [[ "$pid" == "$KDE_EDITOR_PID" ]]; then
            WINDOW_ID="$id"
            return 0
        fi
    done < <(DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" \
        xdotool search --name 'Text Editor' 2>/dev/null || true)
    return 1
}

kde_live_x11_geometry() {
    gtk_frame_geometry "$KDE_XDISPLAY" "$KDE_XAUTHORITY"
}

kde_live_wait_focus() {
    local timeout_s=${1:-15}
    local deadline=$((SECONDS + timeout_s))
    local observed='' focused='' active='' present='' window_id='' x='' y='' width='' height=''
    while (( SECONDS < deadline )); do
        observed=$(gtk_frame_geometry "$KDE_XDISPLAY" "$KDE_XAUTHORITY")
        focused=$(kde_live_get_focused)
        active=$(kde_live_x11_active_window)
        IFS='|' read -r present window_id x y width height <<<"$focused"
        if [[ "$observed" != unavailable && "$present" == true \
            && "$active" == "$WINDOW_ID" && "$observed" == "$x,$y,$width,$height" ]]; then
            KDE_FOCUS_X11="$observed"
            KDE_FOCUS_COMPANION="$focused"
            KDE_FOCUS_ACTIVE="$active"
            return 0
        fi
        sleep 0.2
    done
    KDE_FOCUS_X11="$observed"
    KDE_FOCUS_COMPANION="$focused"
    KDE_FOCUS_ACTIVE="$active"
    return 1
}

kde_live_assert_focus() {
    local focused="$KDE_FOCUS_COMPANION" observed="$KDE_FOCUS_X11" active="$KDE_FOCUS_ACTIVE"
    local present='' window_id='' x='' y='' width='' height=''
    IFS='|' read -r present window_id x y width height <<<"$focused"
    log "OBSERVED KDE focus: companion='$focused' xdotool='$observed' active='$active' window='${WINDOW_ID:-unavailable}'"
    assert_eq 'KDE GetFocusedWindow reports test window' true "$present"
    assert_eq 'KDE focused window has stable identity' 1 "$([[ -n "$window_id" ]] && printf 1 || printf 0)"
    assert_eq 'KDE focus geometry agrees with xdotool' "$observed" "$x,$y,$width,$height"
    assert_eq 'KDE focus active nested X11 window' "$WINDOW_ID" "$active"
    local display_match=0 id='' output_x output_y output_width output_height
    local center_x=$((x + width / 2)) center_y=$((y + height / 2))
    while IFS='|' read -r id output_x output_y output_width output_height; do
        if (( center_x >= output_x && center_x < output_x + output_width \
            && center_y >= output_y && center_y < output_y + output_height )); then
            display_match=1
            break
        fi
    done <"$KDE_ROOT/observer-records"
    assert_eq 'KDE focused frame center is on observer output' 1 "$display_match"
}

kde_live_select_display_targets() {
    local focused='' present='' ignored='' id='' x='' y='' width='' height='' record=''
    local center_x=0 center_y=0
    local -a records=()
    focused=$(kde_live_get_focused)
    IFS='|' read -r present ignored x y width height <<<"$focused"
    if [[ "$present" == true ]]; then
        center_x=$((x + width / 2))
        center_y=$((y + height / 2))
    fi
    while IFS='|' read -r id x y width height; do
        [[ -n "$id" ]] || continue
        [[ "$id" =~ ^-?[0-9]+$ ]] && continue
        records+=("$id|$x|$y|$width|$height")
    done <"$KDE_ROOT/observer-records"
    KDE_A_LINE=''
    KDE_B_LINE=''
    for record in "${records[@]}"; do
        IFS='|' read -r id x y width height <<<"$record"
        if [[ "$present" == true ]] && (( center_x >= x && center_x < x + width \
            && center_y >= y && center_y < y + height )); then
            KDE_A_LINE="$record"
            break
        fi
    done
    # Before the test window exists there is no focused display; fall back to the first output.
    [[ -n "$KDE_A_LINE" ]] || KDE_A_LINE="${records[0]:-}"
    for record in "${records[@]}"; do
        if [[ "$record" != "$KDE_A_LINE" ]]; then
            KDE_B_LINE="$record"
            break
        fi
    done
    IFS='|' read -r KDE_A_ID KDE_A_X KDE_A_Y KDE_A_W KDE_A_H <<<"$KDE_A_LINE"
    IFS='|' read -r KDE_B_ID KDE_B_X KDE_B_Y KDE_B_W KDE_B_H <<<"$KDE_B_LINE"
    KDE_LEFT_HALF="$KDE_A_X,$KDE_A_Y,$((KDE_A_W / 2)),$KDE_A_H"
    KDE_CENTER_THIRD="$((KDE_A_X + KDE_A_W / 3)),$KDE_A_Y,$((KDE_A_W - 2 * (KDE_A_W / 3))),$KDE_A_H"
    KDE_TWO_THIRDS="$KDE_A_X,$KDE_A_Y,$((KDE_A_W - KDE_A_W / 3)),$KDE_A_H"
    KDE_B_TWO_THIRDS="$KDE_B_X,$KDE_B_Y,$((KDE_B_W - KDE_B_W / 3)),$KDE_B_H"
    log "KDE independent observer targets: A=$KDE_A_LINE B=$KDE_B_LINE"
}

kde_live_refresh_targets() {
    # Nested outputs are windows of the outer compositor and can be resized mid-run, so zone
    # expectations are recomputed from the newest observer record before each phase.
    kde_live_observer_records "$KDE_KWIN_LOG" >"$KDE_ROOT/observer-records"
    kde_live_select_display_targets
    log "KDE refreshed targets: A=$KDE_A_LINE B=$KDE_B_LINE"
}

kde_live_zone_rect() {
    # zone name + one observer record -> expected rect, using the App's integer zone arithmetic.
    local zone=$1 x=$2 y=$3 width=$4 height=$5
    case "$zone" in
        left-half) printf '%s,%s,%s,%s' "$x" "$y" "$((width / 2))" "$height" ;;
        center-third)
            printf '%s,%s,%s,%s' "$((x + width / 3))" "$y" "$((width - 2 * (width / 3)))" "$height"
            ;;
        left-two-thirds) printf '%s,%s,%s,%s' "$x" "$y" "$((width - width / 3))" "$height" ;;
        *) printf 'unsupported-zone' ;;
    esac
}

kde_live_assert_zone() {
    # Outputs keep their identity for the whole gate: A is the display the test window started on
    # and B is the other one, so a cross-display move cannot relabel them. Only their rectangles
    # are re-read, from the newest observer record, on every poll.
    local name=$1 zone=$2 target=$3
    local deadline=$((SECONDS + 12)) observed_x11='' focused='' observed_companion=''
    local present='' display='' x='' y='' width='' height=''
    local expected='' ignored='' line='' wanted=''
    if [[ "$target" == B ]]; then
        wanted="$KDE_B_ID"
    else
        wanted="$KDE_A_ID"
    fi
    kde_live_activate_window || true
    while : ; do
        kde_live_observer_records "$KDE_KWIN_LOG" >"$KDE_ROOT/observer-records"
        line=$(sed -n "/^${wanted}|/p" "$KDE_ROOT/observer-records" | sed -n '1p')
        IFS='|' read -r ignored x y width height <<<"$line"
        expected=$(kde_live_zone_rect "$zone" "$x" "$y" "$width" "$height")
        observed_x11=$(gtk_frame_geometry "$KDE_XDISPLAY" "$KDE_XAUTHORITY")
        focused=$(kde_live_get_focused)
        observed_companion="$focused"
        IFS='|' read -r present display x y width height <<<"$focused"
        if [[ "$observed_x11" == "$expected" && "$present" == true \
            && "$x,$y,$width,$height" == "$expected" ]]; then
            break
        fi
        (( SECONDS < deadline )) || break
        sleep 0.2
    done
    log "OBSERVED $name: observer-expected='$expected' companion='$observed_companion' xdotool='$observed_x11'"
    assert_eq "$name independent X11 geometry" "$expected" "$observed_x11"
    assert_eq "$name Companion geometry" "true|$expected" "$present|$x,$y,$width,$height"
}

kde_live_dispatch() {
    local label=$1 hotkey=$2 out="$KDE_ROOT/dispatch-$1.log" rc=0
    if env DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" \
        XDG_RUNTIME_DIR="$KDE_RUNTIME" WAYLAND_DISPLAY="$KDE_SOCKET" \
        DBUS_SESSION_BUS_ADDRESS="$KDE_BUS" XDG_CONFIG_HOME="$KDE_CONFIG" \
        XDG_DATA_HOME="$KDE_DATA" HOME="$KDE_HOME" XDG_SESSION_TYPE=wayland \
        XDG_CURRENT_DESKTOP=KDE XDG_SESSION_DESKTOP=KDE DESKTOP_SESSION=plasma \
        KDE_SESSION_VERSION=6 "$BIN_PATH" --backend auto --config "$CONFIG_PATH" \
        dispatch "$hotkey" >"$out" 2>&1; then
        rc=0
    else
        rc=$?
    fi
    cat "$out" >>"$ASSERT_LOG"
    assert_eq "$label dispatch exit" 0 "$rc"
    assert_text "$label dispatch state" 'Dispatch state: Succeeded' "$(cat "$out" 2>/dev/null || true)"
}

kde_live_inject_key() {
    local sequence=$1
    DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" \
        xdotool key --clearmodifiers "$sequence"
}

kde_live_move() {
    # window x,y,w,h: queue the Companion's MoveWindow directly and wait for the script's result;
    # print `ok` or the completed error.
    local reply='' request='' result='' deadline=$((SECONDS + 10))
    reply=$(kde_live_companion_call MoveWindow siiuu "$1" ${2//,/ } 2>&1 || true)
    request=$(printf '%s\n' "$reply" | sed -n 's/^u //p')
    if [[ -z "$request" ]]; then
        printf '%s' "$reply"
        return 0
    fi
    while (( SECONDS < deadline )); do
        result=$(kde_live_companion_call GetRequestResult s "$request" 2>&1 || true)
        [[ "$result" == 'bss true'* ]] && break
        sleep 0.1
    done
    if [[ "$result" == 'bss true "" ""' ]]; then
        printf 'ok'
    else
        printf '%s' "$result"
    fi
}

kde_live_window_state() {
    # tag window js: run js against the KWin window `w` with that internalId in a throwaway
    # script, then print KWin's own view: tile=<bool> max=<mode> fs=<bool> geo=x,y,w,h.
    local tag=$1 id=$2 body=$3 js="$KDE_ROOT/adr9-$1.js" plugin="window-zones-adr9-$1"
    cat >"$js" <<EOF
'use strict';
const w = Array.prototype.find.call(workspace.windowList(),
    candidate => !candidate.deleted && String(candidate.internalId) === '$id');
if (w) {
    $body
    const g = w.frameGeometry;
    console.warn('KDE_STATE $tag tile=' + !!w.tile + ' max=' + w.maximizeMode
        + ' fs=' + w.fullScreen + ' geo=' + [g.x, g.y, g.width, g.height].map(Math.round).join(','));
} else {
    console.warn('KDE_STATE $tag missing');
}
EOF
    kde_live_load_script "$js" "$plugin"
    kde_live_run_script "$KDE_LAST_SCRIPT_ID"
    wait_for_text "$KDE_KWIN_LOG" "KDE_STATE $tag " 10 || true
    kde_live_unload_script "$plugin"
    sed -n "s/.*KDE_STATE $tag //p" "$KDE_KWIN_LOG" | sed -n '$p'
}

kde_live_adr9_window_checks() {
    # ADR 0009 in nested KWin, observed through Xwayland's xdotool/xprop and KWin's own window
    # state read by a separate script: a captured id outlives a focus change and reports closure;
    # quick-tiled, maximized and fullscreen windows.
    local a="$WINDOW_ID" b='' b_pid='' a_id='' b_id='' out='' before='' b_before='' state=''
    local present='' rest=''
    XON_DISPLAY="$KDE_XDISPLAY"
    XON_XAUTHORITY="$KDE_XAUTHORITY"
    kde_live_refresh_targets
    kde_live_activate_window || true
    IFS='|' read -r present a_id rest <<<"$(kde_live_get_focused)"
    assert_text 'KDE captures the test window id' 'captured' "${a_id:+captured}"
    kde_live_move "$a_id" "$KDE_CENTER_THIRD" >/dev/null
    start_group b_pid "$KDE_ROOT/editor-b.log" env \
        DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" GDK_BACKEND=x11 \
        XDG_RUNTIME_DIR="$KDE_RUNTIME" WAYLAND_DISPLAY="$KDE_SOCKET" \
        DBUS_SESSION_BUS_ADDRESS="$KDE_BUS" XDG_CONFIG_HOME="$KDE_CONFIG" \
        XDG_DATA_HOME="$KDE_DATA" HOME="$KDE_HOME" XDG_SESSION_TYPE=wayland \
        XDG_CURRENT_DESKTOP=KDE XDG_SESSION_DESKTOP=KDE DESKTOP_SESSION=plasma \
        KDE_SESSION_VERSION=6 gnome-text-editor --standalone
    b=$(xwindow_for_pid "$b_pid" 25 || true)
    assert_text 'KDE second test window maps' 'mapped' "${b:+mapped}"
    if [[ -z "$b" || -z "$a_id" ]]; then
        stop_pid "$b_pid"
        return 0
    fi
    xon xdotool windowactivate --sync "$b" >/dev/null 2>&1 || true
    sleep 0.5
    IFS='|' read -r present b_id rest <<<"$(kde_live_get_focused)"
    assert_eq 'KDE focus moves to the second window' "$b" \
        "$(xon xdotool getactivewindow 2>/dev/null || printf unavailable)"
    assert_eq 'KDE Companion reports a different focused id' 1 \
        "$([[ -n "$b_id" && "$b_id" != "$a_id" ]] && printf 1 || printf 0)"
    WINDOW_ID="$b"
    b_before=$(kde_live_x11_geometry)
    WINDOW_ID="$a"
    out=$(kde_live_move "$a_id" "$KDE_LEFT_HALF")
    assert_eq 'KDE captured id move after focus change succeeds' ok "$out"
    assert_eq 'KDE captured id moves the original window' "$KDE_LEFT_HALF" \
        "$(wait_observed kde_live_x11_geometry "$KDE_LEFT_HALF")"
    WINDOW_ID="$b"
    assert_eq 'KDE captured id move leaves the focused window frame' "$b_before" \
        "$(kde_live_x11_geometry)"
    WINDOW_ID="$a"
    assert_eq 'KDE captured id move leaves focus on the second window' "$b" \
        "$(xon xdotool getactivewindow 2>/dev/null || printf unavailable)"

    stop_pid "$b_pid"
    wait_pid "$b_pid" 5 || true
    set +e
    wait_xwindow_gone "$b" 10 || true
    before=$(kde_live_x11_geometry)
    out=$(kde_live_move "$b_id" "$KDE_CENTER_THIRD")
    assert_text 'KDE closed target reports WindowGone' 'WindowGone' "$out"
    sleep 1
    assert_eq 'KDE closed target leaves the other window unchanged' "$before" \
        "$(kde_live_x11_geometry)"

    state=$(kde_live_window_state tile "$a_id" \
        'workspace.activeWindow = w; workspace.slotWindowQuickTileLeft();')
    assert_text 'KDE quick-tiled precondition' 'tile=true' "$state"
    out=$(kde_live_move "$a_id" "$KDE_TWO_THIRDS")
    assert_eq 'KDE quick-tiled window move succeeds' ok "$out"
    assert_text 'KDE quick-tiled window is restored before the frame' \
        "tile=false max=0 fs=false geo=$KDE_TWO_THIRDS" "$(kde_live_window_state tiled-after "$a_id" '')"
    assert_eq 'KDE quick-tiled window gets the target frame' "$KDE_TWO_THIRDS" \
        "$(wait_observed kde_live_x11_geometry "$KDE_TWO_THIRDS")"

    state=$(kde_live_window_state max "$a_id" 'w.setMaximize(true, true);')
    assert_text 'KDE maximized precondition' 'max=3' "$state"
    assert_wm_state 'KDE maximized precondition Xwayland state' "$a" _NET_WM_STATE_MAXIMIZED_HORZ 1
    out=$(kde_live_move "$a_id" "$KDE_CENTER_THIRD")
    assert_eq 'KDE maximized window move succeeds' ok "$out"
    assert_text 'KDE maximized window is restored before the frame' \
        "tile=false max=0 fs=false geo=$KDE_CENTER_THIRD" "$(kde_live_window_state max-after "$a_id" '')"
    assert_wm_state 'KDE maximized window leaves Xwayland maximized state' "$a" _NET_WM_STATE_MAXIMIZED 0
    assert_eq 'KDE maximized window gets the target frame' "$KDE_CENTER_THIRD" \
        "$(wait_observed kde_live_x11_geometry "$KDE_CENTER_THIRD")"

    state=$(kde_live_window_state fs "$a_id" 'w.fullScreen = true;')
    assert_text 'KDE fullscreen precondition' 'fs=true' "$state"
    assert_wm_state 'KDE fullscreen precondition Xwayland state' "$a" _NET_WM_STATE_FULLSCREEN 1
    sleep 1
    before=$(kde_live_x11_geometry)
    out=$(kde_live_move "$a_id" "$KDE_LEFT_HALF")
    assert_text 'KDE fullscreen window move is rejected' \
        'window is fullscreen; leave fullscreen first' "$out"
    assert_text 'KDE fullscreen rejection keeps fullscreen' 'fs=true' \
        "$(kde_live_window_state fs-after "$a_id" '')"
    assert_wm_state 'KDE fullscreen rejection keeps Xwayland state' "$a" _NET_WM_STATE_FULLSCREEN 1
    sleep 1
    assert_eq 'KDE fullscreen rejection keeps the frame' "$before" "$(kde_live_x11_geometry)"
    state=$(kde_live_window_state fs-exit "$a_id" 'w.fullScreen = false;')
    assert_text 'KDE test window leaves fullscreen' 'fs=false' "$state"
    kde_live_move "$a_id" "$KDE_TWO_THIRDS" >/dev/null
    wait_observed kde_live_x11_geometry "$KDE_TWO_THIRDS" >/dev/null
    kde_live_activate_window || true
}

kde_live_atomic_registration() {
    local helper="$KDE_ROOT/atomic-register.js"
    local helper_log="$KDE_ROOT/atomic-register.log"
    local helper_pid='' valid_events_before=0 invalid_events_before=0 invalid_events_after=0
    cat >"$helper" <<'EOF'
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

const address = GLib.getenv('DBUS_SESSION_BUS_ADDRESS');
const valid = GLib.getenv('KDE_VALID_HOTKEY');
const connection = Gio.DBusConnection.new_for_address_sync(
    address,
    Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT |
    Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,
    null,
    null);
const proxy = Gio.DBusProxy.new_sync(
    connection,
    Gio.DBusProxyFlags.NONE,
    null,
    'org.window_zones.KWin',
    '/org/window_zones/KWin',
    'org.window_zones.KWin3',
    null);
const accel = Gio.DBusProxy.new_sync(
    connection, Gio.DBusProxyFlags.NONE, null,
    'org.kde.kglobalaccel', '/kglobalaccel', 'org.kde.KGlobalAccel', null);
function available() {
    return accel.call_sync(
        'isGlobalShortcutAvailable',
        new GLib.Variant('(is)', [0x0d000012, '']),
        Gio.DBusCallFlags.NONE, 5000, null).deep_unpack()[0];
}

function request(keys) {
    const reply = proxy.call_sync(
        'RegisterHotkeys',
        new GLib.Variant('(s)', [JSON.stringify(keys)]),
        Gio.DBusCallFlags.NONE,
        5000,
        null);
    return reply.deep_unpack()[0];
}

function requestResult(id) {
    const reply = proxy.call_sync(
        'GetRequestResult',
        new GLib.Variant('(s)', [String(id)]),
        Gio.DBusCallFlags.NONE,
        5000,
        null);
    return reply.deep_unpack();
}

function waitResult(id, label) {
    let result = [false, '', ''];
    for (let attempt = 0; attempt < 100; attempt++) {
        try {
            result = requestResult(id);
            if (result[0])
                break;
        } catch (_) {
        }
        GLib.usleep(100000);
    }
    print(`${label}_RESULT ${result[0]} ${result[1]} ${result[2]}`);
    return result;
}

function poll(seconds) {
    const deadline = Date.now() + seconds * 1000;
    while (Date.now() < deadline) {
        try {
            const result = proxy.call_sync(
                'GetNextHotkey',
                new GLib.Variant('()', []),
                Gio.DBusCallFlags.NONE,
                5000,
                null).deep_unpack();
            if (result[0])
                print(`EVENT ${result[1]}`);
        } catch (_) {
        }
        GLib.usleep(100000);
    }
}

const validId = request([valid]);
waitResult(validId, 'VALID');
print('VALID_READY');
poll(6);
try {
    const invalidId = request([valid, 'alt+ctrl+f25']);
    waitResult(invalidId, 'INVALID');
} catch (error) {
    // Canonical-vocabulary rejection can happen before the script request is queued.
    print(`INVALID_RESULT true ${error.message}`);
}
print('INVALID_READY');
poll(6);
waitResult(request([]), 'EMPTY');
print(`EMPTY_AVAILABLE ${available()}`);
waitResult(request([valid]), 'READDED');
print(`READDED_AVAILABLE ${available()}`);
waitResult(request(['alt+ctrl+f23']), 'REPLACED');
print(`REPLACED_AVAILABLE ${available()}`);
waitResult(request([valid]), 'RESTORED');
print(`RESTORED_AVAILABLE ${available()}`);
print('LIFECYCLE_READY');
poll(30);
EOF
    : >"$helper_log"
    start_group helper_pid "$helper_log" env \
        DBUS_SESSION_BUS_ADDRESS="$KDE_BUS" KDE_VALID_HOTKEY="$KDE_SAFE_HOTKEY" \
        gjs -m "$helper"
    if wait_for_text "$helper_log" 'VALID_READY' 12; then
        assert_text 'KDE atomic valid registration completes' 'VALID_RESULT true' "$(<"$helper_log")"
    else
        assert_text 'KDE atomic valid registration completes' 'VALID_RESULT true' \
            "$(cat "$helper_log" 2>/dev/null || true)"
    fi
    valid_events_before=$(count_text "$helper_log" "EVENT $KDE_SAFE_HOTKEY")
    local inject_rc=0
    if kde_live_inject_key ctrl+alt+Left; then
        inject_rc=0
    else
        inject_rc=$?
    fi
    assert_rc_zero 'KDE atomic valid accelerator injection' "$inject_rc" 'xdotool XTEST'
    if wait_for_new_text "$helper_log" "EVENT $KDE_SAFE_HOTKEY" "$valid_events_before" 8; then
        assert_text 'KDE atomic valid accelerator reaches Companion' \
            "EVENT $KDE_SAFE_HOTKEY" "$(<"$helper_log")"
    else
        assert_text 'KDE atomic valid accelerator reaches Companion' \
            "EVENT $KDE_SAFE_HOTKEY" "$(cat "$helper_log" 2>/dev/null || true)"
    fi
    if wait_for_text "$helper_log" 'INVALID_READY' 12; then
        assert_text 'KDE atomic rejected accelerator completes' 'INVALID_RESULT true' "$(<"$helper_log")"
        assert_text 'KDE atomic rejected accelerator names f25' 'f25' "$(<"$helper_log")"
    else
        local atomic_log=''
        atomic_log=$(cat "$helper_log" 2>/dev/null || true)
        assert_text 'KDE atomic rejected accelerator completes' 'INVALID_RESULT true' "$atomic_log"
        assert_text 'KDE atomic rejected accelerator names f25' 'f25' "$atomic_log"
    fi
    invalid_events_before=$(count_text "$helper_log" 'EVENT alt+ctrl+f25')
    kde_live_inject_key ctrl+alt+F25 >/dev/null 2>&1 || true
    sleep 0.5
    invalid_events_after=$(count_text "$helper_log" 'EVENT alt+ctrl+f25')
    assert_eq 'KDE atomic rejected accelerator is inactive' "$invalid_events_before" "$invalid_events_after"
    valid_events_before=$(count_text "$helper_log" "EVENT $KDE_SAFE_HOTKEY")
    if kde_live_inject_key ctrl+alt+Left; then
        inject_rc=0
    else
        inject_rc=$?
    fi
    assert_rc_zero 'KDE atomic previous accelerator injection' "$inject_rc" 'xdotool XTEST'
    local previous_active=0
    wait_for_new_text "$helper_log" "EVENT $KDE_SAFE_HOTKEY" "$valid_events_before" 8 \
        && previous_active=1
    assert_eq 'KDE atomic registration preserves previous set' 1 "$previous_active"
    wait_for_text "$helper_log" 'LIFECYCLE_READY' 16 || true
    local lifecycle_log=''
    lifecycle_log=$(cat "$helper_log" 2>/dev/null || true)
    assert_text 'KDE empty set releases native accelerator' 'EMPTY_AVAILABLE true' "$lifecycle_log"
    assert_text 'KDE removed accelerator can be re-added' 'READDED_AVAILABLE false' "$lifecycle_log"
    assert_text 'KDE replacement releases removed native accelerator' 'REPLACED_AVAILABLE true' "$lifecycle_log"
    assert_text 'KDE replacement restores old accelerator without script reload' 'RESTORED_AVAILABLE false' "$lifecycle_log"
    stop_pid "$helper_pid"
    wait_pid "$helper_pid" 5 || true
    local available_after_disconnect='' disconnect_deadline=$((SECONDS + 5))
    while (( SECONDS < disconnect_deadline )); do
        available_after_disconnect=$(busctl --address="$KDE_BUS" call \
            org.kde.kglobalaccel /kglobalaccel org.kde.KGlobalAccel \
            isGlobalShortcutAvailable is 218103826 '' 2>&1 || true)
        [[ "$available_after_disconnect" == 'b true' ]] && break
        sleep 0.1
    done
    assert_eq 'KDE controller disconnect releases native accelerator' 'b true' "$available_after_disconnect"
    set +e
}

kde_live_conflict_registration() {
    KDE_CONFLICT_JS="$KDE_ROOT/conflict.js"
    cat >"$KDE_CONFLICT_JS" <<'EOF'
'use strict';
try {
    registerShortcut(
        'Window Zones conflict incumbent',
        'Window Zones conflict incumbent',
        'Alt+Ctrl+F24',
        () => console.warn('KDE_CONFLICT_FIRED'));
    console.warn('KDE_CONFLICT_READY');
} catch (error) {
    console.warn(`KDE_CONFLICT_ERROR ${error}`);
}
EOF
    kde_live_load_script "$KDE_CONFLICT_JS" window-zones-conflict
    KDE_CONFLICT_SCRIPT_ID="$KDE_LAST_SCRIPT_ID"
    kde_live_run_script "$KDE_CONFLICT_SCRIPT_ID"
    if wait_for_text "$KDE_KWIN_LOG" 'KDE_CONFLICT_READY' 10; then
        assert_text 'KDE conflict fixture registers incumbent' 'KDE_CONFLICT_READY' "$(<"$KDE_KWIN_LOG")"
    else
        assert_text 'KDE conflict fixture registers incumbent' 'KDE_CONFLICT_READY' \
            "$(cat "$KDE_KWIN_LOG" 2>/dev/null || true)"
    fi

    local helper="$KDE_ROOT/conflict-register.js" helper_log="$KDE_ROOT/conflict-register.log"
    local helper_pid='' inject_rc=0 valid_events_before=0 previous_active=0
    cat >"$helper" <<'EOF'
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
const valid = GLib.getenv('KDE_VALID_HOTKEY');
const address = GLib.getenv('DBUS_SESSION_BUS_ADDRESS');
const connection = Gio.DBusConnection.new_for_address_sync(
    address,
    Gio.DBusConnectionFlags.AUTHENTICATION_CLIENT |
    Gio.DBusConnectionFlags.MESSAGE_BUS_CONNECTION,
    null,
    null);
const proxy = Gio.DBusProxy.new_sync(
    connection, Gio.DBusProxyFlags.NONE, null,
    'org.window_zones.KWin', '/org/window_zones/KWin', 'org.window_zones.KWin3', null);
function register(keys, label) {
    const request = proxy.call_sync(
        'RegisterHotkeys',
        new GLib.Variant('(s)', [JSON.stringify(keys)]),
        Gio.DBusCallFlags.NONE,
        5000,
        null).deep_unpack()[0];
    let result = [false, '', ''];
    for (let attempt = 0; attempt < 100; attempt++) {
        result = proxy.call_sync(
            'GetRequestResult',
            new GLib.Variant('(s)', [String(request)]),
            Gio.DBusCallFlags.NONE,
            5000,
            null).deep_unpack();
        if (result[0])
            break;
        GLib.usleep(100000);
    }
    print(`${label}_RESULT ${JSON.stringify(result)}`);
    print(`${label}_READY`);
}

function poll(seconds) {
    const deadline = Date.now() + seconds * 1000;
    while (Date.now() < deadline) {
        try {
            const event = proxy.call_sync(
                'GetNextHotkey',
                new GLib.Variant('()', []),
                Gio.DBusCallFlags.NONE,
                5000,
                null).deep_unpack();
            if (event[0])
                print(`EVENT ${event[1]}`);
        } catch (_) {
        }
        GLib.usleep(100000);
    }
}

register([valid], 'VALID');
poll(6);
register(['alt+ctrl+f24'], 'CONFLICT');
poll(15);
EOF
    : >"$helper_log"
    start_group helper_pid "$helper_log" env \
        DBUS_SESSION_BUS_ADDRESS="$KDE_BUS" KDE_VALID_HOTKEY="$KDE_SAFE_HOTKEY" \
        gjs -m "$helper"
    wait_for_text "$helper_log" 'VALID_READY' 12 || true
    assert_text 'KDE conflict replacement starts with valid set' \
        'VALID_RESULT [true,"",""]' "$(cat "$helper_log" 2>/dev/null || true)"
    valid_events_before=$(count_text "$helper_log" "EVENT $KDE_SAFE_HOTKEY")
    kde_live_inject_key ctrl+alt+Left >/dev/null 2>&1 || true
    wait_for_new_text "$helper_log" "EVENT $KDE_SAFE_HOTKEY" "$valid_events_before" 5 \
        && previous_active=1
    assert_eq 'KDE conflict previous set active before replacement' 1 "$previous_active"
    if wait_for_text "$helper_log" 'CONFLICT_READY' 12; then
        assert_text 'KDE conflicting accelerator result completes' \
            'CONFLICT_RESULT [true,' "$(<"$helper_log")"
    else
        assert_text 'KDE conflicting accelerator result completes' 'CONFLICT_RESULT [true,' \
            "$(cat "$helper_log" 2>/dev/null || true)"
    fi
    assert_text 'KDE conflicting replacement reports conflict error' \
        "KWin rejected shortcut 'alt+ctrl+f24': accelerator is already held by 'Window Zones conflict incumbent'" \
        "$(cat "$helper_log" 2>/dev/null || true)"
    valid_events_before=$(count_text "$helper_log" "EVENT $KDE_SAFE_HOTKEY")
    previous_active=0
    kde_live_inject_key ctrl+alt+Left >/dev/null 2>&1 || true
    wait_for_new_text "$helper_log" "EVENT $KDE_SAFE_HOTKEY" "$valid_events_before" 5 \
        && previous_active=1
    assert_eq 'KDE conflicting replacement preserves previous set' 1 "$previous_active"
    if kde_live_inject_key ctrl+alt+F24; then
        inject_rc=0
    else
        inject_rc=$?
    fi
    assert_rc_zero 'KDE conflicting accelerator injection' "$inject_rc" 'xdotool XTEST'
    if wait_for_text "$KDE_KWIN_LOG" 'KDE_CONFLICT_FIRED' 8; then
        assert_text 'KDE conflict incumbent still receives accelerator' \
            'KDE_CONFLICT_FIRED' "$(<"$KDE_KWIN_LOG")"
    else
        assert_text 'KDE conflict incumbent still receives accelerator' \
            'KDE_CONFLICT_FIRED' "$(cat "$KDE_KWIN_LOG" 2>/dev/null || true)"
    fi
    local conflict_events=0
    conflict_events=$(count_text "$helper_log" 'EVENT alt+ctrl+f24')
    assert_eq 'KDE conflicting accelerator delivers no Companion event' 0 "$conflict_events"
    stop_pid "$helper_pid"
    wait_pid "$helper_pid" 5 || true
    sleep 0.3

    # The user-visible contract: the App must not report a binding as registered when the
    # accelerator belongs to someone else. The raw protocol above only shows what the Companion
    # relayed; this drives the real runtime.
    local conflict_config_log="$KDE_ROOT/conflict-app.log"
    cat >"$CONFIG_PATH" <<EOF
[[bindings]]
hotkey = "alt+ctrl+f24"
action = { type = "move-to-zone", zone = "left-half" }
EOF
    start_fifo_app run auto "$conflict_config_log"
    wait_for_text "$conflict_config_log" 'Hotkey registration refused' 20 || true
    assert_text 'KDE App refuses a conflicting accelerator' \
        'Hotkey registration refused' "$(cat "$conflict_config_log" 2>/dev/null || true)"
    assert_text 'KDE App names the accelerator owner' \
        'Window Zones conflict incumbent' "$(cat "$conflict_config_log" 2>/dev/null || true)"
    stop_app_cleanly 'KDE App with a conflicting accelerator quits cleanly' "$conflict_config_log"
    write_five_bindings_config
    set +e
    kde_live_unload_script window-zones-conflict
}

kde_live_tui_lifecycle() {
    local tui_log="$KDE_ROOT/tui.log" rc=0
    write_five_bindings_config
    start_fifo_app tui auto "$tui_log"
    if wait_for_text "$tui_log" 'Window Zones TUI' 15; then
        assert_text 'KDE TUI starts' 'Window Zones TUI' "$(<"$tui_log")"
    else
        assert_text 'KDE TUI starts' 'Window Zones TUI' "$(cat "$tui_log" 2>/dev/null || true)"
    fi
    tui_control_checks "$tui_log" 'KDE'
    kde_live_activate_window || true
    send_app 'dispatch alt+ctrl+left'
    wait_for_text "$tui_log" 'Last action: alt+ctrl+left' 10 || true
    assert_text 'KDE TUI dispatch action' 'Last action: alt+ctrl+left' \
        "$(cat "$tui_log" 2>/dev/null || true)"
    kde_live_refresh_targets
    kde_live_assert_zone 'KDE TUI dispatch geometry' left-half A
    tui_end KDE "$tui_log"
    set +e
}

run_kde_live() {
    CURRENT_GATE='kde-live'
    KDE_ROOT="$TMP_DIR/kde-live"
    KDE_CONFIG="$KDE_ROOT/config"
    KDE_DATA="$KDE_ROOT/data"
    KDE_CACHE="$KDE_ROOT/cache"
    KDE_STATE="$KDE_ROOT/state"
    KDE_RUNTIME="$KDE_ROOT/runtime"
    KDE_HOME="$KDE_ROOT/home"
    KDE_BUS_FILE="$KDE_ROOT/bus-address"
    KDE_SERVICE_LOG="$KDE_ROOT/companion.log"
    KDE_SERVICE_PID_FILE="$KDE_ROOT/companion.pid"
    KDE_KWIN_PID_FILE="$KDE_ROOT/kwin.pid"
    KDE_KWIN_LOG="$KDE_ROOT/kwin.log"
    KDE_XDISPLAY_FILE="$KDE_ROOT/xdisplay"
    KDE_XAUTHORITY_FILE="$KDE_ROOT/xauthority"
    KDE_CLIENT_BUS_FILE="$KDE_ROOT/client-bus"
    KDE_LAUNCHER="$KDE_ROOT/launch-kwin.sh"
    KDE_PUBLISH_CLIENT_ENV="$KDE_ROOT/publish-client-env.sh"
    KDE_OBSERVER_JS="$KDE_ROOT/observer.js"
    KDE_COMPANION_BIN="$(dirname -- "$BIN_PATH")/window_zones_kwin"
    KDE_SOCKET="wayland-window-zones-kde-$$"
    KDE_DATA_DIRS="${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
    CONFIG_PATH="$KDE_ROOT/config.toml"
    KDE_SAFE_HOTKEY='alt+ctrl+left'
    KDE_INSTALLED_MAIN=''
    KDE_OBSERVER_SCRIPT_ID='-1'
    KDE_CONFLICT_SCRIPT_ID='-1'
    KDE_SERVICE_PID=''
    KDE_KWIN_PID=''
    KDE_EDITOR_PID=''
    APP_PID=''
    APP_FD=''
    WINDOW_ID=''
    mkdir -p "$KDE_ROOT"
    set +e

    if ! kde_live_preflight; then
        set -e
        return 0
    fi

    mkdir -p "$KDE_CONFIG" "$KDE_DATA" "$KDE_CACHE" "$KDE_STATE" "$KDE_RUNTIME" "$KDE_HOME"
    chmod 700 "$KDE_RUNTIME" "$KDE_HOME"
    if ! build_binaries KDE --bin window_zones_kwin; then
        set -e
        return 0
    fi

    local install_output='' install_rc=0
    if install_output=$(env -u KDEHOME HOME="$KDE_HOME" XDG_CONFIG_HOME="$KDE_CONFIG" \
        XDG_DATA_HOME="$KDE_DATA" XDG_CACHE_HOME="$KDE_CACHE" XDG_STATE_HOME="$KDE_STATE" \
        kpackagetool6 --type=KWin/Script --install "$PROJECT_ROOT/kwin-script" 2>&1); then
        install_rc=0
    else
        install_rc=$?
    fi
    log "KDE package install: $install_output"
    assert_rc_zero 'KDE repository KWin package installs' "$install_rc" 'kpackagetool6'

    local config_output='' config_rc=0
    if config_output=$(env -u KDEHOME HOME="$KDE_HOME" XDG_CONFIG_HOME="$KDE_CONFIG" \
        kwriteconfig6 --file kwinrc --group Plugins --key window-zonesEnabled true 2>&1); then
        config_rc=0
    else
        config_rc=$?
    fi
    log "KDE plugin enable config: $config_output"
    assert_rc_zero 'KDE repository KWin script is enabled in private kwinrc' "$config_rc" 'kwriteconfig6 Plugins'
    if config_output=$(env -u KDEHOME HOME="$KDE_HOME" XDG_CONFIG_HOME="$KDE_CONFIG" \
        kwriteconfig6 --file kwinrc --group Xwayland --key XwaylandEisNoPrompt true 2>&1); then
        config_rc=0
    else
        config_rc=$?
    fi
    log "KDE private Xwayland EIS preauthorization: $config_output"
    assert_rc_zero 'KDE private Xwayland EIS preauthorization' "$config_rc" 'kwriteconfig6 Xwayland'

    local package_dir=''
    for package_dir in "$KDE_DATA"/kwin/scripts/window-zones \
        "$KDE_DATA"/kwin-wayland/scripts/window-zones; do
        if [[ -f "$package_dir/metadata.json" && -f "$package_dir/contents/code/main.js" ]]; then
            KDE_INSTALLED_MAIN="$package_dir/contents/code/main.js"
            break
        fi
    done
    local installed_files_ok=0
    if [[ -n "$KDE_INSTALLED_MAIN" ]] \
        && cmp -s "$PROJECT_ROOT/kwin-script/metadata.json" \
            "${KDE_INSTALLED_MAIN%/contents/code/main.js}/metadata.json" \
        && cmp -s "$PROJECT_ROOT/kwin-script/contents/code/main.js" "$KDE_INSTALLED_MAIN"; then
        installed_files_ok=1
    fi
    assert_eq 'KDE installed companion matches repository files' 1 "$installed_files_ok"
    if [[ "$installed_files_ok" -ne 1 ]]; then
        log 'BLOCKED KDE live: installed private KWin package does not match repository metadata/code'
        set -e
        return 0
    fi

    if ! kde_live_start_fixture; then
        assert_eq 'KDE nested KWin fixture starts' 1 0
        set -e
        return 0
    fi
    local kwin_alive=0
    kill -0 "$KDE_KWIN_PID" 2>/dev/null && kwin_alive=1
    assert_eq 'KDE nested KWin fixture starts' 1 "$kwin_alive"
    kde_live_prepare_environment
    local xdisplay_ok=0
    [[ "$KDE_XDISPLAY" == :* ]] && xdisplay_ok=1
    assert_eq 'KDE nested Xwayland DISPLAY is published' 1 "$xdisplay_ok"
    # KWin's rootless Xwayland accepts clients from this user without an auth file, so an empty
    # XAUTHORITY is not a failure; the X11 connection below is the check that matters.
    log "KDE nested Xwayland XAUTHORITY: ${KDE_XAUTHORITY:-<none published>}"
    local x11_root_geometry='' x11_connection_ok=0
    x11_root_geometry=$(DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" \
        xdotool getdisplaygeometry 2>/dev/null || true)
    [[ "$x11_root_geometry" =~ ^[0-9]+[[:space:]]+[0-9]+$ ]] && x11_connection_ok=1
    assert_eq 'KDE nested Xwayland accepts X11 clients' 1 "$x11_connection_ok"
    if which xrandr >/dev/null 2>&1; then
        log "KDE nested Xwayland RandR: $(DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" \
            xrandr --query 2>&1 || true)"
    else
        log 'KDE nested Xwayland RandR: unavailable (optional observer utility not installed)'
    fi

    kde_live_wait_companion_ready 45 || true
    assert_text 'KDE nested KWin owns private D-Bus name' 'org.kde.KWin' "$KDE_KWIN_OWNER_LINE"
    assert_text 'KDE companion owns private D-Bus name' 'org.window_zones.KWin' "$KDE_COMPANION_OWNER_LINE"
    assert_text 'KDE repository KWin script is loaded' 'true' "$KDE_SCRIPT_LOADED_RAW"
    assert_eq 'KDE capability set' 'focused-window|displays|move-resize|hotkeys' "$KDE_CAPABILITIES_RAW"
    local initial_display_lines='' initial_display_count=0
    initial_display_lines=$(kde_live_parse_displays "$KDE_DISPLAYS_RAW")
    initial_display_count=$(printf '%s\n' "$initial_display_lines" | sed '/^$/d' | wc -l | tr -d ' ')
    assert_eq 'KDE GetDisplays reports two outputs at readiness' 2 "$initial_display_count"
    if [[ "$KDE_COMPANION_READY" -ne 1 ]]; then
        log 'BLOCKED KDE live: repository KWin script did not complete its handshake; see nested KWin and Companion logs'
        set -e
        return 0
    fi
    local kwin_diagnostics=''
    kwin_diagnostics=$(sed -n \
        '/Window Zones KWin.*failed/p;/Window Zones KWin.*error/p;/KDE_CONFLICT_ERROR/p' \
        "$KDE_KWIN_LOG" 2>/dev/null || true)
    log "KDE nested KWin script diagnostics: ${kwin_diagnostics:-none}"
    write_five_bindings_config
    local auto_backend_output=''
    auto_backend_output=$(env -u GNOME_DESKTOP_SESSION_ID -u SWAYSOCK \
        -u HYPRLAND_INSTANCE_SIGNATURE DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" \
        XDG_RUNTIME_DIR="$KDE_RUNTIME" WAYLAND_DISPLAY="$KDE_SOCKET" \
        DBUS_SESSION_BUS_ADDRESS="$KDE_BUS" XDG_CONFIG_HOME="$KDE_CONFIG" \
        XDG_DATA_HOME="$KDE_DATA" HOME="$KDE_HOME" XDG_SESSION_TYPE=wayland \
        XDG_CURRENT_DESKTOP=KDE XDG_SESSION_DESKTOP=KDE DESKTOP_SESSION=plasma \
        KDE_SESSION_VERSION=6 "$BIN_PATH" --backend auto --config "$CONFIG_PATH" status 2>&1 || true)
    assert_text 'KDE --backend auto resolves kde-wayland' \
        'Using runtime window backend: kde-wayland' "$auto_backend_output"


    kde_live_observer_script
    kde_live_load_script "$KDE_OBSERVER_JS" window-zones-observer
    KDE_OBSERVER_SCRIPT_ID="$KDE_LAST_SCRIPT_ID"
    local observer_id_ok=0
    [[ "$KDE_OBSERVER_SCRIPT_ID" =~ ^[0-9]+$ ]] && observer_id_ok=1
    assert_eq 'KDE independent observer script loads' 1 "$observer_id_ok"
    kde_live_run_script "$KDE_OBSERVER_SCRIPT_ID"
    if wait_for_text "$KDE_KWIN_LOG" 'KDE_OBSERVER ' 12; then
        assert_text 'KDE independent observer reports screens' 'KDE_OBSERVER ' "$(<"$KDE_KWIN_LOG")"
    else
        assert_text 'KDE independent observer reports screens' 'KDE_OBSERVER ' \
            "$(cat "$KDE_KWIN_LOG" 2>/dev/null || true)"
    fi
    kde_live_observer_records "$KDE_KWIN_LOG" >"$KDE_ROOT/observer-records"
    log "KDE independent observer records: $(cat "$KDE_ROOT/observer-records" 2>/dev/null || true)"
    local observer_layout_ok=0 observer_display_count=0
    observer_display_count=$(python - "$KDE_ROOT/observer-records" <<'PY'
import sys
count = 0
try:
    with open(sys.argv[1], encoding="utf-8") as stream:
        for line in stream:
            fields = line.rstrip("\n").split("|")
            if len(fields) == 4 and all(item.lstrip("-").isdigit() for item in fields):
                count += 1
except OSError:
    pass
print(count)
PY
)
    observer_layout_ok=$(kde_live_observer_layout_ok "$KDE_ROOT/observer-records")
    assert_eq 'KDE independent observer reports two distinct outputs' 1 "$observer_layout_ok"
    assert_eq 'KDE independent observer output count' 2 "$observer_display_count"

    local display_compare='' display_compare_result=''
    display_compare=$(kde_live_compare_displays)
    display_compare_result=$(printf '%s\n' "$display_compare" | sed -n '$p')
    log "KDE GetDisplays versus independent MaximizeArea: $(printf '%s\n' "$display_compare" | sed '$d')"
    if [[ "$observer_layout_ok" -ne 1 || "$observer_display_count" -ne 2 ]]; then
        log 'BLOCKED KDE live: independent observer did not provide two valid nonoverlapping output rectangles'
        set -e
        return 0
    fi
    assert_eq 'KDE GetDisplays matches independent MaximizeArea observer' 1 "$display_compare_result"
    skip_assert 'KDE Plasma panel exclusion' \
        'bare nested KWin runs no plasmashell/panel; this run verifies panel-free per-output areas, not Plasma reserved-area behavior'


    local editor_log="$KDE_ROOT/editor.log" editor_deadline=0 editor_alive=0 window_found=0
    start_group KDE_EDITOR_PID "$editor_log" env \
        DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" GDK_BACKEND=x11 \
        XDG_RUNTIME_DIR="$KDE_RUNTIME" WAYLAND_DISPLAY="$KDE_SOCKET" \
        DBUS_SESSION_BUS_ADDRESS="$KDE_BUS" XDG_CONFIG_HOME="$KDE_CONFIG" \
        XDG_DATA_HOME="$KDE_DATA" HOME="$KDE_HOME" XDG_SESSION_TYPE=wayland \
        XDG_CURRENT_DESKTOP=KDE XDG_SESSION_DESKTOP=KDE DESKTOP_SESSION=plasma \
        KDE_SESSION_VERSION=6 gnome-text-editor --standalone
    kill -0 "$KDE_EDITOR_PID" 2>/dev/null && editor_alive=1
    assert_eq 'KDE test editor process starts' 1 "$editor_alive"
    editor_deadline=$((SECONDS + 25))
    while (( SECONDS < editor_deadline )); do
        if kde_live_find_window; then
            window_found=1
            break
        fi
        sleep 0.2
    done
    assert_eq 'KDE test editor window is observable through Xwayland' 1 "$window_found"
    kde_live_activate_window || true
    kde_live_wait_focus 15 || true
    kde_live_assert_focus
    # Outputs are windows of the outer compositor and can be resized while the fixture starts, and
    # the test window decides which output it opens on, so targets are chosen once the window is
    # focused, from the newest observer record.
    kde_live_observer_records "$KDE_KWIN_LOG" >"$KDE_ROOT/observer-records"
    kde_live_select_display_targets
    log "KDE placement targets after focus: A=$KDE_A_LINE B=$KDE_B_LINE"
    local editor_backend=''
    editor_backend=$(tr '\0' '\n' <"/proc/$KDE_EDITOR_PID/environ" 2>/dev/null \
        | sed -n '/^GDK_BACKEND=/p' | sed -n '1p' || true)
    assert_eq 'KDE test editor uses Xwayland backend' 'GDK_BACKEND=x11' "$editor_backend"

    kde_live_dispatch 'KDE left-half' alt+ctrl+left
    kde_live_assert_zone 'KDE left-half' left-half A
    kde_live_dispatch 'KDE center-third' alt+ctrl+up
    kde_live_assert_zone 'KDE center-third' center-third A
    kde_live_dispatch 'KDE left-two-thirds' alt+ctrl+right
    kde_live_assert_zone 'KDE left-two-thirds' left-two-thirds A
    kde_live_dispatch 'KDE next-display' alt+ctrl+down
    kde_live_assert_zone 'KDE next-display' left-two-thirds B
    kde_live_dispatch 'KDE previous-display' alt+ctrl+shift+right
    kde_live_assert_zone 'KDE previous-display' left-two-thirds A
    kde_live_dispatch 'KDE previous-display wrap' alt+ctrl+shift+right
    kde_live_assert_zone 'KDE previous-display wraps' left-two-thirds B
    kde_live_dispatch 'KDE next-display wrap' alt+ctrl+down
    kde_live_assert_zone 'KDE next-display wraps' left-two-thirds A
    skip_assert 'KDE negative-coordinate output movement' \
        'fixture uses KWin-generated side-by-side nonnegative output origins; no negative-origin layout was configured'

    kde_live_atomic_registration
    kde_live_conflict_registration
    kde_live_adr9_window_checks
    log 'KDE native Wayland placement not exercised; test client used Xwayland'

    write_five_bindings_config
    kde_live_prepare_environment
    local run_log="$KDE_ROOT/run.log" app_bus=''
    start_fifo_app run auto "$run_log"
    if wait_for_text "$run_log" 'Interactive session started' 15; then
        assert_text 'KDE App starts' 'Interactive session started' "$(<"$run_log")"
    else
        assert_text 'KDE App starts' 'Interactive session started' "$(cat "$run_log" 2>/dev/null || true)"
    fi
    app_bus=$(tr '\0' '\n' <"/proc/$APP_PID/environ" 2>/dev/null \
        | sed -n '/^DBUS_SESSION_BUS_ADDRESS=/p' | sed -n '1p' || true)
    assert_eq 'KDE App uses private D-Bus session' "DBUS_SESSION_BUS_ADDRESS=$KDE_BUS" "$app_bus"
    send_app status
    if wait_for_text "$run_log" 'hotkey state: Registered' 12; then
        assert_text 'KDE App hotkeys initially register' 'hotkey state: Registered' "$(<"$run_log")"
    else
        assert_text 'KDE App hotkeys initially register' 'hotkey state: Registered' \
            "$(cat "$run_log" 2>/dev/null || true)"
    fi

    send_app 'dispatch alt+ctrl+up'
    kde_live_refresh_targets
    kde_live_assert_zone 'KDE real hotkey baseline center-third' center-third A
    local inject_rc=0 action_before=0
    action_before=$(count_text "$run_log" 'last action: alt+ctrl+left')
    if kde_live_inject_key ctrl+alt+Left; then
        inject_rc=0
    else
        inject_rc=$?
    fi
    assert_rc_zero 'KDE XTEST accelerator injection' "$inject_rc" 'xdotool key ctrl+alt+Left'
    kde_live_assert_zone 'KDE real XTEST-to-EIS hotkey' left-half A
    send_app status
    if wait_for_new_text "$run_log" 'last action: alt+ctrl+left' "$action_before" 10; then
        assert_text 'KDE real hotkey reports action' 'last action: alt+ctrl+left' "$(<"$run_log")"
    else
        assert_text 'KDE real hotkey reports action' 'last action: alt+ctrl+left' \
            "$(cat "$run_log" 2>/dev/null || true)"
    fi
    skip_assert 'KDE seated accelerator capture' \
        'nested KWin uses a Noop session and does not own the login seat; XTEST/EIS tests compositor dispatch, not physical-seat input or host shortcut arbitration'

    local app_pid_before="$APP_PID" script_unload='' script_unloaded='' unavailable_ok=0
    script_unload=$(kde_live_kwin_call unloadScript s window-zones 2>&1 || true)
    log "KDE repository script unload: $script_unload"
    assert_text 'KDE script disconnect unload call' 'true' "$script_unload"
    script_unloaded=$(kde_live_wait_script_loaded false 15 || true)
    assert_text 'KDE script disconnect unload completes' 'false' "$script_unloaded"
    sleep 6
    wait_for_text "$run_log" 'KWin companion is unavailable' 15 || true
    if file_contains "$run_log" 'KWin companion is unavailable'; then
        unavailable_ok=1
    fi
    assert_eq 'KDE script disconnect surfaces unavailable state' 1 "$unavailable_ok"
    local backend_after='' app_alive=0
    backend_after=$(sed -n 's/^Window backend: //p' "$run_log" | sort -u | tr '\n' ',' | sed 's/,$//')
    assert_eq 'KDE script disconnect keeps native backend' 'kde-wayland' "$backend_after"
    kill -0 "$app_pid_before" 2>/dev/null && app_alive=1
    assert_eq 'KDE App survives script disconnect' 1 "$app_alive"
    kde_live_load_script "$KDE_INSTALLED_MAIN" window-zones
    local reload_id_ok=0 script_reload_ready=0
    [[ "$KDE_LAST_SCRIPT_ID" =~ ^[0-9]+$ ]] && reload_id_ok=1
    assert_eq 'KDE repository script reload starts' 1 "$reload_id_ok"
    kde_live_run_script "$KDE_LAST_SCRIPT_ID"
    kde_live_wait_companion_ready 30 && script_reload_ready=1
    assert_eq 'KDE script reconnect restores capabilities' 1 "$script_reload_ready"
    local app_recovered=0
    wait_for_text "$run_log" 'Hotkey registration now recovered' 20 && app_recovered=1
    assert_eq 'KDE App hotkey registration recovers after script reload' 1 "$app_recovered"
    kill -0 "$app_pid_before" 2>/dev/null && app_alive=1 || app_alive=0
    assert_eq 'KDE App PID unchanged after script reload' 1 "$app_alive"

    local old_service_pid="$KDE_SERVICE_PID" service_stopped=0 restart_log="$KDE_ROOT/companion-restart.log"
    local service_recovered_before
    service_recovered_before=$(count_text "$run_log" 'Hotkey registration now recovered')
    # A killed child stays visible to kill -0 until it is reaped, and a service that ignores
    # SIGTERM would otherwise be reported as stopped only by timeout, so escalate and confirm the
    # bus name is actually gone.
    stop_pid "$old_service_pid"
    wait_pid "$old_service_pid" 5 external || kill -9 "$old_service_pid" 2>/dev/null || true
    wait_pid "$old_service_pid" 5 external || true
    set +e
    local service_owner_gone=0 stop_deadline=$((SECONDS + 10))
    while (( SECONDS < stop_deadline )); do
        if ! busctl --address="$KDE_BUS" list 2>/dev/null | grep -q 'org.window_zones.KWin'; then
            service_owner_gone=1
            break
        fi
        sleep 0.2
    done
    service_stopped=$service_owner_gone
    log "KDE service stop: owner gone=$service_owner_gone pid alive=$(kill -0 "$old_service_pid" 2>/dev/null && printf 1 || printf 0)"
    assert_eq 'KDE Rust service stops independently' 1 "$service_stopped"
    start_group KDE_SERVICE_PID "$restart_log" env -u KDEHOME DBUS_SESSION_BUS_ADDRESS="$KDE_BUS" \
        HOME="$KDE_HOME" XDG_CONFIG_HOME="$KDE_CONFIG" XDG_DATA_HOME="$KDE_DATA" \
        XDG_CACHE_HOME="$KDE_CACHE" XDG_STATE_HOME="$KDE_STATE" XDG_RUNTIME_DIR="$KDE_RUNTIME" \
        XDG_DATA_DIRS="$KDE_DATA_DIRS" "$KDE_COMPANION_BIN"
    if wait_for_text "$restart_log" 'KWin companion service started' 12; then
        assert_text 'KDE Rust service restarts' 'KWin companion service started' "$(<"$restart_log")"
    else
        assert_text 'KDE Rust service restarts' 'KWin companion service started' \
            "$(cat "$restart_log" 2>/dev/null || true)"
    fi
    local service_reload_ready=0
    kde_live_wait_companion_ready 30 && service_reload_ready=1
    assert_eq 'KDE service restart restores capabilities' 1 "$service_reload_ready"
    local service_app_recovered=0
    wait_for_new_text "$run_log" 'Hotkey registration now recovered' "$service_recovered_before" 20 && service_app_recovered=1
    assert_eq 'KDE App hotkey registration recovers after service restart' 1 "$service_app_recovered"
    kill -0 "$app_pid_before" 2>/dev/null && app_alive=1 || app_alive=0
    assert_eq 'KDE App PID unchanged after service restart' 1 "$app_alive"
    send_app 'dispatch alt+ctrl+up'
    kde_live_refresh_targets
    kde_live_assert_zone 'KDE recovered-service baseline center-third' center-third A
    action_before=$(count_text "$run_log" 'last action: alt+ctrl+left')
    kde_live_inject_key ctrl+alt+Left >/dev/null 2>&1 || true
    kde_live_assert_zone 'KDE recovered-service real hotkey moves window' left-half A
    send_app status
    local service_action_recovered=0
    wait_for_new_text "$run_log" 'last action: alt+ctrl+left' "$action_before" 10 && service_action_recovered=1
    assert_eq 'KDE recovered-service real hotkey reports action' 1 "$service_action_recovered"

    send_app 'dispatch alt+ctrl+up'
    kde_live_refresh_targets
    kde_live_assert_zone 'KDE invalid config retention baseline center-third' center-third A
    write_invalid_config
    sleep 0.4
    send_app reload
    sleep 0.3
    send_app reload
    if wait_for_text "$run_log" 'Config reload error' 12; then
        assert_text 'KDE invalid config exposes actionable error' 'Config reload error' "$(<"$run_log")"
    else
        assert_text 'KDE invalid config exposes actionable error' 'Config reload error' \
            "$(cat "$run_log" 2>/dev/null || true)"
    fi
    send_app status
    assert_text 'KDE invalid reload preserves previous binding count' 'binding count: 5' \
        "$(cat "$run_log" 2>/dev/null || true)"
    kde_live_inject_key ctrl+alt+Left >/dev/null 2>&1 || true
    kde_live_assert_zone 'KDE invalid config retains active binding' left-half A

    # ADR 0009: a replacement KWin refuses keeps the old config and its grabs. The conflict
    # incumbent holds Alt+Ctrl+F24, so a config adding it must be refused as a whole.
    local conflict_before=0 refused_before=0 bindings_before=0 hotkeys_before=0
    conflict_before=$(count_text "$KDE_KWIN_LOG" 'KDE_CONFLICT_READY')
    kde_live_load_script "$KDE_CONFLICT_JS" window-zones-conflict
    kde_live_run_script "$KDE_LAST_SCRIPT_ID"
    wait_for_new_text "$KDE_KWIN_LOG" 'KDE_CONFLICT_READY' "$conflict_before" 10 || true
    assert_eq 'KDE rejected replacement incumbent holds the accelerator' 1 \
        "$(( $(count_text "$KDE_KWIN_LOG" 'KDE_CONFLICT_READY') > conflict_before ? 1 : 0 ))"
    refused_before=$(count_text "$run_log" 'were refused')
    write_five_bindings_config
    cat >>"$CONFIG_PATH" <<'EOF'

[[bindings]]
hotkey = "Alt+Ctrl+F24"
action = { type = "move-to-zone", zone = "center-third" }
EOF
    sleep 0.4
    send_app reload
    sleep 0.3
    send_app reload
    wait_for_new_text "$run_log" 'were refused' "$refused_before" 12 || true
    assert_eq 'KDE rejected replacement reports a refused reload' 1 \
        "$(( $(count_text "$run_log" 'were refused') > refused_before ? 1 : 0 ))"
    assert_text 'KDE rejected replacement names the accelerator owner' \
        'Window Zones conflict incumbent' "$(sed -n '/were refused/p' "$run_log")"
    bindings_before=$(count_text "$run_log" 'binding count: 5')
    hotkeys_before=$(count_text "$run_log" 'hotkey state: Registered')
    send_app status
    wait_for_new_text "$run_log" 'binding count: 5' "$bindings_before" 10 || true
    assert_eq 'KDE rejected replacement keeps the previous bindings' 1 \
        "$(( $(count_text "$run_log" 'binding count: 5') > bindings_before ? 1 : 0 ))"
    assert_eq 'KDE rejected replacement keeps hotkeys registered' 1 \
        "$(( $(count_text "$run_log" 'hotkey state: Registered') > hotkeys_before ? 1 : 0 ))"
    send_app 'dispatch alt+ctrl+up'
    kde_live_assert_zone 'KDE rejected replacement baseline center-third' center-third A
    kde_live_inject_key ctrl+alt+Left >/dev/null 2>&1 || true
    kde_live_assert_zone 'KDE rejected replacement keeps the previous grab working' left-half A
    kde_live_unload_script window-zones-conflict
    local loaded_before
    loaded_before=$(count_text "$run_log" 'Config state: Loaded')
    bindings_before=$(count_text "$run_log" 'binding count: 1')
    kde_live_write_changed_config
    sleep 0.4
    send_app reload
    sleep 0.3
    send_app reload
    wait_for_new_text "$run_log" 'Config state: Loaded' "$loaded_before" 30 || true
    assert_eq 'KDE valid changed config recovers' 1 \
        "$(( $(count_text "$run_log" 'Config state: Loaded') > loaded_before ? 1 : 0 ))"
    send_app status
    wait_for_new_text "$run_log" 'binding count: 1' "$bindings_before" 10 || true
    assert_eq 'KDE valid changed config updates binding count' 1 \
        "$(( $(count_text "$run_log" 'binding count: 1') > bindings_before ? 1 : 0 ))"
    loaded_before=$(count_text "$run_log" 'Config state: Loaded')
    bindings_before=$(count_text "$run_log" 'binding count: 5')
    write_five_bindings_config
    sleep 0.4
    send_app reload
    sleep 0.3
    send_app reload
    wait_for_new_text "$run_log" 'Config state: Loaded' "$loaded_before" 30 || true
    assert_eq 'KDE valid config restores bindings' 1 \
        "$(( $(count_text "$run_log" 'Config state: Loaded') > loaded_before ? 1 : 0 ))"
    send_app status
    wait_for_new_text "$run_log" 'binding count: 5' "$bindings_before" 30 || true
    assert_eq 'KDE valid config restores binding count' 1 \
        "$(( $(count_text "$run_log" 'binding count: 5') > bindings_before ? 1 : 0 ))"
    set +e
    stop_app_cleanly 'KDE App quits cleanly' "$run_log"
    set +e

    kde_live_tui_lifecycle
    kde_live_unload_script window-zones-observer
    stop_pid "$KDE_EDITOR_PID"
    wait_pid "$KDE_EDITOR_PID" 5 || true
    set +e
    stop_pid "$KDE_SERVICE_PID"
    stop_pid "$KDE_GROUP_PID"
    set -e
}

run_gnome() {
    CURRENT_GATE='gnome'
    GNOME_ROOT="$TMP_DIR/gnome"
    local output="$TMP_DIR/gnome-native.log" rc=0 checks=0 failures=0
    /usr/bin/python3 "$SCRIPT_DIR/gnome-smoke.py" --binary "$BIN_PATH" \
        --output-dir "$GNOME_ROOT" --keep-logs >"$output" 2>&1 || rc=$?
    log "$(<"$output")"
    if [[ -f "$GNOME_ROOT/results.json" ]]; then
        read -r checks failures < <(/usr/bin/python3 -c \
            'import json, sys; rows = json.load(open(sys.argv[1])); print(len(rows), sum(not row["passed"] for row in rows))' \
            "$GNOME_ROOT/results.json")
        ASSERTIONS=$((ASSERTIONS + checks))
        FAILURES=$((FAILURES + failures))
        if [[ "$rc" -ne 0 && "$failures" -eq 0 ]]; then
            assert_rc_zero 'GNOME native runner exit' "$rc" "$output"
        fi
    else
        assert_rc_zero 'GNOME native evidence manifest' 1 "$output"
    fi

}

build_seam_helper() {
    # Gate-time helper over the public crate API, generated and built per run and never shipped:
    # `dispatch` always acts on the focused window, so captured-id moves drive the X11 seam
    # directly; `state` sends the EWMH request a pager would to put a test window into
    # maximized or fullscreen state; `grab` holds an accelerator as another X client.
    local dir="$TMP_DIR/seam-helper" target_dir="${TARGET_DIR:-$TMP_DIR}/smoke-seam-helper"
    local output='' rc=0
    SEAM_HELPER=''
    mkdir -p "$dir/src"
    cat >"$dir/Cargo.toml" <<EOF
[package]
name = "window_zones_smoke_seam"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
window_zones = { path = "$PROJECT_ROOT" }
x11rb = "0.13"

[workspace]
EOF
    # Start from the repository lockfile so the helper builds against the audited dependency set.
    cp "$PROJECT_ROOT/Cargo.lock" "$dir/Cargo.lock"
    cat >"$dir/src/main.rs" <<'EOF'
use std::{env, process, thread, time::Duration};

use window_zones::config::normalize_hotkey;
use window_zones::{
    HotkeySystem, Rect, WindowId, WindowMove, WindowSystem, WindowSystemError, X11HotkeySystem,
    X11WindowSystem,
};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ClientMessageEvent, ConnectionExt, EventMask};

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["focused"] => match X11WindowSystem::new().focused_window() {
            Ok(Some(window)) => println!("{}", window.id),
            Ok(None) => println!("none"),
            Err(error) => println!("error: {error}"),
        },
        ["move", id, x, y, width, height] => {
            let target = Rect {
                x: x.parse().expect("x"),
                y: y.parse().expect("y"),
                width: width.parse().expect("width"),
                height: height.parse().expect("height"),
            };
            match X11WindowSystem::new().move_window(&WindowMove::new(WindowId::new(*id), target)) {
                Ok(()) => println!("ok"),
                Err(WindowSystemError::WindowGone(_)) => println!("gone"),
                Err(error) => println!("error: {error}"),
            }
        }
        ["state", window, action @ ("add" | "remove"), first, rest @ ..] if rest.len() <= 1 => {
            let (conn, screen) = x11rb::connect(None).expect("connect to DISPLAY");
            let root = conn.setup().roots[screen].root;
            let atom = |name: &str| {
                conn.intern_atom(false, name.as_bytes())
                    .expect("intern atom")
                    .reply()
                    .expect("intern atom reply")
                    .atom
            };
            let data = [
                u32::from(*action == "add"),
                atom(*first),
                rest.first().map_or(0, |name| atom(*name)),
                2,
                0,
            ];
            let window: u32 = window.parse().expect("window");
            let event = ClientMessageEvent::new(32, window, atom("_NET_WM_STATE"), data);
            conn.send_event(
                false,
                root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                event,
            )
            .expect("send _NET_WM_STATE")
            .check()
            .expect("_NET_WM_STATE request");
            println!("sent");
        }
        ["grab", hotkey] => {
            let hotkey = normalize_hotkey(hotkey).expect("hotkey");
            let mut hotkeys = X11HotkeySystem::new();
            if let Err(error) = hotkeys.register_hotkeys(&[hotkey.clone()]) {
                println!("error: {error}");
                process::exit(1);
            }
            println!("GRABBED {hotkey}");
            loop {
                let _ = hotkeys.next_hotkey();
                thread::sleep(Duration::from_millis(100));
            }
        }
        _ => {
            eprintln!("usage: focused | move ID X Y WIDTH HEIGHT | state WINDOW add|remove ATOM [ATOM] | grab HOTKEY");
            process::exit(2);
        }
    }
}
EOF
    if output=$(CARGO_TARGET_DIR="$target_dir" cargo build --manifest-path "$dir/Cargo.toml" 2>&1); then
        rc=0
    else
        rc=$?
    fi
    printf '%s\n' "$output" >>"$ASSERT_LOG"
    assert_rc_zero "$1 seam helper builds against the public API" "$rc" "cargo build $dir"
    if [[ "$rc" -eq 0 ]]; then
        SEAM_HELPER="$target_dir/debug/window_zones_smoke_seam"
    fi
    return "$rc"
}

seam() {
    # Runs on the gate's X display; output is one line naming the outcome.
    xon "$SEAM_HELPER" "$@" 2>&1 || true
}

run_x11_adr9_window_checks() {
    # ADR 0009 on a real window manager, observed through xdotool and xprop: a captured id outlives
    # a focus change and reports closure; maximized, vertically maximized (Openbox has no edge
    # tiling, and vertical maximize is the tiled state the X11 adapter sees) and fullscreen
    # windows are handled before any frame is applied.
    local a="$WINDOW_ID" b='' b_pid='' a_id='' b_id='' out='' b_geometry='' before=''
    XON_DISPLAY="$X11_DISPLAY"
    XON_XAUTHORITY=''
    build_seam_helper X11 || return 0
    start_group b_pid "$X11_ROOT/editor-b.log" env DISPLAY="$X11_DISPLAY" GDK_BACKEND=x11 \
        XDG_CURRENT_DESKTOP=GNOME XDG_SESSION_TYPE=x11 HOME="$X11_HOME" \
        XDG_CONFIG_HOME="$X11_CONFIG" XDG_DATA_HOME="$X11_DATA" \
        gnome-text-editor --standalone
    b=$(xwindow_for_pid "$b_pid" 20 || true)
    assert_text 'X11 second test window maps' 'mapped' "${b:+mapped}"
    if [[ -z "$b" ]]; then
        stop_pid "$b_pid"
        return 0
    fi

    xdotool windowactivate --sync "$a" >/dev/null 2>&1 || true
    sleep 0.5
    a_id=$(seam focused)
    assert_eq 'X11 seam captures the focused window id' "$a" "$a_id"
    xdotool windowactivate --sync "$b" >/dev/null 2>&1 || true
    sleep 0.5
    b_id=$(seam focused)
    assert_eq 'X11 focus moves to the second window' "$b" "$b_id"
    WINDOW_ID="$b"
    x11_geometry_stable 5 || true
    b_geometry=$(x11_geometry)
    WINDOW_ID="$a"
    out=$(seam move "$a_id" 1920 0 800 1080)
    assert_eq 'X11 captured id move after focus change succeeds' ok "$out"
    x11_assert_geometry 'x11-captured-id-moves-original-window' '1920,0,800,1080'
    WINDOW_ID="$b"
    assert_eq 'X11 captured id move leaves the focused window frame' "$b_geometry" "$(x11_geometry)"
    WINDOW_ID="$a"
    assert_eq 'X11 captured id move leaves focus on the second window' "$b" \
        "$(xdotool getactivewindow 2>/dev/null || printf unavailable)"

    stop_pid "$b_pid"
    wait_pid "$b_pid" 5 || true
    wait_xwindow_gone "$b" 10 || true
    before=$(x11_geometry)
    out=$(seam move "$b_id" 0 0 960 1080)
    assert_eq 'X11 closed target reports WindowGone' gone "$out"
    sleep 1
    assert_eq 'X11 closed target leaves the other window unchanged' "$before" "$(x11_geometry)"

    seam state "$a" add _NET_WM_STATE_MAXIMIZED_VERT _NET_WM_STATE_MAXIMIZED_HORZ >/dev/null
    assert_wm_state 'X11 maximized precondition vertical' "$a" _NET_WM_STATE_MAXIMIZED_VERT 1
    assert_wm_state 'X11 maximized precondition horizontal' "$a" _NET_WM_STATE_MAXIMIZED_HORZ 1
    out=$(seam move "$a_id" 640 0 640 1080)
    assert_eq 'X11 maximized window move succeeds' ok "$out"
    assert_wm_state 'X11 maximized window is restored before the frame' "$a" _NET_WM_STATE_MAXIMIZED 0
    x11_assert_geometry 'x11-maximized-window-gets-target-frame' '640,0,640,1080'

    seam state "$a" add _NET_WM_STATE_MAXIMIZED_VERT >/dev/null
    assert_wm_state 'X11 vertically maximized precondition' "$a" _NET_WM_STATE_MAXIMIZED_VERT 1
    out=$(seam move "$a_id" 0 0 960 1080)
    assert_eq 'X11 vertically maximized window move succeeds' ok "$out"
    assert_wm_state 'X11 vertically maximized window is restored before the frame' "$a" \
        _NET_WM_STATE_MAXIMIZED 0
    x11_assert_geometry 'x11-vertically-maximized-window-gets-target-frame' '0,0,960,1080'

    seam state "$a" add _NET_WM_STATE_FULLSCREEN >/dev/null
    assert_wm_state 'X11 fullscreen precondition' "$a" _NET_WM_STATE_FULLSCREEN 1
    x11_geometry_stable 5 || true
    before=$(x11_geometry)
    out=$(seam move "$a_id" 640 0 640 1080)
    assert_text 'X11 fullscreen window move is rejected' 'leave fullscreen first' "$out"
    assert_wm_state 'X11 fullscreen rejection keeps fullscreen' "$a" _NET_WM_STATE_FULLSCREEN 1
    sleep 1
    assert_eq 'X11 fullscreen rejection keeps the frame' "$before" "$(x11_geometry)"
    seam state "$a" remove _NET_WM_STATE_FULLSCREEN >/dev/null
    assert_wm_state 'X11 test window leaves fullscreen' "$a" _NET_WM_STATE_FULLSCREEN 0
    x11_geometry_stable 5 || true
    xdotool windowactivate --sync "$a" >/dev/null 2>&1 || true

    # Transactional grabs at the adapter seam: the ignored real-server test against this
    # isolated display. The App-level replacement is checked in the run session below.
    local test_output='' test_rc=0
    if test_output=$(WZ_X11_TEST_DISPLAY="$X11_DISPLAY" cargo test --locked \
        --manifest-path "$PROJECT_ROOT/Cargo.toml" --lib \
        x11_real_server_grabs_are_exclusive_atomic_and_released -- --ignored 2>&1); then
        test_rc=0
    else
        test_rc=$?
    fi
    printf '%s\n' "$test_output" >>"$ASSERT_LOG"
    assert_rc_zero 'X11 real-server grabs are exclusive, atomic and released' "$test_rc" \
        'cargo test --lib x11_real_server_grabs_are_exclusive_atomic_and_released -- --ignored'
    assert_text 'X11 real-server grab test ran' '1 passed' "$test_output"
}

run_x11() {
    CURRENT_GATE='x11'
    X11_ROOT="$TMP_DIR/x11"
    X11_CONFIG="$X11_ROOT/config"
    X11_DATA="$X11_ROOT/data"
    X11_HOME="$X11_ROOT/home"
    X11_RUNTIME="$X11_ROOT/runtime"
    CONFIG_PATH="$X11_ROOT/config.toml"
    SEAM_HELPER=''
    mkdir -p "$X11_CONFIG" "$X11_DATA" "$X11_HOME" "$X11_RUNTIME" "$X11_ROOT"
    chmod 700 "$X11_RUNTIME"
    export XDG_RUNTIME_DIR="$X11_RUNTIME"
    local display_num="${WINDOW_ZONES_SMOKE_DISPLAY:-}"
    if [[ -z "$display_num" ]]; then
        for display_num in $(seq 90 199); do
            if [[ ! -e "/tmp/.X11-unix/X$display_num" && ! -e "/tmp/.X$display_num-lock" ]]; then
                break
            fi
        done
    fi
    if [[ ! "$display_num" =~ ^[0-9]+$ ]] || [[ -e "/tmp/.X11-unix/X$display_num" || -e "/tmp/.X$display_num-lock" ]]; then
        assert_eq 'X11 display is unowned; refusing to attach' 'free display' "$display_num"
        return 1
    fi
    X11_DISPLAY=":$display_num"
    record_versions_x11
    local xvfb_log="$X11_ROOT/xvfb.log" xvfb_pid openbox_log="$X11_ROOT/openbox.log" openbox_pid wm_check=''
    start_group xvfb_pid "$xvfb_log" Xvfb "$X11_DISPLAY" -screen 0 3520x1080x24 -nolisten tcp -ac +extension XTEST
    local deadline=$((SECONDS + 15))
    while (( SECONDS < deadline )) && [[ ! -S "/tmp/.X11-unix/X$display_num" ]]; do sleep 0.1; done
    assert_eq 'X11 Xvfb starts' 0 "$([[ -S "/tmp/.X11-unix/X$display_num" ]] && printf 0 || printf 1)"
    local extension_text=''
    if command -v xdpyinfo >/dev/null 2>&1; then
        extension_text=$(DISPLAY="$X11_DISPLAY" xdpyinfo 2>&1 \
            | sed -n '/number of extensions:/,/default screen number/p' || true)
    else
        extension_text='xdpyinfo unavailable'
    fi
    assert_text 'X11 XTEST extension' 'XTEST' "$extension_text"
    # Distro defaults may grab Ctrl+Alt+arrows; only the explicit competitor
    # below should own a configured chord on this isolated test desktop.
    cat >"$X11_ROOT/openbox.xml" <<'EOF'
<openbox_config xmlns="http://openbox.org/3.4/rc">
  <keyboard/>
</openbox_config>
EOF
    start_group openbox_pid "$openbox_log" env DISPLAY="$X11_DISPLAY" \
        openbox --replace --config-file "$X11_ROOT/openbox.xml"
    deadline=$((SECONDS + 15))
    while (( SECONDS < deadline )); do
        wm_check=$(DISPLAY="$X11_DISPLAY" xprop -root _NET_SUPPORTING_WM_CHECK 2>&1 || true)
        [[ "$wm_check" == *'_NET_SUPPORTING_WM_CHECK(WINDOW)'* ]] && break
        sleep 0.1
    done
    if kill -0 "$openbox_pid" 2>/dev/null; then
        assert_eq 'X11 Openbox process' 1 1
    else
        assert_eq 'X11 Openbox process' 1 0
    fi
    export DISPLAY="$X11_DISPLAY"
    xrandr --setmonitor left 1920/500x1080/300+0+0 screen
    xrandr --setmonitor right 1600/420x1080/300+1920+0 none
    local monitor_text
    monitor_text=$(xrandr --listmonitors 2>&1 || true)
    assert_text 'X11 left logical monitor' 'left 1920/500x1080/300+0+0' "$monitor_text"
    assert_text 'X11 right logical monitor' 'right 1600/420x1080/300+1920+0' "$monitor_text"
    if [[ "$monitor_text" == *'Monitors: 3'* ]]; then
        log 'Harness limitation X11 RandR: Xvfb retains an automatic aggregate screen monitor alongside the two requested logical monitors'
    fi

    export XDG_CONFIG_HOME="$X11_CONFIG" XDG_DATA_HOME="$X11_DATA" HOME="$X11_HOME"
    export XDG_SESSION_TYPE=x11
    unset WAYLAND_DISPLAY
    write_five_bindings_config
    local auto_backend_output=''
    auto_backend_output=$(env DISPLAY="$X11_DISPLAY" XDG_SESSION_TYPE=x11 \
        XDG_CONFIG_HOME="$X11_CONFIG" XDG_DATA_HOME="$X11_DATA" HOME="$X11_HOME" \
        "$BIN_PATH" --backend auto --config "$CONFIG_PATH" status 2>&1 || true)
    assert_text 'X11 auto backend resolves x11' 'Using runtime window backend: x11' \
        "$auto_backend_output"
    local editor_log="$X11_ROOT/editor.log" editor_pid
    start_group editor_pid "$editor_log" env DISPLAY="$X11_DISPLAY" GDK_BACKEND=x11 \
        XDG_CURRENT_DESKTOP=GNOME XDG_SESSION_TYPE=x11 HOME="$X11_HOME" \
        XDG_CONFIG_HOME="$X11_CONFIG" XDG_DATA_HOME="$X11_DATA" \
        gnome-text-editor --standalone
    deadline=$((SECONDS + 20))
    while (( SECONDS < deadline )); do
        find_last_window && break
        sleep 0.2
    done
    assert_text 'X11 test window launched' 'window' "${WINDOW_ID:+window $WINDOW_ID}"
    xdotool windowactivate --sync "$WINDOW_ID" >/dev/null 2>&1 || true
    sleep 0.5
    local active_window
    active_window=$(xdotool getactivewindow 2>/dev/null || printf 'unavailable')
    assert_eq 'X11 test window is _NET_ACTIVE_WINDOW' "$WINDOW_ID" "$active_window"
    if [[ -n "$WINDOW_ID" ]] && ! x11_geometry_stable 5; then
        log "Harness timing: X11 test window geometry did not settle before dispatch (observed '$(x11_geometry)')"
    fi
    sleep 4
    if [[ -z "$WINDOW_ID" ]]; then
        return 0
    fi

    run_x11_dispatch half alt+ctrl+left
    x11_assert_geometry 'x11-left-half' '0,0,960,1080'
    run_x11_dispatch center-third alt+ctrl+up
    x11_assert_geometry 'x11-center-third' '640,0,640,1080'
    run_x11_dispatch reset-before-two-thirds alt+ctrl+left
    x11_assert_geometry 'x11-reset-before-two-thirds' '0,0,960,1080'
    run_x11_dispatch two-thirds alt+ctrl+right
    x11_assert_geometry 'x11-left-two-thirds' '0,0,1280,1080'
    run_x11_dispatch next-display alt+ctrl+down
    x11_assert_geometry 'x11-next-display' '1920,0,1067,1080'
    local geometry center_x
    geometry=$(x11_geometry)
    if [[ "$geometry" =~ ^(-?[0-9]+),(-?[0-9]+),([0-9]+),([0-9]+)$ ]]; then
        center_x=$((BASH_REMATCH[1] + BASH_REMATCH[3] / 2))
    else
        center_x=-1
    fi
    assert_eq 'X11 cross-display observer sees second monitor' 1 "$((center_x >= 1920 && center_x < 3520 ? 1 : 0))"
    run_x11_adr9_window_checks
    # Put the frame back on the first display as a recognized built-in zone so
    # previous-display exercises index-0 wrapping to the last display.
    DISPLAY="$X11_DISPLAY" xdotool windowmove "$WINDOW_ID" 0 0 >/dev/null 2>&1 || true
    DISPLAY="$X11_DISPLAY" xdotool windowsize "$WINDOW_ID" 1280 1080 >/dev/null 2>&1 || true
    DISPLAY="$X11_DISPLAY" xdotool windowactivate --sync "$WINDOW_ID" >/dev/null 2>&1 || true
    wait_x11_geometry '0,0,1280,1080' 5 >/dev/null || true
    run_x11_dispatch previous-display alt+ctrl+shift+right
    x11_assert_geometry 'x11-previous-display' '1920,0,1067,1080'
    # Move back to the left display before the hotkey baseline.
    DISPLAY="$X11_DISPLAY" xdotool windowmove "$WINDOW_ID" 640 0 >/dev/null 2>&1 || true
    DISPLAY="$X11_DISPLAY" xdotool windowsize "$WINDOW_ID" 640 1080 >/dev/null 2>&1 || true
    DISPLAY="$X11_DISPLAY" xdotool windowactivate --sync "$WINDOW_ID" >/dev/null 2>&1 || true
    if ! wait_x11_geometry '640,0,640,1080' 5 >/dev/null; then
        log "Harness setup: could not reset hotkey baseline geometry (observed '$(x11_geometry)')"
    fi

    run_x11_dispatch center-before-hotkey alt+ctrl+up
    x11_assert_geometry 'x11-center-before-hotkey' '640,0,640,1080'

    # Exclusive XGrabKey accelerator capture, injected through XTEST.
    local run_log="$X11_ROOT/run.log"
    start_fifo_app run x11 "$run_log"
    if wait_for_text "$run_log" 'Interactive session started' 15; then
        assert_text 'X11 run starts' 'Interactive session started' "$(<"$run_log")"
    else
        assert_text 'X11 run starts' 'Interactive session started' "$(cat "$run_log" 2>/dev/null || true)"
    fi
    send_app status
    if wait_for_text "$run_log" 'hotkey state: Registered' 8; then
        assert_text 'X11 hotkeys initially register' 'hotkey state: Registered' "$(<"$run_log")"
    else
        assert_text 'X11 hotkeys initially register' 'hotkey state: Registered' "$(cat "$run_log" 2>/dev/null || true)"
    fi
    local app_display=''
    app_display=$(tr '\0' '\n' <"/proc/$APP_PID/environ" 2>/dev/null | sed -n '/^DISPLAY=/p' | sed -n '1p' || true)
    assert_eq 'X11 App DISPLAY' "DISPLAY=$X11_DISPLAY" "$app_display"
    local real_hotkey_geometry='' real_hotkey_geometry_ok=0 real_hotkey_action_ok=0
    local hotkey_deadline=$((SECONDS + 8)) hotkey_attempts=0
    while (( SECONDS < hotkey_deadline )); do
        hotkey_attempts=$((hotkey_attempts + 1))
        x11_inject_hotkey Left || true
        sleep 0.2
        real_hotkey_geometry=$(x11_geometry)
        if [[ "$real_hotkey_geometry" == '0,0,960,1080' ]]; then
            real_hotkey_geometry_ok=1
            break
        fi
    done
    snapshot_x11 x11-real-hotkey
    assert_eq 'X11 real hotkey moves window' '0,0,960,1080' "$real_hotkey_geometry"

    send_app status
    if wait_for_text "$run_log" 'last action: alt+ctrl+left' 8; then
        real_hotkey_action_ok=1
        assert_text 'X11 real hotkey reports action' 'last action: alt+ctrl+left' "$(<"$run_log")"
    else
        assert_text 'X11 real hotkey reports action' 'last action: alt+ctrl+left' "$(cat "$run_log" 2>/dev/null || true)"
    fi
    log "X11 real-hotkey attempts: $hotkey_attempts"
    if (( real_hotkey_geometry_ok == 0 || real_hotkey_action_ok == 0 )); then
        x11_hotkey_diagnostics "$run_log"
    fi

    sleep 0.3
    write_invalid_config
    send_app reload
    sleep 0.5
    send_app reload
    if wait_for_text "$run_log" 'Config reload error' 8; then
        assert_text 'X11 invalid config exposes actionable error' 'Config reload error' "$(<"$run_log")"
    else
        assert_text 'X11 invalid config exposes actionable error' 'Config reload error' "$(cat "$run_log" 2>/dev/null || true)"
    fi
    send_app status
    sleep 0.3
    assert_text 'X11 invalid reload preserves previous bindings' 'binding count: 5' "$(<"$run_log")"
    write_five_bindings_config
    sleep 0.3
    send_app reload
    sleep 0.5
    send_app reload
    assert_text 'X11 config recovers after valid rewrite' 'Config state: Loaded' "$(<"$run_log")"

    # ADR 0009: a replacement set the X server refuses keeps the old config and its grabs.
    local grab_log="$X11_ROOT/competitor-grab.log" grab_pid='' refused_before=0
    local bindings_before=0 registered_before=0 rejected_geometry=''
    if [[ -n "${SEAM_HELPER:-}" ]]; then
        start_group grab_pid "$grab_log" env DISPLAY="$X11_DISPLAY" "$SEAM_HELPER" grab Alt+Ctrl+F8
        wait_for_text "$grab_log" 'GRABBED' 10 || true
        assert_text 'X11 competitor holds alt+ctrl+f8' 'GRABBED alt+ctrl+f8' \
            "$(cat "$grab_log" 2>/dev/null || true)"
        refused_before=$(count_text "$run_log" 'were refused')
        write_five_bindings_config
        cat >>"$CONFIG_PATH" <<'EOF'

[[bindings]]
hotkey = "Alt+Ctrl+F8"
action = { type = "move-to-zone", zone = "center-third" }
EOF
        sleep 0.3
        send_app reload
        sleep 0.5
        send_app reload
        wait_for_new_text "$run_log" 'were refused' "$refused_before" 10 || true
        assert_eq 'X11 rejected replacement reports a refused reload' 1 \
            "$(( $(count_text "$run_log" 'were refused') > refused_before ? 1 : 0 ))"
        assert_text 'X11 rejected replacement names the held accelerator' \
            'alt+ctrl+f8 is already grabbed by another program' "$(cat "$run_log" 2>/dev/null || true)"
        bindings_before=$(count_text "$run_log" 'binding count: 5')
        registered_before=$(count_text "$run_log" 'hotkey state: Registered')
        send_app status
        wait_for_new_text "$run_log" 'binding count: 5' "$bindings_before" 8 || true
        assert_eq 'X11 rejected replacement keeps the previous bindings' 1 \
            "$(( $(count_text "$run_log" 'binding count: 5') > bindings_before ? 1 : 0 ))"
        assert_eq 'X11 rejected replacement keeps hotkeys registered' 1 \
            "$(( $(count_text "$run_log" 'hotkey state: Registered') > registered_before ? 1 : 0 ))"
        seam move "$WINDOW_ID" 640 0 640 1080 >/dev/null
        wait_x11_geometry '640,0,640,1080' 8 >/dev/null || true
        xdotool windowactivate --sync "$WINDOW_ID" >/dev/null 2>&1 || true
        hotkey_deadline=$((SECONDS + 8))
        while (( SECONDS < hotkey_deadline )); do
            x11_inject_hotkey Left || true
            sleep 0.2
            rejected_geometry=$(x11_geometry)
            [[ "$rejected_geometry" == '0,0,960,1080' ]] && break
        done
        assert_eq 'X11 rejected replacement keeps the previous grab working' '0,0,960,1080' \
            "$rejected_geometry"
        stop_pid "$grab_pid"
        wait_pid "$grab_pid" 5 || true
        write_five_bindings_config
    else
        skip_assert 'X11 rejected replacement keeps the previous grab working' \
            'seam helper did not build; see X11 seam helper build assertion'
    fi
    stop_app_cleanly 'X11 run quits cleanly' "$run_log"

    run_tui_lifecycle x11 'x11'
    stop_pid "$editor_pid"
    stop_pid "$openbox_pid"
    stop_pid "$xvfb_pid"
}

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
