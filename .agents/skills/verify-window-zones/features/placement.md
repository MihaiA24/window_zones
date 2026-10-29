# Window placement and display movement

Users place the focused OS window in a configured zone or move it to the next/previous display while retaining zone intent or proportional placement.

## Sub-features

- `place-zone`: fourteen built-ins: left/right and top/bottom halves, four corners, thirds, two-thirds, and maximize.
- `place-custom`: percentage-based user zones resolve inside the display's usable area.
- `place-display`: next/previous wrap; explicit directional display moves do not. Repeated matching left/right halves cross to the physical neighbour's opposite half without wrapping.
- `place-boundaries`: unknown binding, no focused window, no usable displays or denied movement exposes an error without a false success.
- `place-history`: center keeps size (shrinking to fit); restore returns to the original frame within one runtime. Partial moves retain history; manual moves and restart reset it. Restore does not require a successful display query.

## How to get to it (user POV)

- Press a configured global accelerator while the target window has focus.
- Run `scripts/run.sh … dispatch HOTKEY` for a one-shot action.
- Type `dispatch HOTKEY` or a bare hotkey in `run`/`tui`; the tray runtime also listens for global hotkeys.
- Edit the TOML binding's zone/action, then reload or restart.

## Driving it with verify

Preconditions:

- Baseline/current build. Native proof needs the gate's real test window, focused window manager and displays; core dry-run always supplies a simulated focused window.

- **Core placement.** `"$V" drive cli --expect 'Dispatch state: Succeeded' --expect 'last move: x=0 y=0 w=960 h=1080' -- dispatch Alt+Ctrl+Left` proves only computed dry-run output.
- **Fixed-oracle checks.** Start with [`check behaviors` and `check e2e`](../SKILL.md#checks). Native TUI dispatch needs a completed successful response and independently matching post-dispatch geometry, not only a gate summary.
- **Performance measurement.** Use [`check performance`](../SKILL.md#checks); startup+dispatch measurements do not prove compositor movement or global-hotkey latency.
- **Other built-in variants.** `drive config --zone` supports the eight fixed-oracle zones, not all fourteen. Use `"$V" drive session --mode run --journey starter` for all starter zones and keys; odd-edge calculations remain supplemental `drive tests` coverage.
- **Custom zone.** `"$V" drive config custom`, then `"$V" drive cli --expect 'last move: x=192 y=108 w=960 h=540' -- dispatch Alt+Ctrl+Left`. A second CLI process must read the same custom config. Restore `drive config valid`.
- **Display direction.** `"$V" drive cli --expect 'Dispatch state: Succeeded' -- dispatch Alt+Ctrl+Down` and `"$V" drive cli --expect 'Dispatch state: Succeeded' -- dispatch Alt+Ctrl+Shift+Right` exercise the next/previous actions; do not mistake computed output for OS geometry proof.
- **Actual user movement.** `"$V" check e2e` owns the native X11 journey. `drive smoke kde-live` covers nested KWin; `drive smoke gnome` drives native Wayland/XWayland clients through Mutter virtual input, with independent client geometry and ScreenCast captures. These remain synthetic, not seated hardware evidence.
- **Interactive entry.** `"$V" drive session --mode run --command 'dispatch Alt+Ctrl+Left' --expect 'Dispatch state: Succeeded'`; for TUI use `--mode tui` and expect `Last action: alt+ctrl+left`. Built-in and custom TUI dispatch success also requires the completed response/prompt without `Last error: dispatch:`. Bare `Alt+Ctrl+Left` is a separate line-input entry with the same success checks.
- **Error/empty.** `"$V" drive cli --expect 'NoBindingForHotkey' -- dispatch Alt+Ctrl+F12`; then `drive config empty` and repeat Left with that error. For an intentional TUI failure, pair the dispatch command with `--expect 'Last error: dispatch: no binding configured'`, not a `Last action` label. Restore valid. Native no-focus/no-display/denied paths require those real conditions and an unchanged-window observer.
- **Cancel/persistence.** Quit a runtime without dispatch and compare native geometry before/after when claiming no move. After a successful move, independently observe geometry before and after runtime exit; config survives a new process, but Window Zones does not persist/restore a dispatch history.
- **Starter actions/history.** `"$V" drive session --mode run --journey starter` runs actual `init --force --chord Ctrl+Alt` and a single PTY with literal geometry assertions for all eighteen bindings, including center, repeated halves crossing both ways without wrapping, next/previous display, Restore consumption and history reset on restart. A fresh status process checks the persisted eighteen bindings and clear action state; success restores the normal five-binding fixture. TUI has no geometry field and is not supported for this journey.
- **Native identity/constraints.** Native gates and adapter regressions cover captured IDs, fullscreen/non-resizable refusal, restoration and partial placement. X11's gate temporarily constrains its owned client's minimum size, requires an observed `PartialMove`, then independently checks Restore before restoring the hints. Repeat constrained-client checks on Windows, GNOME, KDE, Sway and Hyprland; switch focus between applications on macOS. One platform's synthetic pass does not close another's native check.
- **Display freshness/work areas.** On the owned X11 display, withdrawn or other-desktop docks must not reserve space; legitimate auto-hide reservations remain. On macOS, use the actual main-thread `run`/`tui` path, change monitor/Dock settings, and independently compare new usable areas without foregrounding the utility.

## Gotchas

- Dry-run records a requested rectangle; it never moves an OS window and is not native geometry evidence.
- The baseline seeds only three built-in zones, no custom zone, and no blank-display/denied adapter condition. `--zone` and `custom` supply zone preconditions; native error preconditions still need real environments. Missing focus/outputs/permissions must be reported, not silently treated as covered.
- Xvfb may retain an aggregate automatic RandR monitor alongside two logical monitors; inspect assertions rather than assuming physical hardware equivalence.
- `kde-live` uses a private virtual framebuffer and bus, not an outer desktop socket. Real accelerator evidence requires the post-injection action plus independent window geometry; compositor debug-log wording is not a placement assertion.
- Placement has no confirmation/cancel dialog; GNOME can animate the transition. Cancel means exiting before dispatch, not interrupting or undoing an already-requested move.
- Native `gnome` covers all fourteen built-ins through both one-shot dispatch and actual Mutter accelerators, plus center/Restore, consumed history and restart reset within the running App. Seated `gnome-live` reuses one unreserved accelerator for all fourteen built-ins and center, then checks interactive Restore shares that runtime's history and a repeated physical Restore reports consumption. Each seated target needs independent Xwayland and companion geometry. Other custom zones, explicit directional display actions, constrained-client history and surface/mode/flag combinations still require their affected native journeys.
- On HiDPI GNOME, Xwayland pixels can differ from Mutter logical coordinates even at fractional monitor scales. The seated observer records `xwayland-layout.json` from independent monitor modes/layout and X root dimensions, then normalizes frame units without changing target rectangles or rounding fractional discrepancies. Nonuniform mappings fail preflight rather than being guessed. Captures must show the compositor monitor containing the owned fixture, not the black Xwayland root.
