# Status and backend selection

Users inspect the resolved backend, loaded configuration, bindings and recent runtime activity before moving windows; the TUI adds capability and diagnostic labels.

## Sub-features

- `status-cli`: one-shot status reports config path/state, count, hotkey state, last action and dispatch state.
- `status-runtime`: interactive status and TUI refresh show the running instance rather than a fresh process.
- `status-routing`: auto/explicit X11 or Wayland routing identifies the real backend; missing companions remain visible without X11 fallback.
- `status-dry-run`: `--backend dry-run` and `--dry-run` provide a safe core-only status path.

## How to get to it (user POV)

- Run `scripts/run.sh` with no arguments (defaults to dry-run status), or explicitly pass `status` and backend/config options.
- Type `status` in `run`; type `status` or `refresh` in `tui`.
- Use `Show status` in the optional tray menu.
- Choose `--backend auto`, `x11`, `wayland`, `dry-run`, or the `--dry-run` alias on Linux. Windows/macOS have their platform-specific backend flags.

## Driving it with verify

Preconditions:

- Baseline profile and current build; `V` is defined in the index.

- **CLI success.** `"$V" drive cli --expect 'config state: Loaded' --expect 'binding count: 5' -- status` reports the private config and five bindings.
- **Flag/discovery variants.** Repeat with `"$V" drive cli --backend-flag alias --discovery --expect 'config state: Loaded' -- status`; also exercise alias/explicit-path and backend/discovery combinations when affected.
- **Runtime views.** `"$V" drive session --mode run --command status --expect 'binding count: 5'` and `"$V" drive session --mode tui --command refresh --expect 'Window Zones TUI'` capture the typed command and returned view. For dispatch state, send dispatch and status in the same session.
- **Empty/error.** Run `"$V" drive config missing`, then `"$V" drive cli --expect 'config state: Missing' --expect 'binding count: 0' -- status`. Repeat with `drive config invalid` and expect `config state: Error`; empty TOML must be `Loaded` with count 0, not Missing. Restore `drive config valid`.
- **Real routing.** `"$V" drive smoke x11` asserts auto resolves X11; `"$V" drive smoke gnome` / `"$V" drive smoke kde-live` cover their own native capabilities/status. Retain the backend/capability logs, not just the gate summary.
- **Persistence/cancel.** Re-run one-shot status after a config mutation and after session quit. Config survives; runtime last action starts at `<none>`. Status is read-only, with no edit/cancel dialog; session quit is its cancellation boundary.

## Gotchas

- `scripts/run.sh` normally prefers a release binary even if debug is newer; the verifier pins the stamped debug binary to avoid stale evidence.
- A new `status` process cannot prove another runtime's last action. Use that runtime's `status` before quitting.
- The helper deliberately removes seated display/session addresses; dry-run cannot establish native backend health or companion absence behavior.
- The GNOME companion declares Shell 50/51 compatibility. Synthetic Shell 51 proof covers companion discovery, capabilities, displays and runtime controls, not seated focus or accelerator capture.
- The no-argument launcher default, tray `Show status`, unavailable-companion routing and platform-specific flags are distinct entry points. Existing synthetic gates do not prove all of them. If affected, record the missing seated/tray/platform precondition and obtain real surface evidence; do not claim a dry-run replacement pass.
- A backend label is selection, not health: X11 status does not connect to X11, while GNOME/KDE status probes companion capabilities. A TUI banner proves redraw, not correct dashboard state; inspect its post-action fields and diagnostics.
