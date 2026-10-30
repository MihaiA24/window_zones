file_contains() {
    local file=$1 needle=$2
    [[ -f "$file" ]] && [[ "$(<"$file")" == *"$needle"* ]]
}

wait_for_text() {
    local file=$1 needle=$2 timeout_s=${3:-30}
    local deadline=$((SECONDS + timeout_s))
    while (( SECONDS < deadline )); do
        if file_contains "$file" "$needle"; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

count_text() {
    local file=$1 needle=$2 count=''
    count=$(grep -c -- "$needle" "$file" 2>/dev/null)
    printf '%s' "${count:-0}"
}

wait_for_new_text() {
    # A status line only proves anything if it was printed after the event under test, so wait for
    # an occurrence beyond the ones already in the log.
    local file=$1 needle=$2 before=$3 timeout_s=${4:-10}
    local deadline=$((SECONDS + timeout_s))
    while (( SECONDS < deadline )); do
        if (( $(count_text "$file" "$needle") > before )); then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

sorted_hotkey() {
    # Modifier order is the App's canonical choice, not the harness's business; compare token sets.
    printf '%s' "$1" | tr '+' '\n' | sed '/^$/d' | sort | tr '\n' '+' | sed 's/+$//'
}

wait_for_reported_action() {
    local file=$1 label=$2 timeout_s=${3:-10}
    local deadline=$((SECONDS + timeout_s)) value=''
    while (( SECONDS < deadline )); do
        value=$(sed -n "s/^ *$label//p" "$file" 2>/dev/null | grep -v '<none>' | tail -1)
        if [[ -n "$value" ]]; then
            break
        fi
        sleep 0.2
    done
    printf '%s' "$value"
}

wait_for_file() {
    local file=$1 timeout_s=${2:-30}
    local deadline=$((SECONDS + timeout_s))
    while (( SECONDS < deadline )); do
        if [[ -s "$file" ]]; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

wait_observed() {
    # observer expected [timeout]: poll an observer function until it prints expected; print the last value.
    local observer=$1 expected=$2 timeout_s=${3:-8} observed=''
    local deadline=$((SECONDS + timeout_s))
    while :; do
        observed=$("$observer")
        [[ "$observed" == "$expected" ]] && break
        (( SECONDS < deadline )) || break
        sleep 0.2
    done
    printf '%s' "$observed"
}
