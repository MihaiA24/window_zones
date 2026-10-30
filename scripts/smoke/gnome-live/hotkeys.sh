gnome_live_inject_hotkey() {
    local output='' rc=0
    output=$(python3 "$SCRIPT_DIR/smoke/fixtures/uinput_key.py" "$GNOME_SAFE_UINPUT_KEY" 2>&1) || rc=$?
    log "GNOME live uinput hotkey $GNOME_SAFE_LEFT_HOTKEY: exit $rc${output:+ output '$output'}"
    return "$rc"
}

gnome_live_choose_hotkeys() {
    local schema raw normalized candidate hotkey canonical reverse reserved conflict
    local -a candidates=(
        'ctrl+cmd+left|<super><control>left'
        'ctrl+cmd+up|<super><control>up'
        'ctrl+cmd+right|<super><control>right'
        'ctrl+cmd+down|<super><control>down'
        'pause|pause'
        'alt+pause|<alt>pause'
        'ctrl+pause|<control>pause'
    )
    GNOME_RESERVED_ACCELERATORS=()
    log 'GNOME live reserved accelerator enumeration:'
    for schema in org.gnome.desktop.wm.keybindings org.gnome.shell.keybindings \
        org.gnome.mutter.keybindings org.gnome.mutter.wayland.keybindings; do
        raw=$(DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" gsettings list-recursively "$schema" 2>&1 || true)
        normalized=$(printf '%s\n' "$raw" | tr '[:upper:]' '[:lower:]' | sed 's/<primary>/<control>/g')
        log "  $schema:"
        while IFS= read -r candidate; do
            [[ -n "$candidate" ]] && log "    $candidate"
        done <<<"$normalized"
        while IFS= read -r reserved; do
            GNOME_RESERVED_ACCELERATORS+=("$reserved")
        done < <(printf '%s\n' "$normalized" | grep -o "'[^']*'" | tr -d "'")
    done

    GNOME_SAFE_HOTKEYS=()
    for candidate in "${candidates[@]}"; do
        IFS='|' read -r hotkey canonical <<<"$candidate"
        reverse=${canonical/<super><control>/<control><super>}
        conflict=0
        for reserved in "${GNOME_RESERVED_ACCELERATORS[@]:-}"; do
            if [[ "$reserved" == "$canonical" || "$reserved" == "$reverse" ]]; then
                conflict=1
                break
            fi
        done
        if [[ "$conflict" -eq 0 ]]; then
            GNOME_SAFE_HOTKEYS+=("$hotkey")
        fi
    done
    log "GNOME live selected safe hotkeys: ${GNOME_SAFE_HOTKEYS[*]}"
    assert_eq 'GNOME live safe binding candidates' 1 \
        "$(( ${#GNOME_SAFE_HOTKEYS[@]} >= 4 ? 1 : 0 ))"
}

run_gnome_live_conflicting_binding() {
    local output='' rc=0
    output=$(gnome_call RegisterHotkeys "['alt+ctrl+left']" 2>&1) || rc=$?
    log "GNOME live conflicting RegisterHotkeys: exit $rc output '$output'"
    assert_rc_nonzero 'GNOME live conflicting binding is refused' "$rc" \
        'org.window_zones.Gnome.Error.Unsupported' "$output"
    assert_text 'GNOME live conflicting binding error name' \
        'org.window_zones.Gnome.Error.Unsupported' "$output"
    assert_text 'GNOME live conflicting binding names accelerator' \
        'alt+ctrl+left' "$output"
}

run_gnome_live_atomic_registration() {
    local helper="$SCRIPT_DIR/smoke/fixtures/gnome_atomic_registration.js"
    local helper_log="$GNOME_ROOT/atomic-helper.log"
    local helper_pid='' signal_count=0 before_count=0 after_count=0
    : >"$helper_log"
    start_group helper_pid "$helper_log" env DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" \
        LIVE_VALID_HOTKEY="$GNOME_SAFE_LEFT_HOTKEY" gjs -m "$helper"
    wait_for_text "$helper_log" 'VALID_OK' 8 || true
    assert_text 'GNOME live atomic valid registration' 'VALID_OK' \
        "$(cat "$helper_log" 2>/dev/null || true)"

    gnome_live_inject_hotkey || true
    wait_for_text "$helper_log" "EVENT $GNOME_SAFE_LEFT_HOTKEY BEFORE" 8 || true
    wait_for_text "$helper_log" 'INVALID_REJECTED' 8 || true
    assert_text 'GNOME live atomic invalid registration is refused' \
        'INVALID_REJECTED org.window_zones.Gnome.Error.Unsupported' \
        "$(cat "$helper_log" 2>/dev/null || true)"
    assert_text 'GNOME live atomic invalid accelerator is named' 'f25' \
        "$(cat "$helper_log" 2>/dev/null || true)"
    gnome_live_inject_hotkey || true
    wait_for_text "$helper_log" "EVENT $GNOME_SAFE_LEFT_HOTKEY AFTER" 8 || true
    before_count=$(count_text "$helper_log" "EVENT $GNOME_SAFE_LEFT_HOTKEY BEFORE")
    after_count=$(count_text "$helper_log" "EVENT $GNOME_SAFE_LEFT_HOTKEY AFTER")
    signal_count=$((before_count + after_count))
    log "GNOME live atomic receiver signals for $GNOME_SAFE_LEFT_HOTKEY: before=$before_count after=$after_count total=$signal_count"
    assert_eq 'GNOME live atomic registration preserves previous set' 1 \
        "$(( before_count >= 1 && after_count >= 1 && signal_count >= 2 ? 1 : 0 ))"
    stop_pid "$helper_pid"
    sleep 0.3
}
