# Runtime controls

Users keep the background utility alive through a line-oriented CLI, ANSI dashboard or optional tray, inspect status, reload/restart and close the runtime cleanly.

## Sub-features

- `runtime-cli`: `run` accepts status/help/reload/restart/dispatch and remains responsive.
- `runtime-tui`: the dashboard exposes backend/capabilities/config/bindings/action/error and accepts status/refresh aliases.
- `runtime-exit`: `quit`, `q`, `exit` and input EOF end the session without stranded listeners.
- `runtime-tray`: Linux/Windows tray menu exposes Show status, Reload, Restart and Quit; unsupported surfaces fail visibly.

## How to get to it (user POV)

- Start `scripts/run.sh … run`, or the binary with no command (the binary defaults to run; the runner itself defaults to safe status).
- Start `scripts/run.sh … tui` and type dashboard commands at `window-zones tui> `.
- Start `scripts/run.sh … run --tray` on a supported desktop, then open `Window Zones` and click Show status, Reload, Restart or Quit.
- Type `help`/`?`, `status`, TUI `refresh`, `reload`, `restart`, `dispatch HOTKEY`, a bare hotkey or a quit alias; close stdin for EOF.

## Driving it with verify

Preconditions:

- Baseline/current build. A real tray-host session is additionally required for menu clicks; the synthetic fixtures have none.

- **Maintained matrix first.** `"$V" check sessions` covers CLI/TUI reload/watcher/cancel variants and all four shutdown forms, with fresh-process persistence and unchanged config checks. It does not cover help/bare-hotkey/default-binary/tray entries; use the recipes below for those paths.
- **CLI lifecycle.** `"$V" drive session --mode run` observes the real prompt, sends status/dispatch/reload/restart, waits for resulting state and exits cleanly.
- **TUI lifecycle.** `"$V" drive session --mode tui` observes `Window Zones TUI`, backend/capability labels, refresh and last action; capture the raw ANSI transcript as the actual surface alongside the readable transcript.
- **Help/refresh.** `"$V" drive session --mode run --command help --expect 'Commands:' --command '?' --expect 'Commands:'`; `"$V" drive session --mode tui --command refresh --expect 'Window Zones TUI'`. A TUI help request redraws the dashboard, not CLI help text.
- **Exit/cancel paths.** Repeat both modes with `--quit quit`, `--quit q`, `--quit exit`, and `--quit eof` for changes affecting shutdown. Require `Session closed.` and exit 0, with no active owned PID afterward. Run a status-only custom session to cancel before dispatch.
- **Error/empty.** `"$V" drive session --mode run --command dispatch --expect 'missing hotkey argument'`; TUI `dispatch` redraws without a new dispatch. Use `drive config empty`/`missing` and inspect dashboard `<none>` bindings or runtime count 0, then restore valid. Unbound dispatch must remain visible rather than close the runtime.
- **Persistence.** `"$V" drive session --mode tui --journey reload` verifies restart reads disk and clears prior runtime action; afterward `"$V" drive cli --expect 'binding count: 5' --expect 'last action: <none>' -- status` confirms a new process.
- **Tray flag validation.** `"$V" drive cli --expect-exit 1 --expect '--tray is only valid' -- status --tray` exercises the invalid combination without starting a desktop tray. It does not exercise menu clicks.
- **Native lifecycle.** `"$V" drive smoke x11` / `drive smoke gnome` / `drive smoke kde-live` reuse their existing TUI/runtime lifecycle assertions. Inspect backend/capability diagnostics and each action, not just a banner or growing log.

## Gotchas

- `verify launch` builds/seeds/probes; it intentionally leaves no idle daemon. Each `drive session` owns a fresh PTY and records exact input/result pairs.
- Native GNOME Shell 50/51 starts with the companion pre-enabled in the private fixture; bus startup alone is not usable readiness. The gate waits for the companion before driving CLI/TUI. Version validation remains enabled.
- The dashboard is line-oriented, not a cursor-key widget. Use actual command strings instead of screen coordinates, tab order or internal setters.
- `--tray` is enabled code on Linux/Windows but needs a real tray host/private supported desktop. No existing smoke journey clicks that menu. An affected tray change is blocked until actual menu action/result evidence is captured; status/dispatch simulation is not a replacement. Use the launch route above; Windows-specific prerequisites are in `docs/runbooks/windows-smoke.md`.
- Explicit backend/config flags, discovery, the binary's default run entry, help aliases and quit aliases are separate affected variants. A default lifecycle pass only covers the entry/options it records.
- Desktop state created by native fixtures differs from hardware/seated sessions. Synthetic evidence does not prove login-seat ownership, real monitor hardware, tray host integration or platform-specific lifecycle behavior.
