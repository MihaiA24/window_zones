# Running Runbook

## Default run (safe)

`./scripts/run.sh` intentionally uses `--backend dry-run` and runs `status` for quick validation.

To exercise live runtime behavior, run directly:

```bash
cargo run -- run --backend native
```

That command uses the native hotkey listener on supported sessions, so configured hotkeys trigger asynchronously while the process is running.

On Wayland, native hotkeys are available when a supported compositor integration exists (currently Sway via `swaymsg`). When supported, global binds are registered through the Wayland backend and dispatched asynchronously while running; otherwise the process falls back to CLI hotkey mode (manual `dispatch`). If the native listener fails after startup (for example, after a compositor restart), runtime falls back to CLI mode for the current session.

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

You can also pass a custom config path:

```bash
./scripts/run.sh --config ./path/to/config.toml status
```
