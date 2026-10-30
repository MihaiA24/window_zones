kde_live_companion_call() {
    local method=$1
    shift
    busctl --address="$KDE_BUS" call \
        org.window_zones.KWin /org/window_zones/KWin org.window_zones.KWin3 "$method" "$@"
}

kde_live_kwin_call() {
    local method=$1
    shift
    busctl --address="$KDE_BUS" call \
        org.kde.KWin /Scripting org.kde.kwin.Scripting "$method" "$@"
}

kde_live_wait_companion_ready() {
    local timeout_s=${1:-30}
    local deadline=$((SECONDS + timeout_s))
    local owners='' owner_line='' loaded='' capabilities='' displays='' count=0
    KDE_COMPANION_READY=0
    KDE_SCRIPT_LOADED_RAW=''
    KDE_CAPABILITIES_RAW=''
    KDE_DISPLAYS_RAW=''
    while (( SECONDS < deadline )); do
        owners=$(busctl --address="$KDE_BUS" list 2>&1 || true)
        owner_line=$(printf '%s\n' "$owners" | sed -n '/org.window_zones.KWin/p' | sed -n '1p')
        loaded=$(kde_live_kwin_call isScriptLoaded s window-zones 2>&1 || true)
        capabilities=$(kde_live_get_capabilities)
        displays=$(kde_live_companion_call GetDisplays 2>&1 || true)
        count=$(kde_live_parse_displays "$displays" | sed '/^$/d' | wc -l | tr -d ' ')
        if [[ -n "$owner_line" && "$loaded" == *'true'* \
            && "$capabilities" == 'focused-window|displays|move-resize|hotkeys' \
            && "$count" -eq 2 ]]; then
            KDE_COMPANION_READY=1
            KDE_SCRIPT_LOADED_RAW="$loaded"
            KDE_CAPABILITIES_RAW="$capabilities"
            KDE_DISPLAYS_RAW="$displays"
            KDE_KWIN_OWNER_LINE=$(printf '%s\n' "$owners" | sed -n '/org.kde.KWin/p' | sed -n '1p')
            KDE_COMPANION_OWNER_LINE="$owner_line"
            return 0
        fi
        sleep 0.2
    done
    KDE_SCRIPT_LOADED_RAW="$loaded"
    KDE_CAPABILITIES_RAW="$capabilities"
    KDE_DISPLAYS_RAW="$displays"
    KDE_KWIN_OWNER_LINE=$(printf '%s\n' "$owners" | sed -n '/org.kde.KWin/p' | sed -n '1p')
    KDE_COMPANION_OWNER_LINE="$owner_line"
    log "KDE companion readiness timed out after ${timeout_s}s: owner='$owner_line' isScriptLoaded='$loaded' capabilities='$capabilities' displays='$count'"
    return 1
}

kde_live_wait_script_loaded() {
    local expected=$1 timeout_s=${2:-10}
    local deadline=$((SECONDS + timeout_s))
    local observed=''
    while (( SECONDS < deadline )); do
        observed=$(kde_live_kwin_call isScriptLoaded s window-zones 2>&1 || true)
        if [[ "$expected" == true && "$observed" == *'true'* ]] \
            || [[ "$expected" == false && "$observed" == *'false'* ]]; then
            printf '%s' "$observed"
            return 0
        fi
        sleep 0.2
    done
    printf '%s' "$observed"
    return 1
}

kde_live_load_script() {
    local script=$1 plugin=$2 output='' id=''
    output=$(kde_live_kwin_call loadScript ss "$script" "$plugin" 2>&1 || true)
    id=$(printf '%s\n' "$output" | sed -n 's/^i //p' | sed -n '1p')
    if [[ "$id" =~ ^-?[0-9]+$ ]]; then
        KDE_LAST_SCRIPT_ID="$id"
    else
        KDE_LAST_SCRIPT_ID='-1'
    fi
    log "KDE loadScript $plugin: output='$output' id=$KDE_LAST_SCRIPT_ID"
}

kde_live_run_script() {
    # Script ids are positional (`const int id = scripts.size()` upstream) and are reused after an
    # unload, so /Scripting/Script<id> can address a different, already-running script. Scripting
    # start() runs every loaded script that is not running yet and needs no id.
    local id=${1:-} output=''
    output=$(busctl --address="$KDE_BUS" call org.kde.KWin /Scripting \
        org.kde.kwin.Scripting start 2>&1 || true)
    log "KDE Scripting start (after load id=${id:-none}): $output"
}

kde_live_unload_script() {
    local plugin=$1 output=''
    output=$(kde_live_kwin_call unloadScript s "$plugin" 2>&1 || true)
    log "KDE unloadScript $plugin: $output"
}

kde_live_record_versions() {
    record_versions_common
    local outer='unknown' outer_version='unknown' xwayland_version='unavailable'
    local qt_version='unavailable' kf_version='unavailable'
    if [[ -n "${XDG_CURRENT_DESKTOP:-}" ]]; then
        outer="$XDG_CURRENT_DESKTOP"
    elif [[ -n "${XDG_SESSION_DESKTOP:-}" ]]; then
        outer="$XDG_SESSION_DESKTOP"
    fi
    if [[ "$outer" == *GNOME* || "$outer" == *gnome* ]] && which gnome-shell >/dev/null 2>&1; then
        outer_version=$(gnome-shell --version 2>&1 | sed -n '1p' || true)
    else
        outer_version=$(ps -eo args= 2>/dev/null \
            | sed -n '/^\(gnome-shell\|kwin_wayland\|sway\|Hyprland\)[[:space:]]/p' \
            | sed -n '1p' || true)
    fi
    [[ -n "$outer_version" ]] || outer_version='unavailable'
    log "VERSION outer compositor: $outer ($outer_version; XDG_SESSION_DESKTOP=${XDG_SESSION_DESKTOP:-unknown} WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-unknown})"
    log "VERSION kwin_wayland: $KDE_KWIN_VERSION"
    if which Xwayland >/dev/null 2>&1; then
        xwayland_version=$(Xwayland -version 2>&1 | sed -n '1p' || true)
    fi
    if which qtpaths6 >/dev/null 2>&1; then
        qt_version=$(qtpaths6 --qt-version 2>&1 | sed -n '1p' || true)
    elif which qmake6 >/dev/null 2>&1; then
        qt_version=$(qmake6 -query QT_VERSION 2>&1 | sed -n '1p' || true)
    fi
    if which kf6-config >/dev/null 2>&1; then

        kf_version=$(kf6-config --version 2>&1 | sed -n '1p' || true)
    fi
    log "VERSION Xwayland: $xwayland_version"
    log "VERSION Qt6: $qt_version"
    log "VERSION KDE Frameworks: $kf_version"
    log 'INPUT nested transport: Xwayland XTEST -> private KWin EIS (XwaylandEisNoPrompt=true)'
    log 'SESSION nested KWin is a Synthetic session; Release gate status remains deferred - unverified'
}

kde_live_preflight() {
    local missing='' tool='' version_ok=1 help_ok=1
    if ! which kwin_wayland >/dev/null 2>&1; then
        assert_eq 'KDE kwin_wayland installed' 1 0
        log 'BLOCKED KDE live: kwin_wayland is not installed; user must install kwin and its runtime dependencies; installation requires unavailable sudo credentials'
        return 1
    fi
    assert_eq 'KDE kwin_wayland installed' 1 1
    local -a required=(kpackagetool6 kwriteconfig6 busctl gjs xdotool xprop \
        dbus-run-session dbus-update-activation-environment python setsid gnome-text-editor)
    for tool in "${required[@]}"; do
        if which "$tool" >/dev/null 2>&1; then
            assert_eq "KDE preflight $tool installed" 1 1
        else
            missing="$missing $tool"
            assert_eq "KDE preflight $tool installed" 1 0
        fi
    done
    if [[ -n "$missing" ]]; then
        log "BLOCKED KDE live: missing required executables:$missing"
        return 1
    fi

    KDE_KWIN_VERSION=$(kwin_wayland --version 2>&1 || true)
    KDE_KWIN_HELP=$(kwin_wayland --help 2>&1 || true)
    [[ "$KDE_KWIN_VERSION" == *'6.'* ]] || version_ok=0
    assert_text 'KDE kwin_wayland is version 6' '6.' "$KDE_KWIN_VERSION"
    local -a flags=(--virtual --socket --output-count --xwayland --no-lockscreen --no-kactivities)
    for tool in "${flags[@]}"; do
        if [[ "$KDE_KWIN_HELP" != *"$tool"* ]]; then
            help_ok=0
        fi
        assert_text "KDE kwin_wayland supports $tool" "$tool" "$KDE_KWIN_HELP"
    done
    kde_live_record_versions
    if [[ "$version_ok" -ne 1 || "$help_ok" -ne 1 ]]; then
        log 'BLOCKED KDE live: installed kwin_wayland is not a supported KWin 6.x nested fixture'
        return 1
    fi
    return 0
}

kde_live_start_fixture() {
    cp "$SCRIPT_DIR/smoke/fixtures/kwin_publish_client_env.sh" "$KDE_PUBLISH_CLIENT_ENV"
    chmod 700 "$KDE_PUBLISH_CLIENT_ENV"
    cp "$SCRIPT_DIR/smoke/fixtures/kwin_launcher.sh" "$KDE_LAUNCHER"
    chmod 700 "$KDE_LAUNCHER"
    start_group KDE_GROUP_PID "$KDE_KWIN_LOG" env \
        -u DISPLAY -u XAUTHORITY -u WAYLAND_DISPLAY -u WAYLAND_SOCKET -u KDEHOME \
        BUS_FILE="$KDE_BUS_FILE" KDE_BUS_FILE="$KDE_BUS_FILE" \
        KDE_COMPANION_BIN="$KDE_COMPANION_BIN" KDE_SERVICE_LOG="$KDE_SERVICE_LOG" \
        KDE_SERVICE_PID_FILE="$KDE_SERVICE_PID_FILE" KDE_KWIN_PID_FILE="$KDE_KWIN_PID_FILE" \
        KDE_SOCKET="$KDE_SOCKET" \
        KDE_PUBLISH_CLIENT_ENV="$KDE_PUBLISH_CLIENT_ENV" \
        KDE_XDISPLAY_FILE="$KDE_XDISPLAY_FILE" KDE_XAUTHORITY_FILE="$KDE_XAUTHORITY_FILE" \
        KDE_CLIENT_BUS_FILE="$KDE_CLIENT_BUS_FILE" \
        HOME="$KDE_HOME" XDG_CONFIG_HOME="$KDE_CONFIG" XDG_DATA_HOME="$KDE_DATA" \
        XDG_CACHE_HOME="$KDE_CACHE" XDG_STATE_HOME="$KDE_STATE" \
        XDG_RUNTIME_DIR="$KDE_RUNTIME" XDG_DATA_DIRS="$KDE_DATA_DIRS" \
        XDG_CURRENT_DESKTOP=KDE XDG_SESSION_DESKTOP=KDE DESKTOP_SESSION=plasma \
        KDE_SESSION_VERSION=6 XDG_SESSION_TYPE=wayland \
        QT_LOGGING_RULES='kwin_libeis.debug=true;kwin_scripting.debug=true' \
        QT_FORCE_STDERR_LOGGING=1 \
        dbus-run-session -- "$KDE_LAUNCHER"
    if wait_for_file "$KDE_BUS_FILE" 20; then
        KDE_BUS=$(cat "$KDE_BUS_FILE")
        export DBUS_SESSION_BUS_ADDRESS="$KDE_BUS"
    else
        assert_text 'KDE private D-Bus session starts' 'unix:' "$(cat "$KDE_KWIN_LOG" 2>/dev/null || true)"
        return 1
    fi
    if wait_for_file "$KDE_SERVICE_PID_FILE" 10; then
        KDE_SERVICE_PID=$(cat "$KDE_SERVICE_PID_FILE")
        owned_process register "$KDE_SERVICE_PID"
    fi
    if wait_for_file "$KDE_KWIN_PID_FILE" 10; then
        KDE_KWIN_PID=$(cat "$KDE_KWIN_PID_FILE")
        owned_process register "$KDE_KWIN_PID"
    fi
    wait_for_text "$KDE_SERVICE_LOG" 'KWin companion service started' 15 || true
    assert_text 'KDE companion service starts before KWin' \
        'KWin companion service started' "$(cat "$KDE_SERVICE_LOG" 2>/dev/null || true)"
    if wait_for_file "$KDE_XDISPLAY_FILE" 30; then
        KDE_XDISPLAY=$(cat "$KDE_XDISPLAY_FILE")
    else
        KDE_XDISPLAY=''
    fi
    if wait_for_file "$KDE_XAUTHORITY_FILE" 30; then
        KDE_XAUTHORITY=$(cat "$KDE_XAUTHORITY_FILE")
    else
        KDE_XAUTHORITY=''
    fi
    KDE_CLIENT_BUS=$(cat "$KDE_CLIENT_BUS_FILE" 2>/dev/null || true)
    log "KDE nested client environment: DISPLAY=$KDE_XDISPLAY XAUTHORITY=$KDE_XAUTHORITY DBUS_SESSION_BUS_ADDRESS=$KDE_CLIENT_BUS"
    return 0
}

kde_live_prepare_environment() {
    export HOME="$KDE_HOME" XDG_CONFIG_HOME="$KDE_CONFIG" XDG_DATA_HOME="$KDE_DATA" \
        XDG_CACHE_HOME="$KDE_CACHE" XDG_STATE_HOME="$KDE_STATE" XDG_RUNTIME_DIR="$KDE_RUNTIME" \
        XDG_DATA_DIRS="$KDE_DATA_DIRS" XDG_SESSION_TYPE=wayland \
        XDG_CURRENT_DESKTOP=KDE XDG_SESSION_DESKTOP=KDE DESKTOP_SESSION=plasma \
        KDE_SESSION_VERSION=6 WAYLAND_DISPLAY="$KDE_SOCKET" \
        DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" GDK_BACKEND=''
    unset GNOME_DESKTOP_SESSION_ID SWAYSOCK HYPRLAND_INSTANCE_SIGNATURE WAYLAND_SOCKET KDEHOME
}
