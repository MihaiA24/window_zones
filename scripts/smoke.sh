#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
BIN_PATH="$PROJECT_ROOT/target/debug/window_zones"

KEEP_LOGS=0
COMMAND=""
for arg in "$@"; do
    case "$arg" in
        --keep-logs) KEEP_LOGS=1 ;;
        gnome|gnome-live|x11|all)
            if [[ -n "$COMMAND" ]]; then
                printf 'Usage: %s [--keep-logs] {gnome|gnome-live|x11|all}\n' "${BASH_SOURCE[0]}" >&2
                exit 2
            fi
            COMMAND="$arg"
            ;;
        -h|--help)
            printf 'Usage: %s [--keep-logs] {gnome|gnome-live|x11|all}\n' "${BASH_SOURCE[0]}"
            exit 0
            ;;
        *)
            printf 'Unknown argument: %s\nUsage: %s [--keep-logs] {gnome|gnome-live|x11|all}\n' "$arg" "${BASH_SOURCE[0]}" >&2
            exit 2
            ;;
    esac
done
if [[ -z "$COMMAND" ]]; then
    printf 'Usage: %s [--keep-logs] {gnome|gnome-live|x11|all}\n' "${BASH_SOURCE[0]}" >&2
    exit 2
fi

umask 077
TMP_DIR="/tmp/window_zones-smoke.$$"
mkdir -p "$TMP_DIR"
ASSERT_LOG="$TMP_DIR/assertions.log"
: >"$ASSERT_LOG"
PID_FILE="$TMP_DIR/pids"
GROUP_PID_FILE="$TMP_DIR/group-pids"
: >"$PID_FILE"
: >"$GROUP_PID_FILE"
PIDS=()
GROUP_PIDS=()
FAILURES=0
ASSERTIONS=0
CURRENT_GATE=""
LAST_SHOT=""
GNOME_LIVE_ACTIVE=0
GNOME_LIVE_NEEDS_RESTORE=0
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
    local name=$1 expected=$2 observed=$3
    ASSERTIONS=$((ASSERTIONS + 1))
    if [[ "$expected" == "$observed" ]]; then
        log "PASS $name: expected '$expected' observed '$observed'"
    else
        FAILURES=$((FAILURES + 1))
        log "FAIL $name: expected '$expected' observed '$observed'"
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
    log "SKIP $name: $reason"
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
    local file=$1 needle=$2
    grep -c -- "$needle" "$file" 2>/dev/null || printf '0'
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
    local log_file=$1
    shift
    setsid -- "$@" >"$log_file" 2>&1 &
    local pid=$!
    printf '%s\n' "$pid" >>"$PID_FILE"
    printf '%s\n' "$pid" >>"$GROUP_PID_FILE"
    printf '%s\n' "$pid"
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
    local pid=$1 timeout_s=${2:-10} rc state
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
    set +e
    wait "$pid"
    rc=$?
    set -e
    return "$rc"
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
m = re.search(r"\((true|false),\s*'([^']*)',\s*(-?\d+),\s*(-?\d+),\s*(?:uint32\s+)?(\d+),\s*(?:uint32\s+)?(\d+)\)", s)
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
    geometry=$(xdotool getwindowgeometry --shell "$WINDOW_ID" 2>/dev/null || true)
    while IFS='=' read -r key value; do
        case "$key" in
            X) x=$value ;;
            Y) y=$value ;;
            WIDTH) width=$value ;;
            HEIGHT) height=$value ;;
        esac
    done <<<"$geometry"
    if [[ "$x" =~ ^-?[0-9]+$ && "$y" =~ ^-?[0-9]+$ \
        && "$width" =~ ^[0-9]+$ && "$height" =~ ^[0-9]+$ ]]; then
        printf '%s,%s,%s,%s' "$x" "$y" "$width" "$height"
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
    local run_log=$1 app_tail='' app_display='' registered='' registration_log="$TMP_DIR/x11-hotkey-registration.log"
    local registration_rc=0 line
    log 'X11 real-hotkey diagnostics:'
    app_display=$(tr '\0' '\n' <"/proc/$APP_PID/environ" 2>/dev/null | sed -n '/^DISPLAY=/p' | sed -n '1p' || true)
    log "  App environment: ${app_display:-DISPLAY unavailable}"
    app_tail=$(tail -n 80 "$run_log" 2>/dev/null || true)
    log '  App stdout/stderr tail:'
    while IFS= read -r line; do
        log "    $line"
    done <<<"$app_tail"
    if [[ "$app_tail" == *'global hotkey listener is unavailable'* ]]; then
        log '  next_hotkey error: global hotkey listener is unavailable'
    else
        log '  next_hotkey error: none observed in App output'
    fi

    printf 'status\nquit\n' | timeout 5 env DISPLAY="$X11_DISPLAY" XDG_SESSION_TYPE=x11 \
        XDG_CONFIG_HOME="$X11_CONFIG" XDG_DATA_HOME="$X11_DATA" HOME="$X11_HOME" \
        "$BIN_PATH" --backend x11 --config "$CONFIG_PATH" tui >"$registration_log" 2>&1 || registration_rc=$?
    registered=$(sed -n '/^Bindings:/,/^Last action:/p' "$registration_log" 2>/dev/null \
        | sed '/^Last action:/d' || true)
    log '  App parsed registered hotkey set:'
    if [[ -n "$registered" ]]; then
        while IFS= read -r line; do
            [[ -n "$line" ]] && log "    $line"
        done <<<"$registered"
    else
        log "    unavailable (diagnostic TUI exit $registration_rc)"
    fi
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
    if [[ -n "$LAST_SHOT" ]]; then
        ASSERTIONS=$((ASSERTIONS + 1))
        if [[ "$observed" == "$expected" ]]; then
            log "PASS $name: expected '$expected' observed '$observed' (screenshot $LAST_SHOT)"
        else
            FAILURES=$((FAILURES + 1))
            log "FAIL $name: expected '$expected' observed '$observed' (screenshot $LAST_SHOT)"
        fi
    else
        assert_eq "$name" "$expected" "$observed"
        log "Harness limitation $name screenshot: no usable screenshot tool"
    fi
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
    APP_FIFO="$fifo"
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

find_gnome_xwayland() {
    GNOME_XDISPLAY=''
    GNOME_XAUTHORITY=''
    local line
    while IFS= read -r line; do
        if [[ "$line" =~ /Xwayland[[:space:]]+:([0-9]+).*-[aA]uth[[:space:]]([^[:space:]]+) ]]; then
            if [[ "${BASH_REMATCH[2]}" == "$GNOME_ROOT"/* ]]; then
                GNOME_XDISPLAY=":${BASH_REMATCH[1]}"
                GNOME_XAUTHORITY="${BASH_REMATCH[2]}"
                return 0
            fi
        fi
    done < <(ps -eo args=)
    return 1
}

gnome_call() {
    DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" gdbus call --session \
        --dest org.window_zones.Gnome --object-path /org/window_zones/Gnome \
        --method "org.window_zones.Gnome1.$1" "${@:2}"
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

gnome_live_geometry() {
    local geometry key value x='' y='' width='' height=''
    if [[ -z "${WINDOW_ID:-}" ]]; then
        printf 'unavailable'
        return 0
    fi
    geometry=$(DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" \
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
    extents=$(DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" \
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

gnome_live_zone_targets() {
    # A live session can drive several monitors and the test window need not open on the first one,
    # so derive every expectation from the display the Companion reports for the focused window,
    # and derive the cross-display expectation from the display a next-display move lands on.
    local focused present display x y w h id
    local area='' next_area='' found=-1 total=0 index=0
    focused=$(get_gnome_focused)
    IFS='|' read -r present display x y w h <<<"$focused"
    [[ "$present" == 'true' ]] || return 0
    while IFS='|' read -r id x y w h; do
        [[ -n "$id" ]] || continue
        if [[ "$id" == "$display" ]]; then
            found=$total
            area="$x|$y|$w|$h"
        fi
        total=$((total + 1))
    done <<<"$GNOME_LIVE_DISPLAY_LINES"
    (( found >= 0 )) || return 0
    while IFS='|' read -r id x y w h; do
        [[ -n "$id" ]] || continue
        if (( index == (found + 1) % total )); then
            next_area="$x|$y|$w|$h"
            GNOME_LIVE_NEXT_DISPLAY_ID="$id"
        fi
        index=$((index + 1))
    done <<<"$GNOME_LIVE_DISPLAY_LINES"
    GNOME_LIVE_DISPLAY_ID="$display"
    IFS='|' read -r GNOME_LIVE_X GNOME_LIVE_Y GNOME_LIVE_W GNOME_LIVE_H <<<"$area"
    GNOME_LIVE_LEFT_HALF="$GNOME_LIVE_X,$GNOME_LIVE_Y,$((GNOME_LIVE_W / 2)),$GNOME_LIVE_H"
    GNOME_LIVE_CENTER_THIRD="$((GNOME_LIVE_X + GNOME_LIVE_W / 3)),$GNOME_LIVE_Y,$((GNOME_LIVE_W - 2 * (GNOME_LIVE_W / 3))),$GNOME_LIVE_H"
    GNOME_LIVE_TWO_THIRDS="$GNOME_LIVE_X,$GNOME_LIVE_Y,$((GNOME_LIVE_W - GNOME_LIVE_W / 3)),$GNOME_LIVE_H"
    GNOME_LIVE_NEXT_TWO_THIRDS=''
    GNOME_LIVE_NEXT_DISPLAY_ID="${GNOME_LIVE_NEXT_DISPLAY_ID:-}"
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
    if [[ -w /dev/uinput ]]; then
        write_uinput_injector
        if output=$(python3 "$UINPUT_INJECTOR" "$GNOME_SAFE_UINPUT_KEY" 2>&1); then
            rc=0
        else
            rc=$?
        fi
        log "GNOME live uinput hotkey $GNOME_SAFE_HOTKEY: exit $rc${output:+ output '$output'}"
        return "$rc"
    fi
    if output=$(DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" \
        xdotool key --clearmodifiers "$GNOME_SAFE_XDOT_KEY" 2>&1); then
        rc=0
    else
        rc=$?
    fi
    log "GNOME live XTEST hotkey $GNOME_SAFE_HOTKEY: exit $rc${output:+ output '$output'} (/dev/uinput not writable; XTEST does not reach compositor grabs)"
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
    local focused='' observed='' active='' present='' display='' x='' y='' width='' height=''
    GNOME_LIVE_FOCUS_X11=''
    GNOME_LIVE_FOCUS_COMPANION=''
    GNOME_LIVE_FOCUS_ACTIVE=''
    while (( SECONDS < deadline )); do
        observed=$(gnome_live_geometry)
        focused=$(get_gnome_focused)
        active=$(gnome_live_active_window)
        IFS='|' read -r present display x y width height <<<"$focused"
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
    local present='' display='' x='' y='' width='' height='' focused_geometry=''
    IFS='|' read -r present display x y width height <<<"$focused"
    focused_geometry="$x,$y,$width,$height"
    log "OBSERVED GNOME live focus: companion='$focused' xdotool='$observed' active='$active' window='$WINDOW_ID'"
    assert_eq 'GNOME live focused-window present' 'true' "$present"
    assert_eq 'GNOME live focus geometry agrees with xdotool' "$observed" "$focused_geometry"
    assert_eq 'GNOME live focus active X11 window' "$WINDOW_ID" "$active"
    gnome_live_capture focused-window
}

gnome_live_assert_geometry() {
    local name=$1 expected=$2 expected_display=${3:-$GNOME_LIVE_DISPLAY_ID}
    local deadline=$((SECONDS + 10))
    local observed_x11='' observed_companion='' focused=''
    local present='' display='' x='' y='' width='' height=''
    local expected_x='' expected_y='' expected_width='' expected_height=''
    IFS=',' read -r expected_x expected_y expected_width expected_height <<<"$expected"
    gnome_live_activate_window || true
    while (( SECONDS < deadline )); do
        observed_x11=$(gnome_live_geometry)
        focused=$(get_gnome_focused)
        IFS='|' read -r present display x y width height <<<"$focused"
        observed_companion="$focused"
        if [[ "$observed_x11" == "$expected" \
            && "$present" == 'true' \
            && "$display" == "$expected_display" \
            && "$x,$y,$width,$height" == "$expected" ]]; then
            break
        fi
        sleep 0.2
    done
    log "OBSERVED $name: companion='$observed_companion' xdotool='$observed_x11'"
    assert_eq "$name xdotool geometry" "$expected" "$observed_x11"
    assert_eq "$name companion geometry" \
        "true|$expected_display|$expected_x|$expected_y|$expected_width|$expected_height" \
        "$observed_companion"
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

snapshot_gnome() {
    local label=$1
    local path="$TMP_DIR/${CURRENT_GATE}-${label}.png" result=''
    LAST_SHOT=''
    if result=$(gnome_shell_call /org/gnome/Shell/Screenshot org.gnome.Shell.Screenshot.Screenshot false false "$path" 2>&1) \
        && [[ "$result" == "(true,"* ]] && [[ -s "$path" ]]; then
        LAST_SHOT="$path"
    elif find_gnome_xwayland \
        && timeout 5 import -display "$GNOME_XDISPLAY" -window root "$path" >/dev/null 2>&1 \
        && [[ -s "$path" ]]; then
        LAST_SHOT="$path"
    elif find_gnome_xwayland && command -v ffmpeg >/dev/null 2>&1 \
        && timeout 5 env DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" \
            ffmpeg -loglevel error -f x11grab -video_size 3520x1080 -i "$GNOME_XDISPLAY.0" -frames:v 1 "$path" >/dev/null 2>&1 \
        && [[ -s "$path" ]]; then
        LAST_SHOT="$path"
    fi
    if [[ -n "$LAST_SHOT" ]]; then
        log "SCREENSHOT $label: $LAST_SHOT"
    else
        log "SCREENSHOT $label: unavailable (GNOME Shell Screenshot/import/ffmpeg failed)"
    fi
}

get_gnome_focused() {
    local raw
    raw=$(gnome_call GetFocusedWindow 2>&1 || true)
    parse_gnome_focused "$raw"
}

wait_gnome_focused() {
    local expected=$1 timeout_s=${2:-8} observed
    local deadline=$((SECONDS + timeout_s))
    while (( SECONDS < deadline )); do
        observed=$(get_gnome_focused)
        if [[ "$observed" == "$expected" ]]; then
            printf '%s' "$observed"
            return 0
        fi
        sleep 0.1
    done
    printf '%s' "$(get_gnome_focused)"
    return 1
}

gnome_assert_geometry() {
    local name=$1 expected=$2 observed
    observed=$(wait_gnome_focused "$expected" 8 || true)
    snapshot_gnome "$name"
    if [[ -n "$LAST_SHOT" ]]; then
        ASSERTIONS=$((ASSERTIONS + 1))
        if [[ "$observed" == "$expected" ]]; then
            log "PASS $name: expected '$expected' observed '$observed' (screenshot $LAST_SHOT)"
        else
            FAILURES=$((FAILURES + 1))
            log "FAIL $name: expected '$expected' observed '$observed' (screenshot $LAST_SHOT)"
        fi
    else
        assert_eq "$name" "$expected" "$observed"
        log "Harness limitation $name screenshot: no usable screenshot tool"
    fi
}

run_gnome_dispatch() {
    local label=$1 hotkey=$2 rc=0
    local out="$TMP_DIR/${CURRENT_GATE}-dispatch-${label}.log"
    if env XDG_SESSION_TYPE=wayland WAYLAND_DISPLAY="$WAYLAND_DISPLAY" \
        XDG_RUNTIME_DIR="$GNOME_RUNTIME" XDG_CONFIG_HOME="$GNOME_CONFIG" XDG_DATA_HOME="$GNOME_DATA" \
        DCONF_PROFILE="$GNOME_DCONF_PROFILE" DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" \
        XDG_CURRENT_DESKTOP=GNOME GNOME_DESKTOP_SESSION_ID=smoke \
        "$BIN_PATH" --backend wayland --config "$CONFIG_PATH" dispatch "$hotkey" >"$out" 2>&1; then
        rc=0
    else
        rc=$?
    fi
    cat "$out" >>"$ASSERT_LOG"
    assert_eq "$label dispatch exit" 0 "$rc"
    assert_text "$label dispatch state" 'Dispatch state: Succeeded' "$(<"$out")"
}

run_x11_dispatch() {
    local label=$1 hotkey=$2 expected=${3:-} rc=0 attempts=0
    local out="$TMP_DIR/${CURRENT_GATE}-dispatch-${label}.log"
    while :; do
        attempts=$((attempts + 1))
        sleep 2
        DISPLAY="$X11_DISPLAY" xdotool windowactivate --sync "$WINDOW_ID" >/dev/null 2>&1 || true
        sleep 0.5
        log "X11 dispatch $label precondition: active=$(DISPLAY="$X11_DISPLAY" xdotool getactivewindow 2>/dev/null || printf unavailable) geometry=$(x11_geometry)"
        if env DISPLAY="$X11_DISPLAY" XDG_SESSION_TYPE=x11 XDG_CONFIG_HOME="$X11_CONFIG" \
            XDG_DATA_HOME="$X11_DATA" HOME="$X11_HOME" "$BIN_PATH" --backend x11 \
            --config "$CONFIG_PATH" dispatch "$hotkey" >"$out" 2>&1; then
            rc=0
        else
            rc=$?
        fi
        if [[ "$rc" -eq 0 && -n "$expected" && "$attempts" -lt 5 ]] \
            && ! wait_x11_geometry "$expected" 2 >/dev/null; then
            log "Harness timing: $label dispatch did not reach '$expected'; retrying (attempt $((attempts + 1))/5)"
            continue
        fi
        break
    done
    cat "$out" >>"$ASSERT_LOG"
    assert_eq "$label dispatch exit" 0 "$rc"
    assert_text "$label dispatch state" 'Dispatch state: Succeeded' "$(<"$out")"
}


run_gnome_atomic_registration() {
    local helper="$GNOME_ROOT/register.js" monitor_log="$GNOME_ROOT/hotkey-monitor.log" helper_log="$GNOME_ROOT/register.log"
    cat >"$helper" <<'EOF'
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
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
    'org.window_zones.Gnome1',
    null);
proxy.call_sync('RegisterHotkeys', new GLib.Variant('(as)', [['alt+ctrl+left']]), Gio.DBusCallFlags.NONE, 5000, null);
print('VALID_OK');
try {
    proxy.call_sync('RegisterHotkeys', new GLib.Variant('(as)', [['alt+ctrl+left', 'alt+ctrl+F25']]), Gio.DBusCallFlags.NONE, 5000, null);
    print('INVALID_ACCEPTED');
} catch (error) {
    print(`INVALID_REJECTED ${Gio.DBusError.get_remote_error(error)}`);
}
GLib.usleep(15000000);
EOF
    : >"$monitor_log"
    local monitor_pid helper_pid rc=0
    monitor_pid=$(start_group "$monitor_log" env DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" gdbus monitor --session \
        --dest org.window_zones.Gnome --object-path /org/window_zones/Gnome)
    helper_pid=$(start_group "$helper_log" env DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" gjs -m "$helper")
    if ! wait_for_text "$helper_log" 'VALID_OK' 8; then
        assert_text 'GNOME RegisterHotkeys valid set' 'VALID_OK' "$(cat "$helper_log" 2>/dev/null || true)"
        return 0
    fi
    assert_text 'GNOME RegisterHotkeys valid set' 'VALID_OK' "$(<"$helper_log")"
    if wait_for_text "$helper_log" 'INVALID_REJECTED' 8; then
        assert_text 'GNOME RegisterHotkeys rejects invalid set' 'INVALID_REJECTED' "$(<"$helper_log")"
    else
        assert_text 'GNOME RegisterHotkeys rejects invalid set' 'INVALID_REJECTED' "$(cat "$helper_log" 2>/dev/null || true)"
    fi
    if find_gnome_xwayland; then
        : >"$monitor_log"
        env DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" \
            xdotool key --clearmodifiers alt+ctrl+Left >/dev/null 2>&1 || true
        if wait_for_text "$monitor_log" 'HotkeyPressed' 5; then
            assert_text 'GNOME atomic registration keeps previous set' 'alt+ctrl+left' "$(<"$monitor_log")"
        else
            assert_text 'GNOME atomic registration keeps previous set' 'alt+ctrl+left' "$(cat "$monitor_log" 2>/dev/null || true)"
            log 'Harness limitation GNOME hotkey injection: headless Shell has no physical input seat'
        fi
    else
        assert_text 'GNOME atomic registration keeps previous set' 'alt+ctrl+left' 'Xwayland display/auth not found'
        log 'Harness limitation GNOME hotkey injection: no Xwayland observer'
    fi
    stop_pid "$helper_pid"
    stop_pid "$monitor_pid"
}

run_gnome_hotkey_and_disconnect() {
    local run_log="$GNOME_ROOT/run.log" monitor_log="$GNOME_ROOT/app-hotkey-monitor.log" before rc=0
    start_fifo_app run wayland "$run_log"
    if wait_for_text "$run_log" 'Interactive session started' 15; then
        assert_text 'GNOME run starts' 'Interactive session started' "$(<"$run_log")"
    else
        assert_text 'GNOME run starts' 'Interactive session started' "$(cat "$run_log" 2>/dev/null || true)"
    fi
    assert_text 'GNOME hotkeys initially register' 'Hotkey registration initially' "$(cat "$run_log" 2>/dev/null || true)"
    : >"$monitor_log"
    local monitor_pid=''
    if monitor_pid=$(start_group "$monitor_log" env DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" gdbus monitor \
        --session --dest org.window_zones.Gnome --object-path /org/window_zones/Gnome); then
        if find_gnome_xwayland; then
            env DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" \
                xdotool key --clearmodifiers alt+ctrl+Left >/dev/null 2>&1 || true
            if wait_for_text "$monitor_log" 'HotkeyPressed' 5; then
                assert_text 'GNOME companion hotkey signal' 'alt+ctrl+left' "$(<"$monitor_log")"
                send_app status
                sleep 0.5
                assert_text 'GNOME signal resolves configured action' 'last action: alt+ctrl+left' "$(<"$run_log")"
            else
                assert_text 'GNOME companion hotkey signal' 'alt+ctrl+left' "$(cat "$monitor_log" 2>/dev/null || true)"
                log 'Harness limitation GNOME hotkey injection: headless Shell has no physical input seat'
            fi
        else
            assert_text 'GNOME companion hotkey signal' 'alt+ctrl+left' 'Xwayland display/auth not found'
            log 'Harness limitation GNOME hotkey injection: no Xwayland observer'
        fi
    fi
    stop_pid "$monitor_pid"

    local disable_out enable_out service_list
    disable_out=$(gnome_shell_call /org/gnome/Shell org.gnome.Shell.Extensions.DisableExtension window-zones@mihai-a24 2>&1 || true)
    local deadline=$((SECONDS + 10))
    while (( SECONDS < deadline )); do
        service_list=$(DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" busctl --user list 2>&1 || true)
        [[ "$service_list" != *'org.window_zones.Gnome'* ]] && break
        sleep 0.2
    done
    assert_text 'GNOME companion disable call' 'true' "$disable_out"
    assert_text 'GNOME disconnect surfaces explicit unavailable state' 'GNOME companion is unavailable' "$(cat "$run_log" 2>/dev/null || true)"

    enable_out=$(gnome_shell_call /org/gnome/Shell org.gnome.Shell.Extensions.EnableExtension window-zones@mihai-a24 2>&1 || true)
    deadline=$((SECONDS + 15))
    while (( SECONDS < deadline )); do
        service_list=$(DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" busctl --user list 2>&1 || true)
        [[ "$service_list" == *'org.window_zones.Gnome'* ]] && break
        sleep 0.2
    done
    assert_text 'GNOME companion restore call' 'true' "$enable_out"
    if wait_for_text "$run_log" 'Hotkey registration now recovered' 12; then
        assert_text 'GNOME companion recovery without restart' 'Hotkey registration now recovered' "$(<"$run_log")"
    else
        assert_text 'GNOME companion recovery without restart' 'Hotkey registration now recovered' "$(cat "$run_log" 2>/dev/null || true)"
    fi
    stop_app_cleanly 'GNOME run quits cleanly' "$run_log"
}
run_tui_lifecycle() {
    local backend=$1 label=$2
    local tui_log="$TMP_DIR/${CURRENT_GATE}-${label}-tui.log" before after rc=0
    start_fifo_app tui "$backend" "$tui_log"
    if wait_for_text "$tui_log" 'Window Zones TUI' 15; then
        assert_text "$label TUI starts" 'Window Zones TUI' "$(<"$tui_log")"
    else
        assert_text "$label TUI starts" 'Window Zones TUI' "$(cat "$tui_log" 2>/dev/null || true)"
    fi

    before=$(stat -c '%s' "$tui_log" 2>/dev/null || printf '0')
    send_app reload
    if wait_for_size_growth "$tui_log" "$before" 8; then
        assert_text "$label TUI reload" 'Config:' "$(<"$tui_log")"
    else
        assert_text "$label TUI reload" 'Config:' ''
    fi

    before=$(stat -c '%s' "$tui_log" 2>/dev/null || printf '0')
    send_app restart
    if wait_for_size_growth "$tui_log" "$before" 8; then
        assert_text "$label TUI restart" 'Hotkey state:' "$(<"$tui_log")"
    else
        assert_text "$label TUI restart" 'Hotkey state:' ''
    fi

    before=$(stat -c '%s' "$tui_log" 2>/dev/null || printf '0')
    send_app status
    if wait_for_size_growth "$tui_log" "$before" 8; then
        assert_text "$label TUI status" 'Commands: reload' "$(<"$tui_log")"
    else
        assert_text "$label TUI status" 'Commands: reload' ''
    fi

    send_app 'dispatch alt+ctrl+left'
    if wait_for_text "$tui_log" 'Last action: alt+ctrl+left' 8; then
        assert_text "$label TUI dispatch" 'Last action: alt+ctrl+left' "$(<"$tui_log")"
    else
        assert_text "$label TUI dispatch" 'Last action: alt+ctrl+left' "$(cat "$tui_log" 2>/dev/null || true)"
    fi

    send_app quit
    close_app_input
    if [[ -n "${APP_PID:-}" ]]; then
        wait_pid "$APP_PID" 12 || rc=$?
        APP_PID=''
    fi
    assert_rc_zero "$label TUI quit" "$rc" "tui log $tui_log"
}

gnome_live_choose_hotkeys() {
    local schema raw normalized candidate hotkey canonical reverse xdotool_key
    local -a candidates=(
        'cmd+ctrl+left|<Super><Control>Left|Super_L+Control_L+Left'
        'cmd+ctrl+up|<Super><Control>Up|Super_L+Control_L+Up'
        'cmd+ctrl+right|<Super><Control>Right|Super_L+Control_L+Right'
        'cmd+ctrl+down|<Super><Control>Down|Super_L+Control_L+Down'
        'pause|Pause|Pause'
        'alt+pause|<Alt>Pause|Alt_L+Pause'
        'ctrl+pause|<Control>Pause|Control_L+Pause'
    )
    GNOME_RESERVED_ACCELERATORS=''
    log 'GNOME live reserved accelerator enumeration:'
    for schema in org.gnome.desktop.wm.keybindings org.gnome.shell.keybindings \
        org.gnome.mutter.keybindings org.gnome.mutter.wayland.keybindings; do
        raw=$(DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" gsettings list-recursively "$schema" 2>&1 || true)
        normalized=$(printf '%s\n' "$raw" | sed 's/<Primary>/<Control>/g')
        log "  $schema:"
        while IFS= read -r candidate; do
            [[ -n "$candidate" ]] && log "    $candidate"
        done <<<"$normalized"
    done

    GNOME_SAFE_HOTKEYS=()
    GNOME_SAFE_XDOT_KEYS=()
    for candidate in "${candidates[@]}"; do
        IFS='|' read -r hotkey canonical xdotool_key <<<"$candidate"
        reverse=${canonical/<Super><Control>/<Control><Super>}
        if [[ "$GNOME_RESERVED_ACCELERATORS" != *"$canonical"* \
            && "$GNOME_RESERVED_ACCELERATORS" != *"$reverse"* ]]; then
            GNOME_SAFE_HOTKEYS+=("$hotkey")
            GNOME_SAFE_XDOT_KEYS+=("$xdotool_key")
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
    'org.window_zones.Gnome1',
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
    monitor_pid=$(start_group "$monitor_log" env DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" \
        gdbus monitor --session --dest org.window_zones.Gnome \
        --object-path /org/window_zones/Gnome)
    helper_pid=$(start_group "$helper_log" env DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" \
        LIVE_VALID_HOTKEY="$GNOME_SAFE_LEFT_HOTKEY" gjs -m "$helper")
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
        log 'Harness limitation GNOME live atomic pre-error XTEST signal was not observed'
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
    local enabled_after='' after_info='' tiling_after_info=''
    local window_after_enabled=0 tiling_after_enabled=0
    enabled_after=$(gnome_live_enabled_extensions)
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
run_gnome_live_app_checks() {
    local run_log="$GNOME_ROOT/live-run.log"
    local registered=0 real_geometry='' real_ok=0 real_path='none'
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
            real_geometry=$(gnome_live_geometry)
            if [[ "$real_geometry" == "$GNOME_LIVE_LEFT_HALF" ]]; then
                real_ok=1
                real_path='XTEST'
                break
            fi
        done
    else
        log 'GNOME live real-hotkey injection not attempted because the live grab did not register'
    fi
    if [[ "$real_ok" -eq 0 ]]; then
        log "Harness limitation GNOME live XTEST did not move the window after $attempts attempts"
        log "MANUAL ACTION REQUIRED: press $GNOME_SAFE_HOTKEY once in the focused Text Editor window"
        deadline=$((SECONDS + 15))
        while (( SECONDS < deadline )); do
            real_geometry=$(gnome_live_geometry)
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
        real_geometry=$(gnome_live_geometry)
        [[ "$real_geometry" == "$GNOME_LIVE_LEFT_HALF" ]] && break
        sleep 0.2
    done
    assert_eq 'GNOME live invalid config keeps previous binding active' \
        "$GNOME_LIVE_LEFT_HALF" "$real_geometry"
    gnome_live_capture invalid-config-binding

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
    local tui_log="$GNOME_ROOT/live-tui.log" before rc=0
    write_gnome_live_valid_config
    start_fifo_app tui wayland "$tui_log"
    if wait_for_text "$tui_log" 'Window Zones TUI' 15; then
        assert_text 'GNOME live TUI starts' 'Window Zones TUI' "$(<"$tui_log")"
    else
        assert_text 'GNOME live TUI starts' 'Window Zones TUI' \
            "$(cat "$tui_log" 2>/dev/null || true)"
    fi

    before=$(stat -c '%s' "$tui_log" 2>/dev/null || printf '0')
    send_app reload
    if wait_for_size_growth "$tui_log" "$before" 8; then
        assert_text 'GNOME live TUI reload' 'Config:' "$(<"$tui_log")"
    else
        assert_text 'GNOME live TUI reload' 'Config:' ''
    fi

    before=$(stat -c '%s' "$tui_log" 2>/dev/null || printf '0')
    send_app restart
    if wait_for_size_growth "$tui_log" "$before" 8; then
        assert_text 'GNOME live TUI restart' 'Hotkey state:' "$(<"$tui_log")"
    else
        assert_text 'GNOME live TUI restart' 'Hotkey state:' ''
    fi

    before=$(stat -c '%s' "$tui_log" 2>/dev/null || printf '0')
    send_app status
    if wait_for_size_growth "$tui_log" "$before" 8; then
        assert_text 'GNOME live TUI status' 'Commands: reload' "$(<"$tui_log")"
    else
        assert_text 'GNOME live TUI status' 'Commands: reload' ''
    fi

    gnome_live_activate_window || true
    send_app "dispatch $GNOME_SAFE_LEFT_HOTKEY"
    assert_eq 'GNOME live TUI dispatch' \
        "$(sorted_hotkey "$GNOME_SAFE_LEFT_HOTKEY")" \
        "$(sorted_hotkey "$(wait_for_reported_action "$tui_log" 'Last action: ' 10)")"

    send_app quit
    close_app_input
    if [[ -n "${APP_PID:-}" ]]; then
        wait_pid "$APP_PID" 12 || rc=$?
        APP_PID=''
    fi
    assert_rc_zero 'GNOME live TUI quit' "$rc" "tui log $tui_log"
}

run_gnome_live() {
    CURRENT_GATE='gnome-live'
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
    GNOME_LIVE_EXT_PRESENT=0
    GNOME_LIVE_WINDOW_ENABLED_BEFORE=0
    GNOME_LIVE_TILING_ENABLED_BEFORE=0
    GNOME_LIVE_SNAPSHOT_READY=0
    GNOME_LIVE_NEEDS_RESTORE=0
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
        GNOME_LIVE_EXT_PRESENT=1
        if ! cp -a -- "$GNOME_LIVE_EXT_DIR" "$GNOME_LIVE_BACKUP_DIR"; then
            snapshot_ok=0
        fi
    else
        log "SNAPSHOT installed extension directory: absent ($GNOME_LIVE_EXT_DIR)"
    fi
    if [[ "$snapshot_ok" -eq 1 ]]; then
        GNOME_LIVE_SNAPSHOT_READY=1
    else
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
            'org.window_zones.Gnome absent or extension remained INACTIVE on GNOME Shell 50.4'
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
    GNOME_SAFE_XDOT_KEY="${GNOME_SAFE_XDOT_KEYS[0]}"
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

    local editor_launcher="$GNOME_ROOT/launch-editor.sh"
    local editor_log="$GNOME_ROOT/editor.log" editor_pid='' editor_deadline
    cat >"$editor_launcher" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
exec gnome-text-editor --standalone
EOF
    chmod +x "$editor_launcher"
    editor_pid=$(start_group "$editor_log" env \
        DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" GDK_BACKEND=x11 \
        XDG_RUNTIME_DIR="$GNOME_RUNTIME" DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" \
        XDG_CONFIG_HOME="$GNOME_CONFIG" XDG_DATA_HOME="$GNOME_DATA" HOME="$GNOME_HOME" \
        XDG_CURRENT_DESKTOP=GNOME XDG_SESSION_TYPE=wayland "$editor_launcher")
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
            "$GNOME_LIVE_NEXT_TWO_THIRDS" "$GNOME_LIVE_NEXT_DISPLAY_ID"
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

    run_gnome_live_conflicting_binding
    run_gnome_live_atomic_registration
    run_gnome_live_app_checks
    run_gnome_live_tui_lifecycle
    stop_pid "$EDITOR_PID"
    GNOME_LIVE_EDITOR_STOPPED=1
    gnome_live_finish
    set -e
}

run_gnome() {
    CURRENT_GATE='gnome'
    GNOME_ROOT="$TMP_DIR/gnome"
    GNOME_CONFIG="$GNOME_ROOT/config"
    GNOME_DATA="$GNOME_ROOT/data"
    GNOME_RUNTIME="$GNOME_ROOT/runtime"
    GNOME_HOME="$GNOME_ROOT/home"
    GNOME_DCONF_PROFILE="$GNOME_ROOT/dconf-profile"
    CONFIG_PATH="$GNOME_ROOT/config.toml"
    WAYLAND_DISPLAY="wayland-window-zones-smoke-$$"
    export XDG_CONFIG_HOME="$GNOME_CONFIG" XDG_DATA_HOME="$GNOME_DATA" \
        XDG_RUNTIME_DIR="$GNOME_RUNTIME" HOME="$GNOME_HOME" \
        XDG_SESSION_TYPE=wayland WAYLAND_DISPLAY
    mkdir -p "$GNOME_CONFIG/dconf" "$GNOME_DATA" "$GNOME_RUNTIME" "$GNOME_HOME" "$GNOME_ROOT/keyfiles"
    chmod 700 "$GNOME_RUNTIME" "$GNOME_HOME"
    printf 'user-db:smoke\n' >"$GNOME_DCONF_PROFILE"
    printf "[org/gnome/shell]\nenabled-extensions=['window-zones@mihai-a24']\n" >"$GNOME_ROOT/keyfiles/shell"
    dconf compile "$GNOME_CONFIG/dconf/smoke" "$GNOME_ROOT/keyfiles"
    mkdir -p "$GNOME_DATA/gnome-shell/extensions/window-zones@mihai-a24"
    install -m 0644 "$PROJECT_ROOT/gnome-extension/metadata.json" \
        "$GNOME_DATA/gnome-shell/extensions/window-zones@mihai-a24/metadata.json"
    install -m 0644 "$PROJECT_ROOT/gnome-extension/extension.js" \
        "$GNOME_DATA/gnome-shell/extensions/window-zones@mihai-a24/extension.js"
    record_versions_gnome
    assert_eq 'GNOME isolated enabled extensions' "['window-zones@mihai-a24']" \
        "$(DCONF_PROFILE="$GNOME_DCONF_PROFILE" XDG_CONFIG_HOME="$GNOME_CONFIG" dconf read /org/gnome/shell/enabled-extensions)"
    local launcher="$GNOME_ROOT/launch-shell.sh" shell_log="$GNOME_ROOT/gnome-shell.log"
    cat >"$launcher" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$DBUS_SESSION_BUS_ADDRESS" >"$BUS_FILE"
exec gnome-shell --headless --wayland-display "$WAYLAND_DISPLAY" \
    --virtual-monitor 1920x1080 --virtual-monitor 1600x900
EOF
    chmod +x "$launcher"
    BUS_FILE="$GNOME_ROOT/bus-address"
    local shell_pid
    shell_pid=$(start_group "$shell_log" env \
        BUS_FILE="$BUS_FILE" WAYLAND_DISPLAY="$WAYLAND_DISPLAY" \
        XDG_CONFIG_HOME="$GNOME_CONFIG" XDG_DATA_HOME="$GNOME_DATA" \
        XDG_RUNTIME_DIR="$GNOME_RUNTIME" DCONF_PROFILE="$GNOME_DCONF_PROFILE" \
        HOME="$GNOME_HOME" XDG_DATA_DIRS=/usr/local/share:/usr/share \
        XDG_CURRENT_DESKTOP=GNOME XDG_SESSION_TYPE=wayland GNOME_DESKTOP_SESSION_ID=smoke \
        dbus-run-session -- "$launcher")
    if wait_for_file "$BUS_FILE" 20; then
        GNOME_BUS=$(<"$BUS_FILE")
        export DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS"
        if ! env DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" \
            XDG_CONFIG_HOME="$GNOME_CONFIG" XDG_DATA_HOME="$GNOME_DATA" \
            XDG_RUNTIME_DIR="$GNOME_RUNTIME" DCONF_PROFILE="$GNOME_DCONF_PROFILE" \
            HOME="$GNOME_HOME" gnome-extensions enable window-zones@mihai-a24 \
            >>"$shell_log" 2>&1; then
            log 'Harness limitation GNOME extension CLI could not enable window-zones@mihai-a24'
        fi
    else
        assert_text 'GNOME session bus address' 'unix:' "$(cat "$shell_log" 2>/dev/null || true)"
        return 0
    fi
    local service_list='' deadline=$((SECONDS + 30))
    while (( SECONDS < deadline )); do
        service_list=$(DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" busctl --user list 2>&1 || true)
        if [[ "$service_list" == *'org.window_zones.Gnome'* ]]; then
            break
        fi
        sleep 0.2
    done
    local extension_result
    extension_result=$(gnome_shell_call /org/gnome/Shell/Extensions \
        org.gnome.Shell.Extensions.EnableExtension "'window-zones@mihai-a24'" 2>&1 || true)
    log "GNOME extension enable: $extension_result"
    assert_text 'GNOME companion owns D-Bus name' 'org.window_zones.Gnome' "$service_list"

    local capabilities displays_raw display_lines first_display second_display
    capabilities=$(gnome_call GetCapabilities 2>&1 || true)
    assert_eq 'GNOME capability set' "(['focused-window', 'displays', 'move-resize', 'hotkeys'],)" "$capabilities"
    displays_raw=$(gnome_call GetDisplays 2>&1 || true)
    display_lines=$(parse_gnome_displays "$displays_raw" || true)
    first_display=$(printf '%s\n' "$display_lines" | sed -n '1p')
    second_display=$(printf '%s\n' "$display_lines" | sed -n '2p')
    assert_text 'GNOME first virtual display' '|' "$first_display"
    assert_text 'GNOME second virtual display' '|' "$second_display"
    if [[ -z "$first_display" || -z "$second_display" ]]; then
        return 0
    fi
    IFS='|' read -r GNOME_DISPLAY_ID GNOME_X GNOME_Y GNOME_W GNOME_H <<<"$first_display"
    IFS='|' read -r GNOME_DISPLAY2_ID GNOME_X2 GNOME_Y2 GNOME_W2 GNOME_H2 <<<"$second_display"
    GNOME_HALF="$GNOME_X,$GNOME_Y,$((GNOME_W / 2)),$GNOME_H"
    GNOME_CENTER="$((GNOME_X + GNOME_W / 3)),$GNOME_Y,$((GNOME_W - 2 * (GNOME_W / 3))),$GNOME_H"
    GNOME_TWO_THIRDS="$GNOME_X,$GNOME_Y,$((GNOME_W - GNOME_W / 3)),$GNOME_H"
    GNOME_SECOND_HALF="$GNOME_X2,$GNOME_Y2,$((GNOME_W2 / 2)),$GNOME_H2"

    local editor_launcher="$GNOME_ROOT/launch-editor.sh" editor_log="$GNOME_ROOT/editor.log" editor_pid
    cat >"$editor_launcher" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
exec gnome-text-editor --standalone
EOF
    chmod +x "$editor_launcher"
    editor_pid=$(start_group "$editor_log" env \
        WAYLAND_DISPLAY="$WAYLAND_DISPLAY" XDG_RUNTIME_DIR="$GNOME_RUNTIME" \
        DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" XDG_CONFIG_HOME="$GNOME_CONFIG" \
        XDG_DATA_HOME="$GNOME_DATA" HOME="$GNOME_HOME" GDK_BACKEND=wayland \
        XDG_CURRENT_DESKTOP=GNOME XDG_SESSION_TYPE=wayland "$editor_launcher")
    if wait_for_text "$editor_log" 'Text Editor' 15; then
        assert_text 'GNOME test window launched' 'Text Editor' "$(<"$editor_log")"
    else
        # GTK may be quiet on a successful Wayland map; retain the process probe as evidence.
        if kill -0 "$editor_pid" 2>/dev/null; then
            assert_text 'GNOME test window launched' 'running editor pid' "running editor pid $editor_pid"
        else
            assert_text 'GNOME test window launched' 'Text Editor' "$(cat "$editor_log" 2>/dev/null || true)"
        fi
    fi
    # A headless Shell has no seat: GetFocusedWindow stays false and grab_accelerator rejects every
    # accelerator (ADR 0007). Record the seat-dependent checks as skipped instead of failing them;
    # gnome-live carries them.
    local focused
    focused=$(wait_gnome_focused 'true|' 5 || true)
    if [[ "$focused" == 'true|'* ]]; then
        assert_text 'GNOME GetFocusedWindow reports test window' 'true|' "$focused"

        run_gnome_dispatch half alt+ctrl+left
        gnome_assert_geometry 'gnome-left-half' "true|$GNOME_DISPLAY_ID|$GNOME_X|$GNOME_Y|$((GNOME_W / 2))|$GNOME_H"
        run_gnome_dispatch center-third alt+ctrl+up
        gnome_assert_geometry 'gnome-center-third' "true|$GNOME_DISPLAY_ID|$((GNOME_X + GNOME_W / 3))|$GNOME_Y|$((GNOME_W - 2 * (GNOME_W / 3)))|$GNOME_H"
        run_gnome_dispatch two-thirds alt+ctrl+right
        gnome_assert_geometry 'gnome-left-two-thirds' "true|$GNOME_DISPLAY_ID|$GNOME_X|$GNOME_Y|$((GNOME_W - GNOME_W / 3))|$GNOME_H"
        run_gnome_dispatch next-display alt+ctrl+down
        gnome_assert_geometry 'gnome-next-display' "true|$GNOME_DISPLAY2_ID|$GNOME_X2|$GNOME_Y2|$GNOME_W2|$GNOME_H2"

        run_gnome_atomic_registration
        write_valid_config
        run_gnome_hotkey_and_disconnect
    else
        local seatless_reason='not exercised: seatless Synthetic session reports no focused window and rejects every accelerator; covered by gnome-live'
        skip_assert 'GNOME GetFocusedWindow reports test window' "$seatless_reason"
        skip_assert 'GNOME zone placement' "$seatless_reason"
        skip_assert 'GNOME cross-display move' "$seatless_reason"
        skip_assert 'GNOME atomic hotkey registration' "$seatless_reason"
        skip_assert 'GNOME companion hotkey signal' "$seatless_reason"
        skip_assert 'GNOME companion disconnect and recovery' "$seatless_reason"
        write_valid_config
    fi

    # Config reload must retain the last valid bindings while exposing a parse error.
    local run_log="$GNOME_ROOT/reload-run.log" reload_rc=0
    start_fifo_app run wayland "$run_log"
    wait_for_text "$run_log" 'Interactive session started' 15 || true
    sleep 0.3
    write_invalid_config
    send_app reload
    sleep 0.5
    send_app reload
    if wait_for_text "$run_log" 'Config reload error' 8; then
        assert_text 'GNOME invalid config exposes actionable error' 'Config reload error' "$(<"$run_log")"
    else
        assert_text 'GNOME invalid config exposes actionable error' 'Config reload error' "$(cat "$run_log" 2>/dev/null || true)"
    fi
    send_app status
    sleep 0.3
    assert_text 'GNOME invalid reload preserves previous bindings' 'binding count: 4' "$(<"$run_log")"
    write_valid_config
    sleep 0.3
    send_app reload
    sleep 0.5
    send_app reload
    assert_text 'GNOME config recovers after valid rewrite' 'Config state: Loaded' "$(<"$run_log")"
    stop_app_cleanly 'GNOME reload run quits cleanly' "$run_log"

    run_tui_lifecycle wayland 'gnome'
    stop_pid "$editor_pid"
    stop_pid "$shell_pid"
}

run_x11() {
    CURRENT_GATE='x11'
    X11_ROOT="$TMP_DIR/x11"
    X11_CONFIG="$X11_ROOT/config"
    X11_DATA="$X11_ROOT/data"
    X11_HOME="$X11_ROOT/home"
    CONFIG_PATH="$X11_ROOT/config.toml"
    mkdir -p "$X11_CONFIG" "$X11_DATA" "$X11_HOME" "$X11_ROOT"
    local display_num
    for display_num in $(seq 90 199); do
        if [[ ! -S "/tmp/.X11-unix/X$display_num" ]]; then
            break
        fi
    done
    X11_DISPLAY=":$display_num"
    record_versions_x11
    local xvfb_log="$X11_ROOT/xvfb.log" xvfb_pid openbox_log="$X11_ROOT/openbox.log" openbox_pid wm_check=''
    xvfb_pid=$(start_group "$xvfb_log" Xvfb "$X11_DISPLAY" -screen 0 3520x1080x24 -nolisten tcp -ac +extension RECORD +extension XTEST)
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
    assert_text 'X11 RECORD extension' 'RECORD' "$extension_text"
    assert_text 'X11 XTEST extension' 'XTEST' "$extension_text"
    openbox_pid=$(start_group "$openbox_log" env DISPLAY="$X11_DISPLAY" openbox --replace)
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
    write_valid_config
    local editor_log="$X11_ROOT/editor.log" editor_pid
    editor_pid=$(start_group "$editor_log" env DISPLAY="$X11_DISPLAY" GDK_BACKEND=x11 \
        XDG_CURRENT_DESKTOP=GNOME XDG_SESSION_TYPE=x11 HOME="$X11_HOME" \
        XDG_CONFIG_HOME="$X11_CONFIG" XDG_DATA_HOME="$X11_DATA" \
        gnome-text-editor --standalone)
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

    run_x11_dispatch half alt+ctrl+left '0,0,960,1080'
    x11_assert_geometry 'x11-left-half' '0,0,960,1080'
    run_x11_dispatch center-third alt+ctrl+up '640,0,640,1080'
    x11_assert_geometry 'x11-center-third' '640,0,640,1080'
    run_x11_dispatch reset-before-two-thirds alt+ctrl+left '0,0,960,1080'
    x11_assert_geometry 'x11-reset-before-two-thirds' '0,0,960,1080'
    run_x11_dispatch two-thirds alt+ctrl+right '0,0,1280,1080'
    x11_assert_geometry 'x11-left-two-thirds' '0,0,1280,1080'
    run_x11_dispatch next-display alt+ctrl+down '1920,0,1067,1080'
    x11_assert_geometry 'x11-next-display' '1920,0,1067,1080'
    local geometry center_x
    geometry=$(x11_geometry)
    if [[ "$geometry" =~ ^(-?[0-9]+),(-?[0-9]+),([0-9]+),([0-9]+)$ ]]; then
        center_x=$((BASH_REMATCH[1] + BASH_REMATCH[3] / 2))
    else
        center_x=-1
    fi
    assert_eq 'X11 cross-display observer sees second monitor' 1 "$((center_x >= 1920 && center_x < 3520 ? 1 : 0))"
    # Move back to the left display before the hotkey baseline; X11 has no
    # configured previous-display binding in this smoke config.
    DISPLAY="$X11_DISPLAY" xdotool windowmove "$WINDOW_ID" 640 0 >/dev/null 2>&1 || true
    DISPLAY="$X11_DISPLAY" xdotool windowsize "$WINDOW_ID" 640 1080 >/dev/null 2>&1 || true
    DISPLAY="$X11_DISPLAY" xdotool windowactivate --sync "$WINDOW_ID" >/dev/null 2>&1 || true
    if ! wait_x11_geometry '640,0,640,1080' 5 >/dev/null; then
        log "Harness setup: could not reset hotkey baseline geometry (observed '$(x11_geometry)')"
    fi

    run_x11_dispatch center-before-hotkey alt+ctrl+up '640,0,640,1080'
    x11_assert_geometry 'x11-center-before-hotkey' '640,0,640,1080'

    # Real XRecord/XTEST hotkey capture through the running App.
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
    assert_text 'X11 invalid reload preserves previous bindings' 'binding count: 4' "$(<"$run_log")"
    write_valid_config
    sleep 0.3
    send_app reload
    sleep 0.5
    send_app reload
    assert_text 'X11 config recovers after valid rewrite' 'Config state: Loaded' "$(<"$run_log")"
    stop_app_cleanly 'X11 run quits cleanly' "$run_log"

    run_tui_lifecycle x11 'x11'
    stop_pid "$editor_pid"
    stop_pid "$openbox_pid"
    stop_pid "$xvfb_pid"
}

main() {
    if [[ "$COMMAND" == gnome-live ]]; then
        if ! cargo build --locked --bin window_zones; then
            assert_rc_zero 'GNOME live cargo build' 1 'cargo build --locked --bin window_zones failed'
        else
            assert_rc_zero 'GNOME live cargo build' 0 ''
            run_gnome_live || true
        fi
    fi
    if [[ "$COMMAND" == gnome || "$COMMAND" == all ]]; then
        if ! cargo build --locked --bin window_zones; then
            assert_rc_zero 'GNOME cargo build' 1 'cargo build --locked --bin window_zones failed'
        else
            assert_rc_zero 'GNOME cargo build' 0 ''
            run_gnome || true
        fi
    fi
    if [[ "$COMMAND" == x11 || "$COMMAND" == all ]]; then
        if [[ ! -x "$BIN_PATH" ]]; then
            if ! cargo build --locked --bin window_zones; then
                assert_rc_zero 'X11 cargo build' 1 'cargo build --locked --bin window_zones failed'
            fi
        fi
        run_x11 || true
    fi
    log "Smoke assertions: $ASSERTIONS; failures: $FAILURES"
    log "Assertion log: $ASSERT_LOG"
    if [[ "$KEEP_LOGS" -eq 0 ]]; then
        log 'Logs removed (rerun with --keep-logs to retain temporary evidence)'
    fi
    [[ "$FAILURES" -eq 0 ]]
}

main
