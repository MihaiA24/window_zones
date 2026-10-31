# Runtime controls

Users keep the background utility alive through a line-oriented CLI, ANSI dashboard or optional tray, inspect status, reload/restart and close the runtime cleanly.

## Sub-features

- `runtime-cli`: `run` accepts status/help/reload/restart/dispatch and remains responsive.
- `runtime-tui`: the dashboard exposes backend/capabilities/config/bindings/action/error and accepts status/refresh aliases.
- `runtime-exit`: `quit`, `q`, `exit` and terminal EOF end plain interactive sessions without stranded listeners. Plain non-terminal `run` survives stdin EOF; TUI exits on any EOF. Supported tray mode has no stdin reader and uses menu Quit.
- `runtime-tray`: Linux/Windows tray menu exposes Show status, Reload, Restart and Quit. Linux waits for successful StatusNotifier/AppIndicator registration before announcing readiness; missing/refusing hosts fail startup, and watcher/worker loss ends the runtime with exit 1. Unsupported platforms warn and continue without tray.
- `runtime-init`: `init` writes starter bindings once; `--force` explicitly replaces an existing file.
- `runtime-owner`: native `run`/`tui` own one session lock; a second exits 75. Unix lock/PID/dispatch files live in an owned private directory; stale PID symlinks cannot overwrite their targets.
- `runtime-notify`: plain CLI and tray runtime problems notify on Linux/macOS when stdout is not a terminal and the backend is native. Config problems/refused hotkeys notify immediately; unavailable hotkeys wait twenty seconds without resetting grace for changed diagnostics. Missing config alone is silent; TUI has no notifier.
- `runtime-perf`: opt-in JSONL logging records dispatch outcomes and available native metrics, persists through restart and disables visibly on write failure.
- `runtime-logging`: explicit `--log-file` appends timestamped session/config/hotkey/dispatch events, stays open across restart and closes on normal Quit. File/diagnostic sink errors do not replace operation exit codes.
- `runtime-install`: installer ownership survives reinstall/uninstall, duplicate exit 75, inactive graphical-session targets and KDE launcher crashes without changing user config.

## How to get to it (user POV)

- Start `scripts/run.sh … run`, or the binary with no command (the binary defaults to run; the runner itself defaults to safe status).
- Start `scripts/run.sh … tui` and type dashboard commands at `window-zones tui> `.
- Start `scripts/run.sh … run --tray` on a supported desktop, then open `Window Zones` and click Show status, Reload, Restart or Quit.
- In plain `run`/`tui`, type `help`/`?`, `status`, TUI `refresh`, `reload`, `restart`, `dispatch HOTKEY`, a bare hotkey or a quit alias. Supported tray mode ignores stdin and uses menu controls. TUI always exits on EOF; plain non-terminal `run` needs explicit Quit or process supervision.

## Driving it with verify

Preconditions:

- Baseline/current build. Tray menu proof additionally needs a host; GNOME's synthetic smoke supplies AppIndicator, while the baseline CLI/PTY fixtures do not.

- **Maintained matrix first.** `"$V" check sessions` covers CLI/TUI reload/watcher/cancel variants and all four shutdown forms, with fresh-process persistence and unchanged config checks. It does not cover help/bare-hotkey/default-binary/tray entries; use the recipes below for those paths.
- **CLI lifecycle.** `"$V" drive session --mode run` observes the real prompt, sends status/dispatch/reload/restart, waits for resulting state and exits cleanly.
- **TUI lifecycle.** `"$V" drive session --mode tui` observes `Window Zones TUI`, backend/capability labels, refresh and last action; capture the raw ANSI transcript as the actual surface alongside the readable transcript.
- **Help/refresh.** `"$V" drive session --mode run --command help --expect 'Commands:' --command '?' --expect 'Commands:'`; `"$V" drive session --mode tui --command refresh --expect 'Window Zones TUI'`. A TUI help request redraws the dashboard, not CLI help text.
- **Exit/cancel paths.** Repeat both modes with `--quit quit`, `--quit q`, `--quit exit`, and `--quit eof` for changes affecting shutdown. Require `Session closed.` and exit 0, with no active owned PID afterward. Run a status-only custom session to cancel before dispatch.
- **Error/empty.** `"$V" drive session --mode run --command dispatch --expect 'missing hotkey argument'`; in TUI first use `--command status --expect 'Hotkey state: Registered'`, then `--command dispatch --expect 'Last event: unknown command: missing hotkey argument'`. This is a parser diagnostic, not a dispatch attempt. Use empty/missing fixtures for zero bindings, restore valid, and assert intentional unbound dispatch errors without closing the runtime.
- **Persistence.** `"$V" drive session --mode tui --journey reload` verifies restart reads disk and clears prior runtime action; afterward `"$V" drive cli --expect 'binding count: 5' --expect 'last action: <none>' -- status` confirms a new process.
- **Tray flag validation.** `"$V" drive cli --expect-exit 1 --expect '--tray is only valid' -- status --tray` exercises the invalid combination without starting a desktop tray. It does not exercise menu clicks.
- **Linux tray failure lifecycle.** `drive tests` runs public CLI cases against private D-Bus sessions: missing bus/watcher, refused registration and post-ready watcher/bus loss. Require exit 1 without panic or a false startup announcement; supported tray mode still ignores stdin. Native `drive smoke gnome` checks the first menu status, actual accelerators, live status rows, Reload, Restart and Quit against AppIndicator.
- **Native lifecycle.** `"$V" drive smoke x11` / `drive smoke gnome` / `drive smoke kde-live` reuse their existing TUI/runtime lifecycle assertions. Inspect backend/capability diagnostics and each action, not just a banner or growing log.
- **Initialization/persistence.** `drive cli -- init --chord Ctrl+Alt` must preserve an existing file. `"$V" drive session --mode run --journey starter` proves forced replacement, all eighteen bindings, within-session history and fresh-process persistence, then restores the baseline fixture. Its explicit chord avoids desktop-dependent defaults.
- **Service ownership/safety.** In an isolated native runtime directory, start `run` with closed stdin, then another `run`/`tui`: require exit 75 and the first process remaining usable. Close the first and require a fresh session to acquire ownership. Refuse symlink/non-private runtime directories; verify a stale PID symlink's unrelated target remains unchanged.
- **Notifications/profiling.** On an explicitly controlled desktop, observe problem/recovery notifications through terminal versus non-terminal and tray versus plain run; dry-run must send none. For `--perf-log PATH`, exercise success, failure and restart and inspect actual JSONL rows and PID scope; unwritable startup paths fail before movement. Sway/Hyprland performance logging belongs to the persistent session, not dispatch clients.
- **Session logging.** `"$V" drive cli -- --log-file target/session-events.log status` exercises the flag through the launcher; a later dispatch process must append its outcome. In recorded `run`/`tui` PTYs using the isolated config, dispatch a valid and unbound key, reload, restart, then explicitly Quit. Require one start/end pair, both outcomes, preserved pre-restart records, and the completed terminal result. Keep separate file paths when combining with `--perf-log`.
- **Sink/error boundaries.** With session logging, `/dev/null/window_zones.log` (open failure) and `/dev/full` (write failure) must leave `status` usable. `tests/logging.rs` also routes stderr to `/dev/full`, requires invalid arguments and fatal performance-log startup errors to remain exit 1, and bounds explicit service Quit to avoid EOF hangs.
- **Installer boundaries.** On Linux, `bash scripts/test-install.sh` drives the real installer with isolated Linux/systemd/XDG and macOS/launchd command boundaries; other hosts explicitly skip this Linux fixture. Require target-qualified Cargo artifact selection despite stale unqualified binaries, cross-prefix XDG-to-systemd cutover, stale-lock/crash recovery, duplicate-owner diagnostics, prefix protection and preserved config. Execute the printed bash/zsh/fish PATH suggestion with an isolated HOME and a literal-metacharacter prefix. These fixtures are not a real systemd/launchd or login-session pass.
- **Native profiling.** After a measured GNOME move, attempt next/previous display on a one-display fixture. Its no-op row must have no previous transition timings. Current-action metrics are consumed once; a capability refresh is a live probe, not an action.

## Gotchas

- `verify launch` builds/seeds/probes; it intentionally leaves no idle daemon. Each `drive session` owns a fresh PTY and records exact input/result pairs.
- Native GNOME Shell 50/51 starts with the companion pre-enabled and its first-login welcome tour already acknowledged in the private fixture. The gate still requires actual native focus and input; failed focus retains a compositor screenshot, Overview state and GTK state. No seated settings are changed.
- The dashboard is line-oriented, not a cursor-key widget. Use actual command strings instead of screen coordinates, tab order or internal setters.
- GNOME's synthetic smoke drives the real AppIndicator menu (Show status, Reload, Restart, status row, Quit) and native placement. All items are root-level; no submenu traversal is needed. Other tray hosts and Windows still require their own menu/action evidence; a CLI-only pass is not tray proof.
- Explicit backend/config flags, discovery, the binary's default run entry, help aliases and quit aliases are separate affected variants. A default lifecycle pass only covers the entry/options it records.
- Desktop state created by native fixtures differs from hardware/seated sessions. Synthetic evidence does not prove login-seat ownership, real monitor hardware, tray host integration or platform-specific lifecycle behavior.
- Native smoke uses Linux pidfds/start identities. KWin publishes and registers its identity before exec can make `/proc/<pid>/environ` unreadable; failed handoffs stop startup. Private registries retain discovered identities for inner and outer cleanup, including reparented helpers. `python3 scripts/smoke/test_helpers.py` covers unreadable environments, reused PIDs, foreign-process refusal, signal exit and completed TUI proof.
- A blocked seated companion-dependent phase emits one phase-level `SKIP` with the prerequisite reason. Its failed preflight assertions remain failures; this does not establish coverage of the checks that could not run.
- `verify drive tests` builds in persistent `target/verification-tests`, separate from the pinned App. Test/dev feature unification must not replace its binaries; generated Python bytecode must not invalidate source freshness.
