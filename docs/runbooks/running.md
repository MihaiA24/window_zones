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

## First run

```bash
window_zones init                      # writes the starter config to the default path
window_zones init --chord "Ctrl+Alt"   # pick another modifier chord
window_zones init --force              # replace an existing config
```

`init` never overwrites an existing config without `--force`. The default chord is `Ctrl+Super` on GNOME (GNOME binds `Ctrl+Alt+arrows` to workspaces and `Super+arrows` to tiling), `Ctrl+Alt+Super` on KDE Plasma, and `Ctrl+Alt` elsewhere.

## Config reference

```toml
[zones]                                  # optional custom zones, percent of the usable area
wide-center = { x = 10, y = 0, width = 80, height = 100 }

[[bindings]]
hotkey = "Ctrl+Super+Left"
action = { type = "move-to-zone", zone = "left-half" }
```

Actions: `move-to-zone` (`zone = NAME`), `move-to-next-display`, `move-to-previous-display`, `move-to-display` (`direction = "left" | "right" | "up" | "down"`), `center` (keeps the size, shrinks to fit), `restore` (returns the window to its geometry before the App first moved it). Built-in zones: `left-half`, `right-half`, `top-half`, `bottom-half`, `top-left`, `top-right`, `bottom-left`, `bottom-right`, `left-third`, `center-third`, `right-third`, `left-two-thirds`, `right-two-thirds`, `maximize`. Repeated left/right halves use the directional crossing behavior below, not width cycling. Explicit next/previous display actions follow the left-to-right, top-to-bottom layout, wrap, and do nothing with one display. Unknown fields anywhere, including on actions, are rejected.

Moves target the window identity captured at discovery, not a newly focused window; a closed target fails as `WindowGone`. Maximized and tiled windows are restored before they are placed; fullscreen and non-resizable windows are refused. `restore` history remembers windows within one `run`/`tui` session; a window you move by hand starts over.

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

## Running in the background

Start the listener with `window_zones run`; it keeps serving hotkeys when stdin is closed. `./scripts/install.sh --autostart` installs the Linux `window-zones` systemd user service or the macOS `org.window_zones.run` LaunchAgent. Only one live `run`/`tui` session owns the hotkeys: a second exits with status `75` and names the owning pid. Do not stop someone else's session to run a smoke check.

On Linux, manage the service with `systemctl --user status|stop|start|restart window-zones`; `systemctl --user disable --now window-zones` turns it off. Logs: `journalctl --user -u window-zones -f`. On macOS, use `launchctl print gui/$(id -u)/org.window_zones.run` and `launchctl kickstart -k gui/$(id -u)/org.window_zones.run`; logs are `~/Library/Logs/window_zones.log`. See [installation](installation.md#macos-launchagent) for loading, ownership, and permissions.

The App prints runtime events once per change, never per retry:

- `Config state: Loaded (N bindings)` / `Config state: Missing; run window_zones init ...`
- `Config reload error: ...` — a broken file, or a file whose hotkeys were refused; the previous bindings stay active.
- `Hotkeys registered: N`, `Hotkey registration now recovered: ...`
- `Hotkey registration refused: ...` — a binding is taken by the desktop or another program; nothing is retried until the config changes.
- `Hotkey registration failed: ...; retrying` — the companion is absent or busy; retried every second.
- `Dispatch failed for <hotkey>: ...` — once per press.

With non-terminal stdout, Linux and macOS notify about background problems; `--backend dry-run` never does. Config errors and refused hotkeys notify at once; unavailable hotkeys notify only after 20 seconds without recovery. Linux updates the notification in place and clears it on matching recovery. macOS sends separate problem/recovery notices through Standard Additions; visibility follows [macOS notification preferences](https://developer.apple.com/library/archive/documentation/LanguagesUtilities/Conceptual/MacAutomationScriptingGuide/DisplayNotifications.html). Actual problem and recovery banners and Notification Center content were observed in the incoming readiness work, attributed to Script Editor; enable notifications for that sender if needed.

`dispatch` exits `1` when the dispatch state is an error (no binding, no focused window, adapter failure); the state is still printed.

## Backend resolution

`--backend auto` reads `XDG_SESSION_TYPE`, `WAYLAND_DISPLAY`, and `DISPLAY` on Linux. It resolves X11 or Wayland only when the signals agree; otherwise it fails with `cannot resolve desktop session from XDG_SESSION_TYPE, WAYLAND_DISPLAY, and DISPLAY: <reason>; use --backend x11|wayland`, where the reason is one of: conflicting `XDG_SESSION_TYPE=x11` and `WAYLAND_DISPLAY`; unrecognized `XDG_SESSION_TYPE`; `WAYLAND_DISPLAY` and `DISPLAY` both set without `XDG_SESSION_TYPE`; all three unset. An explicit `--backend wayland` overrides an unresolved identity; `--backend x11` is refused whenever `WAYLAND_DISPLAY` or `XDG_SESSION_TYPE=wayland` is set, because inside a Wayland session it would only reach XWayland windows.

## Hotkey vocabulary

Config and `dispatch` hotkeys are canonicalized once by the App: modifiers `alt`, `ctrl`, `shift`, `cmd` (aliases `control`, `super`, `win`, `meta`, `command`, `option` are accepted in config and normalized), then one key from `a`-`z`, `0`-`9`, `f1`-`f24`, `escape`, `return`, `space`, `tab`, `backspace`, `delete`, `insert`, `home`, `end`, `pageup`, `pagedown`, `left`, `right`, `up`, `down`, `minus`, `equal`, `comma`, `dot`, `slash`, `quote`, `semicolon`, `leftbracket`, `rightbracket`, `backslash`, `backquote`, `printscreen`, `scrolllock`, `capslock`, `numlock`, `pause`. Key aliases such as `esc`, `enter`, `page_up` normalize to the canonical names; anything else is `unknown key`. Unknown config fields and tables are rejected.

## Sway and Hyprland

Neither compositor exposes a global hotkey capability to the App, so `run` reports the hotkey set as refused there. Bind the compositor's own keys to compositor-bound dispatch (each `dispatch` is a separate process, so `restore` history does not carry over; repeated-half crossing is geometry-based and still works):

```text
# sway
bindsym $mod+Left exec window_zones dispatch ctrl+alt+left
# hyprland
bind = CTRL ALT, left, exec, window_zones dispatch ctrl+alt+left
```

Window and display state come from `swaymsg`/`hyprctl`; both integrations are ungated (no Smoke run on record).

## Linux X11

Placement uses the window frame (client origin translated to root coordinates plus `_NET_FRAME_EXTENTS`) and restores maximized windows before configuring. Displays are RandR monitors minus the `_NET_WM_STRUT_PARTIAL`/`_NET_WM_STRUT` space panels reserve on each monitor. Hotkeys are exclusive `XGrabKey` grabs: a bound combination never reaches the focused application, and a combination another program already grabbed refuses the whole set.

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
./scripts/install.sh --prefix "$HOME/.local" --gnome-extension --init-config
# log out and back in once, then:
$HOME/.local/bin/window_zones status
$HOME/.local/bin/window_zones run
```

Use `docs/runbooks/gnome-wayland.md` for extension installation, reload/restart checks, companion disconnect recovery, and the full two-display smoke checklist.

## macOS

Use an explicit live backend: `./scripts/run.sh --backend macos run`; the runner's
default is dry-run. Native Accessibility references preserve the selected window
across title, focus, and ordering changes. Native display coordinates use
`NSScreen.visibleFrame` converted to global top-left coordinates.

Placement restores exposed zoom/minimized state, releases ordinary AppKit
zoom/tiling through size and position, and verifies the resulting frame. Resize
first releases native state; a resize clamped at the old origin is completed at
the target origin. Fullscreen and unwritable geometry are rejected before
mutation. App-imposed minimum sizes and other frame constraints remain errors.
Let native zoom/tiling animations settle before checking final placement.

If a native listener reports stopped/error, restore input/Accessibility permission
and type `restart`. A live process-owned listener is reused; a recorded stopped
one is replaced. There is no automatic hook reconstruction on registration
retries. A LaunchAgent needs permission for its installed binary rather than
relying on the terminal's grant.

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
