# Starter config

`window_zones init` writes a commented starter config with 18 bindings on one modifier chord (arrows for halves, U/I/J/K corners, D/F/G thirds, E/T two-thirds, Return maximize, C center, Backspace restore, Shift+Left/Right other display), refuses to overwrite an existing file without `--force`, rejects a chord that cannot form valid hotkeys, and can target an explicit `--config` path. A running App applies the new file.

## Sub-features

- `init-default` writes the starter config with the default `Ctrl+Alt` chord where none exists.
- `init-applied` a running session loads the written config and its hotkeys move the window.
- `init-refuse` leaves an existing config untouched and asks for `--force`.
- `init-chord` writes the bindings on another chord with `--chord MODIFIERS --force`.
- `init-bad-chord` rejects a chord that does not form valid hotkeys and keeps the old file.
- `init-path` writes to and reads from an explicit `--config PATH`.

## How to get to it (user POV)

- Run `window_zones init` in a terminal.
- Run `window_zones init --chord <MODIFIERS> --force` to pick another chord or replace the file.
- Run `window_zones --config <PATH> init` to write somewhere else.
- `./scripts/install.sh` also offers a starter config during installation (not driven here).

## Driving it with wzv

Preconditions:

- `RUN` and `R` set as in the README baseline; the run starts without a config (`--no-init`).

- **Start without config.** Run `wzv start --run-id "$RUN" --no-init`. It prints `ready=yes`.
- **No config yet.** Run `wzv exec "$RUN" --expect 'config state: Missing' --expect 'binding count: 0' -- status`. Exit code `0`.
- **Write the starter.** Run `wzv exec "$RUN" --expect-exit 0 --expect 'Wrote starter config to' --expect 'Hotkeys use Ctrl+Alt+<key>' -- init`. The printed path is the run's config path.
- **Session picks it up.** Run `wzv send "$RUN" reload --expect 'Config state: Loaded (18 bindings)'`.
- **Hotkey works.** Run `wzv send "$RUN" 'dispatch ctrl+alt+left' --expect 'last move: x=0 y=0 w=960 h=1080'`.
- **Refuse overwrite.** Run `wzv exec "$RUN" --expect-exit 0 --expect 'pass --force to replace it' -- init`. The file is unchanged and exit code is `0`.
- **Another chord.** Run `wzv exec "$RUN" --expect-exit 0 --expect 'Hotkeys use Ctrl+Super+<key>' -- init --chord Ctrl+Super --force`.
- **New chord active.** Run `wzv send "$RUN" reload --expect 'Config state: Loaded (18 bindings)'`, then `wzv send "$RUN" 'dispatch ctrl+super+right' --expect 'last move: x=960 y=0 w=960 h=1080'`.
- **Old chord gone.** Run `wzv send "$RUN" 'dispatch ctrl+alt+right' --expect 'no binding configured for hotkey'`.
- **Bad chord.** Run `wzv exec "$RUN" --expect-exit 1 --expect 'does not form valid hotkeys' -- init --chord Hyper --force`.
- **Old file kept.** Run `wzv exec "$RUN" --expect-exit 0 --expect 'last move: x=0 y=0 w=960 h=1080' -- dispatch ctrl+super+left`.
- **Explicit path.** Run `wzv exec "$RUN" --expect-exit 0 --expect 'Wrote starter config to' -- --config "$R/scratch/alt.toml" init`, then `wzv exec "$RUN" --expect 'binding count: 18' -- --config "$R/scratch/alt.toml" status`.
- **Proof.** Run `wzv stop "$RUN"`. It prints `exit_code=0`, `live_backend_tool_calls=0`, `instance_locks=0`, `home_files=1`. The transcript holds each `init` output; `side-effects.txt` lists the one config file in the run HOME.
- **Cleanup.** Run `wzv cleanup "$RUN"`. It prints `kept` with the evidence path.

## Gotchas

- `init` on an existing file exits `0`, not an error; assert on `pass --force to replace it`.
- The default chord depends on the desktop. `wzv` clears `XDG_CURRENT_DESKTOP`, so the default is `Ctrl+Alt` here; outside `wzv` check the `Hotkeys use ...` line.
- A running session hot-reloads the file within about 250 ms. Send `reload` before asserting so the check does not race the poll.
- `--chord` takes modifiers only; a key name such as `Hyper` fails the whole command.
- The config path contains a space on macOS (`Library/Application Support`). Quote it.
