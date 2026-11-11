owned_process() {
    /usr/bin/python3 "$SCRIPT_DIR/smoke/owned_process.py" "$1" \
        "$PID_FILE" "$WINDOW_ZONES_SMOKE_OWNER" "$$" "${@:2}"
}

start_group() {
    local pid_variable=$1 log_file=$2
    shift 2
    # Assign in this shell: command substitution would orphan the child.
    setsid -- "$@" >"$log_file" 2>&1 &
    local child_pid=$!
    printf -v "$pid_variable" '%s' "$child_pid"
    owned_process register "$child_pid"
}

stop_pid() {
    [[ -n "$1" ]] || return 0
    owned_process signal "$1" "${2:-TERM}"
}

wait_pid() {
    local pid=$1 timeout_s=${2:-10}
    local deadline=$((SECONDS + timeout_s))
    while owned_process alive "$pid"; do
        (( SECONDS < deadline )) || return 124
        sleep 0.1
    done
    # The initial KWin service is an owned descendant of the launcher, not our child.
    [[ "${3:-child}" == external ]] && return 0
    wait "$pid"
}

cleanup() {
    local rc=0
    if [[ "$GNOME_LIVE_ACTIVE" -eq 1 ]]; then
        restore_gnome_live || GNOME_LIVE_RESTORE_FAILURE=1
    fi
    if ! owned_process cleanup; then
        KEEP_LOGS=1
        rc=1
    fi
    if [[ "$GNOME_LIVE_RESTORE_FAILURE" -ne 0 ]]; then
        KEEP_LOGS=1
        rc=1
    fi
    if [[ "$KEEP_LOGS" -eq 0 ]]; then
        rm -rf "$TMP_DIR" || rc=1
    else
        printf 'Logs retained in %s\n' "$TMP_DIR"
    fi
    return "$rc"
}

smoke_exit_trap() {
    local status=$1
    trap - EXIT
    trap '' INT TERM
    cleanup || status=1
    exit "$status"
}

install_smoke_traps() {
    trap 'smoke_exit_trap $?' EXIT
    trap 'exit 130' INT
    trap 'exit 143' TERM
}

start_fifo_app() {
    local mode=$1 backend=$2 log_file=$3
    shift 3
    local fifo="$TMP_DIR/${CURRENT_GATE}-${mode}.fifo"
    rm -f "$fifo"
    mkfifo "$fifo"
    env DISPLAY="${DISPLAY:-}" XDG_SESSION_TYPE="${XDG_SESSION_TYPE:-}" \
        XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-}" \
        DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-}" \
        GDK_BACKEND="${GDK_BACKEND:-}" \
        HOME="${HOME:-$TMP_DIR/home}" XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-$TMP_DIR/config}" \
        XDG_DATA_HOME="${XDG_DATA_HOME:-$TMP_DIR/data}" \
        "$BIN_PATH" --backend "$backend" --config "$CONFIG_PATH" "$@" "$mode" <"$fifo" >"$log_file" 2>&1 &
    APP_PID=$!
    owned_process register "$APP_PID" || return 1
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
