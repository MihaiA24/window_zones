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

gnome_live_placement_action() {
    local run_log=$1 action=$2 loaded_before
    GNOME_LIVE_EXPECTED_ACTION=$3
    loaded_before=$(count_text "$run_log" 'Config state: Loaded')
    write_gnome_live_valid_config "$action" || return 1
    send_app reload || return 1
    if ! wait_for_new_text "$run_log" 'Config state: Loaded' "$loaded_before" 10; then
        assert_eq 'GNOME live placement reload completes' Loaded 'no fresh successful reload'
        return 1
    fi
}

gnome_live_placement_press() {
    local expected_id=$1 focused_id active
    gnome_live_activate_window || return 1
    focused_id=$(gnome_live_focused_id)
    active=$(gnome_live_active_window)
    if [[ "$focused_id" != "$expected_id" || "$active" != "$WINDOW_ID" ]]; then
        assert_eq 'GNOME live placement input remains on its owned window' \
            "$expected_id|$WINDOW_ID" "$focused_id|$active"
        return 1
    fi
    GNOME_LIVE_DISPATCH_OFFSET=$(stat -c '%s' "$GNOME_LIVE_PERF_LOG") || return 1
    local rc=0
    gnome_live_inject_hotkey || rc=$?
    if (( rc != 0 )); then
        assert_rc_zero 'GNOME live placement accelerator injection' "$rc" "$GNOME_SAFE_LEFT_HOTKEY"
        return 1
    fi
    local overview
    overview=$(gnome_shell_call /org/gnome/Shell org.freedesktop.DBus.Properties.Get \
        org.gnome.Shell OverviewActive 2>&1 || true)
    log "OBSERVED GNOME live after input: focused='$(get_gnome_focused)' active='$(gnome_live_active_window)' overview='$overview'"
}

gnome_live_placement_result() {
    local run_log=$1 label=$2 seen=0
    if python3 - "$GNOME_LIVE_PERF_LOG" "$GNOME_LIVE_DISPATCH_OFFSET" \
        "$APP_PID" "$GNOME_SAFE_LEFT_HOTKEY" "$GNOME_LIVE_EXPECTED_ACTION" <<'PY'
import json
import sys
import time

path, offset, pid, hotkey, action = sys.argv[1:]
expected = dict(pid=int(pid), hotkey=hotkey, action=json.loads(action), result="ok")
deadline = time.monotonic() + 5
while time.monotonic() < deadline:
    with open(path, "rb") as stream:
        stream.seek(int(offset))
        for line in stream:
            if not line.endswith(b"\n"):
                break
            row = json.loads(line)
            if row.get("event") != "dispatch":
                continue
            print("OBSERVED fresh dispatch receipt: " + json.dumps(row), flush=True)
            sys.exit(0 if all(row.get(key) == value for key, value in expected.items()) else 1)
    time.sleep(0.05)
print("No fresh dispatch receipt; cached status is not accelerator delivery proof.", file=sys.stderr)
sys.exit(1)
PY
    then
        seen=1
    fi
    assert_eq "$label reaches the App successfully" 1 "$seen"
    send_app status || return 1
    (( seen == 1 ))
}

run_gnome_live_zone_checks() {
    # Reuse one unreserved accelerator; reload its action without changing the four-key fixture.
    local run_log="$GNOME_ROOT/live-zones-run.log" owned_id baseline zone expected row
    local x=$GNOME_LIVE_X y=$GNOME_LIVE_Y w=$GNOME_LIVE_W h=$GNOME_LIVE_H
    local succeeded_before error_before failures_before=$FAILURES
    baseline=$(gnome_live_geometry)
    owned_id=$(gnome_live_focused_id) || return 1
    [[ -n "$owned_id" ]] || return 1
    write_gnome_live_valid_config || return 1
    GNOME_LIVE_PERF_LOG="$GNOME_ROOT/live-zones-dispatch.jsonl"
    start_fifo_app run wayland "$run_log" --perf-log "$GNOME_LIVE_PERF_LOG" || return 1
    if ! wait_for_text "$run_log" 'Hotkeys registered: 4' 15; then
        assert_text 'GNOME live zone runtime registers its safe accelerators' \
            'Hotkeys registered: 4' "$(<"$run_log")"
        return 1
    fi
    local -a cases=(
        "left-half|$x,$y,$((w / 2)),$h"
        "right-half|$((x + w - w / 2)),$y,$((w / 2)),$h"
        "left-third|$x,$y,$((w / 3)),$h"
        "center-third|$((x + w / 3)),$y,$((w - 2 * (w / 3))),$h"
        "right-third|$((x + w - w / 3)),$y,$((w / 3)),$h"
        "left-two-thirds|$x,$y,$((w - w / 3)),$h"
        "right-two-thirds|$((x + w / 3)),$y,$((w - w / 3)),$h"
        "maximize|$x,$y,$w,$h"
        "top-half|$x,$y,$w,$((h / 2))"
        "bottom-half|$x,$((y + h - h / 2)),$w,$((h / 2))"
        "top-left|$x,$y,$((w / 2)),$((h / 2))"
        "top-right|$((x + w - w / 2)),$y,$((w / 2)),$((h / 2))"
        "bottom-left|$x,$((y + h - h / 2)),$((w / 2)),$((h / 2))"
        "bottom-right|$((x + w - w / 2)),$((y + h - h / 2)),$((w / 2)),$((h / 2))"
    )
    for row in "${cases[@]}"; do
        IFS='|' read -r zone expected <<<"$row"
        gnome_live_placement_action "$run_log" \
            "{ type = \"move-to-zone\", zone = \"$zone\" }" \
            "{\"type\":\"move-to-zone\",\"zone\":\"$zone\"}" || return 1
        gnome_live_placement_press "$owned_id" || return 1
        if ! gnome_live_placement_result "$run_log" "GNOME live accelerator $zone"; then
            gnome_live_capture "GNOME live accelerator $zone missing-matching-dispatch"
            return 1
        fi
        gnome_live_assert_geometry "GNOME live accelerator $zone" "$expected"
        (( FAILURES == failures_before )) || return 1
    done
    gnome_live_placement_action "$run_log" '{ type = "center" }' '{"type":"center"}' || return 1
    gnome_live_placement_press "$owned_id" || return 1
    if ! gnome_live_placement_result "$run_log" 'GNOME live accelerator center'; then
        gnome_live_capture 'GNOME live accelerator center missing-matching-dispatch'
        return 1
    fi
    gnome_live_assert_geometry 'GNOME live accelerator center keeps size' \
        "$((x + (w - w / 2) / 2)),$((y + (h - h / 2) / 2)),$((w / 2)),$((h / 2))"
    (( FAILURES == failures_before )) || return 1
    gnome_live_placement_action "$run_log" '{ type = "restore" }' '{"type":"restore"}' || return 1
    succeeded_before=$(count_text "$run_log" 'Dispatch state: Succeeded')
    send_app "dispatch $GNOME_SAFE_LEFT_HOTKEY" || return 1
    wait_for_new_text "$run_log" 'Dispatch state: Succeeded' "$succeeded_before" 10 || true
    assert_eq 'GNOME live interactive Restore shares accelerator history' 1 \
        "$(( $(count_text "$run_log" 'Dispatch state: Succeeded') > succeeded_before ? 1 : 0 ))"
    gnome_live_assert_geometry 'GNOME live Restore returns to the first original frame' "$baseline"
    (( FAILURES == failures_before )) || return 1
    error_before=$(count_text "$run_log" 'nothing to restore')
    gnome_live_placement_press "$owned_id" || return 1
    send_app status || return 1
    wait_for_new_text "$run_log" 'nothing to restore' "$error_before" 10 || true
    assert_eq 'GNOME live Restore consumes its history' 1 \
        "$(( $(count_text "$run_log" 'nothing to restore') > error_before ? 1 : 0 ))"
    gnome_live_assert_geometry 'GNOME live consumed Restore keeps its frame' "$baseline"
    stop_app_cleanly 'GNOME live zone runtime quits cleanly' "$run_log"
    write_gnome_live_valid_config || return 1
    (( FAILURES == failures_before ))
}
