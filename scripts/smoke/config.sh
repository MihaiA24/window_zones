write_valid_config() {
    cat >"$CONFIG_PATH" <<'EOF'
[[bindings]]
hotkey = "Alt+Ctrl+Left"
action = { type = "move-to-zone", zone = "left-half" }

[[bindings]]
hotkey = "Alt+Ctrl+Up"
action = { type = "move-to-zone", zone = "center-third" }

[[bindings]]
hotkey = "Alt+Ctrl+Right"
action = { type = "move-to-zone", zone = "left-two-thirds" }

[[bindings]]
hotkey = "Alt+Ctrl+Down"
action = { type = "move-to-next-display" }
EOF
}

write_five_bindings_config() {
    write_valid_config
    cat >>"$CONFIG_PATH" <<'EOF'

[[bindings]]
hotkey = "Alt+Ctrl+Shift+Right"
action = { type = "move-to-previous-display" }
EOF
}

write_invalid_config() {
    cat >"$CONFIG_PATH" <<'EOF'
[[bindings]]
hotkey = "Alt+Ctrl+Left"
action = { type = "move-to-zone", zone = "left-half" }
this is not valid toml
EOF
}
