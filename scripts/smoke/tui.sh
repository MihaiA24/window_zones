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
    wait_for_text "$tui_log" 'Window Zones TUI' 15 || true
    assert_text "$label TUI starts" 'Window Zones TUI' "$(cat "$tui_log" 2>/dev/null || true)"

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
