kde_live_dispatch() {
    local label=$1 hotkey=$2 out="$KDE_ROOT/dispatch-$1.log" rc=0
    env DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" \
        XDG_RUNTIME_DIR="$KDE_RUNTIME" WAYLAND_DISPLAY="$KDE_SOCKET" \
        DBUS_SESSION_BUS_ADDRESS="$KDE_BUS" XDG_CONFIG_HOME="$KDE_CONFIG" \
        XDG_DATA_HOME="$KDE_DATA" HOME="$KDE_HOME" XDG_SESSION_TYPE=wayland \
        XDG_CURRENT_DESKTOP=KDE XDG_SESSION_DESKTOP=KDE DESKTOP_SESSION=plasma \
        KDE_SESSION_VERSION=6 "$BIN_PATH" --backend auto --config "$CONFIG_PATH" \
        dispatch "$hotkey" >"$out" 2>&1 || rc=$?
    cat "$out" >>"$ASSERT_LOG"
    assert_eq "$label dispatch exit" 0 "$rc"
    assert_text "$label dispatch state" 'Dispatch state: Succeeded' "$(cat "$out" 2>/dev/null || true)"
}

kde_live_move() {
    # window x,y,w,h: queue the Companion's MoveWindow directly and wait for the script's result;
    # print `ok` or the completed error.
    local reply='' request='' result='' deadline=$((SECONDS + 10))
    reply=$(kde_live_companion_call MoveWindow siiuu "$1" ${2//,/ } 2>&1 || true)
    request=$(printf '%s\n' "$reply" | sed -n 's/^u //p')
    if [[ -z "$request" ]]; then
        printf '%s' "$reply"
        return 0
    fi
    while (( SECONDS < deadline )); do
        result=$(kde_live_companion_call GetRequestResult s "$request" 2>&1 || true)
        [[ "$result" == 'bss true'* ]] && break
        sleep 0.1
    done
    if [[ "$result" == 'bss true "" ""' ]]; then
        printf 'ok'
    else
        printf '%s' "$result"
    fi
}

kde_live_window_state() {
    # tag window js: run js against the KWin window `w` with that internalId in a throwaway
    # script, then print KWin's own view: tile=<bool> max=<mode> fs=<bool> geo=x,y,w,h.
    local tag=$1 id=$2 body=$3 js="$KDE_ROOT/adr9-$1.js" plugin="window-zones-adr9-$1"
    cat >"$js" <<EOF
'use strict';
const w = Array.prototype.find.call(workspace.windowList(),
    candidate => !candidate.deleted && String(candidate.internalId) === '$id');
if (w) {
    $body
    const g = w.frameGeometry;
    console.warn('KDE_STATE $tag tile=' + !!w.tile + ' max=' + w.maximizeMode
        + ' fs=' + w.fullScreen + ' geo=' + [g.x, g.y, g.width, g.height].map(Math.round).join(','));
} else {
    console.warn('KDE_STATE $tag missing');
}
EOF
    kde_live_load_script "$js" "$plugin"
    kde_live_run_script "$KDE_LAST_SCRIPT_ID"
    wait_for_text "$KDE_KWIN_LOG" "KDE_STATE $tag " 10 || true
    kde_live_unload_script "$plugin"
    sed -n "s/.*KDE_STATE $tag //p" "$KDE_KWIN_LOG" | sed -n '$p'
}

kde_live_adr9_window_checks() {
    # ADR 0009 in nested KWin, observed through Xwayland's xdotool/xprop and KWin's own window
    # state read by a separate script: a captured id outlives a focus change and reports closure;
    # quick-tiled, maximized and fullscreen windows.
    local a="$WINDOW_ID" b='' b_pid='' a_id='' b_id='' out='' before='' b_before='' state=''
    local present='' rest=''
    XON_DISPLAY="$KDE_XDISPLAY"
    XON_XAUTHORITY="$KDE_XAUTHORITY"
    kde_live_refresh_targets
    kde_live_activate_window || true
    IFS='|' read -r present a_id rest <<<"$(kde_live_get_focused)"
    assert_text 'KDE captures the test window id' 'captured' "${a_id:+captured}"
    kde_live_move "$a_id" "$KDE_CENTER_THIRD" >/dev/null
    start_group b_pid "$KDE_ROOT/editor-b.log" env \
        DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" GDK_BACKEND=x11 \
        XDG_RUNTIME_DIR="$KDE_RUNTIME" WAYLAND_DISPLAY="$KDE_SOCKET" \
        DBUS_SESSION_BUS_ADDRESS="$KDE_BUS" XDG_CONFIG_HOME="$KDE_CONFIG" \
        XDG_DATA_HOME="$KDE_DATA" HOME="$KDE_HOME" XDG_SESSION_TYPE=wayland \
        XDG_CURRENT_DESKTOP=KDE XDG_SESSION_DESKTOP=KDE DESKTOP_SESSION=plasma \
        KDE_SESSION_VERSION=6 gnome-text-editor --standalone
    b=$(xwindow_for_pid "$b_pid" 25 || true)
    assert_text 'KDE second test window maps' 'mapped' "${b:+mapped}"
    if [[ -z "$b" || -z "$a_id" ]]; then
        stop_pid "$b_pid"
        return 0
    fi
    xon xdotool windowactivate --sync "$b" >/dev/null 2>&1 || true
    sleep 0.5
    IFS='|' read -r present b_id rest <<<"$(kde_live_get_focused)"
    assert_eq 'KDE focus moves to the second window' "$b" \
        "$(xon xdotool getactivewindow 2>/dev/null || printf unavailable)"
    assert_eq 'KDE Companion reports a different focused id' 1 \
        "$([[ -n "$b_id" && "$b_id" != "$a_id" ]] && printf 1 || printf 0)"
    WINDOW_ID="$b"
    b_before=$(kde_live_x11_geometry)
    WINDOW_ID="$a"
    out=$(kde_live_move "$a_id" "$KDE_LEFT_HALF")
    assert_eq 'KDE captured id move after focus change succeeds' ok "$out"
    assert_eq 'KDE captured id moves the original window' "$KDE_LEFT_HALF" \
        "$(wait_observed kde_live_x11_geometry "$KDE_LEFT_HALF")"
    WINDOW_ID="$b"
    assert_eq 'KDE captured id move leaves the focused window frame' "$b_before" \
        "$(kde_live_x11_geometry)"
    WINDOW_ID="$a"
    assert_eq 'KDE captured id move leaves focus on the second window' "$b" \
        "$(xon xdotool getactivewindow 2>/dev/null || printf unavailable)"

    stop_pid "$b_pid"
    wait_pid "$b_pid" 5 || true
    set +e
    wait_xwindow_gone "$b" 10 || true
    before=$(kde_live_x11_geometry)
    out=$(kde_live_move "$b_id" "$KDE_CENTER_THIRD")
    assert_text 'KDE closed target reports WindowGone' 'WindowGone' "$out"
    sleep 1
    assert_eq 'KDE closed target leaves the other window unchanged' "$before" \
        "$(kde_live_x11_geometry)"

    state=$(kde_live_window_state tile "$a_id" \
        'workspace.activeWindow = w; workspace.slotWindowQuickTileLeft();')
    assert_text 'KDE quick-tiled precondition' 'tile=true' "$state"
    out=$(kde_live_move "$a_id" "$KDE_TWO_THIRDS")
    assert_eq 'KDE quick-tiled window move succeeds' ok "$out"
    assert_text 'KDE quick-tiled window is restored before the frame' \
        "tile=false max=0 fs=false geo=$KDE_TWO_THIRDS" "$(kde_live_window_state tiled-after "$a_id" '')"
    assert_eq 'KDE quick-tiled window gets the target frame' "$KDE_TWO_THIRDS" \
        "$(wait_observed kde_live_x11_geometry "$KDE_TWO_THIRDS")"

    state=$(kde_live_window_state max "$a_id" 'w.setMaximize(true, true);')
    assert_text 'KDE maximized precondition' 'max=3' "$state"
    assert_wm_state 'KDE maximized precondition Xwayland state' "$a" _NET_WM_STATE_MAXIMIZED_HORZ 1
    out=$(kde_live_move "$a_id" "$KDE_CENTER_THIRD")
    assert_eq 'KDE maximized window move succeeds' ok "$out"
    assert_text 'KDE maximized window is restored before the frame' \
        "tile=false max=0 fs=false geo=$KDE_CENTER_THIRD" "$(kde_live_window_state max-after "$a_id" '')"
    assert_wm_state 'KDE maximized window leaves Xwayland maximized state' "$a" _NET_WM_STATE_MAXIMIZED 0
    assert_eq 'KDE maximized window gets the target frame' "$KDE_CENTER_THIRD" \
        "$(wait_observed kde_live_x11_geometry "$KDE_CENTER_THIRD")"

    state=$(kde_live_window_state fs "$a_id" 'w.fullScreen = true;')
    assert_text 'KDE fullscreen precondition' 'fs=true' "$state"
    assert_wm_state 'KDE fullscreen precondition Xwayland state' "$a" _NET_WM_STATE_FULLSCREEN 1
    sleep 1
    before=$(kde_live_x11_geometry)
    out=$(kde_live_move "$a_id" "$KDE_LEFT_HALF")
    assert_text 'KDE fullscreen window move is rejected' \
        'window is fullscreen; leave fullscreen first' "$out"
    assert_text 'KDE fullscreen rejection keeps fullscreen' 'fs=true' \
        "$(kde_live_window_state fs-after "$a_id" '')"
    assert_wm_state 'KDE fullscreen rejection keeps Xwayland state' "$a" _NET_WM_STATE_FULLSCREEN 1
    sleep 1
    assert_eq 'KDE fullscreen rejection keeps the frame' "$before" "$(kde_live_x11_geometry)"
    state=$(kde_live_window_state fs-exit "$a_id" 'w.fullScreen = false;')
    assert_text 'KDE test window leaves fullscreen' 'fs=false' "$state"
    kde_live_move "$a_id" "$KDE_TWO_THIRDS" >/dev/null
    wait_observed kde_live_x11_geometry "$KDE_TWO_THIRDS" >/dev/null
    kde_live_activate_window || true
}
