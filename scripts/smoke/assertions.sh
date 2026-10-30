log() {
    printf '%s\n' "$*" | tee -a "$ASSERT_LOG"
}

assert_text() {
    local name=$1 expected=$2 observed=$3
    ASSERTIONS=$((ASSERTIONS + 1))
    if [[ "$observed" == *"$expected"* ]]; then
        log "PASS $name: expected '$expected' observed '$observed'"
    else
        FAILURES=$((FAILURES + 1))
        log "FAIL $name: expected '$expected' observed '$observed'"
    fi
}

assert_eq() {
    local name=$1 expected=$2 observed=$3 screenshot=${4:-}
    ASSERTIONS=$((ASSERTIONS + 1))
    if [[ "$expected" == "$observed" ]]; then
        log "PASS $name: expected '$expected' observed '$observed'${screenshot:+ (screenshot $screenshot)}"
    else
        FAILURES=$((FAILURES + 1))
        log "FAIL $name: expected '$expected' observed '$observed'${screenshot:+ (screenshot $screenshot)}"
    fi
}

assert_rc_zero() {
    local name=$1 rc=$2 observed=$3
    ASSERTIONS=$((ASSERTIONS + 1))
    if [[ "$rc" -eq 0 ]]; then
        log "PASS $name: expected exit 0 observed exit $rc${observed:+ ($observed)}"
    else
        FAILURES=$((FAILURES + 1))
        log "FAIL $name: expected exit 0 observed exit $rc${observed:+ ($observed)}"
    fi
}

assert_rc_nonzero() {
    local name=$1 rc=$2 expected=$3 observed=$4
    ASSERTIONS=$((ASSERTIONS + 1))
    if [[ "$rc" -ne 0 ]]; then
        log "PASS $name: expected $expected observed exit $rc${observed:+ ($observed)}"
    else
        FAILURES=$((FAILURES + 1))
        log "FAIL $name: expected $expected observed exit $rc${observed:+ ($observed)}"
    fi
}

skip_assert() {
    local name=$1 reason=$2
    ASSERTIONS=$((ASSERTIONS + 1))
    SKIPPED=$((SKIPPED + 1))
    log "SKIP $name: $reason"
}

build_binaries() {
    local gate=$1
    shift
    if [[ -n "${WINDOW_ZONES_BINARY:-}" ]]; then
        assert_eq "$gate pinned binary is executable" executable \
            "$([[ -x "$BIN_PATH" ]] && printf executable || printf missing)"
        [[ -x "$BIN_PATH" ]]
        return
    fi
    local output='' rc=0
    output=$(cargo build --locked --manifest-path "$PROJECT_ROOT/Cargo.toml" \
        --bin window_zones "$@" 2>&1) || rc=$?
    printf '%s\n' "$output" >>"$ASSERT_LOG"
    assert_rc_zero "$gate cargo build" "$rc" "$BIN_PATH"
    return "$rc"
}

record_versions_common() {
    local distro='unknown'
    if [[ -r /etc/os-release ]]; then
        # shellcheck disable=SC1091
        . /etc/os-release
        distro=${PRETTY_NAME:-$NAME}
    fi
    log "VERSION kernel: $(uname -srmo)"
    log "VERSION distro: $distro"
}
