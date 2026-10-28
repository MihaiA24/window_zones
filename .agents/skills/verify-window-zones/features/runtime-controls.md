# Runtime controls

Users keep the background utility alive through a line-oriented CLI, ANSI dashboard or optional tray, inspect status, reload/restart and close the runtime cleanly.

## Sub-features

- `runtime-cli`: `run` accepts status/help/reload/restart/dispatch and remains responsive.
- `runtime-tui`: the dashboard exposes backend/capabilities/config/bindings/action/error and accepts status/refresh aliases.
- `runtime-exit`: `quit`, `q`, `exit` and terminal EOF end interactive sessions without stranded listeners; background `run` stays alive when non-terminal stdin closes.
- `runtime-tray`: Linux/Windows tray menu exposes Show status, Reload, Restart and Quit; unsupported surfaces fail visibly.
- `runtime-init`: `init` writes starter bindings once; `--force` explicitly replaces an existing file.
- `runtime-owner`: native `run`/`tui` own one session lock; a second exits 75. Unix lock/PID/dispatch files live in an owned private directory; stale PID symlinks cannot overwrite their targets.
- `runtime-notify`: non-terminal Linux/macOS problems notify after the documented grace period; terminal and dry-run sessions do not notify, including tray mode. Changing outage diagnostics does not restart the grace period.
- `runtime-perf`: opt-in JSONL logging records dispatch outcomes and available native metrics, persists through restart and disables visibly on write failure.
- `runtime-logging`: explicit `--log-file` appends timestamped session/config/hotkey/dispatch events, stays open across restart and closes on normal Quit. File/diagnostic sink errors do not replace operation exit codes.

## How to get to it (user POV)

- Start `scripts/run.sh … run`, or the binary with no command (the binary defaults to run; the runner itself defaults to safe status).
- Start `scripts/run.sh … tui` and type dashboard commands at `window-zones tui> `.
- Start `scripts/run.sh … run --tray` on a supported desktop, then open `Window Zones` and click Show status, Reload, Restart or Quit.
- Type `help`/`?`, `status`, TUI `refresh`, `reload`, `restart`, `dispatch HOTKEY`, a bare hotkey or a quit alias. Terminal EOF ends interactive sessions; service-mode shutdown needs explicit Quit or process supervision.

## Driving it with verify

Preconditions:

- Baseline/current build. Tray menu proof additionally needs a host; GNOME's synthetic smoke supplies AppIndicator, while the baseline CLI/PTY fixtures do not.

- **Maintained matrix first.** `"$V" check sessions` covers CLI/TUI reload/watcher/cancel variants and all four shutdown forms, with fresh-process persistence and unchanged config checks. It does not cover help/bare-hotkey/default-binary/tray entries; use the recipes below for those paths.
- **CLI lifecycle.** `"$V" drive session --mode run` observes the real prompt, sends status/dispatch/reload/restart, waits for resulting state and exits cleanly.
- **TUI lifecycle.** `"$V" drive session --mode tui` observes `Window Zones TUI`, backend/capability labels, refresh and last action; capture the raw ANSI transcript as the actual surface alongside the readable transcript.
- **Help/refresh.** `"$V" drive session --mode run --command help --expect 'Commands:' --command '?' --expect 'Commands:'`; `"$V" drive session --mode tui --command refresh --expect 'Window Zones TUI'`. A TUI help request redraws the dashboard, not CLI help text.
- **Exit/cancel paths.** Repeat both modes with `--quit quit`, `--quit q`, `--quit exit`, and `--quit eof` for changes affecting shutdown. Require `Session closed.` and exit 0, with no active owned PID afterward. Run a status-only custom session to cancel before dispatch.
- **Error/empty.** `"$V" drive session --mode run --command dispatch --expect 'missing hotkey argument'`; TUI `dispatch` redraws without a new dispatch. Use `drive config empty`/`missing` and inspect dashboard `<none>` bindings or runtime count 0, then restore valid. Unbound dispatch must remain visible rather than close the runtime.
- **Persistence.** `"$V" drive session --mode tui --journey reload` verifies restart reads disk and clears prior runtime action; afterward `"$V" drive cli --expect 'binding count: 5' --expect 'last action: <none>' -- status` confirms a new process.
- **Tray flag validation.** `"$V" drive cli --expect-exit 1 --expect '--tray is only valid' -- status --tray` exercises the invalid combination without starting a desktop tray. It does not exercise menu clicks.
- **Native lifecycle.** `"$V" drive smoke x11` / `drive smoke gnome` / `drive smoke kde-live` reuse their existing TUI/runtime lifecycle assertions. Inspect backend/capability diagnostics and each action, not just a banner or growing log.
- **Initialization/persistence.** `drive cli -- init --chord Ctrl+Alt` must preserve an existing file; `--force` replaces it with the starter config. Check a new status process and restore `drive config valid`.
- **Service ownership/safety.** In an isolated native runtime directory, start `run` with closed stdin, then another `run`/`tui`: require exit 75 and the first process remaining usable. Close the first and require a fresh session to acquire ownership. Refuse symlink/non-private runtime directories; verify a stale PID symlink's unrelated target remains unchanged.
- **Notifications/profiling.** On an explicitly controlled desktop, observe problem/recovery notifications through terminal versus non-terminal and tray versus plain run; dry-run must send none. For `--perf-log PATH`, exercise success, failure and restart and inspect actual JSONL rows and PID scope; unwritable startup paths fail before movement. Sway/Hyprland performance logging belongs to the persistent session, not dispatch clients.
- **Session logging.** `"$V" drive cli -- --log-file target/session-events.log status` exercises the flag through the launcher; a later dispatch process must append its outcome. In recorded `run`/`tui` PTYs using the isolated config, dispatch a valid and unbound key, reload, restart, then explicitly Quit. Require one start/end pair, both outcomes, preserved pre-restart records, and the completed terminal result. Keep separate file paths when combining with `--perf-log`.
- **Sink/error boundaries.** With session logging, `/dev/null/window_zones.log` (open failure) and `/dev/full` (write failure) must leave `status` usable. `tests/logging.rs` also routes stderr to `/dev/full`, requires invalid arguments and fatal performance-log startup errors to remain exit 1, and bounds explicit service Quit to avoid EOF hangs.

## Gotchas

- `verify launch` builds/seeds/probes; it intentionally leaves no idle daemon. Each `drive session` owns a fresh PTY and records exact input/result pairs.
- Native GNOME Shell 50/51 starts with the companion pre-enabled in the private fixture; bus startup alone is not usable readiness. The gate waits for the companion before driving CLI/TUI. Version validation remains enabled.
- The dashboard is line-oriented, not a cursor-key widget. Use actual command strings instead of screen coordinates, tab order or internal setters.
- GNOME's synthetic smoke drives the real AppIndicator menu (Show status, Reload, Restart, status row, Quit) and native placement. Other tray hosts and Windows still require their own menu/action evidence; a CLI-only pass is not tray proof.
- Explicit backend/config flags, discovery, the binary's default run entry, help aliases and quit aliases are separate affected variants. A default lifecycle pass only covers the entry/options it records.
- Desktop state created by native fixtures differs from hardware/seated sessions. Synthetic evidence does not prove login-seat ownership, real monitor hardware, tray host integration or platform-specific lifecycle behavior.
