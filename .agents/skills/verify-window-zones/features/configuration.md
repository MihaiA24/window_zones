# Configuration and atomic reload

Users edit TOML bindings/zones, reload without losing the last valid configuration on errors, and restart to read the persisted file again.

## Sub-features

- `config-path`: explicit `--config` and platform config discovery select the startup file.
- `config-replace`: a valid rewrite replaces bindings; watcher polling and `reload` both expose the new state.
- `config-retain`: malformed TOML/validation/read failures expose errors while retaining the last valid runtime config.
- `config-empty`: missing file and empty TOML are distinct states with zero bindings.
- `config-persist`: restart and a new process reload the on-disk file; runtime last action is not durable.

## How to get to it (user POV)

- Edit the discovered config (`$XDG_CONFIG_HOME/window_zones/config.toml` on Linux) or the file passed with `--config`.
- Wait for runtime watcher reload, type `reload` in `run`/`tui`, or choose tray `Reload`.
- Type `restart`, choose tray `Restart`, or relaunch the app to rebuild runtime state from disk.

## Driving it with verify

Preconditions:

- Baseline profile/current build and valid seed. Only the private config may be edited.

- **Maintained matrix first.** `"$V" check sessions` runs both modes, both reload/watch journeys and cancel through all backend-flag/config-entry combinations, checks in-process action reset and unchanged final config content, then compares a fresh-process dispatch against the fixed oracle. Use the individual recipes below only for uncovered paths or diagnosis.
- **Explicit-path TUI.** `"$V" drive session --mode tui --journey reload` loads five bindings, dispatches Left, edits to one right-half binding, waits for the displayed replacement, writes invalid TOML, waits for an Error while retaining right-half, dispatches the retained binding, then observes empty, missing, recovery and restart.
- **CLI loop.** `"$V" drive session --mode run --journey reload` drives the same sequence with `config state`/`binding count` observations and successful retained dispatch.
- **Watcher entry.** `"$V" drive session --mode run --journey watcher` and `"$V" drive session --mode tui --journey watcher` edit each file state and poll the resulting status without an explicit reload during those transitions.
- **Discovery/flag matrix.** Run both recipes with each combination of default flags, `--discovery`, `--backend-flag alias`, and `--backend-flag alias --discovery` when affected. These are eight distinct mode × config-entry × backend-flag journeys; one is not evidence for another.
- **Cold-start empty/error.** `"$V" drive config missing`, then `"$V" drive cli --discovery --expect 'config state: Missing' --expect 'binding count: 0' -- status`. Repeat with `empty` expecting Loaded/count 0; `invalid` expecting Error/count 0. Restore `drive config valid`.
- **Persistence.** `"$V" drive config changed`, then `"$V" drive cli --expect 'last move: x=960 y=0 w=960 h=1080' -- dispatch Alt+Ctrl+Left` in a new process. Repeat status expecting count 1. This confirms disk contents, not only a live runtime cache. Restore valid afterward.
- **Cancel.** `"$V" drive session --mode tui --command status --expect 'Config:' --quit eof` exits without an edit; compare the recorded config hashes. There is no config-edit dialog to cancel, so cancellation leaves the file unchanged.
- **Native variants.** `"$V" drive smoke x11`, `drive smoke gnome` and `drive smoke kde-live` exercise their existing native retention/recovery paths. Keep the parse error, retained count/binding, successful recovery and new status, not just `failures: 0`.

## Gotchas

- The app debounces config changes; the helper polls for specific resulting state instead of sleeping once and trusting a stale Loaded label.
- Watcher polling hashes file content and debounces changes; same-length edits with preserved mtime are detected. Explicit `reload` reads immediately. The parsed config and recorded signature come from the same snapshot, including at startup and restart.
- Invalid reload retains the last good bindings; invalid cold startup has no last-good state and starts empty. Do not weaken one expectation to match the other.
- `restart` reconstructs the app and clears last-dispatch history. Persistence means reading the file again, not retaining an invalid runtime-only fallback forever.
- The reload journey exercises explicit reload; the watcher journey independently waits for file-change transitions without sending reload. Keep their evidence distinct.
- Permission/read-error, duplicate/unknown-zone validation, relative-XDG fallback and explicit-over-discovered precedence need their own real file/environment conditions when affected. The seeds provide TOML parse failure, not every validation/read failure. Missing such preconditions is a failure, not an alternate passing row.
- Tray reload/restart requires a real host: the GNOME synthetic smoke provides AppIndicator menu actions; the baseline CLI/PTY fixtures do not. CLI reload alone is not tray evidence.
