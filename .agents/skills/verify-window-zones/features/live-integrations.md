# Live integrations

On a real desktop the same runtime moves real windows and captures real key presses through a platform backend: the GNOME Shell companion on GNOME Wayland, `XGrabKey` on X11, the KWin script and companion on KDE Plasma Wayland, the native hook on Windows and macOS, and compositor-bound `dispatch` on Sway and Hyprland. `run --tray` adds a tray menu on Linux and Windows, and the installed systemd user service runs `run` at login with desktop notifications for problems. None of this is reachable from the dry-run harness; every recipe below is blocked on a host without the matching desktop and is unverified by this skill.

## Sub-features

- `gnome-wayland` companion load, capability negotiation, displays, usable area, placement, real accelerator capture, companion recovery, TUI lifecycle (blocking gate).
- `x11` frame-geometry zones, next/previous display with wrap, real accelerator capture, invalid-config retention, TUI lifecycle (blocking gate).
- `kde-wayland` the same checks against a nested KWin 6.x (deferred gate).
- `windows` the manual Windows smoke run (deferred gate).
- `macos` AppleScript/`System Events` backend with the grabbing hook (ungated).
- `sway-hyprland` compositor keybindings calling `window_zones dispatch` (ungated).
- `tray` `run --tray` status menu (Linux and Windows).
- `notifications` desktop notifications from the service when stdout is not a terminal.
- `single-instance` a second live `run`/`tui` exits with status `75`.
- `service` `./scripts/install.sh --autostart` and the `window-zones` systemd user service.

## How to get to it (user POV)

- Log in to the desktop and press a bound hotkey while `window_zones run`, `window_zones tui`, or the service runs.
- Run `window_zones --backend auto run` (or `x11`, `wayland`, `windows`, `macos`) in a desktop session.
- Run `window_zones --tray run` on Linux or Windows.
- Bind a Sway or Hyprland key to `window_zones dispatch <hotkey>`.

## Driving it with wzv

Preconditions:

- Not drivable with `wzv`: it only runs `--backend dry-run`. These recipes need the platform named in each bullet, and `docs/runbooks/testing.md` is the record of gate status.
- Record the attempted command and the unmet precondition when a path is blocked; on the macOS recording host every bullet below was blocked.

- **GNOME synthetic gate.** On CachyOS/Linux with `gnome-shell` installed, run `./scripts/smoke.sh --keep-logs gnome`. The last line reads `Smoke assertions (gnome): N; passed: N; failed: 0; skipped: S`. Unverified here.
- **GNOME seated gate.** In a logged-in GNOME Wayland session, run `./scripts/smoke.sh --keep-logs gnome-live`. Same summary line with `failed: 0`. Unverified here.
- **X11 gate.** With Xvfb, Openbox and `xdotool`, run `./scripts/smoke.sh --keep-logs x11`. Same summary line with `failed: 0`; `docs/runbooks/testing.md` says the X11 results must be re-recorded after the `XGrabKey` cutover. Unverified here.
- **KDE gate.** With KWin 6.x and `kpackagetool6`, run `./scripts/smoke.sh --keep-logs kde-live`. Same summary line with `failed: 0`. Unverified here.
- **Windows gate.** Follow `docs/runbooks/windows-smoke.md` on a Windows desktop. Unverified here.
- **macOS backend.** Grant Accessibility to the terminal, run `./scripts/run.sh run`, and press `Ctrl+Alt+Left` on a disposable window. Without the permission dispatch fails with `allow Window Zones (or its terminal) in System Settings > Privacy & Security > Accessibility`. Blocked here: it moves the user's real windows.
- **Single instance.** In a desktop session with nothing else serving hotkeys, start `./scripts/run.sh run` twice; the second exits with status `75`. Never test this against the user's running service. Unverified here.

## Gotchas

- `./scripts/smoke.sh` is Linux-only (`setsid`, GNU `stat`, compositor binaries) and deletes its evidence in `/tmp/window_zones-smoke.$$` unless `--keep-logs` is passed.
- A live `run` or `tui` takes the single-instance lock, so it collides with the user's `window-zones` service; stop nothing of theirs, report the collision instead.
- The macOS and live Linux backends act on whatever window is focused. Only drive them with a disposable test window in front.
- Dry-run results never count as evidence for any row here.
