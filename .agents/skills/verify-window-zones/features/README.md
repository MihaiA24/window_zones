# Window Zones verification map

Maintained source for user-facing verification journeys. Read the index, then the affected feature files. `docs/runbooks/testing.md` remains the release-gate status record; this map does not promote synthetic evidence to a seated release pass.

## Baseline preconditions

- From the repo root set `V=.agents/skills/verify-window-zones/verify`; run `"$V" launch` and `"$V" doctor`.
- Require this checkout's profile, `build: current`, and a free or owned derived display. No TCP server/auth/database exists. A stale or foreign instance is not driveable evidence.
- Verifier freshness includes the thin `verify` launcher and every Python module under `wz_verify/`. After changing that package, cleanup and launch again; the existing safety regression checks module edits/additions/removals and independent workspace cleanup.
- The isolated TOML has Left → left-half, Up → center-third, Right → left-two-thirds, Down → next display, Shift+Right → previous display; each uses Alt+Ctrl.
- Restore `"$V" drive config valid` before a baseline journey. Each CLI/PTY drive starts a new process; mutations persist in the profile's file, not runtime memory.
- Dry-run's fixed display is 1920×1080 at (0,0), with a second 1280×1024 display at (1920,0); it does not move OS windows. Native smoke uses its own fixtures and observations.
- Native prerequisites come from `docs/runbooks/testing.md`. Missing compositor, display, focus, seat, tray host or capture support is a failed precondition for that journey, not permission to replace it with dry-run.

## Driving conventions

- Use `verify` subcommands, the repo launcher and existing smoke/test runners; do not write one-off drive scripts or install a second framework.
- Match post-action text and exact geometry with deadlines. PTY commands use literal prompts/labels; native gates have independent geometry observers. Legacy pacing pauses in the existing smoke script are not assertions.
- Use `drive cli -- …` for one-shot commands; `drive session --mode run|tui` for line input. Repeat paired `--command` and `--expect` for custom input. `--quit quit|q|exit|eof` owns shutdown.
- `--backend-flag backend|alias` and `--discovery` select real flag/config entry variants. Verifier `--dry-run` only prints a plan; it is not app evidence.
- Mutate only the isolated TOML via `drive config`; files and build stamps are recorded alongside action/result transcripts.
- For native paths use `drive smoke x11|gnome|kde-live`. Seated GNOME/Windows/macOS and a real tray host are separate required environments when affected, not covered by synthetic gates.
- Use the skill's `Checks` section first: `check behaviors` owns the CLI flag/config matrix; `check sessions` owns the 24-session reload/watcher/cancel matrix; `check e2e` already includes X11 smoke. Do not reimplement these loops or repeat their underlying drives. `check performance --baseline-ref REF` is separate measurement, not a deterministic timing golden.

## Proof and skip reporting

- Apply the skill's exhaustive entry-point × mode × enabled-flag × outcome matrix. Success, cancel, error, empty and persistence must each be covered or explicitly inapplicable with a reason.
- Bind every proof to feature ID, entry point, actual backend, flag/config variant, current build and artifacts. `actions.jsonl` records exact commands/options and results; annotate the handoff with feature IDs.
- Include both input and result; confirm durable config from a new process and OS movement from an independent observer/screenshot. Exit 0 alone does not prove dispatch/config success.
- Report attempted command and missing precondition for an unreachable journey. A different entry point's pass cannot close it. Existing SKIPs and failed screenshots are not successes.
- Cleanup removes scratch state/owned processes, never evidence. Run `logs --path-only` after cleanup and inspect the named files.

## Features

- [Status and backend selection](status-backends.md): one-shot status, dashboard/status refresh, missing/error config, auto/explicit routing and dry-run alias.
- [Window placement and display movement](placement.md): eight built-in zones, custom zones, next/previous display, dispatch and real accelerators, error/empty boundaries.
- [Configuration and atomic reload](configuration.md): discovery/explicit paths, valid replacement, invalid retention, empty/missing configs, restart and disk persistence.
- [Global hotkeys and dispatch](hotkeys.md): physical accelerators, one-shot and interactive dispatch, aliases, registration failure/recovery and no-binding errors.
- [Runtime controls](runtime-controls.md): CLI/TUI lifecycle, help/refresh, quit aliases/EOF and optional tray menu entry points.
