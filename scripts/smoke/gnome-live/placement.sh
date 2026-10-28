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
    out=$(python3 "$SCRIPT_DIR/smoke/fixtures/uinput_key.py" super+left 2>&1) || log "GNOME live uinput super+left: $out"
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
