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
If no local binary exists, it falls back to `cargo run --release`.

## Common commands

```bash
# Check runtime status
./scripts/run.sh status

# Execute one hotkey dispatch without starting an interactive loop
./scripts/run.sh dispatch "Ctrl+Alt+Left"

# Start an interactive run loop
./scripts/run.sh run --backend dry-run
```

## GNOME Wayland

Install and enable the Shell companion before starting the native Wayland backend:

```bash
./scripts/install.sh --prefix "$HOME/.local"
$HOME/.local/bin/window_zones --backend auto status
$HOME/.local/bin/window_zones --backend auto run
```

Use `docs/runbooks/gnome-wayland.md` for extension installation, reload/restart checks, companion disconnect recovery, and the full two-display smoke checklist.

You can also pass a custom config path:

```bash
./scripts/run.sh --config ./path/to/config.toml status
```
