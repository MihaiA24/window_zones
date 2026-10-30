x11_geometry() {
    local geometry key value x='' y='' width='' height=''
    local extents left=0 right=0 top=0 bottom=0
    geometry=$(xdotool getwindowgeometry --shell "$WINDOW_ID" 2>/dev/null || true)
    while IFS='=' read -r key value; do
        case "$key" in
            X) x=$value ;;
            Y) y=$value ;;
            WIDTH) width=$value ;;
            HEIGHT) height=$value ;;
        esac
    done <<<"$geometry"
    extents=$(xprop -id "$WINDOW_ID" _NET_FRAME_EXTENTS 2>/dev/null || true)
    if [[ "$extents" =~ =[[:space:]]*([0-9]+),[[:space:]]*([0-9]+),[[:space:]]*([0-9]+),[[:space:]]*([0-9]+) ]]; then
        left=${BASH_REMATCH[1]}
        right=${BASH_REMATCH[2]}
        top=${BASH_REMATCH[3]}
        bottom=${BASH_REMATCH[4]}
    fi
    if [[ "$x" =~ ^-?[0-9]+$ && "$y" =~ ^-?[0-9]+$ \
        && "$width" =~ ^[0-9]+$ && "$height" =~ ^[0-9]+$ ]]; then
        # xdotool reports client root geometry; all gate targets describe the frame.
        printf '%s,%s,%s,%s' "$((x - left))" "$((y - top))" \
            "$((width + left + right))" "$((height + top + bottom))"
    else
        printf 'unavailable'
    fi
}

x11_geometry_stable() {
    local timeout_s=${1:-5} observed previous='' stable=0
    local deadline=$((SECONDS + timeout_s))
    while (( SECONDS < deadline )); do
        observed=$(x11_geometry)
        if [[ "$observed" != 'unavailable' && "$observed" == "$previous" ]]; then
            stable=$((stable + 1))
            if (( stable >= 5 )); then
                return 0
            fi
        else
            stable=0
        fi
        previous="$observed"
        sleep 0.2
    done
    return 1
}

wait_x11_geometry() {
    local expected=$1 timeout_s=${2:-8} observed
    local deadline=$((SECONDS + timeout_s))
    while (( SECONDS < deadline )); do
        observed=$(x11_geometry)
        if [[ "$observed" == "$expected" ]]; then
            printf '%s' "$observed"
            return 0
        fi
        sleep 0.1
    done
    printf '%s' "$(x11_geometry)"
    return 1
}

snapshot_x11() {
    local label=$1
    local path="$TMP_DIR/${CURRENT_GATE}-${label}.png"
    LAST_SHOT=''
    if timeout 5 import -display "$X11_DISPLAY" -window "$WINDOW_ID" "$path" >/dev/null 2>&1 && [[ -s "$path" ]]; then
        LAST_SHOT="$path"
    elif command -v ffmpeg >/dev/null 2>&1 \
        && timeout 5 env DISPLAY="$X11_DISPLAY" ffmpeg -loglevel error -f x11grab -video_size 3520x1080 -i "$X11_DISPLAY.0" -frames:v 1 "$path" >/dev/null 2>&1 \
        && [[ -s "$path" ]]; then
        LAST_SHOT="$path"
    fi
    if [[ -n "$LAST_SHOT" ]]; then
        log "SCREENSHOT $label: $LAST_SHOT"
    else
        log "SCREENSHOT $label: unavailable (import/ffmpeg failed)"
    fi
}

x11_assert_geometry() {
    local name=$1 expected=$2 observed
    observed=$(wait_x11_geometry "$expected" 8 || true)
    snapshot_x11 "$name"
    assert_eq "$name" "$expected" "$observed" "$LAST_SHOT"
    if [[ -z "$LAST_SHOT" ]]; then
        log "Harness limitation $name screenshot: no usable screenshot tool"
    fi
}
