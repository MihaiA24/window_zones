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

On GNOME Shell 50 a Wayland session activates extensions only at session start.
`gnome-extensions enable` and the `EnableExtension` D-Bus method update the
enabled list and return success, but the extension stays `State: INACTIVE`, and
`ReloadExtension` now fails with
`org.freedesktop.DBus.Error.NotSupported: ReloadExtension is deprecated and does not work`.
After installing or updating the companion, log out and back in, then confirm:

```bash
gnome-extensions info window-zones@mihai-a24   # expects State: ACTIVE
busctl --user list | grep org.window_zones.Gnome
```

Disable it with:

```bash
gnome-extensions disable window-zones@mihai-a24
```

Disabling also takes effect only after the session restarts.

The companion owns:

- service `org.window_zones.Gnome`;
- object `/org/window_zones/Gnome`;
- interface `org.window_zones.Gnome2` (strict major; update the App and companion together).

`GetFocusedWindow() -> (btiiuu)` returns presence, the `Meta.Window.get_id()`
uint64 identity, and global frame coordinates and dimensions. `GetDisplays()`
returns usable areas, excluding panels and docks. `MoveWindow(tiiuu)` takes the
captured window identity and a target frame, so changing focus between discovery
and movement never moves the new focused window. A closed window returns
`org.window_zones.Gnome.Error.WindowGone`.

Moves restore any maximization direction or edge tiling before applying the
target frame. On Shell 50, `get_maximize_flags()` reports edge tiling as vertical
maximization, and `unmaximize()` takes **no flags argument** and clears both
directions and tiling. Fullscreen and non-resizable windows remain explicit
`InvalidWindowState` rejections; leave fullscreen first. Overview, lock-screen,
desktop/dock, and other Shell surfaces are excluded. The extension declares the
`user` and `unlock-dialog` session modes, so locking the screen keeps the
companion and its registered hotkeys loaded (they only fire in normal mode)
instead of disconnecting the App on every lock.

Inspect the wire contract without registering hotkeys:

```bash
gdbus call --session --dest org.window_zones.Gnome \
  --object-path /org/window_zones/Gnome \
  --method org.window_zones.Gnome2.GetCapabilities
gdbus call --session --dest org.window_zones.Gnome \
  --object-path /org/window_zones/Gnome \
  --method org.window_zones.Gnome2.GetFocusedWindow
```

`RegisterHotkeys(as)` accepts only App-canonical strings such as `ctrl+cmd+left`,
not config spellings such as `Ctrl+Super+Left`. A rejected replacement leaves
the previous complete set active. Only its active controller may replace it;
disconnecting that controller releases all grabs.

## Start the application

Install or build the binary, then inspect the resolved backend:

```bash
./scripts/install.sh --prefix "$HOME/.local"
$HOME/.local/bin/window_zones --backend auto status
```

Expected status identifies `gnome-wayland` and reports the companion capabilities. If the extension is disabled or absent, status remains usable and reports the GNOME companion as unavailable. Actions and hotkey registration fail explicitly until the companion returns; X11 is not selected as a fallback.

Use a config containing at least two bindings. GNOME already owns
`<Control><Alt>Left` and `<Control><Alt>Right` for `switch-to-workspace-left`
and `switch-to-workspace-right`. `<Super>Left` and `<Super>Right` tile windows,
and `<Control><Alt>F1`-`<Control><Alt>F12` switch virtual terminals, so the
companion's `grab_accelerator` refuses them with
`org.window_zones.Gnome.Error.Unsupported`. List what the desktop already
reserves before choosing bindings:

```bash
for schema in org.gnome.desktop.wm.keybindings org.gnome.shell.keybindings \
  org.gnome.mutter.keybindings org.gnome.mutter.wayland.keybindings; do
  gsettings list-recursively "$schema"
done | grep -oE "'<[^']*>[^']*'" | sort -u
```

`Super` (also `Cmd`) maps to `<Super>`. Use `Ctrl+Super` plus an arrow rather than
GNOME's workspace or tiling chords, after checking your session's reserved set:

```toml
[[bindings]]
hotkey = "Ctrl+Super+Left"
action = { type = "move-to-zone", zone = "left-half" }

[[bindings]]
hotkey = "Ctrl+Super+Right"
action = { type = "move-to-zone", zone = "right-half" }

[[bindings]]
hotkey = "Ctrl+Super+Down"
action = { type = "move-to-next-display" }
```

Run the interactive session with the explicit config path when testing an isolated file:

```bash
$HOME/.local/bin/window_zones --backend auto --config ./path/to/config.toml run
```

## Manual smoke checklist

Before the seated run, prepare the session:

```bash
gsettings set org.gnome.shell disable-user-extensions false
gsettings get org.gnome.shell enabled-extensions      # save this list
gsettings set org.gnome.shell enabled-extensions "['window-zones@mihai-a24']"
sudo modprobe uinput && sudo chmod 0666 /dev/uinput   # real accelerator capture
```

The master switch has to be off: with `disable-user-extensions true` the Companion stays `INACTIVE` and `EnableExtension` still returns `true`, which looks exactly like a broken extension. Narrowing `enabled-extensions` keeps tiling extensions from moving the test window. `/dev/uinput` is mandatory: Xwayland XTEST events never reach a compositor accelerator grab on GNOME 50, so `gnome-live` fails with the `modprobe`/`chmod`/udev remediation instead of falling back to `xdotool`. Restore the saved list and `sudo chmod 0600 /dev/uinput` afterwards.

Run the seated GNOME checks with `./scripts/smoke.sh gnome-live` from the repository root, then record the observed result for each item:

1. `status` selects GNOME Wayland and lists `focused-window`, `displays`, `move-resize`, and `hotkeys`.
2. With a normal focused window, move it to left half, right half, a third, two-thirds, maximize, and the next/previous display. Verify frame coordinates, sizes, and usable-area boundaries on two displays, including a display with negative global coordinates.
3. Focus a desktop/dock surface or remove focus and verify the action fails without moving another window.
4. Maximize in either direction or edge-tile a window, then move it to a zone and verify it is restored before the exact target frame is applied. Fullscreen and non-resizable windows must still be rejected without changing their state. Capture one window's id, change focus, and verify `MoveWindow` moves only the captured window; close it and verify `WindowGone`.
5. Trigger each registered hotkey and verify the configured action runs. Change the config while the session is running and verify the new complete hotkey set replaces the old set atomically.
6. Introduce malformed config, reload, and verify the last valid bindings remain active. Restore valid config and verify recovery.
7. Disable the extension. Verify status, reload, and quit remain usable; verify actions and registration report the unavailable companion and do not use X11.
8. Re-enable the extension. Verify the runtime reconnects, re-registers the current bindings, and emits one recovery diagnostic rather than repeating identical errors every retry.
9. Restart the application and repeat one zone move and one display move.

For isolated D-Bus client and real companion-dispatcher contract coverage, run:

```bash
cargo test --lib gnome_integration
gjs -m gnome-extension/tests/run.js
```

The private fake-service tests cover names/signatures, capability gating and caching, connection reuse, negative coordinates, focused-window absence, named move payloads, `WindowGone`, hotkey transport draining, atomic rejection, Busy, incompatible/denied responses, and disconnect/reconnect. The GJS harness invokes the actual `_methodCall` dispatcher with real GLib variants and isolated Shell stubs; it checks reply signatures, focus-change identity, restore-before-move, rejected states, canonical hotkeys, rollback, and owner-vanish cleanup without touching the live desktop.
