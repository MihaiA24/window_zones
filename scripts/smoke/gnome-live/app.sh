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
    env XDG_SESSION_TYPE=wayland WAYLAND_DISPLAY="$WAYLAND_DISPLAY" \
        XDG_RUNTIME_DIR="$GNOME_RUNTIME" DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" \
        XDG_CONFIG_HOME="$GNOME_CONFIG" XDG_DATA_HOME="$GNOME_DATA" \
        HOME="$GNOME_HOME" XDG_CURRENT_DESKTOP=GNOME GNOME_DESKTOP_SESSION_ID=smoke \
        "$BIN_PATH" --backend wayland --config "$CONFIG_PATH" dispatch "$hotkey" >"$out" 2>&1 || rc=$?
    cat "$out" >>"$ASSERT_LOG"
    assert_eq "$label dispatch exit" 0 "$rc"
    assert_text "$label dispatch state" 'Dispatch state: Succeeded' "$(<"$out")"
}

run_gnome_live_app_checks() {
    local run_log="$GNOME_ROOT/live-run.log"
    local registered=0 real_geometry=''
    local injection_rc=0 deadline=0 disable_out='' enable_out=''
    write_gnome_live_valid_config
    start_fifo_app run wayland "$run_log"
    wait_for_text "$run_log" 'Interactive session started' 15 || true
    assert_text 'GNOME live App starts' 'Interactive session started' \
        "$(cat "$run_log" 2>/dev/null || true)"

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
        run_gnome_live_dispatch real-hotkey-baseline "$GNOME_SAFE_CENTER_HOTKEY"
        gnome_live_assert_geometry 'GNOME live real hotkey baseline' "$GNOME_LIVE_CENTER_THIRD"
        gnome_live_activate_window || true
        gnome_live_inject_hotkey || injection_rc=$?
        assert_rc_zero 'GNOME live uinput injection' "$injection_rc" 'one accelerator press'
        real_geometry=$(wait_observed gnome_live_geometry "$GNOME_LIVE_LEFT_HALF" 10)
    else
        log 'GNOME live real-hotkey injection not attempted because the live grab did not register'
    fi
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
    local unavailable_before recovered_before
    unavailable_before=$(count_text "$run_log" 'GNOME companion is unavailable')
    recovered_before=$(count_text "$run_log" 'Hotkey registration now recovered')
    disable_out=$(gnome_shell_call /org/gnome/Shell \
        org.gnome.Shell.Extensions.DisableExtension window-zones@mihai-a24 2>&1 || true)
    log "GNOME live companion disable: $disable_out"
    assert_text 'GNOME live companion disable call' 'true' "$disable_out"
    # The App notices the loss on its next poll of the companion, which is not instant.
    wait_for_new_text "$run_log" 'GNOME companion is unavailable' "$unavailable_before" 45 || true
    assert_eq 'GNOME live disconnect surfaces explicit unavailable state' 1 \
        "$(( $(count_text "$run_log" 'GNOME companion is unavailable') > unavailable_before ? 1 : 0 ))"
    send_app status
    # Every backend line the App ever printed must still name the native path: no X11 fallback.
    backend_after=$(sed -n 's/^Window backend: //p' "$run_log" | sort -u | tr '\n' ',' | sed 's/,$//')
    assert_eq 'GNOME live disconnect keeps native backend' 'gnome-wayland' "$backend_after"
    owned_process alive "$app_pid_before" && pid_alive=1
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
    wait_for_new_text "$run_log" 'Hotkey registration now recovered' "$recovered_before" 45 || true
    assert_eq 'GNOME live companion recovers without App restart' 1 \
        "$(( $(count_text "$run_log" 'Hotkey registration now recovered') > recovered_before ? 1 : 0 ))"
    owned_process alive "$app_pid_before" && pid_same=1
    assert_eq 'GNOME live App PID unchanged after recovery' 1 "$pid_same"
    gnome_live_capture companion-recovered

    run_gnome_live_dispatch config-baseline "$GNOME_SAFE_RIGHT_HOTKEY"
    gnome_live_assert_geometry config-baseline "$GNOME_LIVE_TWO_THIRDS"
    local invalid_before
    invalid_before=$(count_text "$run_log" 'Config reload error')
    write_gnome_live_invalid_config
    sleep 0.4
    send_app reload
    sleep 0.3
    send_app reload
    wait_for_new_text "$run_log" 'Config reload error' "$invalid_before" 10 || true
    assert_eq 'GNOME live invalid config exposes actionable error' 1 \
        "$(( $(count_text "$run_log" 'Config reload error') > invalid_before ? 1 : 0 ))"
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

    local loaded_before
    loaded_before=$(count_text "$run_log" 'Config state: Loaded')
    write_gnome_live_valid_config
    sleep 0.4
    send_app reload
    sleep 0.3
    send_app reload
    wait_for_new_text "$run_log" 'Config state: Loaded' "$loaded_before" 10 || true
    assert_eq 'GNOME live valid config recovers' 1 \
        "$(( $(count_text "$run_log" 'Config state: Loaded') > loaded_before ? 1 : 0 ))"
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
    wait_for_text "$tui_log" 'Window Zones TUI' 15 || true
    assert_text 'GNOME live TUI starts' 'Window Zones TUI' \
        "$(cat "$tui_log" 2>/dev/null || true)"

    tui_control_checks "$tui_log" 'GNOME live'

    run_gnome_live_dispatch tui-baseline "$GNOME_SAFE_CENTER_HOTKEY"
    gnome_live_assert_geometry 'GNOME live TUI baseline' "$GNOME_LIVE_CENTER_THIRD"
    gnome_live_activate_window || true
    tui_dispatch "$tui_log" 'GNOME live TUI dispatch' "$GNOME_SAFE_LEFT_HOTKEY"
    gnome_live_assert_geometry 'GNOME live TUI dispatch' "$GNOME_LIVE_LEFT_HALF"

    stop_app_cleanly 'GNOME live TUI quit' "$tui_log"
}
