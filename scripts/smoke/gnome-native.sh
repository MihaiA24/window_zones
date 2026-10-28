run_gnome() {
    CURRENT_GATE='gnome'
    GNOME_ROOT="$TMP_DIR/gnome"
    local output="$TMP_DIR/gnome-native.log" rc=0 checks=0 failures=0
    /usr/bin/python3 "$SCRIPT_DIR/gnome-smoke.py" --binary "$BIN_PATH" \
        --output-dir "$GNOME_ROOT" --keep-logs >"$output" 2>&1 || rc=$?
    log "$(<"$output")"
    if [[ -f "$GNOME_ROOT/results.json" ]]; then
        read -r checks failures < <(/usr/bin/python3 -c \
            'import json, sys; rows = json.load(open(sys.argv[1])); print(len(rows), sum(not row["passed"] for row in rows))' \
            "$GNOME_ROOT/results.json")
        ASSERTIONS=$((ASSERTIONS + checks))
        FAILURES=$((FAILURES + failures))
        if [[ "$rc" -ne 0 && "$failures" -eq 0 ]]; then
            assert_rc_zero 'GNOME native runner exit' "$rc" "$output"
        fi
    else
        assert_rc_zero 'GNOME native evidence manifest' 1 "$output"
    fi

}
