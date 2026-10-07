# Window Zones

Window Zones is a Rust, cross-platform window positioning utility.

V1 is a background utility, not a replacement window manager. It listens for configured bindings and moves or resizes the currently focused OS-managed window into a named zone or onto another display.

See:

- `CONTEXT.md` for domain language.
- `docs/adr/0001-v1-window-positioning-utility.md` for the V1 boundary and first implementation slice.
- `docs/adr/0002-runtime-config-reload-atomicity.md` for runtime reload error and atomicity behavior.
- `docs/adr/0003-wayland-compositor-integrations.md` for native Wayland routing and companion integration decisions.
- `docs/adr/0007-v1-release-gates-and-verification.md` for V1 Release gates and Synthetic session verification.
- `docs/adr/0009-identified-moves-window-history-and-transactional-hotkeys.md` for identified-window moves, restore/cycling, and transactional hotkey reloads.

## Quick start (GNOME Wayland)

```bash
./scripts/install.sh --prefix "$HOME/.local" --gnome-extension --init-config --autostart
# log out and back in once so GNOME Shell loads the companion extension
window_zones status   # expects gnome-wayland and the companion capabilities
```

`init` writes `~/.config/window_zones/config.toml` with `Ctrl+Super` bindings on GNOME (`Ctrl+Alt` elsewhere): arrows for halves (press again to cycle half → two-thirds → third), `U`/`I`/`J`/`K` corners, `D`/`F`/`G` thirds, `E`/`T` two-thirds, `Return` maximize, `C` center, `Backspace` restore, `Shift+Left`/`Shift+Right` previous/next display. The App picks up edits while it runs; problems are printed to the journal (`journalctl --user -u window-zones`) and shown as desktop notifications.

## Implemented today

The platform-neutral core and runtime paths currently include:

- integer pixel geometry;
- built-in zones (halves, top/bottom halves, quarters, thirds, two-thirds, maximize) and validated custom zones;
- actions: move to zone (repeating a half cycles widths), next/previous display, display by direction, center, restore;
- display-to-display movement calculations;
- TOML config parsing, discovery, `init` starter config, and hot reload that keeps the last valid config when a file or its hotkeys are rejected;
- the `WindowSystem` and `HotkeySystem` adapter contracts, with moves addressed to an identified window;
- action execution and config-driven dispatch;
- GNOME Shell 50 companion integration over the versioned user-session D-Bus contract (`org.window_zones.Gnome2`);
- native KDE Plasma Wayland KWin script and companion integration;
- exclusive global hotkeys on X11 (`XGrabKey`), Windows and macOS (grabbing hook), GNOME, and KDE companion paths;
- CLI, stdlib ANSI TUI, optional Linux/Windows tray, single-instance `run`, and desktop notifications for background problems.

## V1 verification

- Blocking Release gates: GNOME Wayland (`./scripts/smoke.sh gnome` in a Synthetic session plus `./scripts/smoke.sh gnome-live` on the seated session) and Linux X11 (`./scripts/smoke.sh x11`), each including the TUI lifecycle Smoke run.
- KDE Plasma Wayland (`./scripts/smoke.sh kde-live` on a nested KWin) and Windows (`docs/runbooks/windows-smoke.md`) are implemented but deferred and non-blocking until a seated KWin 6.x Smoke run and the manual Windows run pass.
- macOS, Sway, and Hyprland are ungated integrations: they build and ship without a Smoke run on record. Sway and Hyprland have no global hotkey capability and use compositor-bound dispatch (`window_zones dispatch <hotkey>` from the compositor keybinding config).
- `docs/runbooks/testing.md` is the record of record for gate status.

## Configuration discovery

`App::start()` resolves one startup config path using this precedence:

- Linux: `$XDG_CONFIG_HOME/window_zones/config.toml` when `$XDG_CONFIG_HOME` is absolute; otherwise `$HOME/.config/window_zones/config.toml`.
- Windows: `%APPDATA%\window_zones\config.toml` (the roaming application-data directory).
- macOS: `~/Library/Application Support/window_zones/config.toml`.

A missing file boots with empty bindings and `ConfigState::Missing`; `window_zones init` creates a starter config. Discovery, read, and TOML parse failures do not panic; the App keeps the last valid bindings and exposes an actionable `ConfigState::Error`. `App::start_at(path)` provides an explicit path for launchers and deterministic tests.

## Scripts and runbooks

Use these helper scripts for common workflows:

- `./scripts/install.sh` — build and install the binary; optional `--gnome-extension`, `--kwin-script`, `--init-config`, `--autostart` (systemd user service), `--uninstall`.
- `./scripts/run.sh` — run the binary with safe defaults for quick checks.
- `./scripts/test.sh` — run formatting, test, lint, and GNOME companion contract checks.
- `./scripts/smoke.sh` — run GNOME Wayland or Linux X11 Synthetic session Smoke runs; `gnome-live` runs the seated GNOME session checks.

- `docs/runbooks/installation.md`
- `docs/runbooks/running.md`
- `docs/runbooks/testing.md`
- `docs/runbooks/gnome-wayland.md`
- `docs/runbooks/kde-wayland.md`

## macOS adapter caveats

The macOS backend uses AppleScript (`osascript`) and `System Events`.
Focus and move calls require Accessibility permissions for the host process under
`System Settings -> Privacy & Security -> Accessibility`.
Without permission, backend actions fail with a `WindowSystemError::Platform` diagnostic.
