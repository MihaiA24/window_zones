build_seam_helper() {
    # Gate-time helper over the public crate API, generated and built per run and never shipped:
    # `dispatch` always acts on the focused window, so captured-id moves drive the X11 seam
    # directly; `state` sends the EWMH request a pager would to put a test window into
    # maximized or fullscreen state; `grab` holds an accelerator as another X client.
    local dir="$TMP_DIR/seam-helper" target_dir=''
    local output='' rc=0
    SEAM_HELPER=''
    # A pinned application skips its build, not Cargo's persistent helper cache.
    # Never put target/ beneath scratch: the verifier retains scratch as evidence.
    if [[ -z "$TARGET_DIR" ]]; then
        TARGET_DIR=$(cargo metadata --locked --no-deps --format-version 1 \
            --manifest-path "$PROJECT_ROOT/Cargo.toml" \
            | /usr/bin/python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])') || rc=$?
        assert_rc_zero "$1 seam helper target directory resolves" "$rc" 'cargo metadata'
        [[ "$rc" -eq 0 ]] || return "$rc"
    fi
    target_dir="$TARGET_DIR/smoke-seam-helper"
    mkdir -p "$dir/src"
    cat >"$dir/Cargo.toml" <<EOF
[package]
name = "window_zones_smoke_seam"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
window_zones = { path = "$PROJECT_ROOT" }
x11rb = "0.13"

[workspace]
EOF
    # Start from the repository lockfile so the helper builds against the audited dependency set.
    cp "$PROJECT_ROOT/Cargo.lock" "$dir/Cargo.lock"
    cp "$SCRIPT_DIR/smoke/fixtures/x11_seam.rs" "$dir/src/main.rs"
    output=$(CARGO_TARGET_DIR="$target_dir" cargo build --manifest-path "$dir/Cargo.toml" 2>&1) || rc=$?
    printf '%s\n' "$output" >>"$ASSERT_LOG"
    assert_rc_zero "$1 seam helper builds against the public API" "$rc" "cargo build $dir"
    if [[ "$rc" -eq 0 ]]; then
        SEAM_HELPER="$target_dir/debug/window_zones_smoke_seam"
    fi
    return "$rc"
}

seam() {
    # Runs on the gate's X display; output is one line naming the outcome.
    xon "$SEAM_HELPER" "$@" 2>&1 || true
}
