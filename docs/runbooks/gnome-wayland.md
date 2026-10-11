# GNOME Wayland Runbook

The GNOME integration targets GNOME Shell 50 and 51 and uses the in-repository Shell extension as the compositor companion. Both versions are admitted by `metadata.json`; Shell version validation must remain enabled. The Rust process is a client of the user-session D-Bus service; it does not fall back to X11 inside a Wayland session. Admission and API compatibility are not a substitute for a passing Smoke run; exact verified configurations are recorded in [the testing runbook](testing.md).

## Target version

Record the exact point release before a manual run:

```bash
gnome-shell --version
# Current device: GNOME Shell 51.0; earlier reference: GNOME Shell 50.4
```

## Install and enable the companion

For an upgrade, stop the App and disable the already-installed companion with
`gnome-extensions disable window-zones@mihai-a24` before replacing its files.
From the repository root:

```bash
extension_dir="$HOME/.local/share/gnome-shell/extensions/window-zones@mihai-a24"
mkdir -p "$extension_dir"
install -m 0644 gnome-extension/metadata.json "$extension_dir/metadata.json"
install -m 0644 gnome-extension/extension.js "$extension_dir/extension.js"
```

After a new manual installation or source/metadata upgrade, log out and back in
at a convenient time. Copying files does not refresh the running Shell's
extension metadata or imported JavaScript. An extension already marked
`OUT OF DATE` can therefore retain that state even after the corrected
`metadata.json` is on disk. On Wayland, do not attempt an in-place Shell restart.
The `ReloadExtension` D-Bus method is not a workaround: GNOME 50 and 51 return
`org.freedesktop.DBus.Error.NotSupported: ReloadExtension is deprecated and does not work`.
Do not disable Shell version validation to bypass this admission gate.

After logging back in, enable the companion and confirm both Shell state and
the actual service:

```bash
gnome-extensions enable window-zones@mihai-a24
gnome-extensions info window-zones@mihai-a24   # expects State: ACTIVE
gdbus call --session --dest org.window_zones.Gnome \
  --object-path /org/window_zones/Gnome \
  --method org.window_zones.Gnome1.GetCapabilities
```

The service should report `focused-window`, `displays`, `move-resize`, and
`hotkeys`. Enabling/disabling an already-discovered compatible extension takes
effect in the running session on **both GNOME 50 and 51**; it does not require
logout. Settings changes are processed asynchronously, so allow the transition
to finish before inspecting state. `EnableExtension` returning success means
the enabled-list update succeeded, not that the extension is `ACTIVE`.

If it remains inactive, inspect:

```bash
gsettings get org.gnome.shell disable-user-extensions
gnome-extensions info window-zones@mihai-a24
gdbus call --session --dest org.gnome.Shell --object-path /org/gnome/Shell \
  --method org.gnome.Shell.Extensions.GetExtensionErrors window-zones@mihai-a24
```

With `disable-user-extensions true`, user extensions cannot activate. If you
intend to enable them, use the Extensions application's master switch; this
affects all user extensions, not just Window Zones. `OUT OF DATE` calls for
compatible files and a new login, not repeated enable calls. Report extension
errors rather than bypassing validation.

Disable the loaded companion immediately with:

```bash
gnome-extensions disable window-zones@mihai-a24
```

Disable releases accelerator grabs, disconnects the display signal,
unsubscribes the client-owner watcher, unexports the D-Bus object, and releases
the service name. Re-enable it with the same `gnome-extensions enable` command;
the App can reconnect and re-register its current hotkeys without restarting.
An empty hotkey set or the active controller's session-bus disconnect also
releases its grabs and ownership of the registered set. Other App instances
cannot replace a live controller's nonempty set.

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

Save bindings to `$XDG_CONFIG_HOME/window_zones/config.toml`, or
`~/.config/window_zones/config.toml` when `XDG_CONFIG_HOME` is unset. A missing
file means zero bindings; starting `run` or `tui` does not create defaults.

Use a config containing at least two bindings. GNOME already owns
`<Control><Alt>Left` and `<Control><Alt>Right` for `switch-to-workspace-left`
and `switch-to-workspace-right`, and `<Control><Alt>F1`-`<Control><Alt>F12`
switch virtual terminals, so the companion's `grab_accelerator` refuses them
with `org.window_zones.Gnome.Error.Unsupported`. List what the desktop already
reserves before choosing bindings:

```bash
for schema in org.gnome.desktop.wm.keybindings org.gnome.shell.keybindings \
  org.gnome.mutter.keybindings org.gnome.mutter.wayland.keybindings; do
  gsettings list-recursively "$schema"
done | grep -oE "'<[^']*>[^']*'" | sort -u
```

`Cmd` maps to `<Super>`, and `<Super><Control>` plus an arrow is unreserved on a
default GNOME session:

```toml
[[bindings]]
hotkey = "Cmd+Ctrl+Left"
action = { type = "move-to-zone", zone = "left-half" }

[[bindings]]
hotkey = "Cmd+Ctrl+Right"
action = { type = "move-to-zone", zone = "right-half" }

[[bindings]]
hotkey = "Cmd+Ctrl+Down"
action = { type = "move-to-next-display" }

[[bindings]]
hotkey = "Cmd+Ctrl+Up"
action = { type = "move-to-previous-display" }
```

With that default config saved, run either interactive interface:

```bash
$HOME/.local/bin/window_zones --backend auto run
$HOME/.local/bin/window_zones --backend auto tui
```

Use `--config ./path/to/config.toml` when testing an isolated file.

## Manual smoke checklist

Before a seated run, inspect and save the current extension settings:

```bash
gsettings get org.gnome.shell disable-user-extensions
gsettings get org.gnome.shell enabled-extensions
gsettings get org.gnome.shell disabled-extensions
```

The companion must be `ACTIVE` and its service reachable before dependent
checks can pass. Do not replace the user's entire enabled-extension list to
isolate the test. If another tiling extension interferes, disable only that
extension with the user's approval and restore it afterwards.

Prefer physical key presses when the harness asks for a hotkey. Xwayland
XTEST events do not prove compositor-level accelerator capture. The seated
harness uses its narrow virtual-keyboard injector when `/dev/uinput` is already
writable, otherwise it asks for physical input. Without either path, record
hotkey delivery as unverified; a direct dispatch is not equivalent evidence.

If an administrator explicitly approves temporary injector access, grant it
only to the current user, save the original ACL, and restore it even if the
run fails. Never make `/dev/uinput` world-writable:

```bash
(
  set -e
  acl_backup=$(mktemp)
  getfacl -p /dev/uinput > "$acl_backup"
  trap 'sudo setfacl --restore="$acl_backup" && rm -f "$acl_backup"' EXIT
  sudo setfacl -m "u:$(id -un):rw" /dev/uinput
  ./scripts/smoke.sh gnome-live
)
```

This optional path needs `getfacl`, `setfacl`, and authorized `sudo`; do not
attempt it unattended when those prerequisites are unavailable. If restoration
fails, keep the backup and restore the ACL before leaving injector access open.

Run the seated GNOME checks with `./scripts/smoke.sh gnome-live` from the repository root, then record the observed result for each item:

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

For real interactive Wayland and XWayland windows on an isolated GNOME 51
compositor, run:

```bash
./scripts/smoke.sh --keep-logs gnome
```

This exercises native keyboard and mouse input, every zone, monitor moves,
config rollback, CLI/TUI/tray lifecycle, companion recovery, and compositor
screenshots through Mutter's RemoteDesktop and ScreenCast APIs. It does not
need `/dev/uinput`, change the seated extension settings, or bypass Shell
security. See [testing prerequisites](testing.md#v1-verification-matrix).

## Compatibility and lifecycle sources

The GNOME 51 API review used the installed `Meta-51.gir` and the JavaScript
resources embedded in `libshell-51.so` (whose `Config.PACKAGE_VERSION` is
`51.0`). The companion's existing focus, frame, work-area, move/resize, and
accelerator calls remain available. For cross-monitor moves, the companion
calls `move_to_monitor` before `move_resize_frame` so Wayland clients receive
the destination's size bounds; otherwise returning to a larger monitor can
clip the requested height to the smaller monitor. Mutter's size constraints
and asynchronous client configuration still affect settled geometry.

Primary references:

- [GNOME 51 extension manager](https://gitlab.gnome.org/GNOME/gnome-shell/-/blob/51.0/js/ui/extensionSystem.js) and [GNOME 50 extension manager](https://gitlab.gnome.org/GNOME/gnome-shell/-/blob/50.4/js/ui/extensionSystem.js): live settings-driven enable/disable, metadata validation, and imported-module lifecycle.
- [GNOME 51 Shell D-Bus implementation](https://gitlab.gnome.org/GNOME/gnome-shell/-/blob/51.0/js/ui/shellDBus.js) and [GNOME 50 implementation](https://gitlab.gnome.org/GNOME/gnome-shell/-/blob/50.4/js/ui/shellDBus.js): `ReloadExtension` is unsupported.
- [GNOME 51 layout manager](https://gitlab.gnome.org/GNOME/gnome-shell/-/blob/51.0/js/ui/layout.js): `getWorkAreaForMonitor` returns workspace work areas rather than full monitor bounds.
- [Mutter 51 window implementation](https://gitlab.gnome.org/GNOME/mutter/-/blob/51.0/src/core/window.c) and [Mutter 50 public window API](https://gitlab.gnome.org/GNOME/mutter/-/blob/50.4/src/meta/window.h): frame placement, maximize flags, fullscreen and resize restrictions. Built-in side tiling sets the vertical maximize flag.
- [GIO name ownership lifecycle](https://docs.gtk.org/gio/func.bus_own_name.html): export before name acquisition, teardown on name loss, and asynchronous ownership callbacks.
