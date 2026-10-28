gtk_frame_geometry() {
    local display=$1 authority=$2
    local geometry='' key='' value='' x='' y='' width='' height=''
    if [[ -z "${WINDOW_ID:-}" ]]; then
        printf 'unavailable'
        return 0
    fi
    geometry=$(DISPLAY="$display" XAUTHORITY="$authority" \
        xdotool getwindowgeometry --shell "$WINDOW_ID" 2>/dev/null || true)
    while IFS='=' read -r key value; do
        case "$key" in
            X) x=$value ;;
            Y) y=$value ;;
            WIDTH) width=$value ;;
            HEIGHT) height=$value ;;
        esac
    done <<<"$geometry"
    # A GTK client under XWayland owns its shadow: the X window is _GTK_FRAME_EXTENTS larger on
    # every side than the frame the compositor places. Inset it so the observer and the Companion
    # describe the same rectangle.
    local extents left=0 right=0 top=0 bottom=0
    extents=$(DISPLAY="$display" XAUTHORITY="$authority" \
        xprop -id "$WINDOW_ID" _GTK_FRAME_EXTENTS 2>/dev/null | sed -n 's/.*= //p')
    if [[ "$extents" =~ ^([0-9]+),\ ([0-9]+),\ ([0-9]+),\ ([0-9]+)$ ]]; then
        left=${BASH_REMATCH[1]}
        right=${BASH_REMATCH[2]}
        top=${BASH_REMATCH[3]}
        bottom=${BASH_REMATCH[4]}
    fi
    if [[ "$x" =~ ^-?[0-9]+$ && "$y" =~ ^-?[0-9]+$ \
        && "$width" =~ ^[0-9]+$ && "$height" =~ ^[0-9]+$ ]]; then
        printf '%s,%s,%s,%s' "$((x + left))" "$((y + top))" \
            "$((width - left - right))" "$((height - top - bottom))"
    else
        printf 'unavailable'
    fi
}

xon() {
    # Run an X client on the gate's display: $XON_DISPLAY plus $XON_XAUTHORITY when it has one.
    if [[ -n "${XON_XAUTHORITY:-}" ]]; then
        DISPLAY="$XON_DISPLAY" XAUTHORITY="$XON_XAUTHORITY" "$@"
    else
        DISPLAY="$XON_DISPLAY" "$@"
    fi
}

xwindow_for_pid() {
    # pid [timeout]: print the visible editor window that process mapped.
    local pid=$1 timeout_s=${2:-20} id='' owner=''
    local deadline=$((SECONDS + timeout_s))
    while (( SECONDS < deadline )); do
        owned_process alive "$pid" || return 1
        while read -r id; do
            [[ -n "$id" ]] || continue
            owner=$(xon xdotool getwindowpid "$id" 2>/dev/null || true)
            if [[ "$owner" == "$pid" ]] && owned_process alive "$pid"; then
                printf '%s' "$id"
                return 0
            fi
        done < <(xon xdotool search --onlyvisible --name 'Text Editor' 2>/dev/null || true)
        sleep 0.2
    done
    return 1
}

wait_xwindow_gone() {
    # window [timeout]: the X window is destroyed and the window manager stopped listing it.
    local id=$1 timeout_s=${2:-10} clients='' hex
    local deadline=$((SECONDS + timeout_s))
    hex=$(printf '0x%x' "$id")
    while (( SECONDS < deadline )); do
        clients=$(xon xprop -root _NET_CLIENT_LIST 2>/dev/null || true)
        if ! xon xdotool getwindowname "$id" >/dev/null 2>&1 \
            && [[ ! "$clients" =~ (^|[^0-9a-fx])$hex([^0-9a-f]|$) ]]; then
            return 0
        fi
        sleep 0.2
    done
    return 1
}

assert_wm_state() {
    # name window needle want: want 1 = _NET_WM_STATE lists needle, 0 = it does not.
    local name=$1 window=$2 needle=$3 want=$4
    local deadline=$((SECONDS + 5)) state='' has=0
    while :; do
        state=$(xon xprop -id "$window" _NET_WM_STATE 2>&1 || true)
        [[ "$state" == *"$needle"* ]] && has=1 || has=0
        [[ "$has" == "$want" ]] && break
        (( SECONDS < deadline )) || break
        sleep 0.1
    done
    log "OBSERVED $name: $state"
    assert_eq "$name" "$want" "$has"
}
