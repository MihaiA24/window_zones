# Window Zones

Window Zones is a Rust, cross-platform window positioning utility.

V1 is a background utility, not a replacement window manager. It listens for configured bindings and moves or resizes the currently focused OS-managed window into a named zone or onto another display.

See:

- `CONTEXT.md` for domain language.
- `docs/adr/0001-v1-window-positioning-utility.md` for the V1 boundary and first implementation slice.
- `docs/adr/0002-runtime-config-reload-atomicity.md` for runtime reload error and atomicity behavior.
- `docs/adr/0003-wayland-compositor-integrations.md` for native Wayland routing and companion integration decisions.
- `docs/adr/0007-v1-release-gates-and-verification.md` for V1 Release gates and Synthetic session verification.

## Implemented today

The platform-neutral core and runtime paths currently include:

- integer pixel geometry;
- built-in and validated custom zones;
- display-to-display movement calculations;
- TOML config parsing, discovery, reload, and atomic rollback;
- the `WindowSystem` and `HotkeySystem` adapter contracts;
- action execution and config-driven dispatch;
- GNOME Shell 50 companion integration over the versioned user-session D-Bus contract;
- native KDE Plasma Wayland KWin script and companion integration;
- global hotkey registration on supported non-Wayland paths, GNOME, and KDE companion paths;
- CLI, stdlib ANSI TUI, and optional Linux/Windows tray runtime controls.

## Remaining V1 verification

- Run the GNOME Wayland and Linux X11 blocking Release gates, including TUI lifecycle Smoke runs, in Synthetic sessions.
- KDE Plasma Wayland and Windows are implemented but deferred and non-blocking until a real KWin 6.x Smoke run and the manual run in `docs/runbooks/windows-smoke.md`, respectively.
- macOS remains source-compatible and non-blocking; Sway and Hyprland remain constrained backends.

## Configuration discovery

`App::start()` resolves one startup config path using this precedence:

- Linux: `$XDG_CONFIG_HOME/window_zones/config.toml` when `$XDG_CONFIG_HOME` is absolute; otherwise `$HOME/.config/window_zones/config.toml`.
- Windows: `%APPDATA%\window_zones\config.toml` (the roaming application-data directory).
- macOS: `~/Library/Application Support/window_zones/config.toml`.

A missing file boots with empty bindings and `ConfigState::Missing`. Discovery, read, and TOML parse failures do not panic; the App keeps empty bindings and exposes an actionable `ConfigState::Error`. `App::start_at(path)` provides an explicit path for launchers and deterministic tests.

## Scripts and runbooks

Use these helper scripts for common workflows:

- `./scripts/install.sh` — build and install the `window_zones` binary.
- `./scripts/run.sh` — run the binary with safe defaults for quick checks.
- `./scripts/test.sh` — run formatting, test, and lint checks.
- `./scripts/smoke.sh` — run GNOME Wayland or Linux X11 Synthetic session Smoke runs.

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
