tui_command_frame() {
    local tui_log=$1 command=$2 expected=$3 before
    before=$(stat -c '%s' "$tui_log")
    send_app "$command"
    /usr/bin/python3 "$SCRIPT_DIR/smoke/tui_frame.py" "$tui_log" "$before" "$expected" 10
}

tui_control_checks() {
    local tui_log=$1 label=$2 command expected frame
    while IFS='|' read -r command expected; do
        frame=$(tui_command_frame "$tui_log" "$command" "$expected") || true
        assert_text "$label TUI $command" "$expected" "$frame"
        assert_text "$label TUI $command completes successfully" 'Last error: none' "$frame"
    done <<EOF
reload|Config: $CONFIG_PATH (Loaded)
restart|Last action: <none>
status|Hotkey state: Registered
EOF
}

tui_dispatch() {
    local tui_log=$1 label=$2 hotkey=$3 frame reported
    frame=$(tui_command_frame "$tui_log" "dispatch $hotkey" "Last action: $hotkey") || true
    reported=$(printf '%s\n' "$frame" | sed -n 's/^Last action: //p')
    assert_eq "$label" "$(sorted_hotkey "$hotkey")" "$(sorted_hotkey "$reported")"
    assert_text "$label completes successfully" 'Last error: none' "$frame"
}

tui_begin() {
    local backend=$1 label=$2 tui_log=$3
    start_fifo_app tui "$backend" "$tui_log"
    wait_for_text "$tui_log" 'Window Zones TUI' 15 || true
    assert_text "$label TUI starts" 'Window Zones TUI' "$(cat "$tui_log" 2>/dev/null || true)"

    tui_control_checks "$tui_log" "$label"
}
