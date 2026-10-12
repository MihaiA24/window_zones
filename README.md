# Window Zones

Window Zones is a Rust, cross-platform window positioning utility.

V1 is a background utility, not a replacement window manager. It listens for configured bindings and moves or resizes the currently focused OS-managed window into a named zone or onto another display.

See:

- `CONTEXT.md` for domain language.
- `docs/adr/0001-v1-window-positioning-utility.md` for the V1 boundary and first implementation slice.
- `docs/adr/0002-runtime-config-reload-atomicity.md` for runtime reload error and atomicity behavior.
- `docs/adr/0003-wayland-compositor-integrations.md` for native Wayland routing and companion integration decisions.
- `docs/adr/0007-v1-release-gates-and-verification.md` for V1 Release gates and Synthetic session verification.
- `docs/adr/0009-identified-moves-window-history-and-transactional-hotkeys.md` for identified-window moves, restore/history, and transactional hotkey reloads.

## Quick start (GNOME Wayland)

```bash
./scripts/install.sh --prefix "$HOME/.local" --gnome-extension --init-config --autostart
# log out and back in once so GNOME Shell loads the companion extension
window_zones status   # expects gnome-wayland and the companion capabilities
```

`init` writes `~/.config/window_zones/config.toml` with `Ctrl+Super` bindings on GNOME (`Ctrl+Alt+Super` on KDE, `Ctrl+Alt` elsewhere): arrows for halves (repeat Left/Right at a matching half to cross to the nearest physical horizontal display's opposite half, without wrapping), `U`/`I`/`J`/`K` corners, `D`/`F`/`G` thirds, `E`/`T` two-thirds, `Return` maximize, `C` center, `Backspace` restore, `Shift+Left`/`Shift+Right` previous/next display. The App picks up edits while it runs; Linux service problems are printed to the journal (`journalctl --user -u window-zones`) and shown as desktop notifications.

## Implemented today

The platform-neutral core and runtime paths currently include:

- integer pixel geometry;
- built-in zones (halves, top/bottom halves, quarters, thirds, two-thirds, maximize) and validated custom zones;
- actions: move to zone, next/previous display, display by direction, center, restore;
- display-to-display movement calculations;
- repeated left/right-half actions cross to the nearest physical neighbouring display, landing on its opposite (source-facing) half without wrapping;
- TOML config parsing, discovery, `init` starter config, and transactional hot reload that keeps the last valid config when a file or its hotkeys are rejected;
- the `WindowSystem` and `HotkeySystem` adapter contracts, with moves addressed to an identified window;
- action execution and config-driven dispatch;
- GNOME Shell 50/51 companion integration over the versioned user-session D-Bus contract (`org.window_zones.Gnome2`);
- automatic restoration of GNOME-maximized/tiled windows for zones and display moves, preserving their layout on the destination display;
- GNOME compositor animations for placement and display moves, respecting disabled animations and reduced motion;
- native KDE Plasma Wayland KWin script and protocol-2 companion integration;
- exclusive global hotkeys on X11 (`XGrabKey`), Windows and macOS (grabbing hook), GNOME, and KDE companion paths;
- CLI, stdlib ANSI TUI, optional Linux/Windows tray, single-instance `run`, and desktop notifications for background problems;
- opt-in `--perf-log PATH` JSONL profiling for dispatch/transition timing and Linux process RSS/peak RSS; see [profiling commands and measurement scope](docs/runbooks/running.md#performance-logging).

## V1 verification

- Blocking Release gates: GNOME Wayland (`./scripts/smoke.sh gnome` in a Synthetic session plus `./scripts/smoke.sh gnome-live` on the seated session) and Linux X11 (`./scripts/smoke.sh x11`), each including the TUI lifecycle Smoke run.
- KDE Plasma Wayland (`./scripts/smoke.sh kde-live` on a nested KWin) and Windows (`docs/runbooks/windows-smoke.md`) are implemented but deferred and non-blocking until a seated KWin 6.x Smoke run and the manual Windows run pass.
- macOS is ungated, with native window, hotkey-restart, LaunchAgent, and visible problem/recovery notification evidence in `docs/runbooks/testing.md`. Sway and Hyprland have no Smoke run on record or global hotkey capability and use compositor-bound dispatch (`window_zones dispatch <hotkey>` from the compositor keybinding config).
- `docs/runbooks/testing.md` is the record of record for gate status. The combined revision passed the isolated X11 and GNOME 51 runs, including CLI/TUI/tray, animation and profiling checks. GNOME's seated gate remains pending; historical platform results do not verify the combined revision elsewhere.

## Configuration discovery

`App::start()` resolves one startup config path using this precedence:

- Linux: `$XDG_CONFIG_HOME/window_zones/config.toml` when `$XDG_CONFIG_HOME` is absolute; otherwise `$HOME/.config/window_zones/config.toml`.
- Windows: `%APPDATA%\window_zones\config.toml` (the roaming application-data directory).
- macOS: `~/Library/Application Support/window_zones/config.toml`.

A missing file boots with empty bindings and `ConfigState::Missing`; `window_zones init` creates a starter config. Discovery, read, and TOML parse failures do not panic; the App keeps the last valid bindings and exposes an actionable `ConfigState::Error`. `App::start_at(path)` provides an explicit path for launchers and deterministic tests.

## Scripts and runbooks

Use these helper scripts for common workflows:

- `./scripts/install.sh` — build and install the binary; optional `--gnome-extension`, `--kwin-script`, `--init-config`, `--autostart` (Linux systemd user service or macOS LaunchAgent), `--uninstall`.
- `./scripts/run.sh` — run the binary with safe defaults for quick checks.
- `./scripts/test.sh` — run formatting, test, lint, and companion contract/syntax checks.
- `./scripts/smoke.sh` — run GNOME Wayland or Linux X11 Synthetic session Smoke runs; GNOME uses real interactive GTK windows and compositor-provided virtual input. `gnome-live` checks the seated GNOME session.

- `docs/runbooks/installation.md`
- `docs/runbooks/running.md`
- `docs/runbooks/testing.md`
- `docs/runbooks/gnome-wayland.md`
- `docs/runbooks/kde-wayland.md`

## macOS adapter caveats

The macOS backend retains native Accessibility window references and reads
`NSScreen.visibleFrame`; titles and focus changes do not redirect captured moves.
Allow Window Zones (or its terminal) under
`System Settings -> Privacy & Security -> Accessibility`.
Fullscreen and non-resizable windows are refused before changes. App-imposed
frame constraints are reported explicitly, not as successful placement.
A recorded failed hotkey listener is replaced by runtime `restart`; a live one is
reused. See the installation and running runbooks for LaunchAgent permissions.
