kde_live_inject_key() {
    local sequence=$1
    DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" \
        xdotool key --clearmodifiers "$sequence"
}

write_kde_hotkey_client() {
    cp "$SCRIPT_DIR/smoke/fixtures/kwin_hotkey_client.js" "$1"
}

kde_live_atomic_registration() {
    local helper="$KDE_ROOT/atomic-register.js"
    local helper_log="$KDE_ROOT/atomic-register.log"
    local helper_pid='' valid_events_before=0 invalid_events_before=0 invalid_events_after=0
    write_kde_hotkey_client "$helper"
    cat "$SCRIPT_DIR/smoke/fixtures/kwin_atomic_registration.js" >>"$helper"
    : >"$helper_log"
    start_group helper_pid "$helper_log" env \
        DBUS_SESSION_BUS_ADDRESS="$KDE_BUS" KDE_VALID_HOTKEY="$KDE_SAFE_HOTKEY" \
        gjs -m "$helper"
    wait_for_text "$helper_log" 'VALID_READY' 12 || true
    assert_text 'KDE atomic valid registration completes' 'VALID_RESULT true' \
        "$(cat "$helper_log" 2>/dev/null || true)"
    valid_events_before=$(count_text "$helper_log" "EVENT $KDE_SAFE_HOTKEY")
    local inject_rc=0
    kde_live_inject_key ctrl+alt+Left || inject_rc=$?
    assert_rc_zero 'KDE atomic valid accelerator injection' "$inject_rc" 'xdotool XTEST'
    wait_for_new_text "$helper_log" "EVENT $KDE_SAFE_HOTKEY" "$valid_events_before" 8 || true
    assert_text 'KDE atomic valid accelerator reaches Companion' \
        "EVENT $KDE_SAFE_HOTKEY" "$(cat "$helper_log" 2>/dev/null || true)"
    wait_for_text "$helper_log" 'INVALID_READY' 12 || true
    local atomic_log=''
    atomic_log=$(cat "$helper_log" 2>/dev/null || true)
    assert_text 'KDE atomic rejected accelerator completes' 'INVALID_RESULT true' "$atomic_log"
    assert_text 'KDE atomic rejected accelerator names f25' 'f25' "$atomic_log"
    invalid_events_before=$(count_text "$helper_log" 'EVENT alt+ctrl+f25')
    kde_live_inject_key ctrl+alt+F25 >/dev/null 2>&1 || true
    sleep 0.5
    invalid_events_after=$(count_text "$helper_log" 'EVENT alt+ctrl+f25')
    assert_eq 'KDE atomic rejected accelerator is inactive' "$invalid_events_before" "$invalid_events_after"
    valid_events_before=$(count_text "$helper_log" "EVENT $KDE_SAFE_HOTKEY")
    if kde_live_inject_key ctrl+alt+Left; then
        inject_rc=0
    else
        inject_rc=$?
    fi
    assert_rc_zero 'KDE atomic previous accelerator injection' "$inject_rc" 'xdotool XTEST'
    local previous_active=0
    wait_for_new_text "$helper_log" "EVENT $KDE_SAFE_HOTKEY" "$valid_events_before" 8 \
        && previous_active=1
    assert_eq 'KDE atomic registration preserves previous set' 1 "$previous_active"
    wait_for_text "$helper_log" 'LIFECYCLE_READY' 16 || true
    local lifecycle_log=''
    lifecycle_log=$(cat "$helper_log" 2>/dev/null || true)
    assert_text 'KDE empty set releases native accelerator' 'EMPTY_AVAILABLE true' "$lifecycle_log"
    assert_text 'KDE removed accelerator can be re-added' 'READDED_AVAILABLE false' "$lifecycle_log"
    assert_text 'KDE replacement releases removed native accelerator' 'REPLACED_AVAILABLE true' "$lifecycle_log"
    assert_text 'KDE replacement restores old accelerator without script reload' 'RESTORED_AVAILABLE false' "$lifecycle_log"
    stop_pid "$helper_pid"
    wait_pid "$helper_pid" 5 || true
    local available_after_disconnect='' disconnect_deadline=$((SECONDS + 5))
    while (( SECONDS < disconnect_deadline )); do
        available_after_disconnect=$(busctl --address="$KDE_BUS" call \
            org.kde.kglobalaccel /kglobalaccel org.kde.KGlobalAccel \
            isGlobalShortcutAvailable is 218103826 '' 2>&1 || true)
        [[ "$available_after_disconnect" == 'b true' ]] && break
        sleep 0.1
    done
    assert_eq 'KDE controller disconnect releases native accelerator' 'b true' "$available_after_disconnect"
}

kde_live_conflict_registration() {
    KDE_CONFLICT_JS="$KDE_ROOT/conflict.js"
    cp "$SCRIPT_DIR/smoke/fixtures/kwin_conflict_incumbent.js" "$KDE_CONFLICT_JS"
    kde_live_load_script "$KDE_CONFLICT_JS" window-zones-conflict
    KDE_CONFLICT_SCRIPT_ID="$KDE_LAST_SCRIPT_ID"
    kde_live_run_script "$KDE_CONFLICT_SCRIPT_ID"
    wait_for_text "$KDE_KWIN_LOG" 'KDE_CONFLICT_READY' 10 || true
    assert_text 'KDE conflict fixture registers incumbent' 'KDE_CONFLICT_READY' \
        "$(cat "$KDE_KWIN_LOG" 2>/dev/null || true)"

    local helper="$KDE_ROOT/conflict-register.js" helper_log="$KDE_ROOT/conflict-register.log"
    local helper_pid='' inject_rc=0 valid_events_before=0 previous_active=0
    write_kde_hotkey_client "$helper"
    cat "$SCRIPT_DIR/smoke/fixtures/kwin_conflict_registration.js" >>"$helper"
    : >"$helper_log"
    start_group helper_pid "$helper_log" env \
        DBUS_SESSION_BUS_ADDRESS="$KDE_BUS" KDE_VALID_HOTKEY="$KDE_SAFE_HOTKEY" \
        gjs -m "$helper"
    wait_for_text "$helper_log" 'VALID_READY' 12 || true
    assert_text 'KDE conflict replacement starts with valid set' \
        'VALID_RESULT [true,"",""]' "$(cat "$helper_log" 2>/dev/null || true)"
    valid_events_before=$(count_text "$helper_log" "EVENT $KDE_SAFE_HOTKEY")
    kde_live_inject_key ctrl+alt+Left >/dev/null 2>&1 || true
    wait_for_new_text "$helper_log" "EVENT $KDE_SAFE_HOTKEY" "$valid_events_before" 5 \
        && previous_active=1
    assert_eq 'KDE conflict previous set active before replacement' 1 "$previous_active"
    wait_for_text "$helper_log" 'CONFLICT_READY' 12 || true
    assert_text 'KDE conflicting accelerator result completes' 'CONFLICT_RESULT [true,' \
        "$(cat "$helper_log" 2>/dev/null || true)"
    assert_text 'KDE conflicting replacement reports conflict error' \
        "KWin rejected shortcut 'alt+ctrl+f24': accelerator is already held by 'Window Zones conflict incumbent'" \
        "$(cat "$helper_log" 2>/dev/null || true)"
    valid_events_before=$(count_text "$helper_log" "EVENT $KDE_SAFE_HOTKEY")
    previous_active=0
    kde_live_inject_key ctrl+alt+Left >/dev/null 2>&1 || true
    wait_for_new_text "$helper_log" "EVENT $KDE_SAFE_HOTKEY" "$valid_events_before" 5 \
        && previous_active=1
    assert_eq 'KDE conflicting replacement preserves previous set' 1 "$previous_active"
    kde_live_inject_key ctrl+alt+F24 || inject_rc=$?
    assert_rc_zero 'KDE conflicting accelerator injection' "$inject_rc" 'xdotool XTEST'
    wait_for_text "$KDE_KWIN_LOG" 'KDE_CONFLICT_FIRED' 8 || true
    assert_text 'KDE conflict incumbent still receives accelerator' \
        'KDE_CONFLICT_FIRED' "$(cat "$KDE_KWIN_LOG" 2>/dev/null || true)"
    local conflict_events=0
    conflict_events=$(count_text "$helper_log" 'EVENT alt+ctrl+f24')
    assert_eq 'KDE conflicting accelerator delivers no Companion event' 0 "$conflict_events"
    stop_pid "$helper_pid"
    wait_pid "$helper_pid" 5 || true
    sleep 0.3

    # The user-visible contract: the App must not report a binding as registered when the
    # accelerator belongs to someone else. The raw protocol above only shows what the Companion
    # relayed; this drives the real runtime.
    local conflict_config_log="$KDE_ROOT/conflict-app.log"
    cat >"$CONFIG_PATH" <<EOF
[[bindings]]
hotkey = "alt+ctrl+f24"
action = { type = "move-to-zone", zone = "left-half" }
EOF
    start_fifo_app run auto "$conflict_config_log"
    wait_for_text "$conflict_config_log" 'Hotkey registration refused' 20 || true
    assert_text 'KDE App refuses a conflicting accelerator' \
        'Hotkey registration refused' "$(cat "$conflict_config_log" 2>/dev/null || true)"
    assert_text 'KDE App names the accelerator owner' \
        'Window Zones conflict incumbent' "$(cat "$conflict_config_log" 2>/dev/null || true)"
    stop_app_cleanly 'KDE App with a conflicting accelerator quits cleanly' "$conflict_config_log"
    write_five_bindings_config
    kde_live_unload_script window-zones-conflict
}
