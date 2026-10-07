# One-shot commands

`window_zones status` prints the runtime state and exits; `window_zones dispatch HOTKEY` runs the action bound to one hotkey once, prints the state and the resulting move, and exits non-zero when the dispatch fails. Compositor keybindings on Sway and Hyprland call exactly this path.

## Sub-features

- `status-report` prints backend, config path, binding count, config, hotkey and dispatch state.
- `status-missing` reports a missing config without failing.
- `dispatch-ok` moves the focused window and exits `0`.
- `dispatch-fresh` starts from a fresh process: no half cycling or restore memory carries over.
- `dispatch-unbound` reports an unbound hotkey and exits `1`.
- `cli-errors` rejects a missing hotkey argument and an unknown command with usage and exit `1`.
- `cli-help` prints usage and session commands and exits `0`.

## How to get to it (user POV)

- Run `window_zones status` or `window_zones dispatch Ctrl+Alt+Left` in a terminal.
- Bind a compositor key to `window_zones dispatch <hotkey>` (Sway `bindsym`, Hyprland `bind`).
- Run `window_zones --help`.

## Driving it with wzv

Preconditions:

- `RUN` and `R` set as in the README baseline. The session that `start` launches stays idle; one-shot commands only borrow its isolated environment and starter config.

- **Start.** Run `wzv start --run-id "$RUN"`. It prints `ready=yes`.
- **Status.** Run `wzv exec "$RUN" --expect-exit 0 --expect 'Using runtime window backend: dry-run' --expect 'binding count: 18' --expect 'hotkey state: Unregistered' -- status`.
- **Dispatch.** Run `wzv exec "$RUN" --expect-exit 0 --expect 'Dispatch state: Succeeded' --expect 'last move: x=0 y=0 w=960 h=1080' -- dispatch ctrl+alt+left`.
- **No carry-over.** Run the same command again: `wzv exec "$RUN" --expect-exit 0 --expect 'last move: x=0 y=0 w=960 h=1080' -- dispatch ctrl+alt+left`. The width stays `960`; a session would cycle to `1280`.
- **Restore has nothing.** Run `wzv exec "$RUN" --expect-exit 1 --expect 'nothing to restore' -- dispatch ctrl+alt+backspace`.
- **Unbound hotkey.** Run `wzv exec "$RUN" --expect-exit 1 --expect 'error: no binding configured for hotkey: ctrl+alt+f9' -- dispatch ctrl+alt+f9`.
- **Missing argument.** Run `wzv exec "$RUN" --expect-exit 1 --expect 'requires a hotkey argument' -- dispatch`.
- **Unknown command.** Run `wzv exec "$RUN" --expect-exit 1 --expect 'unexpected argument' -- frobnicate`.
- **Help.** Run `wzv exec "$RUN" --expect-exit 0 --expect 'Session commands: status, reload, restart, dispatch HOTKEY, help, q/quit/exit' -- --help`.
- **Missing config.** Run `wzv exec "$RUN" --expect-exit 0 --expect 'config state: Missing' -- --config "$R/scratch/none.toml" status`, then `wzv exec "$RUN" --expect-exit 1 --expect 'no binding configured' -- --config "$R/scratch/none.toml" dispatch ctrl+alt+left`.
- **Proof.** Run `wzv stop "$RUN"`. It prints `exit_code=0`, `live_backend_tool_calls=0`, `instance_locks=0`. Every one-shot command, its output, and its `exit_code=` are in `"$R/evidence/transcript.log"`.
- **Cleanup.** Run `wzv cleanup "$RUN"`.

## Gotchas

- Each `dispatch` is a new process. Half cycling and `restore` never carry over between one-shot presses; prove those in a session (`move-window.md`).
- `status` from a one-shot process always says `hotkey state: Unregistered`; only `run`/`tui` register hotkeys.
- A failed dispatch still prints the full status block before the error; assert on the `error:` line and the exit code, not on the absence of output.
- `wzv exec` refuses `--backend` and any `--config` outside the run directory.
- `dispatch` does not validate key names: `dispatch ctrl+alt+nokey` reports `no binding configured for hotkey: ctrl+alt+nokey` (exit `1`), not a parse error. Malformed hotkeys are only rejected when a config or `--chord` is loaded.
- Relative `--config` paths resolve from the current directory; `wzv exec` still requires them to land inside the run directory.
