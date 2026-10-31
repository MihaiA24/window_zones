# Configuration and atomic reload

Users edit TOML bindings/zones, reload without losing the last valid configuration on errors, and restart to read the persisted file again.

## Sub-features

- `config-path`: explicit `--config` and platform config discovery select the startup file.
- `config-replace`: a valid rewrite replaces bindings only after capture accepts it; watcher polling and `reload` expose acceptance.
- `config-retain`: malformed TOML/validation/read failures expose errors while retaining the last valid runtime config.
- `config-pending`: unavailable capture retains the active config and reports a pending replacement; an explicit reload reports its outcome even during an unchanged outage.
- `config-empty`: missing file and empty TOML are distinct states with zero bindings.
- `config-persist`: restart and a new process reload the on-disk file; runtime last action is not durable.
- `config-gnome-animation`: GNOME's native per-extension GSettings key `org.gnome.shell.extensions.window-zones animation-duration-ms` is separate from TOML. It defaults to 150 ms, accepts unsigned 0..1000 ms, and uses 0 for no custom Window Zones animation.

## How to get to it (user POV)

- Edit the discovered config (`$XDG_CONFIG_HOME/window_zones/config.toml` on Linux) or the file passed with `--config`.
- Wait for runtime watcher reload, type `reload` in `run`/`tui`, or choose tray `Reload`.
- Type `restart`, choose tray `Restart`, or relaunch the app to rebuild runtime state from disk.
- For GNOME placement duration, use `gsettings --schemadir "$HOME/.local/share/gnome-shell/extensions/window-zones@mihai-a24/schemas" set org.gnome.shell.extensions.window-zones animation-duration-ms 150`; `reset` instead of `set … 150` restores the schema default. See the [GNOME setting instructions](../../../../docs/runbooks/gnome-wayland.md#placement-animation-duration-gnome-only) for 0/custom values and installation.

## Driving it with verify

Preconditions:

- Baseline profile/current build and valid seed. Only the private config may be edited.

- **Maintained matrix first.** Start with [`check sessions`](../SKILL.md#checks) for the full mode × reload/watcher/cancel × backend-flag/config-entry matrix and fresh-process persistence checks. Use individual recipes below only for uncovered paths or diagnosis.
- **Explicit-path TUI.** `"$V" drive session --mode tui --journey reload` loads five bindings, dispatches Left, edits to one right-half binding, waits for the displayed replacement, writes invalid TOML, waits for an Error while retaining right-half, dispatches the retained binding, then observes empty, missing, recovery and restart.
- **CLI loop.** `"$V" drive session --mode run --journey reload` drives the same sequence with `config state`/`binding count` observations and successful retained dispatch.
- **Watcher entry.** `"$V" drive session --mode run --journey watcher` and `"$V" drive session --mode tui --journey watcher` edit each file state and poll the resulting status without an explicit reload during those transitions.
- **Read-error recovery.** `"$V" drive session --mode run --journey read-error` replaces the private config file with an empty directory, observes Error and retained successful dispatch, restores identical bytes and requires automatic Loaded recovery. It repeats the directory error, removes it without recreating the file, then requires Missing/count 0 and unbound dispatch without `reload`, before restoring valid config and dispatch. Repeat in TUI and with both backend flags and config-entry variants. The journey owns all edits in one PTY command; a concurrent `drive config` is refused by the checkout lock.
- **Cold-start empty/error.** `"$V" drive config missing`, then `"$V" drive cli --discovery --expect 'config state: Missing' --expect 'binding count: 0' -- status`. Repeat with `empty` expecting Loaded/count 0; `invalid` expecting Error/count 0. Restore `drive config valid`.
- **Persistence.** `"$V" drive config changed`, then `"$V" drive cli --expect 'last move: x=960 y=0 w=960 h=1080' -- dispatch Alt+Ctrl+Left` in a new process. Repeat status expecting count 1. This confirms disk contents, not only a live runtime cache. Restore valid afterward.
- **Cancel.** `"$V" drive session --mode tui --command status --expect 'Config:' --quit eof` exits without an edit; compare the recorded config hashes. There is no config-edit dialog to cancel, so cancellation leaves the file unchanged.
- **Native variants.** `"$V" drive smoke x11`, `drive smoke gnome` and `drive smoke kde-live` exercise their existing native retention/recovery paths. Keep the parse error, retained count/binding, successful recovery and new status, not just `failures: 0`.
- **Deferred replacement.** In an isolated native session with unavailable capture, replace valid TOML and request `reload`. Require `Config reload pending`, unchanged active bindings and no dispatch through the unaccepted binding. Restore capture and require the replacement to become active.
- **Stable symlink path.** On Sway/Hyprland, start through an owned config symlink, retarget it to another valid file and reload. New clients using the same pathname must still reach the runtime, including after in-process `restart`; config and dispatch identity must not freeze different targets.
- **Native GNOME setting/live reload.** `drive smoke gnome` owns an isolated keyfile and real compiled extension schemas; it never changes seated settings. Its existing performance TUI resets to 150, sets 0 and the 1000 ms maximum, rejects 1001 while retaining 1000, then resets live to 150. Require the native `gsettings` exit/readback evidence, a real keyboard move after each accepted/rejected setting, independent Mutter/client geometry, completed timing appropriate to that duration and the same App PID throughout. Keep `animation-duration.jsonl`, raw `transitions.jsonl` and category-labelled logs. The private 250 ms baseline is restored before later existing cases; approved global-animation/reduced-motion assertions remain unchanged.

## Gotchas

- The app debounces config changes; the helper polls for specific resulting state instead of sleeping once and trusting a stale Loaded label.
- Watcher polling hashes file content and debounces changes; same-length edits with preserved mtime are detected. Explicit `reload` reads immediately. The parsed config and recorded signature come from the same snapshot, including at startup and restart.
- Invalid reload retains the last good bindings; invalid cold startup has no last-good state and starts empty. Do not weaken one expectation to match the other.
- `restart` reconstructs the app and clears last-dispatch history. Persistence means reading the file again, not retaining an invalid runtime-only fallback forever.
- The reload journey exercises explicit reload; the watcher journey independently waits for file-change transitions without sending reload. Keep their evidence distinct.
- Permission/read-error, duplicate/unknown-zone validation, relative-XDG fallback and explicit-over-discovered precedence need their own real file/environment conditions when affected. The seeds provide TOML parse failure, not every validation/read failure. Missing such preconditions is a failure, not an alternate passing row.
- Tray reload/restart requires a real host: the GNOME synthetic smoke provides AppIndicator menu actions; the baseline CLI/PTY fixtures do not. CLI reload alone is not tray evidence.
- This GNOME-only setting is read on the next move without TOML watcher/reload, App restart or companion reload; a move already in progress keeps its selected duration. Disabled GNOME animations and GNOME 51 reduced motion override nonzero values. Deploy source/schema upgrades with the installer and start a fresh login first; copying source into a running Shell does not activate the new setting.
