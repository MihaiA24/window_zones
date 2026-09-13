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

Start the background listener with `window_zones run`, or install with `./scripts/install.sh --autostart` to start it automatically at desktop login (`run` keeps running after stdin closes when stdin is not a terminal).

`dispatch` exits `1` when the dispatch state is an error (no binding, no focused window, adapter failure); the state is still printed.

## Backend resolution

`--backend auto` reads `XDG_SESSION_TYPE`, `WAYLAND_DISPLAY`, and `DISPLAY` on Linux. It resolves X11 or Wayland only when the signals agree; otherwise it fails with `cannot resolve desktop session from XDG_SESSION_TYPE, WAYLAND_DISPLAY, and DISPLAY: <reason>; use --backend x11|wayland`, where the reason is one of: conflicting `XDG_SESSION_TYPE=x11` and `WAYLAND_DISPLAY`; unrecognized `XDG_SESSION_TYPE`; `WAYLAND_DISPLAY` and `DISPLAY` both set without `XDG_SESSION_TYPE`; all three unset. An explicit `--backend x11|wayland` overrides an unresolved identity, but `--backend x11` inside a resolved Wayland session is still rejected.

## Hotkey vocabulary

Config and `dispatch` hotkeys are canonicalized once by the App: modifiers `alt`, `ctrl`, `shift`, `cmd` (aliases `control`, `super`, `win`, `meta`, `command`, `option` are accepted in config and normalized), then one key from `a`-`z`, `0`-`9`, `f1`-`f24`, `escape`, `return`, `space`, `tab`, `backspace`, `delete`, `insert`, `home`, `end`, `pageup`, `pagedown`, `left`, `right`, `up`, `down`, `minus`, `equal`, `comma`, `dot`, `slash`, `quote`, `semicolon`, `leftbracket`, `rightbracket`, `backslash`, `backquote`, `printscreen`, `scrolllock`, `capslock`, `numlock`, `pause`. Key aliases such as `esc`, `enter`, `page_up` normalize to the canonical names; anything else is `unknown key`. Unknown config fields and tables are rejected.

## Sway and Hyprland

Neither compositor exposes a global hotkey capability to the App, so `run` reports hotkeys as unavailable there. Bind the compositor's own keys to compositor-bound dispatch:

```text
# sway
bindsym $mod+Left exec window_zones dispatch ctrl+alt+left
# hyprland
bind = CTRL ALT, left, exec, window_zones dispatch ctrl+alt+left
```

Window and display state come from `swaymsg`/`hyprctl`; both integrations are ungated (no Smoke run on record).

## Linux X11

Placement uses the window frame (client origin translated to root coordinates plus `_NET_FRAME_EXTENTS`) and removes `_NET_WM_STATE_MAXIMIZED_*` before configuring. Displays are RandR monitors; panels advertising `_NET_WM_STRUT` are not subtracted from the usable area.

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
