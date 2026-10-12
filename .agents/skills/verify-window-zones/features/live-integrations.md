# Live integrations

On a real desktop the runtime moves windows and captures keys through the GNOME companion, X11 exclusive grabs, KDE companion, or native Windows/macOS hook; Sway and Hyprland use compositor-bound dispatch. Linux/Windows can add a tray menu. Installed Linux systemd services and macOS LaunchAgents run at login and notify about background problems. These paths are outside `wzv`'s dry-run isolation; evidence is scoped to the exact backend, fixture, and entry point exercised.

## Sub-features

- `gnome-wayland` companion load, capability negotiation, displays, usable area, placement, real accelerator capture, companion recovery, TUI lifecycle (blocking gate).
- `x11` frame-geometry zones, next/previous display with wrap, real accelerator capture, invalid-config retention, TUI lifecycle (blocking gate).
- `kde-wayland` the same checks against a nested KWin 6.x (deferred gate).
- `windows` the manual Windows smoke run (deferred gate).
- `macos` retained native Accessibility window identity, usable-area placement, and grabbing-hook restart recovery (ungated).
- `sway-hyprland` compositor keybindings calling `window_zones dispatch` (ungated).
- `tray` `run --tray` status menu (Linux and Windows).
- `notifications` desktop notifications from the service when stdout is not a terminal.
- `single-instance` a second live `run`/`tui` exits with status `75`.
- `service` `./scripts/install.sh --autostart`: Linux `window-zones` systemd user service or macOS `org.window_zones.run` LaunchAgent.

## How to get to it (user POV)

- Log in to the desktop and press a bound hotkey while `window_zones run`, `window_zones tui`, or the service runs.
- Run `window_zones --backend auto run` (or `x11`, `wayland`, `windows`, `macos`) in a desktop session.
- Run `window_zones --tray run` on Linux or Windows.
- Bind a Sway or Hyprland key to `window_zones dispatch <hotkey>`.

## Driving it with wzv

Preconditions:

- Not drivable with `wzv`: it only runs `--backend dry-run`. These recipes need the platform named in each bullet, and `docs/runbooks/testing.md` is the record of gate status.
- Own the runtime, config, HOME/TMPDIR, and disposable windows. Record a blocked command and its unmet precondition; do not stop a user's service or borrow their windows.

- **GNOME synthetic gate.** With GNOME Shell 50/51, native RemoteDesktop input and ScreenCast/PipeWire capture, run `./scripts/smoke.sh --keep-logs gnome`. Final combined GNOME 51 run: 256 assertions passed, zero failures/skips, including native/XWayland windows, CLI/TUI/tray, restoration, animations and profiling. Evidence: `target/verify-window-zones/merged-final-gnome-20261012/`. This does not close the seated gate.
- **GNOME seated gate.** In a GNOME 50/51 login session with the upgraded companion active and authorized writable `/dev/uinput`, run `./scripts/smoke.sh --keep-logs gnome-live`. Expect zero failures, including ADR 0009 identity/state/refused-reload checks. Not exercised for the combined tree; follow the saved/restored per-user ACL procedure in the GNOME runbook.
- **X11 gate.** With Xvfb, Openbox and `xdotool`, run `./scripts/smoke.sh --keep-logs x11`. Final combined run: 87 assertions passed, zero failures/skips, including exclusive grabs, ADR 0009, rollback and TUI lifecycle. Evidence: `target/verify-window-zones/merged-final-x11-20261012/`.
- **KDE synthetic gate.** With KWin 6.x and working XTEST-to-EIS transport, run `./scripts/smoke.sh --keep-logs kde-live`. Window/state and explicit dispatch paths were exercised; input-dependent checks failed on this container. Follow the current [KDE evidence and prerequisites](../../../../docs/runbooks/kde-wayland.md#nested-kwin-gate). A passing nested run still does not close the seated KDE gate.
- **Windows gate.** Follow `docs/runbooks/windows-smoke.md` on Windows. Unverified here.
- **macOS backend.** Grant Accessibility/input capture to the responsible process. With an owned HOME/TMPDIR and config, run `target/debug/window_zones --backend macos --config "$CONFIG" run`; keep only a disposable window frontmost, trigger its configured key, and check the independent frame plus runtime `status`. Native public API and this raw CLI path were exercised with CGEvent-injected F8; physical-key capture and the `scripts/run.sh` entry point were not separately exercised.
- **macOS expansion regression.** In a dedicated fixture process, put a normal resizable window low on the primary display, keep it frontmost, and run `WZ_MACOS_TEST_PID="$FIXTURE_PID" cargo test --locked --lib expansion_finishes_at_target_origin -- --ignored --test-threads=1`. It refuses a different window owner and asserts exact full-height half placement. Exercised; independent AppKit readback agreed. For native zoom/menu-tiling checks, wait for the external animation to settle, then move and sample the resulting frame after the move.
- **macOS service.** On an otherwise unused GUI label, run `/bin/bash scripts/install.sh --debug --prefix "$PREFIX" --autostart` with an isolated installer HOME and a bootstrap wrapper that injects that HOME/TMPDIR/XDG environment into the owned plist before real `launchctl bootstrap`. Require a running PID, the chosen executable and config state; reinstall, replace prefixes, uninstall the stale prefix, then uninstall the replacement. Exercised: stale uninstall preserved the replacement PID/program; final uninstall removed the agent and kept config/logs. See the installation runbook for normal user commands.
- **macOS notifications.** Run the owned live runtime with stdout redirected to a log. Reload invalid config, inspect `Window Zones needs attention` in Notification Center, restore valid config and inspect matching `Window Zones` recovery. Capture only the owned notice. Exercised: actual banner pixels and Notification Center text, Script Editor sender, quoted text unchanged, no preference changes.
- **Single instance.** With no borrowed service, start two owned live `run` processes with the same isolated HOME/TMPDIR and explicit live backend; the second must exit `75`. The exact live CLI recipe remains unverified here.

## Gotchas

- `./scripts/smoke.sh` is Linux-only (`setsid`, GNU `stat`, compositor binaries) and deletes its evidence in `/tmp/window_zones-smoke.$$` unless `--keep-logs` is passed.
- A live `run` or `tui` takes the single-instance lock. Use an owned TMPDIR; if a user's service collides, report it rather than stopping it.
- Live dispatch operates on captured native window identity, but capture still starts from the focused window. Drive only disposable windows; verify the fixture's GUI PID before key injection.
- An isolated installer HOME does not isolate the GUI LaunchAgent's loaded label or inherited environment. Inject the owned environment before bootstrap, verify program ownership before teardown, and leave an existing user's label untouched.
- Empty sentinel logs cannot observe native Framework calls. Direct API proof does not prove a different CLI/TUI entry point.
- Dry-run results never count as evidence for any row here.
