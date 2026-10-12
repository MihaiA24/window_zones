# Live integrations

On a real desktop the runtime moves windows and captures keys through the GNOME companion, X11 exclusive grabs, KDE companion, or native Windows/macOS hook; Sway and Hyprland use compositor-bound dispatch. Linux/Windows can add a tray menu. Installed Linux systemd services and macOS LaunchAgents run at login and notify about background problems. These paths are outside `wzv`'s dry-run isolation; scope verification to the exact backend, fixture, and entry point you drive.

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

- Not drivable with `wzv`: it only runs `--backend dry-run`. These recipes need the platform named in each bullet; `docs/runbooks/testing.md` contains reusable procedures and gate policy, not run results.
- Own the runtime, config, HOME/TMPDIR, and disposable windows. Report a blocked command and its unmet precondition only in the PR description; do not stop a user's service or borrow their windows.

- **GNOME synthetic gate.** With GNOME Shell 50/51, native RemoteDesktop input and ScreenCast/PipeWire capture, run `./scripts/smoke.sh --keep-logs gnome`. Require zero failures and check native/XWayland windows, CLI/TUI/tray, restoration, animations and profiling. A synthetic run does not close the seated gate.
- **GNOME seated gate.** In a GNOME 50/51 login session with the upgraded companion active and authorized writable `/dev/uinput`, run `./scripts/smoke.sh --keep-logs gnome-live`. Expect zero failures, including ADR 0009 identity/state/refused-reload checks; follow the saved/restored per-user ACL procedure in the GNOME runbook.
- **X11 gate.** With Xvfb, Openbox and `xdotool`, run `./scripts/smoke.sh --keep-logs x11`. Require zero failures, including exclusive grabs, ADR 0009, rollback and TUI lifecycle checks.
- **KDE synthetic gate.** With KWin 6.x and working XTEST-to-EIS transport, run `./scripts/smoke.sh --keep-logs kde-live`. Check window/state, explicit dispatch and input-dependent paths. Follow the [nested KWin prerequisites and procedure](../../../../docs/runbooks/kde-wayland.md#nested-kwin-gate). A passing nested run still does not close the seated KDE gate.
- **Windows gate.** Follow `docs/runbooks/windows-smoke.md` on Windows.
- **macOS backend.** Grant Accessibility/input capture to the responsible process. With an owned HOME/TMPDIR and config, run `target/debug/window_zones --backend macos --config "$CONFIG" run`; keep only a disposable window frontmost, trigger its configured key, and check the independent frame plus runtime `status`. Distinguish physical-key capture from CGEvent injection and native API checks from the raw CLI or `scripts/run.sh` entry point.
- **macOS expansion regression.** In a dedicated fixture process, put a normal resizable window low on the primary display, keep it frontmost, and run `WZ_MACOS_TEST_PID="$FIXTURE_PID" cargo test --locked --lib expansion_finishes_at_target_origin -- --ignored --test-threads=1`. It refuses a different window owner and asserts exact full-height half placement; compare independent AppKit readback. For native zoom/menu-tiling checks, wait for the external animation to settle, then move and sample the resulting frame after the move. See the reusable [macOS native smoke procedure](../../../../docs/runbooks/testing.md#macos-native-smoke-ungated).
- **macOS service.** On an otherwise unused GUI label, run `/bin/bash scripts/install.sh --debug --prefix "$PREFIX" --autostart` with an isolated installer HOME and a bootstrap wrapper that injects that HOME/TMPDIR/XDG environment into the owned plist before real `launchctl bootstrap`. Require a running PID, the chosen executable and config state; reinstall, replace prefixes, uninstall the stale prefix, then uninstall the replacement. Require the stale uninstall to preserve the replacement PID/program and the final uninstall to remove the agent while keeping config/logs. See the installation runbook for normal user commands.
- **macOS notifications.** Run the owned live runtime with stdout redirected to a log. Reload invalid config, inspect `Window Zones needs attention` in Notification Center, restore valid config and inspect matching `Window Zones` recovery. Capture only the owned notice; check banner pixels, Notification Center text and quoted text without changing notification preferences.
- **Single instance.** With no borrowed service, start two owned live `run` processes with the same isolated HOME/TMPDIR and explicit live backend; the second must exit `75`.

## Gotchas

- `./scripts/smoke.sh` is Linux-only (`setsid`, GNU `stat`, compositor binaries) and deletes its temporary evidence in `/tmp/window_zones-smoke.$$` unless `--keep-logs` is passed. Report observed results and limitations only in the PR description; afterward remove any logs, captures and fixture directories you retained, without touching user data. Do not check these artifacts in.
- A live `run` or `tui` takes the single-instance lock. Use an owned TMPDIR; if a user's service collides, report it rather than stopping it.
- Live dispatch operates on captured native window identity, but capture still starts from the focused window. Drive only disposable windows; verify the fixture's GUI PID before key injection.
- An isolated installer HOME does not isolate the GUI LaunchAgent's loaded label or inherited environment. Inject the owned environment before bootstrap, verify program ownership before teardown, and leave an existing user's label untouched.
- Empty sentinel logs cannot observe native Framework calls. Direct API proof does not prove a different CLI/TUI entry point.
- Dry-run results never count as evidence for any row here.
