parse_gnome_displays() {
    local raw=$1
    python - "$raw" <<'PY'
import re, sys
s = sys.argv[1]
# gdbus prints `uint32` only on the first value of a type in a container, so later tuples omit it.
for item in re.findall(r"\('([^']+)',\s*(-?\d+),\s*(-?\d+),\s*(?:uint32\s+)?(\d+),\s*(?:uint32\s+)?(\d+)\)", s):
    print('|'.join(item))
PY
}

parse_gnome_focused() {
    local raw=$1
    python - "$raw" <<'PY'
import re, sys
s = sys.argv[1]
m = re.search(r"\((true|false),\s*(?:uint64\s+)?(\d+),\s*(-?\d+),\s*(-?\d+),\s*(?:uint32\s+)?(\d+),\s*(?:uint32\s+)?(\d+)\)", s)
if m:
    print('|'.join(m.groups()))
PY
}

gnome_call() {
    DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" gdbus call --session \
        --dest org.window_zones.Gnome --object-path /org/window_zones/Gnome \
        --method "org.window_zones.Gnome2.$1" "${@:2}"
}

gnome_shell_call() {
    DBUS_SESSION_BUS_ADDRESS="$GNOME_BUS" gdbus call --session \
        --dest org.gnome.Shell --object-path "$1" --method "$2" "${@:3}"
}

get_gnome_focused() {
    local raw
    raw=$(gnome_call GetFocusedWindow 2>&1 || true)
    parse_gnome_focused "$raw"
}

gnome_live_geometry() {
    gtk_frame_geometry "$GNOME_XDISPLAY" "$GNOME_XAUTHORITY"
}

gnome_live_zone_targets() {
    # Derive the current display from the frame center; the wire id identifies the window.
    local focused present window_id x y w h id center_x center_y
    local area='' next_area='' display_lines found=-1 total=0 index=0
    focused=$(get_gnome_focused)
    IFS='|' read -r present window_id x y w h <<<"$focused"
    [[ "$present" == 'true' ]] || return 0
    center_x=$((x + w / 2))
    center_y=$((y + h / 2))
    # Match the executor's stable spatial ordering, including enumeration order on ties.
    display_lines=$(printf '%s\n' "$GNOME_LIVE_DISPLAY_LINES" | LC_ALL=C sort -s -t '|' -k2,2n -k3,3n)
    while IFS='|' read -r id x y w h; do
        [[ -n "$id" ]] && (( w > 0 && h > 0 )) || continue
        if (( found < 0 && center_x >= x && center_x < x + w \
            && center_y >= y && center_y < y + h )); then
            found=$total
            area="$x|$y|$w|$h"
            GNOME_LIVE_DISPLAY_ID="$id"
        fi
        total=$((total + 1))
    done <<<"$display_lines"
    (( found >= 0 )) || return 0
    while IFS='|' read -r id x y w h; do
        [[ -n "$id" ]] && (( w > 0 && h > 0 )) || continue
        if (( index == (found + 1) % total )); then
            next_area="$x|$y|$w|$h"
        fi
        index=$((index + 1))
    done <<<"$display_lines"
    IFS='|' read -r GNOME_LIVE_X GNOME_LIVE_Y GNOME_LIVE_W GNOME_LIVE_H <<<"$area"
    GNOME_LIVE_LEFT_HALF="$GNOME_LIVE_X,$GNOME_LIVE_Y,$((GNOME_LIVE_W / 2)),$GNOME_LIVE_H"
    GNOME_LIVE_CENTER_THIRD="$((GNOME_LIVE_X + GNOME_LIVE_W / 3)),$GNOME_LIVE_Y,$((GNOME_LIVE_W - 2 * (GNOME_LIVE_W / 3))),$GNOME_LIVE_H"
    GNOME_LIVE_TWO_THIRDS="$GNOME_LIVE_X,$GNOME_LIVE_Y,$((GNOME_LIVE_W - GNOME_LIVE_W / 3)),$GNOME_LIVE_H"
    GNOME_LIVE_NEXT_TWO_THIRDS=''
    GNOME_LIVE_DISPLAY_TOTAL=$total
    if (( total > 1 )) && [[ -n "$next_area" ]]; then
        local next_x next_y next_w next_h
        IFS='|' read -r next_x next_y next_w next_h <<<"$next_area"
        # A cross-display move keeps a recognised zone, so left-two-thirds stays left-two-thirds.
        GNOME_LIVE_NEXT_TWO_THIRDS="$next_x,$next_y,$((next_w - next_w / 3)),$next_h"
    fi
    log "GNOME live zones follow $GNOME_LIVE_DISPLAY_ID ($area); next-display two-thirds: ${GNOME_LIVE_NEXT_TWO_THIRDS:-none}"
}

gnome_live_active_window() {
    DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" \
        xdotool getactivewindow 2>/dev/null || printf 'unavailable'
}

gnome_live_activate_window() {
    [[ -n "${WINDOW_ID:-}" ]] || return 1
    DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" \
        xdotool windowactivate --sync "$WINDOW_ID" >/dev/null 2>&1 || true
    sleep 0.3
}

gnome_live_find_window() {
    WINDOW_ID=''
    local id pid last=''
    while read -r id; do
        [[ -n "$id" ]] || continue
        last=$id
        pid=$(DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" \
            xdotool getwindowpid "$id" 2>/dev/null || true)
        if [[ "$pid" == "$EDITOR_PID" ]]; then
            WINDOW_ID=$id
            return 0
        fi
    done < <(DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" \
        xdotool search --onlyvisible --name 'Text Editor' 2>/dev/null || true)
    WINDOW_ID=$last
    [[ -n "$WINDOW_ID" ]]
}

gnome_live_capture() {
    local label=$1
    local path="$TMP_DIR/${CURRENT_GATE}-${label}.png"
    local capture_log="$TMP_DIR/${CURRENT_GATE}-${label}-ffmpeg.log"
    local geometry width height
    LAST_SHOT=''
    geometry=$(DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" \
        xdotool getdisplaygeometry 2>/dev/null || true)
    if [[ "$geometry" =~ ^([0-9]+)[[:space:]]+([0-9]+)$ ]]; then
        width=${BASH_REMATCH[1]}
        height=${BASH_REMATCH[2]}
        if timeout 5 env DISPLAY="$GNOME_XDISPLAY" XAUTHORITY="$GNOME_XAUTHORITY" \
            ffmpeg -nostdin -loglevel error -f x11grab \
            -video_size "${width}x${height}" -i "${GNOME_XDISPLAY}.0+0,0" \
            -frames:v 1 -y "$path" >"$capture_log" 2>&1 \
            && [[ -s "$path" ]]; then
            LAST_SHOT="$path"
            log "SCREENSHOT $label: $path (ffmpeg x11grab root ${width}x${height})"
        else
            log "SCREENSHOT $label: unavailable (ffmpeg x11grab failed; log $capture_log: $(cat "$capture_log" 2>/dev/null || true))"
        fi
    else
        log "SCREENSHOT $label: unavailable (xdotool root geometry unavailable; ffmpeg not attempted)"
    fi
}

gnome_live_focus_baseline() {
    local timeout_s=${1:-10}
    local deadline=$((SECONDS + timeout_s))
    local focused='' observed='' active='' present='' window_id='' x='' y='' width='' height=''
    GNOME_LIVE_FOCUS_X11=''
    GNOME_LIVE_FOCUS_COMPANION=''
    GNOME_LIVE_FOCUS_ACTIVE=''
    while (( SECONDS < deadline )); do
        observed=$(gtk_frame_geometry "$GNOME_XDISPLAY" "$GNOME_XAUTHORITY")
        focused=$(get_gnome_focused)
        active=$(gnome_live_active_window)
        IFS='|' read -r present window_id x y width height <<<"$focused"
        if [[ "$observed" != 'unavailable' && "$present" == 'true' \
            && "$x,$y,$width,$height" == "$observed" && "$active" == "$WINDOW_ID" ]]; then
            GNOME_LIVE_FOCUS_X11="$observed"
            GNOME_LIVE_FOCUS_COMPANION="$focused"
            GNOME_LIVE_FOCUS_ACTIVE="$active"
            return 0
        fi
        sleep 0.2
    done
    GNOME_LIVE_FOCUS_X11="$observed"
    GNOME_LIVE_FOCUS_COMPANION="$focused"
    GNOME_LIVE_FOCUS_ACTIVE="$active"
    return 1
}

gnome_live_assert_focus() {
    local focused=$GNOME_LIVE_FOCUS_COMPANION observed=$GNOME_LIVE_FOCUS_X11
    local active=$GNOME_LIVE_FOCUS_ACTIVE
    local present='' window_id='' x='' y='' width='' height='' focused_geometry=''
    IFS='|' read -r present window_id x y width height <<<"$focused"
    focused_geometry="$x,$y,$width,$height"
    log "OBSERVED GNOME live focus: companion='$focused' xdotool='$observed' active='$active' window='$WINDOW_ID'"
    assert_eq 'GNOME live focused-window present' 'true' "$present"
    assert_eq 'GNOME live focus geometry agrees with xdotool' "$observed" "$focused_geometry"
    assert_eq 'GNOME live focus active X11 window' "$WINDOW_ID" "$active"
    gnome_live_capture focused-window
}

gnome_live_assert_geometry() {
    local name=$1 expected=$2
    local deadline=$((SECONDS + 10))
    local observed_x11='' observed_companion='' focused=''
    local present='' window_id='' x='' y='' width='' height=''
    gnome_live_activate_window || true
    while (( SECONDS < deadline )); do
        observed_x11=$(gtk_frame_geometry "$GNOME_XDISPLAY" "$GNOME_XAUTHORITY")
        focused=$(get_gnome_focused)
        IFS='|' read -r present window_id x y width height <<<"$focused"
        observed_companion="$focused"
        if [[ "$observed_x11" == "$expected" \
            && "$present" == 'true' \
            && "$x,$y,$width,$height" == "$expected" ]]; then
            break
        fi
        sleep 0.2
    done
    log "OBSERVED $name: companion='$observed_companion' xdotool='$observed_x11'"
    assert_eq "$name xdotool geometry" "$expected" "$observed_x11"
    assert_eq "$name companion geometry" "true|$expected" "$present|$x,$y,$width,$height"
    gnome_live_capture "$name"
}
