---
name: verify-window-zones
description: "Drive and prove window_zones (Rust window-zone/hotkey utility) the way a user does: the one-shot CLI (init, status, dispatch), the line-driven `run` session, and the ANSI `tui`, all on owned, isolated `--backend dry-run` instances via the bundled `wzv` helper, with transcripts and side-effect evidence that survive cleanup. Use it whenever you change window_zones behavior and must show it works, reproduce a CLI/session/TUI bug, or check the recorded scenario still passes. Live desktop backends (GNOME, X11, KDE, macOS, Windows, tray) are mapped here but are verified with scripts/smoke.sh on a Linux desktop, not by this helper."
---

# Verify window_zones

window_zones moves and resizes the focused window into zones on hotkeys. Users touch it through the CLI (`init`, `status`, `dispatch HOTKEY`), the `run` session (stdin commands plus global hotkeys) and the `tui` dashboard. This skill drives those surfaces through the repo's own runner, `scripts/run.sh`, with `--backend dry-run`: a built-in fake desktop (focused window `dry-run-window` at x=40 y=40 w=640 h=480, `display-0` 0,0 1920x1080, `display-1` at 1920,0 1280x1024) with no-op hotkey registration. Typed session commands go over a FIFO, the same control path `scripts/smoke.sh` uses for its TUI checks.

Paths below are relative to the repository root. Put the helpers on `PATH` once per shell:

```bash
export PATH="$PWD/.agents/skills/verify-window-zones/scripts:$PATH"
```

## Prerequisites and baseline

- macOS or Linux, `bash` (3.2 is enough), `cargo` (1.95 used for the recording; `cargo build --locked --bin window_zones` must pass), `mkfifo`, `ps -o lstart=`.
- Nothing else: no desktop session, no Accessibility permission, no D-Bus, no network after the first build.
- Record the baseline before you change anything: `git rev-parse HEAD`, `git status --porcelain`, and `./scripts/test.sh` (fmt, tests, clippy, script syntax; it skips `gjs`/`qmllint` steps when those tools are missing). `wzv start` stores HEAD and the local-change count in `meta.env`; `wzv doctor` prints them.
- Never drive an instance you did not start in this run, never the user's installed `window_zones`, and never the `window-zones` systemd user service.

## Launch

```bash
wzv start --run-id my-check            # run session; add --mode tui for the dashboard
```

`start` builds (`cargo build --locked --bin window_zones`, log in `evidence/build.log`), creates `target/verify-window-zones/my-check/`, runs `window_zones init` (the 18 Ctrl+Alt starter bindings; `--no-init` boots with no config), launches `scripts/run.sh --backend dry-run run|tui` reading from a FIFO, and returns once the ready line (`Interactive session started` for `run`, `Window Zones TUI` for `tui`) is printed and startup output settles. It prints:

```text
run_id=my-check
run_dir=<repo>/target/verify-window-zones/my-check
evidence_dir=<repo>/target/verify-window-zones/my-check/evidence
mode=run
pid=<app pid>
config_path=<run_dir>/scratch/home/Library/Application Support/window_zones/config.toml   # Linux: scratch/home/.config/window_zones/config.toml
ready=yes
```

If the app never gets ready, `start` stops what it started, keeps the evidence, prints the last app output and exits 1. Teardown is `wzv stop` then `wzv cleanup` (below).

## Doctor

Run it first whenever anything looks off; it is read-only:

```bash
wzv doctor my-check
```

One `ok`/`FAIL` line per check, then `doctor=ok` (exit 0) or `doctor=fail` (exit 1): app pid alive and still the process this run started (pid + start time), stdin FIFO holder alive, binary checksum unchanged since `start` (rebuilt? start a new run), ready line seen, `Window backend: dry-run`, zero live-backend program calls, zero instance lock files, then `info source:` with the starting HEAD and current HEAD.

## Drive

| Command | Does | Passes when |
|---|---|---|
| `wzv send RUN LINE [--expect T]... [--timeout S]` | types one session line: `status`, `reload`, `restart`, `help`, `dispatch HOTKEY`, a bare hotkey, `q`/`quit`/`exit`; TUI also `refresh` | every `T` appears in the output this line produced (default 8 s); prints that output, ANSI stripped |
| `wzv config RUN FILE [--expect T]...` | saves FILE over the app's config with write+rename (as editors do), snapshots it to `evidence/config-NN.toml` | every `T` appears in the app output after the save (the app polls every 250 ms) |
| `wzv exec RUN [--expect-exit N] [--expect T]... -- ARGS` | runs one-shot `window_zones --backend dry-run ARGS` (`status`, `dispatch HOTKEY`, `init [--chord M] [--force]`, `--config PATH ...`) in the run's isolated environment; prints stdout, stderr, `exit_code=N`. Refuses `--backend`; `--config PATH` (relative to the current directory) must be inside the run directory | exit code is N (without `--expect-exit`, `exec` exits with the app's code) and every `T` is in stdout/stderr |
| `wzv stop RUN` | types `quit` (or notices the app already quit), stops the FIFO holder, writes `evidence/side-effects.txt` | app exit code 0; prints `exit_code= live_backend_tool_calls= instance_locks= home_files= side_effects= evidence_dir=` |

Hotkeys are case-insensitive on input (`dispatch Ctrl+Alt+Left` and `dispatch ctrl+alt+left` are the same); the app reports them canonically (`alt+ctrl+shift+cmd+key`). In `run`, a line that is not a session command is dispatched as a hotkey. Each recipe in `features/` uses `--expect` with the exact text the app prints, so a recipe command exiting 0 is the check. Use `"$R/scratch/"` (with `R=target/verify-window-zones/RUN`, from the repository root) for fixture files; cleanup removes it.

## Evidence and proof standards

Everything lives in `target/verify-window-zones/<run-id>/` (override the root with `WZV_ROOT`; `cargo clean` deletes `target/`, so copy evidence elsewhere if it must outlive that):

| Path | Content |
|---|---|
| `meta.env`, `pids` | run id, mode, binary path and checksum, git HEAD and change count at start, config path; `role pid start-time` for `wrapper`, `holder`, `app` |
| `evidence/transcript.log` | every typed line (`> line`), the output it caused, one-shot commands with stdout/stderr/`exit_code=`, config saves, `[expect ok]`/`[expect FAIL]` markers |
| `evidence/app.log` | raw app stdout+stderr including ANSI (TUI frames) |
| `evidence/config-NN.toml` | `00-init` is the starter config; later numbers are each `wzv config` save |
| `evidence/side-effects.txt` | files left in the run HOME/TMPDIR/XDG_RUNTIME_DIR, live-backend programs invoked, app exit code |
| `evidence/sentinel.log` | one line per live-backend program call (must stay empty) |
| `evidence/exit-code`, `evidence/build.log` | app exit status; cargo output |

Proof standards:

- Drive the user path: session lines, saved config files and CLI arguments, never internal setters or unit-test hooks. Capture the action and the resulting state (`last move: x= y= w= h=`, `Config state:`, TUI `Last action:`/`Last error:` lines), not only the final screen.
- Check side effects next to visible output: `wzv stop` must report `live_backend_tool_calls=0`, `instance_locks=0`, and `home_files=` equal to the files you expect (1 after `init`: the config).
- Dry-run is verified, not trusted: the instance runs under `env -i` with its own HOME, `XDG_*`, `TMPDIR`, `XDG_RUNTIME_DIR`, a dead `DBUS_SESSION_BUS_ADDRESS`, and `PATH=<scratch>/bin:/usr/bin:/bin`, where `osascript swaymsg hyprctl gdbus dbus-daemon dbus-launch dbus-send busctl notify-send xdotool kpackagetool6 qdbus` are shims that log to `sentinel.log` and exit 127. Positive control, observed on macOS: the same environment with `--backend macos` logged an `osascript -l JavaScript` call there. Dry-run also skips the single-instance lock, so parallel runs are safe.
- Dry-run proves the runtime: config handling, bindings, zone math, cycling, restore, display moves, session/TUI commands, exit codes. It does not prove that a real compositor moved a real window or that a real key press was captured; never report a live backend as verified from a dry-run.
- With every claim, report the run id, the commands, and the evidence path.

## Cleanup

```bash
wzv cleanup --dry-run my-check   # plan only: would-kill / would-remove lines, nothing changes
wzv cleanup my-check             # kill what is still running, remove scratch/, keep evidence
```

`cleanup` acts only on the pids in `<run>/pids`, and only when the pid's current start time equals the recorded one (`skip-not-owned` otherwise: a reused pid is never signalled). It removes `scratch/` (HOME, TMPDIR, FIFO, fixtures) and prints `kept <evidence_dir>`; it never deletes `evidence/`, `meta.env` or `pids`. A TUI exits on its own when stdin closes; `run` keeps serving without a terminal and gets SIGTERM. Run it after every failed attempt too. `wzv cleanup --all` covers every run under the root, including other agents' live runs in this checkout: run `wzv cleanup --all --dry-run` first. Delete old evidence yourself with `rm -rf target/verify-window-zones/<run-id>` once nobody needs it.

## Helpers

- `.agents/skills/verify-window-zones/scripts/wzv` (`wzv --help`): `start`, `doctor`, `send`, `config`, `exec`, `stop`, `cleanup` as above. Exit 0 success, 1 failed check or runtime error (message on stderr names the missing text or the dead process), 2 usage error.
- `.agents/skills/verify-window-zones/scripts/scenario.sh` (`scenario.sh --help`): replays the recorded scenario and the failure/cleanup drill below.

## Recorded scenario

Starting state: fresh empty run HOME (no config), `--backend dry-run`, focused window at x=40 y=40 w=640 h=480 on `display-0`; recorded on macOS 27.0 arm64 at commit `b0f37cd` with a clean tree plus this skill's files. Replay:

```bash
scenario.sh --run-id replay-$(date +%s)
```

It prints `PASS`/`FAIL` per step, ends with `RESULT: PASS` (exit 0) or `RESULT: FAIL` (exit 1, after cleaning up the runs it started) and tees everything to `target/verify-window-zones/<prefix>.log`. The same steps by hand:

```bash
RUN=s1-$(date +%s); R=target/verify-window-zones/$RUN
wzv start --run-id "$RUN"                                       # init writes 18 Ctrl+Alt bindings
wzv doctor "$RUN"                                               # doctor=ok
wzv send "$RUN" status --expect 'binding count: 18'
wzv send "$RUN" 'dispatch ctrl+alt+left' --expect 'last move: x=0 y=0 w=960 h=1080'
wzv send "$RUN" 'dispatch ctrl+alt+left' --expect 'last move: x=0 y=0 w=1280 h=1080'
wzv send "$RUN" 'dispatch ctrl+alt+left' --expect 'last move: x=0 y=0 w=640 h=1080'
wzv send "$RUN" 'dispatch ctrl+alt+backspace' --expect 'last move: x=40 y=40 w=640 h=480'
wzv send "$RUN" 'dispatch ctrl+alt+shift+right' --expect 'last move: x=1947 y=38 w=427 h=455'
{ cat "$R/evidence/config-00-init.toml"; echo 'this is not valid toml'; } >"$R/scratch/broken.toml"
wzv config "$RUN" "$R/scratch/broken.toml" --expect 'Config reload error' --expect 'the previous bindings remain active'
wzv send "$RUN" 'dispatch ctrl+alt+right' --expect 'last move: x=2560 y=0 w=640 h=1024'
wzv config "$RUN" "$R/evidence/config-00-init.toml" --expect 'Config state: Loaded (18 bindings)'
wzv exec "$RUN" --expect-exit 1 --expect 'no binding configured for hotkey' -- dispatch ctrl+alt+f9
wzv stop "$RUN"                     # exit_code=0 live_backend_tool_calls=0 instance_locks=0 home_files=1
wzv cleanup --dry-run "$RUN"        # would-remove .../scratch; nothing changes
wzv cleanup "$RUN"                  # removed .../scratch; kept .../evidence
```

Recorded result, run `replay-3` (2026-10-08, commit `b0f37cd`): 21/21 steps `PASS`, `RESULT: PASS`, every recorded pid gone afterwards; evidence in `target/verify-window-zones/replay-3-s1/evidence/` and `replay-3-failed/evidence/`, log `target/verify-window-zones/replay-3.log`.

## Failure drill and cleanup safety

`scenario.sh` also proves, in run `<prefix>-failed` (TUI mode):

- `cleanup --dry-run` on a live run prints `would-kill` for holder, app and wrapper, and afterwards the app pid is alive, `scratch/` exists and `doctor` still passes.
- A wrong expectation (`--expect 'Last action: alt+ctrl+up'` after dispatching right) exits 1 with `expected text not seen within 2s: ...`, prints the output it did see, and leaves `[expect FAIL]` in the transcript.
- `cleanup` after that failure kills only the recorded holder, the app exits, `scratch/` is gone, `app.log`/`exit-code`/transcript remain; a second `cleanup` prints `already-removed` and kills nothing.
- A run record whose pid now belongs to an unrelated process (start time differs) gets `skip-not-owned`, and that process stays alive.

Checked by hand, not by `scenario.sh`: usage errors exit 2 (`wzv send RUN` without a line, `wzv exec RUN -- --backend macos status`, `wzv frobnicate`, `scenario.sh --bogus`), and `wzv exec` rejects `--config` paths containing `..` or outside the run directory with exit 2.

A scenario step that fails makes `scenario.sh` print `FAIL <step>` and `RESULT: FAIL`, clean up every run it started, and exit 1. Observed with a deliberately wrong expectation in a throwaway copy (run `faildrill-1`): app and holder killed, `scratch/` removed, evidence kept with the `[expect FAIL]` line.

## Coverage

| Path | Status | Where |
|---|---|---|
| Starter config: `init`, `--chord`, `--force`, existing-config refusal, invalid chord, `--config` | exercised (dry-run), run `map-starter-config-005616` | `features/starter-config.md` |
| One-shot `status`/`dispatch`, no carry-over, exit codes, missing config, argument errors, `--help` | exercised (dry-run), run `map-one-shot-commands-005643` | `features/one-shot-commands.md` |
| Zones, half cycling, center, maximize, restore, next/previous display with wrap, custom zones, `move-to-display` | exercised (dry-run), run `map-move-window-005616` | `features/move-window.md` |
| `run` session commands: `status`, `help`, `reload`, `restart`, bare hotkey, unknown hotkey, `exit` | exercised (dry-run), run `map-run-session-005616` | `features/run-session.md` |
| Live config reload: parse error, validation errors, recovery, added and removed binding | exercised (dry-run), run `map-config-reload-005616` | `features/config-reload.md` |
| TUI dashboard: frame, dispatch, error line, `refresh`, `reload`, `restart`, config error, quit | exercised (dry-run), run `map-tui-005616` | `features/tui.md` |
| `wzv` on Linux | unverified: written for GNU and BSD userland, run only on macOS | this file |
| `start` readiness-timeout cleanup branch | unverified: dry-run is always ready in under a second | `wzv` |
| GNOME Wayland, Linux X11, KDE Plasma Wayland gates | blocked here (needs a Linux desktop): `./scripts/smoke.sh gnome`, `gnome-live`, `x11`, `kde-live` | `features/live-integrations.md` |
| macOS live backend, Windows, Sway/Hyprland, tray, desktop notifications, single-instance lock (status 75), systemd service | blocked here (real windows, other OS, or the user's session) | `features/live-integrations.md` |

## Isolation

Each run has its own HOME, XDG dirs, TMPDIR, XDG_RUNTIME_DIR and FIFO, and dry-run takes no instance lock, so any number of runs can go side by side; give each a unique `--run-id` (`start` refuses an existing one). `exec` refuses `--backend` and any `--config` that resolves outside the run directory. Only `cleanup --all` touches other runs.

## Loading this skill

- OMP (primary): discovered from `.agents/skills/` in this repo when a session starts (a session that was already running before the skill existed does not see it; start a new one). Read it as `skill://verify-window-zones`, feature files as `skill://verify-window-zones/features/<file>.md`. Observed: a fresh `omp -p` session in this repo read `skill://verify-window-zones` and got `name: verify-window-zones`.
- Hermes (secondary): it only discovers this repo's `.agents/skills/` after a human runs `hermes skills trust` for the directory. Agents must not run that themselves; until it is done, load the file explicitly with `skill_view` on `.agents/skills/verify-window-zones/SKILL.md` (and the feature files the same way).

## Keeping it honest

`features/README.md` is the maintained map. When a feature, command, output line or default changes, update its feature file and rerun its recipe; mark anything not rerun `unverified`. `/maintain-verification-skill` does that upkeep.
