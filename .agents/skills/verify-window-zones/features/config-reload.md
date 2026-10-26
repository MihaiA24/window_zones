# Live config reload

While the App runs it watches its config file and applies every saved change. A file that fails to parse or validate is reported with the path and reason, and the previous bindings stay active until a valid file is saved; added and removed bindings take effect without a restart.

## Sub-features

- `reload-parse-error` reports invalid TOML and keeps the previous bindings.
- `reload-keeps-working` previous bindings still dispatch while the file is broken.
- `reload-invalid-zone` reports a binding that names an unknown zone.
- `reload-duplicate` reports two bindings for the same hotkey.
- `reload-recover` loads the next valid file.
- `reload-add-remove` registers added bindings and drops removed ones.

## How to get to it (user POV)

- Edit and save the config file while `window_zones run` or `window_zones tui` is running.
- Type `reload` in the session to re-read the file immediately.

## Driving it with wzv

Preconditions:

- `RUN` and `R` set as in the README baseline; the run uses the starter config, kept in `"$R/evidence/config-00-init.toml"`.

- **Start.** Run `wzv start --run-id "$RUN"` and `wzv doctor "$RUN"`.
- **Broken TOML.** Write `{ cat "$R/evidence/config-00-init.toml"; echo 'this is not valid toml'; } >"$R/scratch/broken.toml"` and save it with `wzv config "$RUN" "$R/scratch/broken.toml" --expect 'Config reload error: failed to parse config at' --expect 'the previous bindings remain active'`.
- **Still serving.** Run `wzv send "$RUN" 'dispatch ctrl+alt+left' --expect 'last move: x=0 y=0 w=960 h=1080'`.
- **Error in status.** Run `wzv send "$RUN" status --expect 'binding count: 18' --expect 'config state: Error'`.
- **Unknown zone.** Write `{ cat "$R/evidence/config-00-init.toml"; printf '[[bindings]]\nhotkey = "Ctrl+Alt+Q"\naction = { type = "move-to-zone", zone = "nope" }\n'; } >"$R/scratch/badzone.toml"` and save it with `wzv config "$RUN" "$R/scratch/badzone.toml" --expect 'unknown zone nope; the previous bindings remain active'`.
- **Duplicate hotkey.** Write `{ cat "$R/evidence/config-00-init.toml"; printf '[[bindings]]\nhotkey = "Ctrl+Alt+Left"\naction = { type = "center" }\n'; } >"$R/scratch/dup.toml"` and save it with `wzv config "$RUN" "$R/scratch/dup.toml" --expect 'duplicate binding for hotkey alt+ctrl+left; the previous bindings remain active'`.
- **Recover.** Run `wzv config "$RUN" "$R/evidence/config-00-init.toml" --expect 'Config state: Loaded (18 bindings)'`.
- **Add a binding.** Write `{ cat "$R/evidence/config-00-init.toml"; printf '[[bindings]]\nhotkey = "Ctrl+Alt+Shift+Down"\naction = { type = "move-to-zone", zone = "bottom-right" }\n'; } >"$R/scratch/added.toml"`, save it with `wzv config "$RUN" "$R/scratch/added.toml" --expect 'Hotkeys registered: 19' --expect 'Config state: Loaded (19 bindings)'`, then run `wzv send "$RUN" 'dispatch ctrl+alt+shift+down' --expect 'last move: x=960 y=540 w=960 h=540'`.
- **Remove it.** Run `wzv config "$RUN" "$R/evidence/config-00-init.toml" --expect 'Config state: Loaded (18 bindings)'`, then `wzv send "$RUN" 'dispatch ctrl+alt+shift+down' --expect 'no binding configured for hotkey'`.
- **Proof.** Run `wzv stop "$RUN"`. It prints `exit_code=0`, `live_backend_tool_calls=0`, `instance_locks=0`. `"$R/evidence/config-01.toml"` through `config-06.toml` are the saved files in order; the transcript pairs each save with the App's reaction.
- **Cleanup.** Run `wzv cleanup "$RUN"`.

## Gotchas

- The App polls about every 250 ms. Wait for the reaction text (`wzv config --expect`), never a fixed sleep.
- Saving identical content can produce no event; change the file or send `reload` when you need a fresh `Config state:` line.
- Parse errors print a multi-line TOML excerpt between `Config reload error:` and `; the previous bindings remain active`.
- Hotkeys in errors are canonical (`alt+ctrl+left`), not as written in the file.
