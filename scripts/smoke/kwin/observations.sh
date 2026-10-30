kde_live_parse_capabilities() {
    python - "$1" <<'PY'
import re, sys
print("|".join(re.findall(r'"([^"]*)"', sys.argv[1])))
PY
}

kde_live_parse_displays() {
    python - "$1" <<'PY'
import re, sys
pattern = re.compile(
    r'"([^"]*)"\s+(?:int32\s+)?(-?\d+)\s+'
    r'(?:int32\s+)?(-?\d+)\s+(?:(?:uint32|int32)\s+)?(\d+)\s+'
    r'(?:(?:uint32|int32)\s+)?(\d+)'
)
for match in pattern.finditer(sys.argv[1]):
    print("|".join(match.groups()))
PY
}

kde_live_parse_focused() {
    python - "$1" <<'PY'
import re, sys
match = re.search(
    r'\b(true|false)\s+"([^"]*)"\s+'
    r'(?:int32\s+)?(-?\d+)\s+(?:int32\s+)?(-?\d+)\s+'
    r'(?:(?:uint32|int32)\s+)?(\d+)\s+(?:(?:uint32|int32)\s+)?(\d+)',
    sys.argv[1],
)
if match:
    print("|".join(match.groups()))
PY
}

kde_live_get_capabilities() {
    kde_live_parse_capabilities "$(kde_live_companion_call GetCapabilities 2>&1 || true)"
}

kde_live_get_focused() {
    local raw
    raw=$(kde_live_companion_call GetFocusedWindow 2>&1 || true)
    kde_live_parse_focused "$raw"
}

kde_live_observer_records() {
    python - "$1" <"$SCRIPT_DIR/smoke/fixtures/kwin_observer_records.py"
}

kde_live_observer_layout_ok() {
    python - "$1" <"$SCRIPT_DIR/smoke/fixtures/kwin_observer_layout.py"
}

kde_live_compare_displays() {
    local companion_file="$KDE_ROOT/companion-displays.raw"
    local observer_file="$KDE_ROOT/observer-records"
    printf '%s\n' "$KDE_DISPLAYS_RAW" >"$companion_file"
    kde_live_parse_displays "$KDE_DISPLAYS_RAW" >"$KDE_ROOT/companion-displays"
    kde_live_observer_records "$KDE_KWIN_LOG" >"$observer_file"
    python - "$KDE_ROOT/companion-displays" "$observer_file" \
        <"$SCRIPT_DIR/smoke/fixtures/kwin_compare_displays.py"
}

kde_live_x11_active_window() {
    DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" \
        xdotool getactivewindow 2>/dev/null || printf 'unavailable'
}

kde_live_activate_window() {
    [[ -n "${WINDOW_ID:-}" ]] || return 1
    DISPLAY="$KDE_XDISPLAY" XAUTHORITY="$KDE_XAUTHORITY" \
        xdotool windowactivate --sync "$WINDOW_ID" >/dev/null 2>&1 || true
    sleep 0.3
}

kde_live_x11_geometry() {
    gtk_frame_geometry "$KDE_XDISPLAY" "$KDE_XAUTHORITY"
}

kde_live_wait_focus() {
    local timeout_s=${1:-15}
    local deadline=$((SECONDS + timeout_s))
    local observed='' focused='' active='' present='' window_id='' x='' y='' width='' height=''
    while (( SECONDS < deadline )); do
        observed=$(gtk_frame_geometry "$KDE_XDISPLAY" "$KDE_XAUTHORITY")
        focused=$(kde_live_get_focused)
        active=$(kde_live_x11_active_window)
        IFS='|' read -r present window_id x y width height <<<"$focused"
        if [[ "$observed" != unavailable && "$present" == true \
            && "$active" == "$WINDOW_ID" && "$observed" == "$x,$y,$width,$height" ]]; then
            KDE_FOCUS_X11="$observed"
            KDE_FOCUS_COMPANION="$focused"
            KDE_FOCUS_ACTIVE="$active"
            return 0
        fi
        sleep 0.2
    done
    KDE_FOCUS_X11="$observed"
    KDE_FOCUS_COMPANION="$focused"
    KDE_FOCUS_ACTIVE="$active"
    return 1
}

kde_live_assert_focus() {
    local focused="$KDE_FOCUS_COMPANION" observed="$KDE_FOCUS_X11" active="$KDE_FOCUS_ACTIVE"
    local present='' window_id='' x='' y='' width='' height=''
    IFS='|' read -r present window_id x y width height <<<"$focused"
    log "OBSERVED KDE focus: companion='$focused' xdotool='$observed' active='$active' window='${WINDOW_ID:-unavailable}'"
    assert_eq 'KDE GetFocusedWindow reports test window' true "$present"
    assert_eq 'KDE focused window has stable identity' 1 "$([[ -n "$window_id" ]] && printf 1 || printf 0)"
    assert_eq 'KDE focus geometry agrees with xdotool' "$observed" "$x,$y,$width,$height"
    assert_eq 'KDE focus active nested X11 window' "$WINDOW_ID" "$active"
    local display_match=0 id='' output_x output_y output_width output_height
    local center_x=$((x + width / 2)) center_y=$((y + height / 2))
    while IFS='|' read -r id output_x output_y output_width output_height; do
        if (( center_x >= output_x && center_x < output_x + output_width \
            && center_y >= output_y && center_y < output_y + output_height )); then
            display_match=1
            break
        fi
    done <"$KDE_ROOT/observer-records"
    assert_eq 'KDE focused frame center is on observer output' 1 "$display_match"
}

kde_live_select_display_targets() {
    local focused='' present='' ignored='' id='' x='' y='' width='' height='' record=''
    local center_x=0 center_y=0
    local -a records=()
    focused=$(kde_live_get_focused)
    IFS='|' read -r present ignored x y width height <<<"$focused"
    if [[ "$present" == true ]]; then
        center_x=$((x + width / 2))
        center_y=$((y + height / 2))
    fi
    while IFS='|' read -r id x y width height; do
        [[ -n "$id" ]] || continue
        [[ "$id" =~ ^-?[0-9]+$ ]] && continue
        records+=("$id|$x|$y|$width|$height")
    done <"$KDE_ROOT/observer-records"
    KDE_A_LINE=''
    KDE_B_LINE=''
    for record in "${records[@]}"; do
        IFS='|' read -r id x y width height <<<"$record"
        if [[ "$present" == true ]] && (( center_x >= x && center_x < x + width \
            && center_y >= y && center_y < y + height )); then
            KDE_A_LINE="$record"
            break
        fi
    done
    # Before the test window exists there is no focused display; fall back to the first output.
    [[ -n "$KDE_A_LINE" ]] || KDE_A_LINE="${records[0]:-}"
    for record in "${records[@]}"; do
        if [[ "$record" != "$KDE_A_LINE" ]]; then
            KDE_B_LINE="$record"
            break
        fi
    done
    IFS='|' read -r KDE_A_ID KDE_A_X KDE_A_Y KDE_A_W KDE_A_H <<<"$KDE_A_LINE"
    IFS='|' read -r KDE_B_ID KDE_B_X KDE_B_Y KDE_B_W KDE_B_H <<<"$KDE_B_LINE"
    KDE_LEFT_HALF="$KDE_A_X,$KDE_A_Y,$((KDE_A_W / 2)),$KDE_A_H"
    KDE_CENTER_THIRD="$((KDE_A_X + KDE_A_W / 3)),$KDE_A_Y,$((KDE_A_W - 2 * (KDE_A_W / 3))),$KDE_A_H"
    KDE_TWO_THIRDS="$KDE_A_X,$KDE_A_Y,$((KDE_A_W - KDE_A_W / 3)),$KDE_A_H"
    KDE_B_TWO_THIRDS="$KDE_B_X,$KDE_B_Y,$((KDE_B_W - KDE_B_W / 3)),$KDE_B_H"
    log "KDE independent observer targets: A=$KDE_A_LINE B=$KDE_B_LINE"
}

kde_live_refresh_targets() {
    # Nested outputs are windows of the outer compositor and can be resized mid-run, so zone
    # expectations are recomputed from the newest observer record before each phase.
    kde_live_observer_records "$KDE_KWIN_LOG" >"$KDE_ROOT/observer-records"
    kde_live_select_display_targets
    log "KDE refreshed targets: A=$KDE_A_LINE B=$KDE_B_LINE"
}

kde_live_zone_rect() {
    # zone name + one observer record -> expected rect, using the App's integer zone arithmetic.
    local zone=$1 x=$2 y=$3 width=$4 height=$5
    case "$zone" in
        left-half) printf '%s,%s,%s,%s' "$x" "$y" "$((width / 2))" "$height" ;;
        center-third)
            printf '%s,%s,%s,%s' "$((x + width / 3))" "$y" "$((width - 2 * (width / 3)))" "$height"
            ;;
        left-two-thirds) printf '%s,%s,%s,%s' "$x" "$y" "$((width - width / 3))" "$height" ;;
        *) printf 'unsupported-zone' ;;
    esac
}

kde_live_assert_zone() {
    # Outputs keep their identity for the whole gate: A is the display the test window started on
    # and B is the other one, so a cross-display move cannot relabel them. Only their rectangles
    # are re-read, from the newest observer record, on every poll.
    local name=$1 zone=$2 target=$3
    local deadline=$((SECONDS + 12)) observed_x11='' focused='' observed_companion=''
    local present='' display='' x='' y='' width='' height=''
    local expected='' ignored='' line='' wanted=''
    if [[ "$target" == B ]]; then
        wanted="$KDE_B_ID"
    else
        wanted="$KDE_A_ID"
    fi
    kde_live_activate_window || true
    while : ; do
        kde_live_observer_records "$KDE_KWIN_LOG" >"$KDE_ROOT/observer-records"
        line=$(sed -n "/^${wanted}|/p" "$KDE_ROOT/observer-records" | sed -n '1p')
        IFS='|' read -r ignored x y width height <<<"$line"
        expected=$(kde_live_zone_rect "$zone" "$x" "$y" "$width" "$height")
        observed_x11=$(gtk_frame_geometry "$KDE_XDISPLAY" "$KDE_XAUTHORITY")
        focused=$(kde_live_get_focused)
        observed_companion="$focused"
        IFS='|' read -r present display x y width height <<<"$focused"
        if [[ "$observed_x11" == "$expected" && "$present" == true \
            && "$x,$y,$width,$height" == "$expected" ]]; then
            break
        fi
        (( SECONDS < deadline )) || break
        sleep 0.2
    done
    log "OBSERVED $name: observer-expected='$expected' companion='$observed_companion' xdotool='$observed_x11'"
    assert_eq "$name independent X11 geometry" "$expected" "$observed_x11"
    assert_eq "$name Companion geometry" "true|$expected" "$present|$x,$y,$width,$height"
}
