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

record_versions_gnome() {
    record_versions_common
    log "VERSION gnome-shell: $(gnome-shell --version 2>&1)"
    log "VERSION gnome-text-editor: $(gnome-text-editor --version 2>&1 | sed -n '1p')"
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

    local displays_raw display_lines display_count first_display
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
