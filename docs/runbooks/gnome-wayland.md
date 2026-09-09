# GNOME Wayland Runbook

The GNOME integration targets GNOME Shell 50 and uses the in-repository Shell extension as the compositor companion. The Rust process is a client of the user-session D-Bus service; it does not fall back to X11 inside a Wayland session.

## Target version

Record the exact point release before a manual run:

```bash
gnome-shell --version
# Local reference run: GNOME Shell 50.4
```

## Install and enable the companion

From the repository root:

```bash
extension_dir="$HOME/.local/share/gnome-shell/extensions/window-zones@mihai-a24"
mkdir -p "$extension_dir"
install -m 0644 gnome-extension/metadata.json "$extension_dir/metadata.json"
install -m 0644 gnome-extension/extension.js "$extension_dir/extension.js"
gnome-extensions enable window-zones@mihai-a24
gnome-extensions info window-zones@mihai-a24
```

On Wayland, log out and back in if GNOME Shell does not load a newly installed extension immediately. Disable it with:

```bash
gnome-extensions disable window-zones@mihai-a24
```

The companion owns:

- service `org.window_zones.Gnome`;
- object `/org/window_zones/Gnome`;
- interface `org.window_zones.Gnome1`.

## Start the application

Install or build the binary, then inspect the resolved backend:

```bash
./scripts/install.sh --prefix "$HOME/.local"
$HOME/.local/bin/window_zones --backend auto status
```

Expected status identifies `gnome-wayland` and reports the companion capabilities. If the extension is disabled or absent, status remains usable and reports the GNOME companion as unavailable. Actions and hotkey registration fail explicitly until the companion returns; X11 is not selected as a fallback.

Use a config containing at least two bindings, for example:

```toml
[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }

[[bindings]]
hotkey = "Ctrl+Alt+Right"
action = { type = "move-to-zone", zone = "right-half" }

[[bindings]]
hotkey = "Ctrl+Alt+Down"
action = { type = "move-to-display", direction = "next" }
```

Run the interactive session with the explicit config path when testing an isolated file:

```bash
$HOME/.local/bin/window_zones --backend auto --config ./path/to/config.toml run
```

## Manual smoke checklist

Record the observed result for each item:

1. `status` selects GNOME Wayland and lists `focused-window`, `displays`, `move-resize`, and `hotkeys`.
2. With a normal focused window, move it to left half, right half, a third, two-thirds, maximize, and the next/previous display. Verify frame coordinates, sizes, and usable-area boundaries on two displays, including a display with negative global coordinates.
3. Focus a desktop/dock surface or remove focus and verify the action fails without moving another window.
4. Maximize, tile, or fullscreen a window and verify move/resize is rejected without changing the window state.
5. Trigger each registered hotkey and verify the configured action runs. Change the config while the session is running and verify the new complete hotkey set replaces the old set atomically.
6. Introduce malformed config, reload, and verify the last valid bindings remain active. Restore valid config and verify recovery.
7. Disable the extension. Verify status, reload, and quit remain usable; verify actions and registration report the unavailable companion and do not use X11.
8. Re-enable the extension. Verify the runtime reconnects, re-registers the current bindings, and emits one recovery diagnostic rather than repeating identical errors every retry.
9. Restart the application and repeat one zone move and one display move.

For D-Bus-level contract coverage, run:

```bash
cargo test --locked gnome_integration
```

The private fake-service tests cover the stable names and signatures, capability gating, negative coordinates, focused-window absence, exact move payloads, hotkey signals, atomic registration rejection, Busy, incompatible/denied responses, and disconnect/reconnect.
