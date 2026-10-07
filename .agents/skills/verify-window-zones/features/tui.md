# TUI dashboard

`window_zones tui` serves hotkeys like `run` but redraws an ANSI dashboard after every command: backend and capabilities, hotkey state, config path and state, all bindings, the last action, the last error, the last event, and the command list, followed by the `window-zones tui> ` prompt.

## Sub-features

- `tui-frame` shows backend, capabilities, hotkey state, config state, and every binding.
- `tui-dispatch` updates `Last action:` after `dispatch HOTKEY` or a bare hotkey.
- `tui-error` shows a failed dispatch in `Last error:`.
- `tui-refresh` redraws on `refresh` or `status`.
- `tui-reload` shows the reloaded config in `Last event:`.
- `tui-restart` clears the last action and error and registers the hotkeys again.
- `tui-config-error` shows a broken saved config.
- `tui-quit` prints `Session closed.` and exits `0`.

## How to get to it (user POV)

- Run `window_zones tui` in a terminal and type commands at the `window-zones tui> ` prompt.
- Press bound hotkeys while it runs (live backends only).

## Driving it with wzv

Preconditions:

- `RUN` and `R` set as in the README baseline; the run uses the starter config.

- **Start.** Run `wzv start --run-id "$RUN" --mode tui`. It prints `mode=tui` and `ready=yes` once `Window Zones TUI` is drawn.
- **Frame.** Run `wzv send "$RUN" refresh --expect 'Window backend: dry-run' --expect 'Hotkey state: Registered' --expect 'alt+ctrl+left -> MoveToZone { zone: "left-half" }' --expect 'Commands: reload | restart | status/refresh | dispatch HOTKEY | quit'`.
- **Dispatch.** Run `wzv send "$RUN" 'dispatch ctrl+alt+left' --expect 'Last action: alt+ctrl+left' --expect 'Last error: none'`.
- **Bare hotkey.** Run `wzv send "$RUN" 'ctrl+alt+up' --expect 'Last action: alt+ctrl+up'`.
- **Error.** Run `wzv send "$RUN" 'dispatch bogus' --expect 'Last error: dispatch: no binding configured for hotkey: bogus'`.
- **Reload.** Run `wzv send "$RUN" reload --expect 'Last event: Config state: Loaded (18 bindings)'`.
- **Restart.** Run `wzv send "$RUN" restart --expect 'Last action: <none>' --expect 'Last error: none' --expect 'Last event: Hotkeys registered: 18'`.
- **Status redraw.** Run `wzv send "$RUN" status --expect 'Window Zones TUI' --expect 'window-zones tui> '`.
- **Config error.** Write `{ cat "$R/evidence/config-00-init.toml"; echo 'this is not valid toml'; } >"$R/scratch/broken.toml"` and save it with `wzv config "$RUN" "$R/scratch/broken.toml" --expect 'the previous bindings remain active'`.
- **Recover.** Run `wzv config "$RUN" "$R/evidence/config-00-init.toml" --expect 'Config state: Loaded (18 bindings)'`.
- **Quit.** Run `wzv stop "$RUN"`. It prints `exit_code=0`, `live_backend_tool_calls=0`, `instance_locks=0`.
- **Proof.** Run `grep -q 'Session closed.' "$R/evidence/transcript.log"`. `"$R/evidence/app.log"` keeps the raw frames with ANSI codes; the transcript has them stripped.
- **Cleanup.** Run `wzv cleanup "$RUN"`.

## Gotchas

- Every command redraws the whole frame, so old values stay visible in the output; assert on the specific line (`Last action: ...`), not on the presence of a binding name.
- `help` prints no help text in the TUI; it only redraws the frame. Use `refresh` to redraw and `window_zones --help` for usage.
- `restart` clears `Last action:` and `Last error:`; assert on them before restarting.
- The TUI exits by itself when stdin closes; `wzv cleanup` then reports `gone` for the app.
