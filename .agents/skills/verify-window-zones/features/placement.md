# Window placement and display movement

Users place the focused OS window in a configured zone or move it to the next/previous display while retaining zone intent or proportional placement.

## Sub-features

- `place-zone`: left/right halves, left/center/right thirds, left/right two-thirds and maximize.
- `place-custom`: percentage-based user zones resolve inside the display's usable area.
- `place-display`: next/previous display and wraparound preserve recognized zones or proportional geometry.
- `place-boundaries`: unknown binding, no focused window, no usable displays or denied movement exposes an error without a false success.

## How to get to it (user POV)

- Press a configured global accelerator while the target window has focus.
- Run `scripts/run.sh … dispatch HOTKEY` for a one-shot action.
- Type `dispatch HOTKEY` or a bare hotkey in `run`/`tui`; the tray runtime also listens for global hotkeys.
- Edit the TOML binding's zone/action, then reload or restart.

## Driving it with verify

Preconditions:

- Baseline/current build. Native proof needs the gate's real test window, focused window manager and displays; core dry-run always supplies a simulated focused window.

- **Core placement.** `"$V" drive cli --expect 'Dispatch state: Succeeded' --expect 'last move: x=0 y=0 w=960 h=1080' -- dispatch Alt+Ctrl+Left` proves only computed dry-run output.
- **Fixed-oracle check.** `"$V" check behaviors` compares 15 zone/display/error cases across all four CLI backend/config variants against the reviewed `expected.json`, with fixed seeds/wall clock and per-field JSON differences. `"$V" check e2e` compares the existing native X11 journey and retained PNGs, not only the gate summary; native TUI dispatch must have a completed successful response and independently matching post-dispatch geometry.
- **Performance measurement.** `"$V" check performance --baseline-ref HEAD --runs 5` alternates at least five confirmed startup+dispatch samples per side, using the ref's own runner/binary. Require the full report's work confirmations, median and max−min spread; this does not measure compositor movement or global-hotkey latency.
- **Other built-in variants.** Use `"$V" drive config valid --zone right-third`, then dispatch Left and expect `x=1280 y=0 w=640 h=1080`. Repeat for every affected zone via `drive config --help`, including odd-edge cases in the existing `drive tests`; restore `drive config valid`.
- **Custom zone.** `"$V" drive config custom`, then `"$V" drive cli --expect 'last move: x=192 y=108 w=960 h=540' -- dispatch Alt+Ctrl+Left`. A second CLI process must read the same custom config. Restore `drive config valid`.
- **Display direction.** `"$V" drive cli --expect 'Dispatch state: Succeeded' -- dispatch Alt+Ctrl+Down` and `"$V" drive cli --expect 'Dispatch state: Succeeded' -- dispatch Alt+Ctrl+Shift+Right` exercise the next/previous actions; do not mistake computed output for OS geometry proof.
- **Actual user movement.** `"$V" drive smoke x11` runs the existing two-logical-display fixture, dispatches left-half/center-third/two-thirds/next/previous once each, injects a real hotkey and checks exact geometry with xdotool plus screenshots. X11 configure requests are server-acknowledged before the connection closes; the gate does not retry dropped requests. Its TUI dispatch must move from the independently checked center-third baseline to left-half. Inspect each assertion and image. `drive smoke kde-live` covers a real nested KWin; `drive smoke gnome` cannot prove focused placement without a seat.
- **Interactive entry.** `"$V" drive session --mode run --command 'dispatch Alt+Ctrl+Left' --expect 'Dispatch state: Succeeded'`; for TUI use `--mode tui` and expect `Last action: alt+ctrl+left`. Built-in and custom TUI dispatch success also requires the completed response/prompt without `Last error: dispatch:`. Bare `Alt+Ctrl+Left` is a separate line-input entry with the same success checks.
- **Error/empty.** `"$V" drive cli --expect 'NoBindingForHotkey' -- dispatch Alt+Ctrl+F12`; then `drive config empty` and repeat Left with that error. For an intentional TUI failure, pair the dispatch command with `--expect 'Last error: dispatch: no binding configured'`, not a `Last action` label. Restore valid. Native no-focus/no-display/denied paths require those real conditions and an unchanged-window observer.
- **Cancel/persistence.** Quit a runtime without dispatch and compare native geometry before/after when claiming no move. After a successful move, independently observe geometry before and after runtime exit; config survives a new process, but Window Zones does not persist/restore a dispatch history.

## Gotchas

- Dry-run records a requested rectangle; it never moves an OS window and is not native geometry evidence.
- The baseline seeds only three built-in zones, no custom zone, and no blank-display/denied adapter condition. `--zone` and `custom` supply zone preconditions; native error preconditions still need real environments. Missing focus/outputs/permissions must be reported, not silently treated as covered.
- Xvfb may retain an aggregate automatic RandR monitor alongside two logical monitors; inspect assertions rather than assuming physical hardware equivalence.
- `kde-live` uses a private virtual framebuffer and bus, not an outer desktop socket. Real accelerator evidence requires the post-injection action plus independent window geometry; compositor debug-log wording is not a placement assertion.
- Placement is immediate: there is no cancel/undo dialog after dispatch. Cancel means exiting before dispatch; do not invent a UI path.
- The existing gate does not cover every built-in/custom zone or every surface/mode/flag. If those are affected, expand the maintained runner and matrix rather than citing one convenient left-half screenshot.
