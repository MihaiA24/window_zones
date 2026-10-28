kde_live_write_changed_config() {
    cat >"$CONFIG_PATH" <<'EOF'
[[bindings]]
hotkey = "Alt+Ctrl+Up"
action = { type = "move-to-zone", zone = "center-third" }
EOF
}

kde_live_tui_lifecycle() {
    local tui_log="$KDE_ROOT/tui.log" rc=0
    write_five_bindings_config
    start_fifo_app tui auto "$tui_log"
    wait_for_text "$tui_log" 'Window Zones TUI' 15 || true
    assert_text 'KDE TUI starts' 'Window Zones TUI' "$(cat "$tui_log" 2>/dev/null || true)"
    tui_control_checks "$tui_log" 'KDE'
    kde_live_activate_window || true
    send_app 'dispatch alt+ctrl+left'
    wait_for_text "$tui_log" 'Last action: alt+ctrl+left' 10 || true
    assert_text 'KDE TUI dispatch action' 'Last action: alt+ctrl+left' \
        "$(cat "$tui_log" 2>/dev/null || true)"
    kde_live_refresh_targets
    kde_live_assert_zone 'KDE TUI dispatch geometry' left-half A
    tui_end KDE "$tui_log"
    set +e
}

run_kde_live() {
    CURRENT_GATE='kde-live'
    KDE_ROOT="$TMP_DIR/kde-live"
    KDE_CONFIG="$KDE_ROOT/config"
    KDE_DATA="$KDE_ROOT/data"
    KDE_CACHE="$KDE_ROOT/cache"
    KDE_STATE="$KDE_ROOT/state"
    KDE_RUNTIME="$KDE_ROOT/runtime"
    KDE_HOME="$KDE_ROOT/home"
    KDE_BUS_FILE="$KDE_ROOT/bus-address"
    KDE_SERVICE_LOG="$KDE_ROOT/companion.log"
    KDE_SERVICE_PID_FILE="$KDE_ROOT/companion.pid"
    KDE_KWIN_PID_FILE="$KDE_ROOT/kwin.pid"
    KDE_KWIN_LOG="$KDE_ROOT/kwin.log"
    KDE_XDISPLAY_FILE="$KDE_ROOT/xdisplay"
    KDE_XAUTHORITY_FILE="$KDE_ROOT/xauthority"
    KDE_CLIENT_BUS_FILE="$KDE_ROOT/client-bus"
    KDE_LAUNCHER="$KDE_ROOT/launch-kwin.sh"
    KDE_PUBLISH_CLIENT_ENV="$KDE_ROOT/publish-client-env.sh"
    KDE_OBSERVER_JS="$KDE_ROOT/observer.js"
    KDE_COMPANION_BIN="$(dirname -- "$BIN_PATH")/window_zones_kwin"
    KDE_SOCKET="wayland-window-zones-kde-$$"
    KDE_DATA_DIRS="${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
    CONFIG_PATH="$KDE_ROOT/config.toml"
    KDE_SAFE_HOTKEY='alt+ctrl+left'
    KDE_INSTALLED_MAIN=''
    KDE_OBSERVER_SCRIPT_ID='-1'
    KDE_CONFLICT_SCRIPT_ID='-1'
    KDE_SERVICE_PID=''
    KDE_KWIN_PID=''
    KDE_EDITOR_PID=''
    APP_PID=''
    APP_FD=''
    WINDOW_ID=''
    mkdir -p "$KDE_ROOT"
    set +e

    if ! kde_live_preflight; then
        set -e
        return 0
    fi

    mkdir -p "$KDE_CONFIG" "$KDE_DATA" "$KDE_CACHE" "$KDE_STATE" "$KDE_RUNTIME" "$KDE_HOME"
    chmod 700 "$KDE_RUNTIME" "$KDE_HOME"
    if ! build_binaries KDE --bin window_zones_kwin; then
        set -e
        return 0
    fi

    local install_output='' install_rc=0
    install_output=$(env -u KDEHOME HOME="$KDE_HOME" XDG_CONFIG_HOME="$KDE_CONFIG" \
        XDG_DATA_HOME="$KDE_DATA" XDG_CACHE_HOME="$KDE_CACHE" XDG_STATE_HOME="$KDE_STATE" \
        kpackagetool6 --type=KWin/Script --install "$PROJECT_ROOT/kwin-script" 2>&1) || install_rc=$?
    log "KDE package install: $install_output"
    assert_rc_zero 'KDE repository KWin package installs' "$install_rc" 'kpackagetool6'

    local config_output='' config_rc=0
    config_output=$(env -u KDEHOME HOME="$KDE_HOME" XDG_CONFIG_HOME="$KDE_CONFIG" \
        kwriteconfig6 --file kwinrc --group Plugins --key window-zonesEnabled true 2>&1) || config_rc=$?
    log "KDE plugin enable config: $config_output"
    assert_rc_zero 'KDE repository KWin script is enabled in private kwinrc' "$config_rc" 'kwriteconfig6 Plugins'
    if config_output=$(env -u KDEHOME HOME="$KDE_HOME" XDG_CONFIG_HOME="$KDE_CONFIG" \
        kwriteconfig6 --file kwinrc --group Xwayland --key XwaylandEisNoPrompt true 2>&1); then
        config_rc=0
    else
        config_rc=$?
    fi
    log "KDE private Xwayland EIS preauthorization: $config_output"
    assert_rc_zero 'KDE private Xwayland EIS preauthorization' "$config_rc" 'kwriteconfig6 Xwayland'

    local package_dir=''
    for package_dir in "$KDE_DATA"/kwin/scripts/window-zones \
        "$KDE_DATA"/kwin-wayland/scripts/window-zones; do
        if [[ -f "$package_dir/metadata.json" && -f "$package_dir/contents/code/main.js" ]]; then
            KDE_INSTALLED_MAIN="$package_dir/contents/code/main.js"
            break
        fi
    done
    local installed_files_ok=0
    if [[ -n "$KDE_INSTALLED_MAIN" ]] \
        && cmp -s "$PROJECT_ROOT/kwin-script/metadata.json" \
            "${KDE_INSTALLED_MAIN%/contents/code/main.js}/metadata.json" \
        && cmp -s "$PROJECT_ROOT/kwin-script/contents/code/main.js" "$KDE_INSTALLED_MAIN"; then
        installed_files_ok=1
    fi
    assert_eq 'KDE installed companion matches repository files' 1 "$installed_files_ok"
    if [[ "$installed_files_ok" -ne 1 ]]; then
        log 'BLOCKED KDE live: installed private KWin package does not match repository metadata/code'
        set -e
        return 0
    fi

    if ! kde_live_start_fixture; then
        assert_eq 'KDE nested KWin fixture starts' 1 0
        set -e
        return 0
    fi
    local kwin_alive=0
    kill -0 "$KDE_KWIN_PID" 2>/dev/null && kwin_alive=1
    assert_eq 'KDE nested KWin fixture starts' 1 "$kwin_alive"
    kde_live_prepare_environment
    local xdisplay_ok=0
    [[ "$KDE_XDISPLAY" == :* ]] && xdisplay_ok=1
    assert_eq 'KDE nested Xwayland DISPLAY is published' 1 "$xdisplay_ok"
    # KWin's rootless Xwayland accepts clients from this user without an auth file, so an empty
    # XAUTHORITY is not a failure; the X11 connection below is the check that matters.
    log "KDE nested Xwayland XAUTHORITY: ${KDE_XAUTHORITY:-<none published>}"
    local x11_root_geometry='' x11_connection_ok=0
    x11_root_geometry=$(DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" \
        xdotool getdisplaygeometry 2>/dev/null || true)
    [[ "$x11_root_geometry" =~ ^[0-9]+[[:space:]]+[0-9]+$ ]] && x11_connection_ok=1
    assert_eq 'KDE nested Xwayland accepts X11 clients' 1 "$x11_connection_ok"
    if which xrandr >/dev/null 2>&1; then
        log "KDE nested Xwayland RandR: $(DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" \
            xrandr --query 2>&1 || true)"
    else
        log 'KDE nested Xwayland RandR: unavailable (optional observer utility not installed)'
    fi

    kde_live_wait_companion_ready 45 || true
    assert_text 'KDE nested KWin owns private D-Bus name' 'org.kde.KWin' "$KDE_KWIN_OWNER_LINE"
    assert_text 'KDE companion owns private D-Bus name' 'org.window_zones.KWin' "$KDE_COMPANION_OWNER_LINE"
    assert_text 'KDE repository KWin script is loaded' 'true' "$KDE_SCRIPT_LOADED_RAW"
    assert_eq 'KDE capability set' 'focused-window|displays|move-resize|hotkeys' "$KDE_CAPABILITIES_RAW"
    local initial_display_lines='' initial_display_count=0
    initial_display_lines=$(kde_live_parse_displays "$KDE_DISPLAYS_RAW")
    initial_display_count=$(printf '%s\n' "$initial_display_lines" | sed '/^$/d' | wc -l | tr -d ' ')
    assert_eq 'KDE GetDisplays reports two outputs at readiness' 2 "$initial_display_count"
    if [[ "$KDE_COMPANION_READY" -ne 1 ]]; then
        log 'BLOCKED KDE live: repository KWin script did not complete its handshake; see nested KWin and Companion logs'
        set -e
        return 0
    fi
    local kwin_diagnostics=''
    kwin_diagnostics=$(sed -n \
        '/Window Zones KWin.*failed/p;/Window Zones KWin.*error/p;/KDE_CONFLICT_ERROR/p' \
        "$KDE_KWIN_LOG" 2>/dev/null || true)
    log "KDE nested KWin script diagnostics: ${kwin_diagnostics:-none}"
    write_five_bindings_config
    local auto_backend_output=''
    auto_backend_output=$(env -u GNOME_DESKTOP_SESSION_ID -u SWAYSOCK \
        -u HYPRLAND_INSTANCE_SIGNATURE DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" \
        XDG_RUNTIME_DIR="$KDE_RUNTIME" WAYLAND_DISPLAY="$KDE_SOCKET" \
        DBUS_SESSION_BUS_ADDRESS="$KDE_BUS" XDG_CONFIG_HOME="$KDE_CONFIG" \
        XDG_DATA_HOME="$KDE_DATA" HOME="$KDE_HOME" XDG_SESSION_TYPE=wayland \
        XDG_CURRENT_DESKTOP=KDE XDG_SESSION_DESKTOP=KDE DESKTOP_SESSION=plasma \
        KDE_SESSION_VERSION=6 "$BIN_PATH" --backend auto --config "$CONFIG_PATH" status 2>&1 || true)
    assert_text 'KDE --backend auto resolves kde-wayland' \
        'Using runtime window backend: kde-wayland' "$auto_backend_output"


    kde_live_observer_script
    kde_live_load_script "$KDE_OBSERVER_JS" window-zones-observer
    KDE_OBSERVER_SCRIPT_ID="$KDE_LAST_SCRIPT_ID"
    local observer_id_ok=0
    [[ "$KDE_OBSERVER_SCRIPT_ID" =~ ^[0-9]+$ ]] && observer_id_ok=1
    assert_eq 'KDE independent observer script loads' 1 "$observer_id_ok"
    kde_live_run_script "$KDE_OBSERVER_SCRIPT_ID"
    wait_for_text "$KDE_KWIN_LOG" 'KDE_OBSERVER ' 12 || true
    assert_text 'KDE independent observer reports screens' 'KDE_OBSERVER ' \
        "$(cat "$KDE_KWIN_LOG" 2>/dev/null || true)"
    kde_live_observer_records "$KDE_KWIN_LOG" >"$KDE_ROOT/observer-records"
    log "KDE independent observer records: $(cat "$KDE_ROOT/observer-records" 2>/dev/null || true)"
    local observer_layout_ok=0 observer_display_count=0
    observer_display_count=$(python - "$KDE_ROOT/observer-records" <<'PY'
import sys
count = 0
try:
    with open(sys.argv[1], encoding="utf-8") as stream:
        for line in stream:
            fields = line.rstrip("\n").split("|")
            if len(fields) == 4 and all(item.lstrip("-").isdigit() for item in fields):
                count += 1
except OSError:
    pass
print(count)
PY
)
    observer_layout_ok=$(kde_live_observer_layout_ok "$KDE_ROOT/observer-records")
    assert_eq 'KDE independent observer reports two distinct outputs' 1 "$observer_layout_ok"
    assert_eq 'KDE independent observer output count' 2 "$observer_display_count"

    local display_compare='' display_compare_result=''
    display_compare=$(kde_live_compare_displays)
    display_compare_result=$(printf '%s\n' "$display_compare" | sed -n '$p')
    log "KDE GetDisplays versus independent MaximizeArea: $(printf '%s\n' "$display_compare" | sed '$d')"
    if [[ "$observer_layout_ok" -ne 1 || "$observer_display_count" -ne 2 ]]; then
        log 'BLOCKED KDE live: independent observer did not provide two valid nonoverlapping output rectangles'
        set -e
        return 0
    fi
    assert_eq 'KDE GetDisplays matches independent MaximizeArea observer' 1 "$display_compare_result"
    skip_assert 'KDE Plasma panel exclusion' \
        'bare nested KWin runs no plasmashell/panel; this run verifies panel-free per-output areas, not Plasma reserved-area behavior'


    local editor_log="$KDE_ROOT/editor.log" editor_deadline=0 editor_alive=0 window_found=0
    start_group KDE_EDITOR_PID "$editor_log" env \
        DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" GDK_BACKEND=x11 \
        XDG_RUNTIME_DIR="$KDE_RUNTIME" WAYLAND_DISPLAY="$KDE_SOCKET" \
        DBUS_SESSION_BUS_ADDRESS="$KDE_BUS" XDG_CONFIG_HOME="$KDE_CONFIG" \
        XDG_DATA_HOME="$KDE_DATA" HOME="$KDE_HOME" XDG_SESSION_TYPE=wayland \
        XDG_CURRENT_DESKTOP=KDE XDG_SESSION_DESKTOP=KDE DESKTOP_SESSION=plasma \
        KDE_SESSION_VERSION=6 gnome-text-editor --standalone
    kill -0 "$KDE_EDITOR_PID" 2>/dev/null && editor_alive=1
    assert_eq 'KDE test editor process starts' 1 "$editor_alive"
    editor_deadline=$((SECONDS + 25))
    while (( SECONDS < editor_deadline )); do
        if kde_live_find_window; then
            window_found=1
            break
        fi
        sleep 0.2
    done
    assert_eq 'KDE test editor window is observable through Xwayland' 1 "$window_found"
    kde_live_activate_window || true
    kde_live_wait_focus 15 || true
    kde_live_assert_focus
    # Outputs are windows of the outer compositor and can be resized while the fixture starts, and
    # the test window decides which output it opens on, so targets are chosen once the window is
    # focused, from the newest observer record.
    kde_live_observer_records "$KDE_KWIN_LOG" >"$KDE_ROOT/observer-records"
    kde_live_select_display_targets
    log "KDE placement targets after focus: A=$KDE_A_LINE B=$KDE_B_LINE"
    local editor_backend=''
    editor_backend=$(tr '\0' '\n' <"/proc/$KDE_EDITOR_PID/environ" 2>/dev/null \
        | sed -n '/^GDK_BACKEND=/p' | sed -n '1p' || true)
    assert_eq 'KDE test editor uses Xwayland backend' 'GDK_BACKEND=x11' "$editor_backend"

    kde_live_dispatch 'KDE left-half' alt+ctrl+left
    kde_live_assert_zone 'KDE left-half' left-half A
    kde_live_dispatch 'KDE center-third' alt+ctrl+up
    kde_live_assert_zone 'KDE center-third' center-third A
    kde_live_dispatch 'KDE left-two-thirds' alt+ctrl+right
    kde_live_assert_zone 'KDE left-two-thirds' left-two-thirds A
    kde_live_dispatch 'KDE next-display' alt+ctrl+down
    kde_live_assert_zone 'KDE next-display' left-two-thirds B
    kde_live_dispatch 'KDE previous-display' alt+ctrl+shift+right
    kde_live_assert_zone 'KDE previous-display' left-two-thirds A
    kde_live_dispatch 'KDE previous-display wrap' alt+ctrl+shift+right
    kde_live_assert_zone 'KDE previous-display wraps' left-two-thirds B
    kde_live_dispatch 'KDE next-display wrap' alt+ctrl+down
    kde_live_assert_zone 'KDE next-display wraps' left-two-thirds A
    skip_assert 'KDE negative-coordinate output movement' \
        'fixture uses KWin-generated side-by-side nonnegative output origins; no negative-origin layout was configured'

    kde_live_atomic_registration
    kde_live_conflict_registration
    kde_live_adr9_window_checks
    log 'KDE native Wayland placement not exercised; test client used Xwayland'

    write_five_bindings_config
    kde_live_prepare_environment
    local run_log="$KDE_ROOT/run.log" app_bus=''
    start_fifo_app run auto "$run_log"
    wait_for_text "$run_log" 'Interactive session started' 15 || true
    assert_text 'KDE App starts' 'Interactive session started' "$(cat "$run_log" 2>/dev/null || true)"
    app_bus=$(tr '\0' '\n' <"/proc/$APP_PID/environ" 2>/dev/null \
        | sed -n '/^DBUS_SESSION_BUS_ADDRESS=/p' | sed -n '1p' || true)
    assert_eq 'KDE App uses private D-Bus session' "DBUS_SESSION_BUS_ADDRESS=$KDE_BUS" "$app_bus"
    send_app status
    wait_for_text "$run_log" 'hotkey state: Registered' 12 || true
    assert_text 'KDE App hotkeys initially register' 'hotkey state: Registered' \
        "$(cat "$run_log" 2>/dev/null || true)"

    send_app 'dispatch alt+ctrl+up'
    kde_live_refresh_targets
    kde_live_assert_zone 'KDE real hotkey baseline center-third' center-third A
    local inject_rc=0 action_before=0
    action_before=$(count_text "$run_log" 'last action: alt+ctrl+left')
    kde_live_inject_key ctrl+alt+Left || inject_rc=$?
    assert_rc_zero 'KDE XTEST accelerator injection' "$inject_rc" 'xdotool key ctrl+alt+Left'
    kde_live_assert_zone 'KDE real XTEST-to-EIS hotkey' left-half A
    send_app status
    wait_for_new_text "$run_log" 'last action: alt+ctrl+left' "$action_before" 10 || true
    assert_text 'KDE real hotkey reports action' 'last action: alt+ctrl+left' \
        "$(cat "$run_log" 2>/dev/null || true)"
    skip_assert 'KDE seated accelerator capture' \
        'nested KWin uses a Noop session and does not own the login seat; XTEST/EIS tests compositor dispatch, not physical-seat input or host shortcut arbitration'

    local app_pid_before="$APP_PID" script_unload='' script_unloaded='' unavailable_ok=0
    script_unload=$(kde_live_kwin_call unloadScript s window-zones 2>&1 || true)
    log "KDE repository script unload: $script_unload"
    assert_text 'KDE script disconnect unload call' 'true' "$script_unload"
    script_unloaded=$(kde_live_wait_script_loaded false 15 || true)
    assert_text 'KDE script disconnect unload completes' 'false' "$script_unloaded"
    sleep 6
    wait_for_text "$run_log" 'KWin companion is unavailable' 15 || true
    if file_contains "$run_log" 'KWin companion is unavailable'; then
        unavailable_ok=1
    fi
    assert_eq 'KDE script disconnect surfaces unavailable state' 1 "$unavailable_ok"
    local backend_after='' app_alive=0
    backend_after=$(sed -n 's/^Window backend: //p' "$run_log" | sort -u | tr '\n' ',' | sed 's/,$//')
    assert_eq 'KDE script disconnect keeps native backend' 'kde-wayland' "$backend_after"
    kill -0 "$app_pid_before" 2>/dev/null && app_alive=1
    assert_eq 'KDE App survives script disconnect' 1 "$app_alive"
    kde_live_load_script "$KDE_INSTALLED_MAIN" window-zones
    local reload_id_ok=0 script_reload_ready=0
    [[ "$KDE_LAST_SCRIPT_ID" =~ ^[0-9]+$ ]] && reload_id_ok=1
    assert_eq 'KDE repository script reload starts' 1 "$reload_id_ok"
    kde_live_run_script "$KDE_LAST_SCRIPT_ID"
    kde_live_wait_companion_ready 30 && script_reload_ready=1
    assert_eq 'KDE script reconnect restores capabilities' 1 "$script_reload_ready"
    local app_recovered=0
    wait_for_text "$run_log" 'Hotkey registration now recovered' 20 && app_recovered=1
    assert_eq 'KDE App hotkey registration recovers after script reload' 1 "$app_recovered"
    kill -0 "$app_pid_before" 2>/dev/null && app_alive=1 || app_alive=0
    assert_eq 'KDE App PID unchanged after script reload' 1 "$app_alive"

    local old_service_pid="$KDE_SERVICE_PID" service_stopped=0 restart_log="$KDE_ROOT/companion-restart.log"
    local service_recovered_before
    service_recovered_before=$(count_text "$run_log" 'Hotkey registration now recovered')
    # A killed child stays visible to kill -0 until it is reaped, and a service that ignores
    # SIGTERM would otherwise be reported as stopped only by timeout, so escalate and confirm the
    # bus name is actually gone.
    stop_pid "$old_service_pid"
    wait_pid "$old_service_pid" 5 external || kill -9 "$old_service_pid" 2>/dev/null || true
    wait_pid "$old_service_pid" 5 external || true
    set +e
    local service_owner_gone=0 stop_deadline=$((SECONDS + 10))
    while (( SECONDS < stop_deadline )); do
        if ! busctl --address="$KDE_BUS" list 2>/dev/null | grep -q 'org.window_zones.KWin'; then
            service_owner_gone=1
            break
        fi
        sleep 0.2
    done
    service_stopped=$service_owner_gone
    log "KDE service stop: owner gone=$service_owner_gone pid alive=$(kill -0 "$old_service_pid" 2>/dev/null && printf 1 || printf 0)"
    assert_eq 'KDE Rust service stops independently' 1 "$service_stopped"
    start_group KDE_SERVICE_PID "$restart_log" env -u KDEHOME DBUS_SESSION_BUS_ADDRESS="$KDE_BUS" \
        HOME="$KDE_HOME" XDG_CONFIG_HOME="$KDE_CONFIG" XDG_DATA_HOME="$KDE_DATA" \
        XDG_CACHE_HOME="$KDE_CACHE" XDG_STATE_HOME="$KDE_STATE" XDG_RUNTIME_DIR="$KDE_RUNTIME" \
        XDG_DATA_DIRS="$KDE_DATA_DIRS" "$KDE_COMPANION_BIN"
    wait_for_text "$restart_log" 'KWin companion service started' 12 || true
    assert_text 'KDE Rust service restarts' 'KWin companion service started' \
        "$(cat "$restart_log" 2>/dev/null || true)"
    local service_reload_ready=0
    kde_live_wait_companion_ready 30 && service_reload_ready=1
    assert_eq 'KDE service restart restores capabilities' 1 "$service_reload_ready"
    local service_app_recovered=0
    wait_for_new_text "$run_log" 'Hotkey registration now recovered' "$service_recovered_before" 20 && service_app_recovered=1
    assert_eq 'KDE App hotkey registration recovers after service restart' 1 "$service_app_recovered"
    kill -0 "$app_pid_before" 2>/dev/null && app_alive=1 || app_alive=0
    assert_eq 'KDE App PID unchanged after service restart' 1 "$app_alive"
    send_app 'dispatch alt+ctrl+up'
    kde_live_refresh_targets
    kde_live_assert_zone 'KDE recovered-service baseline center-third' center-third A
    action_before=$(count_text "$run_log" 'last action: alt+ctrl+left')
    kde_live_inject_key ctrl+alt+Left >/dev/null 2>&1 || true
    kde_live_assert_zone 'KDE recovered-service real hotkey moves window' left-half A
    send_app status
    local service_action_recovered=0
    wait_for_new_text "$run_log" 'last action: alt+ctrl+left' "$action_before" 10 && service_action_recovered=1
    assert_eq 'KDE recovered-service real hotkey reports action' 1 "$service_action_recovered"

    send_app 'dispatch alt+ctrl+up'
    kde_live_refresh_targets
    kde_live_assert_zone 'KDE invalid config retention baseline center-third' center-third A
    write_invalid_config
    sleep 0.4
    send_app reload
    sleep 0.3
    send_app reload
    wait_for_text "$run_log" 'Config reload error' 12 || true
    assert_text 'KDE invalid config exposes actionable error' 'Config reload error' \
        "$(cat "$run_log" 2>/dev/null || true)"
    send_app status
    assert_text 'KDE invalid reload preserves previous binding count' 'binding count: 5' \
        "$(cat "$run_log" 2>/dev/null || true)"
    kde_live_inject_key ctrl+alt+Left >/dev/null 2>&1 || true
    kde_live_assert_zone 'KDE invalid config retains active binding' left-half A

    # ADR 0009: a replacement KWin refuses keeps the old config and its grabs. The conflict
    # incumbent holds Alt+Ctrl+F24, so a config adding it must be refused as a whole.
    local conflict_before=0 refused_before=0 bindings_before=0 hotkeys_before=0
    conflict_before=$(count_text "$KDE_KWIN_LOG" 'KDE_CONFLICT_READY')
    kde_live_load_script "$KDE_CONFLICT_JS" window-zones-conflict
    kde_live_run_script "$KDE_LAST_SCRIPT_ID"
    wait_for_new_text "$KDE_KWIN_LOG" 'KDE_CONFLICT_READY' "$conflict_before" 10 || true
    assert_eq 'KDE rejected replacement incumbent holds the accelerator' 1 \
        "$(( $(count_text "$KDE_KWIN_LOG" 'KDE_CONFLICT_READY') > conflict_before ? 1 : 0 ))"
    refused_before=$(count_text "$run_log" 'were refused')
    write_five_bindings_config
    cat >>"$CONFIG_PATH" <<'EOF'

[[bindings]]
hotkey = "Alt+Ctrl+F24"
action = { type = "move-to-zone", zone = "center-third" }
EOF
    sleep 0.4
    send_app reload
    sleep 0.3
    send_app reload
    wait_for_new_text "$run_log" 'were refused' "$refused_before" 12 || true
    assert_eq 'KDE rejected replacement reports a refused reload' 1 \
        "$(( $(count_text "$run_log" 'were refused') > refused_before ? 1 : 0 ))"
    assert_text 'KDE rejected replacement names the accelerator owner' \
        'Window Zones conflict incumbent' "$(sed -n '/were refused/p' "$run_log")"
    bindings_before=$(count_text "$run_log" 'binding count: 5')
    hotkeys_before=$(count_text "$run_log" 'hotkey state: Registered')
    send_app status
    wait_for_new_text "$run_log" 'binding count: 5' "$bindings_before" 10 || true
    assert_eq 'KDE rejected replacement keeps the previous bindings' 1 \
        "$(( $(count_text "$run_log" 'binding count: 5') > bindings_before ? 1 : 0 ))"
    assert_eq 'KDE rejected replacement keeps hotkeys registered' 1 \
        "$(( $(count_text "$run_log" 'hotkey state: Registered') > hotkeys_before ? 1 : 0 ))"
    send_app 'dispatch alt+ctrl+up'
    kde_live_assert_zone 'KDE rejected replacement baseline center-third' center-third A
    kde_live_inject_key ctrl+alt+Left >/dev/null 2>&1 || true
    kde_live_assert_zone 'KDE rejected replacement keeps the previous grab working' left-half A
    kde_live_unload_script window-zones-conflict
    local loaded_before
    loaded_before=$(count_text "$run_log" 'Config state: Loaded')
    bindings_before=$(count_text "$run_log" 'binding count: 1')
    kde_live_write_changed_config
    sleep 0.4
    send_app reload
    sleep 0.3
    send_app reload
    wait_for_new_text "$run_log" 'Config state: Loaded' "$loaded_before" 30 || true
    assert_eq 'KDE valid changed config recovers' 1 \
        "$(( $(count_text "$run_log" 'Config state: Loaded') > loaded_before ? 1 : 0 ))"
    send_app status
    wait_for_new_text "$run_log" 'binding count: 1' "$bindings_before" 10 || true
    assert_eq 'KDE valid changed config updates binding count' 1 \
        "$(( $(count_text "$run_log" 'binding count: 1') > bindings_before ? 1 : 0 ))"
    loaded_before=$(count_text "$run_log" 'Config state: Loaded')
    bindings_before=$(count_text "$run_log" 'binding count: 5')
    write_five_bindings_config
    sleep 0.4
    send_app reload
    sleep 0.3
    send_app reload
    wait_for_new_text "$run_log" 'Config state: Loaded' "$loaded_before" 30 || true
    assert_eq 'KDE valid config restores bindings' 1 \
        "$(( $(count_text "$run_log" 'Config state: Loaded') > loaded_before ? 1 : 0 ))"
    send_app status
    wait_for_new_text "$run_log" 'binding count: 5' "$bindings_before" 30 || true
    assert_eq 'KDE valid config restores binding count' 1 \
        "$(( $(count_text "$run_log" 'binding count: 5') > bindings_before ? 1 : 0 ))"
    set +e
    stop_app_cleanly 'KDE App quits cleanly' "$run_log"
    set +e

    kde_live_tui_lifecycle
    kde_live_unload_script window-zones-observer
    stop_pid "$KDE_EDITOR_PID"
    wait_pid "$KDE_EDITOR_PID" 5 || true
    set +e
    stop_pid "$KDE_SERVICE_PID"
    stop_pid "$KDE_GROUP_PID"
    set -e
}
