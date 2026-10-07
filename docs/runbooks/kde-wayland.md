# KDE Plasma Wayland Runbook

Window Zones uses a KWin-owned integration on KDE Plasma Wayland. The integration
has two parts:

- `kwin-script/` — a KWin JavaScript script that reads KWin window/output state,
  moves an identified window, and owns global shortcut registration;
- `window_zones_kwin` — a Rust companion process that exposes the narrow
  `org.window_zones.KWin2` D-Bus contract on the user session bus.

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
an update so KWin reloads `main.js` and removes old shortcut registrations.

Protocol 2 is a clean cutover: upgrade the script package with
`kpackagetool6 --type=KWin/Script --upgrade ./kwin-script`, reload the script,
and restart `window_zones_kwin` after installing the matching binary. An older
script or companion is incompatible; there is no protocol-1 fallback.

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
  org.window_zones.KWin2.GetCapabilities
busctl --user introspect org.window_zones.KWin /org/window_zones/KWin
```

Expected service identity:

- name: `org.window_zones.KWin`;
- object: `/org/window_zones/KWin`;
- interface: `org.window_zones.KWin2` (protocol major 2).

If the service is absent, start `window_zones_kwin` and retry. If the script is
absent or disabled, reload the KWin script and check the KWin journal/log for
JavaScript errors. If D-Bus reports access denied, verify that both processes
run as the same graphical user on the same session bus and that no sandbox
policy blocks user-session D-Bus calls.

A KDE signal with `DISPLAY` set still selects the native KDE Wayland backend.
It does not permit an implicit X11 fallback. Missing, incompatible, or denied
KWin integration is reported as an explicit runtime error with remediation.

### Protocol 2 window requests

`GetFocusedWindow` returns `(present, window_id, x, y, width, height)`. The
identity is KWin's `String(window.internalId)`, stable for that Window's lifetime,
not a Display identity. Coordinates and dimensions describe the global Window
frame; `GetDisplays` continues to report each output's usable `MaximizeArea`.
`MoveWindow(window_id, x, y, width, height)` queues a move to that identity even
if focus changes before the script receives it.

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
Geometry updates connect once per Window and disconnect when it is removed.

`CompleteRequest(request_id_string, ok, error_name, message)` preserves the
failure class. `GetRequestResult(request_id_string)` returns
`(complete, error_name, message)`, with empty error fields on success.
`org.window_zones.KWin.Error.WindowGone` carries the missing Window identity
and maps to the App's `WindowGone`; shortcut conflicts and non-canonical keys
use `Unsupported` and map to `Rejected` without replacing the previous set.
Unavailable preflight or transport failures are retryable.

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
5. Verify each configured supported hotkey registers and dispatches its action.
6. Edit the config, run `reload`, and verify the new binding set is active.
7. Run `restart`, then verify hotkeys register again and dispatch still works.
8. Stop or disable the companion and confirm status/TUI show an in-band
   unavailable diagnostic rather than using X11.
9. Re-enable/reload the KWin script and restart the companion; verify recovery.

KWin's documented JavaScript API exposes shortcut registration but no reliable
runtime unregister operation. Window Zones replaces the configured active set
and sends an empty set when no bindings remain; removed shortcut callbacks
become inactive and an empty set releases the App's Active controller claim.
Dropping the App's hotkey connection sends an empty set; an unexpected controller
disconnect also queues an empty set so its callbacks become inactive.
Before touching any QAction the script asks KGlobalAccel `action(int)` who
would win each requested accelerator; if any is held by another action the
whole set is rejected with `KWin rejected shortcut '<hotkey>': accelerator is
already held by '<action>'` and the previous set stays active. Disable/re-enable
the script after binding removal or upgrades to let KWin clean up stale
global-accelerator entries.

## Nested KWin gate

`./scripts/smoke.sh kde-live` runs this checklist mechanically against a real KWin, started on its own `--virtual` framebuffer backend with two outputs and its own Xwayland. It needs the `kwin` package (`kwin_wayland`, `kpackagetool6`, `kwriteconfig6`) and touches nothing in the current session: private session bus, private `HOME` and XDG directories, its own script package and `kwinrc`.

It is not a substitute for this checklist on a seated Plasma session. KWin's nested and virtual backends use a `Session::Type::Noop` session and never own the login seat; the gate's accelerator capture goes through Xwayland XTEST into KWin's EIS input path, which evidences compositor dispatch rather than physical-seat input. Three limitations are recorded as named skips rather than silently passed: seated accelerator capture, Plasma panel exclusion (the fixture runs no `plasmashell`), and negative-coordinate output movement (KWin places the virtual outputs side by side at non-negative origins).

Its first run found two defects, both fixed:

- `workspace.clientArea(KWin.WorkArea, output, desktop)` ignores the output and returns the desktop-wide union, so every display reported the same rectangle and zones were computed against all monitors joined. The per-output area is `KWin.MaximizeArea`.
- KWin's scripting `registerShortcut` returns success for any callable handler: it hands the sequence to KGlobalAccel and discards the result, so an accelerator another action already owns is accepted and then never fires. The script now preflights every accelerator through KGlobalAccel `action(int)` (the dispatch winner, unlike the unordered `getGlobalShortcutsByKey`) before any registration and the companion re-verifies the winner read-only.

