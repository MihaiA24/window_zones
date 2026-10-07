# window_zones verification map

This directory is the maintained source for verifying the user-facing behavior of window_zones. Read the index before driving the app, then use the matching feature file as the recipe. Every recipe drives an owned `--backend dry-run` instance through `wzv` (see `../SKILL.md`).

## Baseline preconditions

- Work from the repository root with `cargo build --locked --bin window_zones` passing; `wzv start` runs that build.
- Put the helpers on `PATH`: `export PATH="$PWD/.agents/skills/verify-window-zones/scripts:$PATH"`.
- Name the run and its directory: `RUN=<feature>-$(date +%s)` and `R="${WZV_ROOT:-target/verify-window-zones}/$RUN"`.
- Each recipe starts its own run with `wzv start --run-id "$RUN"` and requires `wzv doctor "$RUN"` to print `doctor=ok`.
- The dry-run desktop is fixed: focused window `dry-run-window` at x=40 y=40 w=640 h=480 on `display-0` (0,0 1920x1080); `display-1` sits at 1920,0 with 1280x1024.
- `wzv start` writes the starter config (18 `Ctrl+Alt` bindings) unless the recipe passes `--no-init`.
- Never drive an instance this run did not start, the user's installed `window_zones`, or the `window-zones` systemd user service.

## Driving conventions

- Start every recipe from the baseline and run its commands in order; later steps depend on earlier window state.
- Treat every command as literal. Keep quoting unchanged; session lines are single-quoted.
- Type session lines with `wzv send`, save config files with `wzv config`, and run one-shot CLI commands with `wzv exec ... -- ARGS`. Never pass `--backend`; `wzv` always uses `dry-run`.
- `--expect` holds the exact text the app prints. A recipe command that exits 0 has passed its check; `wzv exec --expect-exit 1` passes when the app exits 1.
- Hotkeys are case-insensitive on input; the app reports them in canonical `alt+ctrl+shift+cmd+key` order.
- Write fixture files under `"$R/scratch/"`; cleanup removes them.
- Finish with `wzv stop "$RUN"` and `wzv cleanup "$RUN"`. Cleanup keeps `"$R/evidence/"`.

## Proof and skip reporting

- Proof is the run's `evidence/` directory: `transcript.log` (each typed line, its output, one-shot commands with `exit_code=`, `[expect ok]`/`[expect FAIL]`), `app.log`, `config-NN.toml`, `side-effects.txt`, `sentinel.log`, `exit-code`.
- Capture the user action and the resulting state: the `last move:` geometry, `Config state:`, `Dispatch state:`, or the TUI `Last action:`/`Last error:`/`Last event:` lines.
- `wzv stop` must report `exit_code=0`, `live_backend_tool_calls=0`, and `instance_locks=0`; report `home_files` with the files you expected.
- Report the feature file, the run id, and the evidence path with every result.
- Dry-run proves runtime behavior, not real window movement or real key capture. Never report a live backend as verified through dry-run.
- Report an unreachable path with the attempted command and the unmet precondition (see `live-integrations.md`).
- Do not report a skipped entry point as verified through a different one: a one-shot `dispatch` does not prove the session path, and the reverse.

## Feature entry contract

Each feature file starts with an H1 title and one paragraph describing the user-visible behavior. It then uses exactly four H2 sections in this order.

1. `Sub-features` lists short IDs with one line for each behavior.
2. `How to get to it (user POV)` lists every user entry point.
3. `Driving it with wzv` starts with `Preconditions:` and uses labeled bullets that pair each user action with an exact command and observable result.
4. `Gotchas` lists traps that can waste or invalidate a verification run.

Keep implementation details out of the map. Name only user paths, stable handles, required state, commands, and observable proof. Mark a recipe `unverified` until its commands have been run against the current build.

## Features

- [Starter config](./starter-config.md) covers `init` with the default chord, a chosen chord, `--force`, refusal to overwrite, an invalid chord, and `--config` paths. Exercised.
- [One-shot commands](./one-shot-commands.md) covers `status`, `dispatch HOTKEY`, exit codes, missing config, and argument errors. Exercised.
- [Move the window](./move-window.md) covers every starter zone, half cycling, center, maximize, restore, next and previous display with wrap, custom zones, and `move-to-display`. Exercised.
- [Run session](./run-session.md) covers the `run` stdin commands: `status`, `help`, `dispatch`, bare hotkeys, unknown hotkeys, `reload`, `restart`, and `exit`. Exercised.
- [Live config reload](./config-reload.md) covers saving broken, invalid, fixed, and extended configs while the App runs. Exercised.
- [TUI dashboard](./tui.md) covers the dashboard frame, dispatch, error and event lines, `refresh`, `reload`, `restart`, config errors, and quit. Exercised.
- [Live integrations](./live-integrations.md) covers the GNOME, X11, KDE, macOS, Windows, Sway/Hyprland, tray, notification, single-instance, and service paths. Blocked on this host; verified with `scripts/smoke.sh` on a Linux desktop.
