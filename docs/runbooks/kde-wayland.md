# KDE Plasma Wayland Runbook

Window Zones uses a KWin-owned integration on KDE Plasma Wayland. The integration
has two parts:

- `kwin-script/` — a KWin JavaScript script that reads KWin window/output state,
  moves an identified window, and owns global shortcut registration;
- `window_zones_kwin` — a Rust companion process that exposes the narrow
  `org.window_zones.KWin3` D-Bus contract on the user session bus.

The KDE path never routes native Wayland windows through X11.

## Requirements

- KDE Plasma 6 running a Wayland session;
- a user D-Bus session bus;
- `kpackagetool6` (provided by the Plasma development/runtime packages);
- a Linux build environment with the `libdbus-1` development package;
- the application and companion running as the same user in the same graphical
  session.

The Rust source can be built on other platforms, but the KWin companion is a
Linux-only binary and cannot be verified without a Plasma session.

## Build and install

Build the application and companion from the repository:

```bash
cargo build --release --locked --bin window_zones --bin window_zones_kwin
```

Install both binaries somewhere on the user's `PATH`:

```bash
install -Dm755 target/release/window_zones "$HOME/.local/bin/window_zones"
install -Dm755 target/release/window_zones_kwin "$HOME/.local/bin/window_zones_kwin"
```

Install the KWin script package:

```bash
kpackagetool6 --type=KWin/Script --install ./kwin-script
```

Enable **Window Zones** in **System Settings -> Window Management -> KWin
Scripts**. If the script was already installed, disable and re-enable it after
an update so KWin reloads `main.js`. Binding removal during normal operation
releases its native accelerator without reloading the script.

Protocol 3 is a clean cutover: upgrade the script package with
`kpackagetool6 --type=KWin/Script --upgrade ./kwin-script`, reload the script,
and restart `window_zones_kwin` after installing the matching binary. Older
scripts and companions are incompatible; there is no protocol-2 fallback.

Start the companion before launching Window Zones:

```bash
"$HOME/.local/bin/window_zones_kwin"
```

Keep this process running for the lifetime of the KWin integration. A service
manager may supervise it, but it must use the graphical user's session bus;
do not run it as root or from a different user session.

## Runtime checks

With the script enabled and the companion running:

```bash
"$HOME/.local/bin/window_zones" --backend auto status
"$HOME/.local/bin/window_zones" --backend auto run
```

`status` must report `kde-wayland` and the KWin companion capabilities. The
interactive loop accepts the existing `reload`, `restart`, `status`, and
`dispatch HOTKEY` commands. The `tui` command is also available:

```bash
"$HOME/.local/bin/window_zones" --backend auto tui
```

## D-Bus diagnostics

Inspect the service and its advertised capabilities from the same user session:

```bash
qdbus6 org.window_zones.KWin /org/window_zones/KWin \
  org.window_zones.KWin3.GetCapabilities
busctl --user introspect org.window_zones.KWin /org/window_zones/KWin
```

Expected service identity:

- name: `org.window_zones.KWin`;
- object: `/org/window_zones/KWin`;
- interface: `org.window_zones.KWin3` (protocol major 3).

If the service is absent, start `window_zones_kwin` and retry. If the script is
absent or disabled, reload the KWin script and check the KWin journal/log for
JavaScript errors. If D-Bus reports access denied, verify that both processes
run as the same graphical user on the same session bus and that no sandbox
policy blocks user-session D-Bus calls.

A KDE signal with `DISPLAY` set still selects the native KDE Wayland backend.
It does not permit an implicit X11 fallback. Missing, incompatible, or denied
KWin integration is reported as an explicit runtime error with remediation.

### Protocol 3 window requests

`GetFocusedWindow` returns `(present, window_id, x, y, width, height)`. The
identity is KWin's `String(window.internalId)`, stable for that Window's lifetime,
not a Display identity. Coordinates and dimensions describe the global Window
frame; `GetDisplays` continues to report each output's usable `MaximizeArea`.
`MoveWindow(window_id, x, y, width, height)` queues a move to that identity even
if focus changes before the script receives it.

Before reading `GetDisplays`, the App queues `RefreshDisplays` and waits for the
script to publish fresh `MaximizeArea` rectangles. Panel/reservation changes are
therefore reflected on the next action, even without a screen-list change.
`currentDesktopChanged` also publishes the new desktop's usable areas.
KWin's [WorkspaceWrapper signals](https://github.com/KDE/kwin/blob/Plasma/6.5/src/scripting/workspace_wrapper.h)
expose desktop and screen changes but no work-area/strut notification; the
request-time refresh avoids guessing a nonexistent signal.

The script resolves the Window from `workspace.windowList()` or `stackingOrder`.
It rejects fullscreen with `leave fullscreen first`, restores maximized and
quick/custom tiled state, then applies the requested frame. KWin 6.5 exposes
[`Tile.unmanage(window)`](https://github.com/KDE/kwin/blob/Plasma/6.5/src/tiles/tile.h);
earlier KWin 6 exposes writable `window.tile` but clearing it alone leaves the
quick-tile mode set. On those releases the script detaches the tile and uses a
maximize/restore transition to clear that mode before placement, as implemented
in [KWin 6.0's maximize handling](https://github.com/KDE/kwin/blob/v6.0.5/src/xdgshellwindow.cpp).
[`Window.setMaximize(false, false)`](https://github.com/KDE/kwin/blob/Plasma/6.5/src/window.h)
is script-invokable; `setQuickTileMode` is not.

The script-facing `NextRequest` returns
`(request_id, kind, window_id, x, y, width, height, hotkeys_json)`.
An idle call has a deferred reply for up to one second, not an immediate empty
poll loop. Queued work wakes it on the companion's 25 ms service tick, without
blocking other D-Bus requests. A three-second script watchdog retries a lost
poll; the native [KWin 6 script engine exposes `QTimer`](https://github.com/KDE/kwin/blob/Plasma/6.5/src/scripting/scripting.cpp).
Display refresh also has a bounded completion: an omitted `UpdateDisplays`
reply fails the request as unavailable instead of retaining the processing
slot forever. Late display replies and obsolete poll callbacks cannot complete
work in a recovered connection.
Geometry updates connect once per Window and disconnect when it is removed.

`CompleteRequest(request_id_string, ok, error_name, message)` preserves the
failure class. `GetRequestResult(request_id_string)` returns
`(complete, error_name, message)`, with empty error fields on success.
`org.window_zones.KWin.Error.WindowGone` carries the missing Window identity
and maps to the App's `WindowGone`; shortcut conflicts and non-canonical keys
use `Unsupported` and map to `Rejected` without replacing the previous set.
Unavailable preflight or transport failures are retryable.

`RegisterHotkeys` claims the active controller; a completed empty registration
releases it. `GetNextHotkey` never claims ownership: an unowned poll returns no
event, and a different active controller is rejected without consuming events.

## Manual smoke checklist

1. Start a normal, movable, resizable application window.
2. Confirm `--backend auto status` selects `kde-wayland`.
3. Run `dispatch` for a configured zone and verify the focused window moves and
   resizes to the KWin work area.
   Repeat with a maximized and a quick/custom tiled Window; each must be
   restored before placement. Fullscreen must fail without changing the Window.
   Capture one Window's identity, switch focus, and verify `MoveWindow` still
   changes only the captured Window; close it and verify `WindowGone`.
4. With two outputs, including an output with negative global coordinates,
   verify next/previous-display movement uses the correct output work areas.
   Resize/move a Plasma panel, change its reservation/auto-hide setting, and
   switch virtual desktop; the next action must use the current usable area.
5. Verify each configured supported hotkey registers and dispatches its action.
6. Edit the config, run `reload`, and verify the new binding set is active.
   Remove a binding and verify another shortcut can acquire its old key, then
   release that shortcut and re-add the binding without reloading the script.
7. Run `restart`, then verify hotkeys register again and dispatch still works.
8. Stop or disable the companion and confirm status/TUI show an in-band
   unavailable diagnostic rather than using X11.
9. Re-enable/reload the KWin script and restart the companion; verify recovery.

After every accelerator passes preflight, the script creates any missing
QActions and uses the companion's script-only `SetShortcut(hotkey, qt_key)`
method to assign the desired keys and clear removed ones (`qt_key = 0`).
The companion calls KGlobalAccel's typed
[`setForeignShortcutKeys`](https://github.com/KDE/kglobalacceld/blob/master/src/kglobalacceld.cpp)
with an empty key list for removal: `GlobalShortcut::setKeys` ungrabs the old keys,
and `yourShortcutsChanged` updates KWin's QAction cache. Re-adding assigns keys
to the existing QAction; empty sets and controller disconnects use the same
native release path. The JS callbacks/QActions remain script-owned until script
unload, but cleared actions hold no native accelerators.

Before touching any QAction the script asks KGlobalAccel `action(int)` who
would win each requested accelerator; if another action owns any key, the
whole set is rejected and the previous set stays active. The companion checks
the whole accepted set in each of six settlement rounds, with five 120 ms gaps
(600 ms total explicit sleep regardless of binding count). Accelerator state
is queried, not changed, during this verification.

## Nested KWin gate

`./scripts/smoke.sh kde-live` runs this checklist mechanically against a real KWin, started on its own `--virtual` framebuffer backend with two outputs and its own Xwayland. It needs the `kwin` package (`kwin_wayland`, `kpackagetool6`, `kwriteconfig6`) and touches nothing in the current session: private session bus, private `HOME` and XDG directories, its own script package and `kwinrc`.

It is not a substitute for this checklist on a seated Plasma session. KWin's nested and virtual backends use a `Session::Type::Noop` session and never own the login seat; the gate's accelerator capture goes through Xwayland XTEST into KWin's EIS input path, which checks compositor dispatch rather than physical-seat input. Three limitations are recorded as named skips rather than silently passed: seated accelerator capture, Plasma panel exclusion (the fixture runs no `plasmashell`), and negative-coordinate output movement (KWin places the virtual outputs side by side at non-negative origins).

The fixture also requires `dbus-update-activation-environment`. It seeds the
private bus with its own `WAYLAND_DISPLAY` before KWin starts: Xwayland can
activate the desktop portal before the first graphical client connects.
KWin 6.3.6 [launches Xwayland with `-enable-ei-portal` when supported](https://raw.githubusercontent.com/KDE/kwin/v6.3.6/src/xwayland/xwaylandlauncher.cpp);
that path needs a working graphical portal and authorized EIS input delivery.
Installing KWin alone does not prove the transport.

Record the environment, observed results, and limitations only in the PR
description. Verification logs and screenshots are temporary artifacts and
must not be checked in. Synthetic results do not replace native-Wayland client
placement and seated Plasma checks.

For ADR 0009 the gate also starts a second Text Editor and, through `MoveWindow` on the companion bus, moves the first window's captured id after focus changed, expects `WindowGone` once the second window is closed, and expects a quick-tiled (`slotWindowQuickTileLeft`) and a maximized window to be restored before the frame is applied and a fullscreen window to be rejected unchanged. It reads KWin's own `tile`, `maximizeMode`, `fullScreen`, and `frameGeometry` through throwaway scripts and `_NET_WM_STATE` through `xprop`. It then reloads a config adding the incumbent's `Alt+Ctrl+F24` and expects `were refused` naming the owner, the previous five bindings, and `Alt+Ctrl+Left` still moving the window. These checks require KWin 6 scripting with the `tile` property.

### KWin API constraints

- `workspace.clientArea(KWin.WorkArea, output, desktop)` ignores the output and returns the desktop-wide union, not a per-output work area. Use `KWin.MaximizeArea` for per-output placement.
- KWin's scripting `registerShortcut` returns success for any callable handler: it hands the sequence to KGlobalAccel and discards the result, so an accelerator another action already owns can be accepted without firing. The script preflights every accelerator through KGlobalAccel `action(int)` (the dispatch winner, unlike the unordered `getGlobalShortcutsByKey`) before any registration and the companion re-verifies the winner read-only.

