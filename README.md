# Window Zones

Window Zones is planned as a Rust, cross-platform, Rectangle-style window positioning utility.

V1 is a background utility, not a replacement window manager. It will listen for configured bindings and move or resize the currently focused OS-managed window into a named zone or onto another display.

See:

- `CONTEXT.md` for domain language.
- `docs/adr/0001-v1-window-positioning-utility.md` for the accepted v1 boundary and first implementation slice.
- `docs/adr/0002-runtime-config-reload-atomicity.md` for runtime reload error and atomicity behavior.

## First slice

The implemented core slices are platform-neutral only:

- integer pixel geometry
- built-in zones
- display-to-display movement calculations
- TOML config parsing
- a `WindowSystem` adapter contract
- an `execute_action` executor tested with fake adapters
- a `dispatch_hotkey` binding dispatcher tested with fake adapters

Platform adapters are available for Linux (X11/Wayland), Windows, and macOS.
They provide focused-window detection, display enumeration, and move/resize execution.

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

Runbook docs:

- `docs/runbooks/installation.md`
- `docs/runbooks/running.md`
- `docs/runbooks/testing.md`

## macOS adapter caveats

The macOS backend uses AppleScript (`osascript`) and `System Events`.
Focus and move calls require Accessibility permissions for the host process under
`System Settings -> Privacy & Security -> Accessibility`.
Without permission, backend actions fail with a `WindowSystemError::Platform` diagnostic.
