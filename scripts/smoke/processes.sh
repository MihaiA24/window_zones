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
    wait "$pid"
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
