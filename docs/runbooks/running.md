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

## Directional half placement

Left/right-half actions snap within the current display first. Repeating Left
when the window already occupies the left half moves it to the right half of
the nearest display physically to the left; repeating Right does the reverse.
The neighbour must overlap vertically. There is no double-press time limit,
and the outer edge stays put rather than wrapping. Explicit next/previous
display actions still wrap and preserve the window's layout.

GNOME animates placement for 250 ms using compositor transforms and a fading
snapshot, not repeated client resize requests. Desktop animation settings and
reduced motion are respected. Maximized/tiled restoration completes before
the destination frame is requested, preventing a late Wayland restore
acknowledgement from overwriting it.

## Performance logging

Logging is opt-in for `dispatch`, `run` (including tray mode), and `tui`:

```bash
$HOME/.local/bin/window_zones --backend auto \
  --perf-log "$HOME/window-zones-perf.jsonl" tui

# Optional: inspect individual moves with jq.
jq 'select(.event == "dispatch") | {hotkey, result, dispatch_ms, request_ms, completion_ms, rss_kib, peak_rss_kib, error, metrics_error}' \
  "$HOME/window-zones-perf.jsonl"
```

The file is append-only JSONL: a startup `session` row and one `dispatch` row
per attempted action, including failures. Each row has a timestamp, PID,
action/result/error, and memory scope. Choose a fresh path for an isolated
measurement; runtime `restart` preserves the open log.

| Field | Measurement |
|---|---|
| `dispatch_ms` | Rust action execution, including adapter calls and completion waits; excludes hotkey queue/polling latency, metrics retrieval, memory sampling, and log I/O |
| `request_ms` | GNOME elapsed time until the destination move/resize request is submitted, including any restoration wait |
| `completion_ms` | GNOME elapsed time through frame settling and the completed visual transition, not just request acknowledgement |
| `rss_kib` | Current Linux `VmRSS` of the `window_zones` process, in KiB |
| `peak_rss_kib` | Linux process-lifetime `VmHWM`, not the peak of an individual transition |
| `metrics_error` | Failed optional timing retrieval; action success is independent |

`memory_scope` is `window_zones-process`: it excludes the shared GNOME Shell
process, compositor/GPU snapshot memory, and other applications. Memory is
sampled at startup and after dispatch, not continuously. Unsupported memory
or companion timings are `null`, never fabricated zeroes.

An unwritable destination fails startup visibly. A later write failure emits
a diagnostic and disables logging without stopping window movement. Without
`--perf-log`, no log file, process-memory sampling, or timing-retrieval RPC
is performed.

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
