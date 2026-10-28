run_x11_dispatch() {
    local label=$1 hotkey=$2 rc=0
    local out="$TMP_DIR/${CURRENT_GATE}-dispatch-${label}.log"
    DISPLAY="$X11_DISPLAY" xdotool windowactivate --sync "$WINDOW_ID" >/dev/null 2>&1 || true
    log "X11 dispatch $label precondition: active=$(DISPLAY="$X11_DISPLAY" xdotool getactivewindow 2>/dev/null || printf unavailable) geometry=$(x11_geometry)"
    env DISPLAY="$X11_DISPLAY" XDG_SESSION_TYPE=x11 XDG_CONFIG_HOME="$X11_CONFIG" \
        XDG_DATA_HOME="$X11_DATA" HOME="$X11_HOME" "$BIN_PATH" --backend x11 \
        --config "$CONFIG_PATH" dispatch "$hotkey" >"$out" 2>&1 || rc=$?
    cat "$out" >>"$ASSERT_LOG"
    assert_eq "$label dispatch exit" 0 "$rc"
    assert_text "$label dispatch state" 'Dispatch state: Succeeded' "$(<"$out")"
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
    test_output=$(WZ_X11_TEST_DISPLAY="$X11_DISPLAY" cargo test --locked \
        --manifest-path "$PROJECT_ROOT/Cargo.toml" --lib \
        x11_real_server_grabs_are_exclusive_atomic_and_released -- --ignored 2>&1) || test_rc=$?
    printf '%s\n' "$test_output" >>"$ASSERT_LOG"
    assert_rc_zero 'X11 real-server grabs are exclusive, atomic and released' "$test_rc" \
        'cargo test --lib x11_real_server_grabs_are_exclusive_atomic_and_released -- --ignored'
    assert_text 'X11 real-server grab test ran' '1 passed' "$test_output"
}
