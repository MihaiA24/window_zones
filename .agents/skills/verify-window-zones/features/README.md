# Window Zones verification map

Maintained source for user-facing verification journeys. Read the index, then the affected feature files. `docs/runbooks/testing.md` remains the release-gate status record; this map does not promote synthetic evidence to a seated release pass.

## Baseline preconditions

- Complete the skill's [Launch](../SKILL.md#launch) and [Doctor](../SKILL.md#doctor) steps first; they define `V`, checkout ownership and build freshness.
- The isolated TOML has Left → left-half, Up → center-third, Right → left-two-thirds, Down → next display, Shift+Right → previous display; each uses Alt+Ctrl.
- Restore `"$V" drive config valid` before a baseline journey. Each CLI/PTY drive starts a new process; mutations persist in the profile's file, not runtime memory.
- Dry-run's fixed display is 1920×1080 at (0,0), with a second 1280×1024 display at (1920,0); it does not move OS windows. Native smoke uses its own fixtures and observations.
- Native prerequisites come from `docs/runbooks/testing.md`. Missing compositor, display, focus, seat, tray host or capture support is a failed precondition for that journey, not permission to replace it with dry-run.

## Driving conventions

- Follow [Drive](../SKILL.md#drive) for CLI/PTY commands, flag/config variants, deadlines and isolated config mutations.
- Choose maintained [Checks](../SKILL.md#checks) before individual recipes; `check e2e` already owns X11 smoke. Use individual drives only for uncovered paths or diagnosis.
- Seated GNOME/Windows/macOS and a real tray host are separate required environments when affected, not covered by synthetic gates.

## Proof and skip reporting

- Apply the skill's [Evidence](../SKILL.md#evidence) matrix and [Cleanup](../SKILL.md#cleanup) procedure. Bind each proof to feature ID, entry point, actual backend, flag/config variant, current build and artifacts; annotate the handoff with feature IDs.
- Native summaries must name the selected gate and have consistent assertion/pass/fail/skip counts. Missing or ambiguous summaries fail verification; unrelated log text saying zero failures is not proof.

## Features

- [Status and backend selection](status-backends.md): one-shot status, dashboard/status refresh, missing/error config, auto/explicit routing and dry-run alias.
- [Window placement and display movement](placement.md): fourteen built-in zones, custom zones, directional/cyclic displays, half crossing, center/restore, identified moves and partial failures.
- [Configuration and atomic reload](configuration.md): discovery/explicit paths, valid replacement, invalid retention, empty/missing configs, restart and disk persistence.
- [Global hotkeys and dispatch](hotkeys.md): physical accelerators, one-shot/interactive/compositor-bound dispatch, exclusive controller ownership, transactional registration and recovery.
- [Runtime controls](runtime-controls.md): CLI/TUI/tray lifecycle, initialization, single-instance ownership, notifications, best-effort session logs and performance logging.
