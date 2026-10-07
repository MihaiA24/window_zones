# Move the window

A bound hotkey moves and resizes the focused window into a zone of its current display's usable area: halves, corners, thirds, two-thirds, maximize, center, a custom percentage zone, back to where the App first found it (restore), or onto the next, previous, or directional display with proportional scaling. Pressing a half again on the same window cycles half, two-thirds, one third.

## Sub-features

- `zone-halves` left/right/top/bottom half.
- `zone-cycle` repeated left or right half cycles 1/2, 2/3, 1/3 of the width.
- `zone-corners` top-left, top-right, bottom-left, bottom-right quarters.
- `zone-thirds` left, center, right third and left/right two-thirds.
- `zone-maximize` fills the usable area.
- `center` centers the window without resizing.
- `custom-zone` moves into a `[zones]` percentage zone.
- `restore` returns to the geometry before the App's first move.
- `display-next-prev` moves to the next or previous display with proportional scaling, wrapping at the ends.
- `display-direction` moves to the display in a direction (`move-to-display`).

## How to get to it (user POV)

- Press a bound hotkey while `window_zones run` or `window_zones tui` serves hotkeys.
- Type `dispatch <hotkey>` (or the bare hotkey) into a `run` or `tui` session.
- Run `window_zones dispatch <hotkey>` once (no cycling or restore memory; see `one-shot-commands.md`).

## Driving it with wzv

Preconditions:

- `RUN` and `R` set as in the README baseline; the run uses the starter config.
- Steps run in order in one session; each geometry depends on the previous step.

- **Start.** Run `wzv start --run-id "$RUN"` and `wzv doctor "$RUN"`. It prints `doctor=ok`.
- **Left half.** Run `wzv send "$RUN" 'dispatch ctrl+alt+left' --expect 'last move: x=0 y=0 w=960 h=1080'`.
- **Left cycle.** Run `wzv send "$RUN" 'dispatch ctrl+alt+left' --expect 'last move: x=0 y=0 w=1280 h=1080'`, then `wzv send "$RUN" 'dispatch ctrl+alt+left' --expect 'last move: x=0 y=0 w=640 h=1080'`.
- **Right cycle.** Run `wzv send "$RUN" 'dispatch ctrl+alt+right' --expect 'last move: x=960 y=0 w=960 h=1080'`, `wzv send "$RUN" 'dispatch ctrl+alt+right' --expect 'last move: x=640 y=0 w=1280 h=1080'`, and `wzv send "$RUN" 'dispatch ctrl+alt+right' --expect 'last move: x=1280 y=0 w=640 h=1080'`.
- **Top and bottom.** Run `wzv send "$RUN" 'dispatch ctrl+alt+up' --expect 'last move: x=0 y=0 w=1920 h=540'` and `wzv send "$RUN" 'dispatch ctrl+alt+down' --expect 'last move: x=0 y=540 w=1920 h=540'`.
- **Corners.** Run `wzv send "$RUN" 'dispatch ctrl+alt+u' --expect 'last move: x=0 y=0 w=960 h=540'`, `wzv send "$RUN" 'dispatch ctrl+alt+i' --expect 'last move: x=960 y=0 w=960 h=540'`, `wzv send "$RUN" 'dispatch ctrl+alt+j' --expect 'last move: x=0 y=540 w=960 h=540'`, and `wzv send "$RUN" 'dispatch ctrl+alt+k' --expect 'last move: x=960 y=540 w=960 h=540'`.
- **Thirds.** Run `wzv send "$RUN" 'dispatch ctrl+alt+d' --expect 'last move: x=0 y=0 w=640 h=1080'`, `wzv send "$RUN" 'dispatch ctrl+alt+f' --expect 'last move: x=640 y=0 w=640 h=1080'`, and `wzv send "$RUN" 'dispatch ctrl+alt+g' --expect 'last move: x=1280 y=0 w=640 h=1080'`.
- **Two-thirds.** Run `wzv send "$RUN" 'dispatch ctrl+alt+e' --expect 'last move: x=0 y=0 w=1280 h=1080'` and `wzv send "$RUN" 'dispatch ctrl+alt+t' --expect 'last move: x=640 y=0 w=1280 h=1080'`.
- **Maximize.** Run `wzv send "$RUN" 'dispatch ctrl+alt+return' --expect 'last move: x=0 y=0 w=1920 h=1080'`.
- **Center.** Shrink to a corner, then center: `wzv send "$RUN" 'dispatch ctrl+alt+u' --expect 'last move: x=0 y=0 w=960 h=540'` and `wzv send "$RUN" 'dispatch ctrl+alt+c' --expect 'last move: x=480 y=270 w=960 h=540'`.
- **Custom zone config.** Write the fixture with `{ cat "$R/evidence/config-00-init.toml"; printf '\n[[bindings]]\nhotkey = "Ctrl+Alt+W"\naction = { type = "move-to-zone", zone = "wide-center" }\n\n[[bindings]]\nhotkey = "Ctrl+Alt+Shift+Up"\naction = { type = "move-to-display", direction = "right" }\n\n[zones]\nwide-center = { x = 10, y = 0, width = 80, height = 100 }\n'; } >"$R/scratch/custom.toml"` and save it with `wzv config "$RUN" "$R/scratch/custom.toml" --expect 'Config state: Loaded (20 bindings)'`.
- **Custom zone.** Run `wzv send "$RUN" 'dispatch ctrl+alt+w' --expect 'last move: x=192 y=0 w=1536 h=1080'`.
- **Display to the right.** Run `wzv send "$RUN" 'dispatch ctrl+alt+shift+up' --expect 'last move: x=2048 y=0 w=1024 h=1024'`.
- **No display further right.** Run `wzv send "$RUN" 'dispatch ctrl+alt+shift+up' --expect 'Dispatch state: Error'`.
- **Restore.** Run `wzv send "$RUN" 'dispatch ctrl+alt+backspace' --expect 'last move: x=40 y=40 w=640 h=480'`.
- **Next display.** Run `wzv send "$RUN" 'dispatch ctrl+alt+shift+right' --expect 'last move: x=1947 y=38 w=427 h=455'`.
- **Previous display.** Run `wzv send "$RUN" 'dispatch ctrl+alt+shift+left' --expect 'last move: x=41 y=40 w=641 h=480'`. Scaling there and back rounds, so the window lands one pixel off its restored geometry.
- **Previous wraps.** Run `wzv send "$RUN" 'dispatch ctrl+alt+shift+left' --expect 'last move: x=1947 y=38 w=427 h=455'`.
- **Proof.** Run `wzv stop "$RUN"`. It prints `exit_code=0`, `live_backend_tool_calls=0`, `instance_locks=0`. `"$R/evidence/transcript.log"` pairs each `> dispatch ...` line with its `last move:` geometry; `config-01.toml` is the custom config.
- **Cleanup.** Run `wzv cleanup "$RUN"`.

## Gotchas

- Geometry is the dry-run desktop's; a live desktop subtracts panels and docks from the usable area and reports frame-adjusted numbers.
- Cycling applies when the same half is pressed again on the same window; another zone in between starts a new cycle (observed: `right` after `left`-cycling starts at `w=960`).
- `restore` before the App has moved the window fails with `nothing to restore: the App has not moved the focused window`.
- Moves between displays scale position and size proportionally, so expect non-round numbers such as `x=1947 y=38 w=427 h=455`.
- Center keeps the size, so centering a maximized window prints the unchanged maximized geometry.
