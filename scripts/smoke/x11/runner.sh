record_versions_x11() {
    record_versions_common
    log "VERSION Xvfb executable: $(command -v Xvfb)"
    log "VERSION Xvfb package: $(pacman -Q xorg-server-xvfb 2>/dev/null || true)"
    log "VERSION openbox: $(openbox --version 2>&1 | sed -n '1p')"
    log "VERSION xdotool: $(xdotool --version 2>&1 | sed -n '1p')"
}

x11_inject_hotkey() {
    local key=$1 output='' rc=0
    output=$(DISPLAY="$X11_DISPLAY" xdotool key --clearmodifiers "alt+ctrl+$key" 2>&1) || rc=$?
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

run_x11_tui_lifecycle() {
    local tui_log="$TMP_DIR/${CURRENT_GATE}-x11-tui.log"
    tui_begin x11 x11 "$tui_log"

    run_x11_dispatch tui-baseline alt+ctrl+up
    x11_assert_geometry 'X11 TUI baseline' '640,0,640,1080'

    tui_dispatch "$tui_log" 'x11 TUI dispatch' alt+ctrl+left
    x11_assert_geometry 'X11 TUI dispatch moves window' '0,0,960,1080'

    stop_app_cleanly 'x11 TUI quit' "$tui_log"
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
    XON_DISPLAY="$X11_DISPLAY"
    XON_XAUTHORITY=''
    WINDOW_ID=$(xwindow_for_pid "$editor_pid" 20 || true)
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
    run_x11_constrained_restore_check
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
    wait_for_text "$run_log" 'Interactive session started' 15 || true
    assert_text 'X11 run starts' 'Interactive session started' "$(cat "$run_log" 2>/dev/null || true)"
    send_app status
    wait_for_text "$run_log" 'hotkey state: Registered' 8 || true
    assert_text 'X11 hotkeys initially register' 'hotkey state: Registered' "$(cat "$run_log" 2>/dev/null || true)"
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

    local invalid_before loaded_before bindings_before
    invalid_before=$(count_text "$run_log" 'Config reload error')
    sleep 0.3
    write_invalid_config
    send_app reload
    sleep 0.5
    send_app reload
    wait_for_new_text "$run_log" 'Config reload error' "$invalid_before" 8 || true
    assert_eq 'X11 invalid config exposes actionable error' 1 \
        "$(( $(count_text "$run_log" 'Config reload error') > invalid_before ? 1 : 0 ))"
    bindings_before=$(count_text "$run_log" 'binding count: 5')
    send_app status
    wait_for_new_text "$run_log" 'binding count: 5' "$bindings_before" 8 || true
    assert_eq 'X11 invalid reload preserves previous bindings' 1 \
        "$(( $(count_text "$run_log" 'binding count: 5') > bindings_before ? 1 : 0 ))"
    loaded_before=$(count_text "$run_log" 'Config state: Loaded')
    write_five_bindings_config
    sleep 0.3
    send_app reload
    sleep 0.5
    send_app reload
    wait_for_new_text "$run_log" 'Config state: Loaded' "$loaded_before" 10 || true
    assert_eq 'X11 config recovers after valid rewrite' 1 \
        "$(( $(count_text "$run_log" 'Config state: Loaded') > loaded_before ? 1 : 0 ))"

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

    run_x11_tui_lifecycle
    stop_pid "$editor_pid"
    stop_pid "$openbox_pid"
    stop_pid "$xvfb_pid"
}
