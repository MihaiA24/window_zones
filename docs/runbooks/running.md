# Running Runbook

## Default run (safe)

The app can manipulate real windows by default. Use dry-run for a safe local sanity check:

```bash
./scripts/run.sh
```

This command runs:
- `--backend dry-run`
- `status`

If a debug or release binary exists in `target/debug` or `target/release`, `run.sh` uses it directly.
If no local binary exists, it falls back to `cargo run --release --bin window_zones`.

## Common commands

```bash
# Check runtime status
./scripts/run.sh status

# Execute one hotkey dispatch without starting an interactive loop
./scripts/run.sh dispatch "Ctrl+Alt+Left"

# Start an interactive run loop
./scripts/run.sh run --backend dry-run
```

## KDE Plasma Wayland

Install and enable the KWin script, then start the companion before launching
the native KDE backend:

```bash
cargo build --release --locked --bin window_zones --bin window_zones_kwin
kpackagetool6 --type=KWin/Script --install ./kwin-script
./target/release/window_zones_kwin
```

Use `docs/runbooks/kde-wayland.md` for Plasma setup, D-Bus diagnostics,
hotkey cleanup, and the full native-window smoke checklist.

## GNOME Wayland

Install and enable the Shell companion before starting the native Wayland backend:

```bash
./scripts/install.sh --prefix "$HOME/.local"
$HOME/.local/bin/window_zones --backend auto status
$HOME/.local/bin/window_zones --backend auto run
```

Use `docs/runbooks/gnome-wayland.md` for extension installation, reload/restart checks, companion disconnect recovery, and the full two-display smoke checklist.

## TUI

Start the stdlib ANSI dashboard with the same backend and config options:

```bash
./scripts/run.sh --backend dry-run tui
```

The dashboard keeps the existing line-oriented input model. Use:

```text
status
refresh
reload
restart
dispatch Ctrl+Alt+Left
quit
```

For a release-gated manual smoke run, verify the dashboard on each supported
backend, confirm the resolved window and hotkey capability labels, exercise
reload/restart/status/dispatch, and exit with `quit`. Missing GNOME or KDE
companions must remain visible as in-band diagnostics; the TUI must not fall
back to X11.

You can also pass a custom config path:

```bash
./scripts/run.sh --config ./path/to/config.toml status
```
