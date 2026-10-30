# GNOME Wayland Runbook

The GNOME integration targets GNOME Shell 50 and 51 and uses the in-repository Shell extension as the compositor companion. The Rust process is a client of the user-session D-Bus service; it does not fall back to X11 inside a Wayland session.

## Target version

Record the exact point release before a manual run:

```bash
gnome-shell --version
```

Record observed pass/fail results, exact Shell versions and synthetic/seated limitations in the PR description. A previous revision's pass does not verify an upgraded binary or companion.

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
  --method org.window_zones.Gnome2.GetCapabilities
```

The service should report `focused-window`, `displays`, `move-resize`,
`hotkeys`, and optional `transition-metrics`. Enabling/disabling an already-discovered compatible extension takes
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

The companion exports its object only while it owns the service name. If an
incumbent owns the name at enable time, queued acquisition exports the service
when that incumbent leaves. A compatible but degraded companion lacking hotkey
capture remains retryable; replacing it with the complete companion recovers
without editing bindings. Actual shortcut conflicts remain transactional rejections.

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

If a failed request changes that Window's frame, the companion returns
`org.window_zones.Gnome.Error.PartialMove` with JSON fields `window_id`
(decimal string), `observed` (`[x,y,width,height]`), and `message`. The App
validates the identity and positive frame dimensions before retaining the
original Restore point, including after a partially failed Restore. Readback
uses the captured Window, never the new focused Window. Unchanged or unreadable
frames remain ordinary failures; a closed Window still returns `WindowGone`.

Optional `transition-metrics` enables `GetLastTransitionMetrics() -> (dd)`,
with request/completion milliseconds from the same D-Bus caller's last
successful move. Metrics are cleared on caller disconnect; unsupported
timings are absent, not zero.

Moves restore any maximization direction or edge tiling before applying the
target frame. On Shell 50, `get_maximize_flags()` reports edge tiling as vertical
maximization, and `unmaximize()` takes **no flags argument** and clears both
directions and tiling. Fullscreen and non-resizable windows remain explicit
`InvalidWindowState` rejections; leave fullscreen first. Overview, lock-screen,
desktop/dock, and other Shell surfaces are excluded.

Also inspect the captured-window contract without registering hotkeys:

```bash
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
Explicit status/capability probes query the current companion, so TUI refresh
can observe disappearance and recovery without first attempting a move.

Zone and display actions automatically restore a maximized or GNOME-tiled
window before applying the requested frame. A maximized window moved to
another display fills that display's usable area; a tiled half retains its
half on the destination. Fullscreen and genuinely non-resizable windows
remain rejected without changing their state.

Repeated left/right-half placement crosses to the nearest physically
left/right display and lands on the half nearest the source. With no
neighbour it stays at the outer edge; explicit next/previous-display actions
still wrap. Normal, maximized, and tiled placements use a single 250 ms
compositor transition, respecting disabled animations. GNOME 51's reduced-motion
preference is optional; its absent API does not disable GNOME 50 placement.
Use [`--perf-log PATH`](running.md#performance-logging) to record per-move
request/completion times and Linux App-process memory.

Save bindings to `$XDG_CONFIG_HOME/window_zones/config.toml`, or
`~/.config/window_zones/config.toml` when `XDG_CONFIG_HOME` is unset. A missing
file means zero bindings; starting `run` or `tui` does not create defaults.

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

The companion must be `ACTIVE`, `disable-user-extensions` must be `false`,
and the `Gnome2` service must be reachable before dependent checks can pass.
The master switch suppresses user extensions even when `EnableExtension`
returns success. Do not replace the user's entire enabled-extension list to
isolate the test. If another tiling extension interferes, disable only that
extension with the user's approval and restore it afterwards.

The disconnect/recovery check re-enables the companion, which can append its
UUID to `enabled-extensions`. The harness restores the exact original order
when membership is unchanged; it fails without overwriting unrelated user
changes when membership differs.

Automated `gnome-live` requires writable `/dev/uinput` and fails its preflight
without it; it has no XTEST or physical-key fallback. Physical key presses can
provide separate manual evidence, but a direct dispatch is not
accelerator-delivery evidence.
The automated gate presses each placement accelerator once, then waits for
observed geometry. It only adopts the fixture's PID; a matching title in
another application is not ownership.

If an administrator explicitly approves temporary injector access, grant it
only to the current user, save the original ACL, and restore it even if the
run fails. Never make `/dev/uinput` world-writable:

```bash
bash -c '
  set -e
  sudo modprobe uinput
  acl_backup=$(mktemp)
  restore_acl() {
    sudo setfacl -P --restore="$acl_backup" && rm -f "$acl_backup"
  }
  getfacl -p /dev/uinput > "$acl_backup"
  trap restore_acl EXIT
  sudo setfacl -m "u:$(id -un):rw" /dev/uinput
  ./scripts/smoke.sh gnome-live
'
```

This authorized path needs `getfacl`, `setfacl`, and authorized `sudo`; do not
attempt it unattended when those prerequisites are unavailable. If restoration
fails, keep the backup and restore the ACL before leaving injector access open.
Run the block from the repository root. The explicit `bash -c` wrapper also
works from fish. Physical ACL restoration (`-P`) does not follow symbolic links.

Run the seated GNOME checks with `./scripts/smoke.sh gnome-live` from the repository root, then record the observed result for each item in the PR description:

1. `status` selects GNOME Wayland and lists `focused-window`, `displays`, `move-resize`, and `hotkeys`; the combined companion also advertises optional `transition-metrics`.
2. With a normal focused window, move it to every built-in and a custom zone, the next/previous display, and a display in each available explicit direction. Check center (size retained unless it must shrink) and restore in the same runtime session. Verify frame coordinates, sizes, and usable-area boundaries on two displays, including a display with negative global coordinates.
3. Focus a desktop/dock surface or remove focus and verify the action fails without moving another window.
4. Maximize in either direction or edge-tile a resizable window, then apply a built-in/custom zone or move to the next/previous display. Verify restoration before the exact target frame and preservation of full/half destination layout. Fullscreen and non-resizable windows must reject placement without changing their state. Capture one window's id, change focus, and verify `MoveWindow` moves only the captured window; close it and verify `WindowGone`.
5. Trigger each registered hotkey and verify the configured action runs. Change the config while the session is running and verify the new complete hotkey set replaces the old set atomically.
6. Introduce malformed config, reload, and verify the last valid bindings remain active. Restore valid config and verify recovery.
7. Disable the extension. Verify status, reload, and quit remain usable; verify actions and registration report the unavailable companion and do not use X11.
8. Re-enable the extension. Verify the runtime reconnects, re-registers the current bindings, and emits one recovery diagnostic rather than repeating identical errors every retry.
9. Restart the application and repeat one zone move and one display move.
10. Repeat Ctrl+Super+Left/Right at a matching half. Verify crossing to the neighbouring display's nearest half, different-size destinations, and no directional wrap at the outer edge.
11. Start `tui` with `--perf-log PATH`; press Right twice rapidly and verify both moves complete and have distinct successful records. Repeat from maximization, with animations disabled, and with reduced motion. Restore desktop preferences afterwards; check pointer interaction after completion. Memory fields describe only the App process, not Shell/GPU use.
12. On a disposable client with controllable minimum size, force a zone placement to apply only partially and confirm Restore still reaches its pre-placement frame. Also snap a small window to a larger zone, set a minimum width below that placement but above its original width, and Restore: after removing the constraint, retrying Restore must reach the original frame rather than the partially restored one.

The `gnome-live` gate mechanically runs the [identified-window and hotkey-rollback checks](testing.md#v1-verification-matrix) on disposable Text Editor windows; its refused replacement must retain four bindings and the previous accelerator.

Its placement runtime also reloads one of those safe bindings through all
fourteen built-in zones and center, injecting each accelerator with the test
window's identity and focus checked before input. Each key needs a new
`--perf-log` dispatch receipt after the input checkpoint, matching the runtime
PID, canonical hotkey, resolved action and successful result; cached status is
not delivery proof. Independent Xwayland/companion frame observations must also
match. Xwayland pixels are normalized using independent monitor/X server layout
metadata; target rectangles remain in Mutter logical units. Captures come from
the actual compositor monitor, not the black Xwayland root. Post-input focus and
Overview state are recorded. Close Overview and Shell menus (including Quick
Settings) before the gate and leave keyboard/mouse idle until it finishes:
Shell modal grabs can intercept input without changing those focus observations.
Interactive Restore then returns to
the frame before the first accelerator move; a second physical Restore must
report consumed history without changing the frame. The original four-binding
fixture is restored before the remaining lifecycle/rollback checks.

The atomic-registration fixture receives `HotkeyPressed` on the same private
D-Bus connection that registers the binding; the companion intentionally
unicasts signals only to its controller. The gate requires separate receiver
events before and after the invalid replacement, at least two events total,
and retains them in `atomic-helper.log`. An independent bus monitor cannot
prove delivery to this controller.

The blocking GNOME gate requires both [Synthetic and seated checks](testing.md#v1-verification-matrix)
on the exact upgraded GNOME 50 or 51 configuration being claimed.
Record GNOME/Mutter versions, display layout, assertion summary, limitations,
and retained evidence paths in the PR description. Preserve proof with
`--keep-logs`; verifier cleanup retains evidence. Do not check artifacts in.
Contract tests and Synthetic results do not replace seated evidence.

For isolated D-Bus client and real companion-dispatcher contract coverage, run:

```bash
cargo test --lib gnome_integration
gjs -m gnome-extension/tests/run.js
```

The private fake-service tests cover names/signatures, capability gating and caching, connection reuse, negative coordinates, focused-window absence, identified move payloads, partial-placement and partial-Restore history, malformed/foreign-window partial errors, `WindowGone`, hotkey transport draining, atomic rejection, Busy, incompatible/denied responses, and disconnect/reconnect. The GJS harness invokes the actual `_methodCall` dispatcher with real GLib variants and isolated Shell stubs; it checks reply signatures, focus-change identity, restore-before-move, failed/unchanged/unreadable frame reporting, rejected states, canonical hotkeys, rollback, and owner-vanish cleanup without touching the live desktop.

For real interactive Wayland and XWayland windows on an isolated GNOME 51
compositor, run:

```bash
./scripts/smoke.sh --keep-logs gnome
```

This exercises native keyboard and mouse input, all fourteen built-ins through
CLI dispatch and actual accelerators, same-runtime center/Restore and restart
reset, monitor moves, config rollback, CLI/TUI/tray lifecycle, companion recovery,
and compositor screenshots through Mutter's RemoteDesktop and ScreenCast APIs.
Capture explicitly links only the private stream and owned consumer; it does
not depend on automatic session-manager linking. It does not need `/dev/uinput`,
change the seated extension settings, or bypass Shell security.
GNOME 50 skips only its unavailable reduced-motion preference, not the
enabled/disabled animation cases. See [testing prerequisites](testing.md#v1-verification-matrix).

## Compatibility and lifecycle sources

The companion uses Shell/Mutter focus, frame, work-area, move/resize, and
accelerator APIs. For cross-monitor moves, the companion
waits for the restore `size-changed` notification, then calls `move_to_monitor`
before `move_resize_frame`. This avoids a late restore acknowledgement
overwriting the target and supplies the destination's Wayland size bounds.
Completion waits for settled geometry and animation, and rechecks the final
frame; request submission alone is not success.

Shell-owned windows are excluded using the cached process ID from
`Gio.Credentials.get_unix_pid()`. GNOME 51 has no `global.get_pid()` method;
the contract harness leaves that nonexistent API unstubbed and uses real GIO
credentials for the process-exclusion regression.

`Window.unmaximize()` takes no arguments in the Mutter 50/51 APIs and clears
maximization and native side tiling. Resize eligibility uses the intrinsic `resizeable`
property: `allows_resize()` also returns false solely because a window is
maximized, so using it as a client restriction would incorrectly reject the
requested restoration. Target validation, fullscreen, and intrinsic client
restrictions are checked before changing state. The App calculates display
placement from the original frame, not the window's saved restore rectangle.

Primary references:

- [GNOME 51 extension manager](https://gitlab.gnome.org/GNOME/gnome-shell/-/blob/51.0/js/ui/extensionSystem.js) and [GNOME 50 extension manager](https://gitlab.gnome.org/GNOME/gnome-shell/-/blob/50.4/js/ui/extensionSystem.js): live settings-driven enable/disable, metadata validation, and imported-module lifecycle.
- [GNOME 51 Shell D-Bus implementation](https://gitlab.gnome.org/GNOME/gnome-shell/-/blob/51.0/js/ui/shellDBus.js) and [GNOME 50 implementation](https://gitlab.gnome.org/GNOME/gnome-shell/-/blob/50.4/js/ui/shellDBus.js): `ReloadExtension` is unsupported.
- [GNOME 51 layout manager](https://gitlab.gnome.org/GNOME/gnome-shell/-/blob/51.0/js/ui/layout.js): `getWorkAreaForMonitor` returns workspace work areas rather than full monitor bounds.
- [Mutter 51 window implementation](https://gitlab.gnome.org/GNOME/mutter/-/blob/51.0/src/core/window.c) and [Mutter 50 public window API](https://gitlab.gnome.org/GNOME/mutter/-/blob/50.4/src/meta/window.h): frame placement, maximize flags, fullscreen and resize restrictions. Built-in side tiling sets the vertical maximize flag.
- [GNOME 51 window animation lifecycle](https://gitlab.gnome.org/GNOME/gnome-shell/-/blob/51.0/js/ui/windowManager.js): compositor snapshots, actor freeze/thaw, transforms, effect cancellation, and the 250 ms size-transition duration.
- [GIO name ownership lifecycle](https://docs.gtk.org/gio/func.bus_own_name.html): export before name acquisition, teardown on name loss, and asynchronous ownership callbacks.
- [Rectangle window execution](https://github.com/rxhanson/Rectangle/blob/main/Rectangle/WindowManager.swift) and [next/previous display calculation](https://github.com/rxhanson/Rectangle/blob/main/Rectangle/WindowCalculation/NextPrevDisplayCalculation.swift): frame-based placement, replayed layouts, and maximized geometry on destination displays.
