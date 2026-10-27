---
name: verify-window-zones
description: Drives Window Zones' Linux CLI, ANSI TUI and synthetic desktop window-placement paths with checkout-owned profiles and preserved evidence. Use when verifying runtime controls, config reload, hotkeys, backend selection, window geometry or changes to the verifier.
---

# Verify Window Zones

Read [the feature index](features/README.md) before choosing journeys. The primary surface is a background desktop window-positioning utility; CLI, ANSI TUI and optional tray are controls, not a web app. This helper runs on Linux; Windows/macOS require their actual desktops, not a Linux pass.

## Launch

From the checkout root:

```sh
V=.agents/skills/verify-window-zones/verify
"$V" launch --dry-run
"$V" launch
"$V" doctor
```

`launch` uses `cargo build --locked --bins` (debug), pins `WINDOW_ZONES_BINARY` for `scripts/run.sh`, creates empty private HOME/XDG directories, seeds five documented bindings, then runs real `status`. Ready requires `config state: Loaded` and `binding count: 5`, followed by `READY checkout-…`. No auth, database, migrations or external seed service exists.

This is not a daemon launcher: each CLI command exits; each `drive session` starts and closes its own PTY. No idle app is left to attach to. Requirements: Python 3, Cargo/Rust and the native build dependencies in `docs/runbooks/installation.md`. Native gate prerequisites are checked before starting; see `drive smoke --help` and `docs/runbooks/testing.md`.

The physical checkout path hashes to a profile, X11 display number and private `/tmp/wz-<checkout-key>/` scratch/data directory, keeping native Unix-socket paths short regardless of checkout depth. Evidence/build metadata remain under `target/verification/<checkout-key>/`. Non-private, foreign-owned or symlinked profiles are refused. No TCP port exists: Xvfb has `-nolisten tcp`. `doctor` reports the actual X socket/lock owner PID, command, cwd and checkout marker. Different checkouts get separate profiles/displays/private D-Bus sessions; display collisions fail rather than attach. One mutating command per checkout. The wrapper excludes seated `gnome-live`, which attaches to the shared user desktop.

Teardown: `"$V" cleanup --dry-run`, then `"$V" cleanup`. An existing profile requires cleanup before relaunch; evidence is retained.

## Doctor

`"$V" doctor` is read-only. Require exit 0, the expected checkout/profile/config, `build: current`, and no foreign display owner. `active_process: null` between short-lived drives is normal. During a drive it reports the recorded PID/start identity. Never borrow a live instance or copy another checkout's owner/build metadata.

Direct-child ownership uses the stable `/proc` PID/start identity, not cwd/environment that can disappear during exit. Already-reaped commands retain their true output and exit status; a live child with unreadable stable identity fails and is stopped/reaped.

Freshness compares content hashes of Rust/Cargo inputs, compositor code, scripts, the `verify` launcher and every `wz_verify/**/*.py` module, fixed-clock C source and default expected-result JSON, the Rust toolchain/build environment, and both binary hashes. Adding, changing or removing an implementation module invalidates old proof; bytecode caches are not inputs. A stale build is **not evidence**. Run cleanup, launch and the affected journeys again after source changes. Doctor does not rebuild to hide staleness. Each drive/check validates freshness before and after; a mid-run change invalidates the proof.

## Drive

```sh
"$V" drive cli --expect 'config state: Loaded' -- status
"$V" drive cli --expect 'last move: x=0 y=0 w=960 h=1080' -- dispatch Alt+Ctrl+Left
"$V" drive session --mode tui --journey reload
"$V" drive session --mode run --backend-flag alias --discovery --journey reload --quit eof
"$V" drive config custom
"$V" drive cli --expect 'last move: x=192 y=108 w=960 h=540' -- dispatch Alt+Ctrl+Left
"$V" drive config valid
"$V" drive smoke x11
"$V" drive tests
```

`drive cli` goes through the repo launcher. `--backend-flag alias` exercises the app's `--dry-run`; the verifier's own `--dry-run` plans without running anything. `--discovery` omits `--config` and uses the isolated XDG config. `drive config --help` lists fixtures and all eight built-in zone variants. CLI error states can exit 0: assert their output as well as exit code.

`drive session` types real line input into a fresh PTY. Built-in journeys use `status`, TUI `refresh`, `dispatch`, `reload`, `restart` and an explicit quit alias or EOF. The reload journey edits the user's TOML, observes valid replacement, invalid-config retention, empty/missing state, recovery and restart; `--journey watcher` performs those file transitions without explicit reload. For other paths use paired `--command` / `--expect`; see `drive session --help`. It waits on post-action prompts and state, with deadlines, not fixed settling sleeps.

`drive smoke` delegates to `scripts/smoke.sh --keep-logs`, not a second compositor test framework. It selects checkout-derived X11 display/private state and copies proof files out before cleanup. GNOME Shell 50/51 starts with its companion pre-enabled in private dconf and version validation enabled; readiness requires the companion/capabilities, not just a bus-address file. `gnome` is seatless. `kde-live` uses KWin's private `--virtual` framebuffer and requires no outer display, despite its name. Existing native gates contain legacy pacing pauses; those are not readiness or proof. Require their observable geometry/config/exit assertions, inspect SKIPs and capture failures. A screenshot alone or elapsed time cannot satisfy an end state. See the map for paths those gates cannot cover.

## Checks

```sh
"$V" check behaviors
"$V" check sessions
"$V" check e2e
"$V" check performance --baseline-ref HEAD --runs 5 --max-regression-percent 25
```

Checks print JSON with `status`, structured `difference` paths and `evidence_path`; exit 0 means pass, nonzero means mismatch or an unmet precondition. Full reports live in the launched run's `checks/<scenario-id>/report.json`, alongside raw outputs; pre-launch/ownership errors go under `target/verification/<checkout-key>/check-errors/`. `--dry-run` prints a JSON plan without side effects. `--expected-file` accepts a reviewed fixed oracle and records its SHA-256; never generate/bless an oracle from the output under test.

Prefer these maintained checks over agent-written loops or drive scripts. Run `behaviors` for CLI/config/geometry changes, `sessions` for interactive/config/lifecycle changes, and `e2e` for native X11 movement/hotkeys; a whole-map pass needs all three plus the other applicable native gates. Run serially. Do not duplicate `e2e` with `drive smoke x11`. Use individual `drive` commands only for a changed path the fixed checks do not cover or to diagnose a failed case. Never retry until green, bless actual output, or replace a failed native assertion with a dry-run pass.

Fixtures are literal TOML/hotkeys/geometry, seed 0, UTC and epoch `1704067200`. A C compiler builds `fixed_clock.c` into scratch; wall time is frozen only in `window_zones` and the separately compiled `clock-probe`. Shells, desktop services and other tools retain real wall time, verified by the safety regression. Monotonic clocks stay real for debounce, readiness deadlines and measurement; freezing harness clocks would stall native waits. Verifier fixture mtimes are fixed; the native gate retains kernel mtime transitions for its watcher. Artifact timestamps/PIDs/display IDs are evidence metadata, not golden results.

`behaviors` compares all eight built-ins, custom geometry, both display directions, unbound dispatch and empty/missing/invalid config: 15 cases × both backend flag forms × explicit/discovered config. It uses `scripts/run.sh`, not internal setters. `e2e` reuses the real X11 smoke runner and compares independent geometry, real accelerator, reload retention/recovery, captures, quit, a new process reading persisted config, and a separate cancel-before-edit TUI path. It remains synthetic X11 evidence, not seated/tray/other-platform coverage.

`sessions` runs a fixed 24-session matrix: CLI/TUI × both backend flag forms × explicit/discovered config × reload/watcher/cancel. All four quit forms occur in both modes. It reuses the PTY journeys, health-checks before each session, asserts restart clears action history, checks completed TUI dispatch responses for errors, compares config content before/after, and verifies persisted configuration through a new process against the existing dispatch oracle. Reports name each variant; PTY transcripts and input/result events remain the evidence. Terminal local echo is disabled; output assertions consume the post-result prompt, not just a matching input or a previous prompt.

`performance` resolves a local baseline ref to its exact commit, archives it without checkout/ref changes, and builds its binary separately using the same debug Cargo profile and toolchain. Missing cached baseline dependencies fail offline. After one confirmed warmup per side, at least five measured runs **per side alternate baseline/candidate**. Every sample must actually load five bindings and produce the expected successful left-half dispatch. The JSON reports raw samples, work confirmation, median/min/max and spread = max−min; the console omits raw samples but links the full report. The default gate permits a 25% median regression, configurable explicitly. Compilation, warmups and result comparison are excluded; runner/process/capture/ownership overhead is included. Do not run tests or other benchmarks concurrently. This measures debug CLI startup+dispatch, not native hotkey/compositor latency.

Failures retain proof and remove the owned profile; fix the reported cause, then launch again. Successful checks restore the valid fixture. Run the repository's `drive tests` as the supplemental correctness gate, not as a substitute for these actual user paths.

## Evidence

- Build a coverage matrix from the map: **every entry point, mode and enabled flag variant the change can affect**, each through **success, cancel, error, empty and persistence** paths. Include CLI `status`/`dispatch`, runtime `run`/`tui`/tray, config discovery/explicit path, backend auto/explicit/dry-run alias and quit aliases when affected. Record genuinely inapplicable outcomes with a reason; record unmet preconditions as failures, never a weaker passing journey.
- Drive real user paths. No internal setters, injected dispatch events or test-only endpoints. If eval is needed, use it **only to read resulting state after the user path ran**. Existing tests/fake compositor services supplement, never replace, real surface proof. Mocks belong only at an already-isolated production boundary.
- Capture the action **and** resulting state: `actions.jsonl`, CLI command/stdout/stderr/exit, raw `.ansi` and readable `.txt` PTY transcripts, native assertion logs/screenshots and independently observed exact geometry. Check durable side effects through a second status/process and file hashes/content, not just a success message. The app persists config files, not last-dispatch history; prove that distinction on restart.
- Dry-run is a scope, not a guarantee. The app's dry-run replaces window movement but still constructs a global-hotkey listener in interactive modes. The helper removes seated display/bus addresses and uses private HOME/XDG; per-drive file hashes expose writes. For a claim about no window/network/browser/git effects, observe those boundaries before/after (or trace them); a dry-run label, tests or missing address alone does not prove absence. Native geometry proof requires a native gate.
- `logs --path-only` prints the run directory: `target/verification/<checkout-key>/evidence/<run-id>/`. `launch.json` binds proof to build hashes. Failed drives retain artifacts and automatically clean their profile; clean up interrupted attempts too. Never cite a `drive-result: FAIL`, stale build, SKIP or unavailable capture as success. Evidence under `target` survives verifier cleanup, not `cargo clean`; copy it elsewhere before cleaning build outputs.

## Cleanup

```sh
"$V" logs --path-only
"$V" cleanup --dry-run
"$V" cleanup
"$V" logs
"$V" doctor
```

Cleanup validates UID, checkout/PID start identity and private-runtime markers, including native runtime directories nested beneath owned scratch; symlinked runtimes and sibling-prefix paths are not adopted. It terminates only owned processes (including orphaned native helpers), waits for them to stop after TERM/KILL, unmounts private portal/GVFS mounts, then removes scratch while preserving evidence/build metadata. Ownership survives a helper that will not stop or a partial removal failure so cleanup can be retried. Never kill by process name or remove another display's lock/socket. Confirm evidence remains and doctor shows no launched profile/active instance/display; exit 1 after cleanup is expected (`launched: false`), while freshness remains separately visible. The smoke runner also tears down its compositor/window-manager/editor children.

## Helpers

`verify` and `test_verify.py` are executable. Use `"$V" --help`, `"$V" drive --help`, and each subcommand's `--help`; destructive operations accept `--dry-run`. Run the safety regression directly with `python3 .agents/skills/verify-window-zones/test_verify.py` or through the existing `scripts/test.sh` (`drive tests` preserves its output).

`verify` is only the executable entry point. Its `wz_verify/` package contains `runtime.py` (workspace, ownership, freshness, evidence and process lifecycle), `journeys.py` (PTY/config/native driving), `checks.py` (observations, scenarios and reports), and `cli.py` (argument parsing and dispatch). Pass one `runtime.Workspace` explicitly to journeys/checks; independent workspaces carry their own paths instead of mutable module globals. Dependencies flow from CLI to checks/journeys to runtime. The safety regression imports these modules normally and checks workspace isolation and freshness after module additions, edits and removals. Keep the package with the launcher; command syntax, evidence locations and approved `expected.json` are unchanged.

OMP discovers this directory under project-local `.agents/skills/`. Hermes requires human-run trust; until then use `skill_view` on `.agents/skills/verify-window-zones/SKILL.md` explicitly, without modifying global settings. Maintain this map with `/maintain-verification-skill` when app paths change.
