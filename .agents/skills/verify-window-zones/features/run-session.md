# Run session

`window_zones run` serves the configured hotkeys until told to quit and reads line commands from stdin: `status`, `reload`, `restart`, `dispatch HOTKEY`, `help`, and `q`/`quit`/`exit`. Any other line is dispatched as a hotkey. It prints runtime events (config state, hotkey registration, dispatch results) once per change.

## Sub-features

- `session-ready` prints the backend, the session banner, the config state, and the registered hotkey count.
- `session-status` prints the runtime status block, including the last action.
- `session-help` prints usage and the session command list.
- `session-dispatch` runs a binding and prints the dispatch state and move.
- `session-bare-hotkey` treats an unknown line as a hotkey.
- `session-unbound` reports an unbound hotkey and keeps serving.
- `session-reload` re-reads the config on request.
- `session-restart` rebuilds the runtime and registers the hotkeys again.
- `session-quit` ends the session with `Session closed.` and exit code `0`.

## How to get to it (user POV)

- Run `window_zones run` in a terminal and type commands.
- Run it as the `window-zones` systemd user service (`./scripts/install.sh --autostart`); stdin is closed there and it keeps serving (not driven here).
- Global hotkeys reach the same runtime (live backends only; see `live-integrations.md`).

## Driving it with wzv

Preconditions:

- `RUN` and `R` set as in the README baseline; the run uses the starter config.

- **Start.** Run `wzv start --run-id "$RUN"` and `wzv doctor "$RUN"`. `"$R/evidence/app.log"` begins with `Window backend: dry-run`, `Interactive session started.`, `Config state: Loaded (18 bindings)`, and `Hotkeys registered: 18`.
- **Status.** Run `wzv send "$RUN" status --expect 'binding count: 18' --expect 'hotkey state: Registered' --expect 'last action: <none>'`.
- **Help.** Run `wzv send "$RUN" help --expect 'Session commands: status, reload, restart, dispatch HOTKEY, help, q/quit/exit'`.
- **Dispatch.** Run `wzv send "$RUN" 'dispatch Ctrl+Alt+Left' --expect 'Dispatch state: Succeeded' --expect 'last move: x=0 y=0 w=960 h=1080'`.
- **Status follows.** Run `wzv send "$RUN" status --expect 'last action: alt+ctrl+left' --expect 'dispatch state: Succeeded'`.
- **Bare hotkey.** Run `wzv send "$RUN" 'ctrl+alt+right' --expect 'last move: x=960 y=0 w=960 h=1080'`.
- **Unbound hotkey.** Run `wzv send "$RUN" bogus --expect 'error: no binding configured for hotkey: bogus'`, then `wzv doctor "$RUN"`. The session is still alive.
- **Reload.** Run `wzv send "$RUN" reload --expect 'Reload requested.' --expect 'Config state: Loaded (18 bindings)'`.
- **Restart.** Run `wzv send "$RUN" restart --expect 'Runtime restarted.' --expect 'Hotkeys registered: 18'`.
- **State after restart.** Run `wzv send "$RUN" status --expect 'last action: <none>' --expect 'dispatch state: Idle'`.
- **Quit alias.** Run `wzv send "$RUN" exit --expect 'Session closed.'`.
- **Proof.** Run `wzv stop "$RUN"`. It notices the session already ended and prints `exit_code=0`, `live_backend_tool_calls=0`, `instance_locks=0`.
- **Cleanup.** Run `wzv cleanup "$RUN"`.

## Gotchas

- A typo in a command is dispatched as a hotkey and fails as `no binding configured`; it does not end the session.
- Closing stdin does not stop `run`; it keeps serving hotkeys. Quit explicitly, or `wzv cleanup` sends SIGTERM.
- On a live backend only one `run`/`tui` may own the hotkeys; a second one exits with status `75`. Dry-run skips that lock, so this is not provable here.
- On Linux, when stdout is not a terminal, problem events also raise a desktop notification; `wzv` points the session bus at a dead socket so no notification can leave the run (unverified on Linux).
