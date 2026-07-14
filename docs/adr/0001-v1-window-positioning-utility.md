# ADR 0001: V1 is a window positioning utility

Date: 2026-06-25

## Status

Accepted

## Context

The product should provide Rectangle-style window placement across macOS, Windows, and Linux. The initial feature set includes named zones such as halves, thirds, and two-thirds, moving windows between displays, and user-configurable hotkeys through a config file.

A true window manager would own broader desktop behavior such as focus policy, stacking, workspaces, tiling layout, snapping previews, and session restore. Those responsibilities are platform-specific and much larger than the requested first version.

## Decision

V1 will be a background window positioning utility, not a replacement window manager.

The app will listen for configured bindings and move or resize the currently focused OS-managed window into a named zone or onto another display.

## Consequences

- The app does not replace the user's desktop environment or operating-system window manager.
- The first implementation can focus on focused-window discovery, display geometry, zone calculation, config parsing, and hotkey dispatch.
- Focus ownership, stacking policy, workspaces, full tiling layout ownership, snapping previews, and session restore are out of scope for v1.
- Linux support can target integration with existing desktop sessions/window protocols rather than becoming a compositor/window manager.

## Accepted implementation scope

- The core will stay platform-neutral: config, zones, actions, display/window geometry math, and dispatch contracts must not depend on platform-specific window handles or APIs.
- Native platform adapters will own focused-window discovery, move/resize operations, display enumeration, and hotkey registration.
- Windows and Linux X11 are the first real implementation targets.
- Linux Wayland is a future todo because arbitrary window positioning is compositor/protocol constrained; it may later need compositor-specific support or a reduced capability model.
- macOS should remain architecture-compatible but is not required for the first working milestone.

## Accepted first executable slice

Build the platform-neutral Rust core first, before any real OS window movement.

This slice includes:

- Rust workspace or crate scaffold.
- Domain types for display geometry, window geometry, zones, actions, and bindings.
- Zone calculation for left half, right half, left third, center third, right third, left two-thirds, right two-thirds, and maximize.
- Config file parsing into bindings and actions.
- Unit tests proving zone math across one and multiple displays.

This slice excludes global hotkey registration, focused-window detection, moving real windows, OS-specific Windows/X11/macOS code, tray/menu/UI work, and Wayland support.

Implementation status: the first executable slice is implemented in the platform-neutral Rust core. The verified core includes integer geometry, built-in zone calculation, display movement geometry, TOML config parsing, and unit tests. Platform adapters remain deferred.

## Accepted v1 config shape

V1 config uses TOML. The first executable slice parsed config content only; the runtime now resolves and loads the platform-standard config path at startup while preserving missing, read-error, and parse-error states.

Hotkey strings remain opaque in the core so platform adapters can validate and register them later. Config action names use kebab-case strings and parse into Rust enums internally. Zones are built-in named zones first; custom user-defined zones are deferred.

Example:

```toml
[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }

[[bindings]]
hotkey = "Ctrl+Alt+Right"
action = { type = "move-to-zone", zone = "right-half" }

[[bindings]]
hotkey = "Ctrl+Alt+Shift+Right"
action = { type = "move-to-next-display" }

[[bindings]]
hotkey = "Ctrl+Alt+Shift+Left"
action = { type = "move-to-previous-display" }
```

## Accepted display movement behavior

Display movement in the core uses source display usable area, target display usable area, and current window geometry.

For `move-to-next-display` and `move-to-previous-display`, if the current window exactly or nearly matches a known built-in zone on the source display, the same zone is applied on the target display. Otherwise, the window is mapped proportionally from the source display usable area to the target display usable area.

Display movement calculations use usable area rather than full pixel bounds when the platform adapter can provide it. Display ordering, animation, cursor-following behavior, and remembering prior zones per app are deferred.

## Accepted geometry and rounding rules

Public domain geometry uses integer pixel rectangles: `x: i32`, `y: i32`, `width: u32`, and `height: u32`. Zone/layout functions centralize rounding so adapters do not each invent their own rounding behavior.

Zone fractions use rational math internally where practical. For widths or heights that do not divide evenly, edge-anchored zones preserve their outer edge exactly: right-anchored zones end on the usable area's right edge, bottom-anchored zones end on the usable area's bottom edge, and maximize equals the usable area exactly. Generated zones must not exceed the display usable area.

For thirds, left third starts at the usable area's left edge and uses floor division, right third ends exactly at the right edge, and center third occupies the deterministic middle span between them. Two-thirds zones preserve their corresponding outer edge exactly.

## Accepted next slice: adapter contract and action executor

The next implementation slice is a platform-neutral adapter contract plus an action executor, tested with fake adapters only.

This slice includes:

- A platform adapter contract for focused-window discovery, display listing in adapter-defined order, and applying a move/resize target to the focused window.
- Platform-neutral value types for focused-window state and requested window moves.
- An executor that turns `Action::MoveToZone`, `Action::MoveToNextDisplay`, and `Action::MoveToPreviousDisplay` into target rectangles using the existing core geometry functions.
- Unit tests with an in-memory fake adapter covering zone movement, next/previous display movement, wrapping display behavior, no-focused-window errors, and missing-display errors.

Next/previous display behavior uses the display order returned by the platform adapter. V1 wraps: next from the last display targets the first display, and previous from the first display targets the last display.

This slice excludes real Windows APIs, real X11 APIs, global hotkey registration, config file path discovery, tray/UI work, and Wayland support.

Accepted public seam names for this slice:

- Module: `src/window_system.rs`
- Trait: `WindowSystem`
- Value types: `FocusedWindow` and `WindowMove`
- Executor module: `src/executor.rs`
- Main executor function: `execute_action(action, window_system)`

The seam name `WindowSystem` identifies the adapter capability boundary without implying that the app replaces the operating-system window manager.

Accepted executor error shape:

- `focused_window()` returns `Ok(None)` when no window is focused.
- Executor errors distinguish normal core states from platform failures: `NoFocusedWindow`, `NoDisplays`, and `FocusedWindowDisplayMissing { display_id }` are core executor errors.
- Actual adapter/API failures are represented as `WindowSystemError::Platform(String)` and wrapped by the executor without interpretation.

Implementation status: this adapter-contract/executor slice is implemented and verified in the platform-neutral Rust core. The verified core includes `WindowSystem`, `FocusedWindow`, `WindowMove`, `execute_action`, wrapping next/previous display behavior, executor error classification, and fake-adapter unit tests. Real OS adapters remain deferred.

## Accepted next slice: config-driven binding dispatcher

The next implementation slice is a platform-neutral dispatcher that connects parsed TOML bindings to the action executor.

This slice includes:

- A new `src/dispatcher.rs` module.
- A public `dispatch_hotkey(config, hotkey, window_system)` function.
- Binding lookup by exact `Binding.hotkey` string match.
- Calling `execute_action(&binding.action, window_system)` for the matched binding.
- Dispatcher errors for `NoBindingForHotkey { hotkey }` and propagated executor failures.
- Fake-adapter tests proving known hotkeys dispatch to zone movement, known hotkeys dispatch to display movement, unknown hotkeys return `NoBindingForHotkey`, and executor errors propagate.

For duplicate hotkeys, v1 uses first-match-wins. Duplicate binding validation is deferred.

The dispatcher takes `&AppConfig` directly and uses exact case-sensitive hotkey string matching. Precomputed binding maps, hotkey normalization, case-insensitive matching, indexing, and duplicate validation are deferred.

This slice excludes real global hotkey registration, config file path discovery, config watching/reload, platform-specific hotkey syntax validation, real Windows/X11 APIs, tray/UI work, and Wayland support.

Implementation status: this dispatcher slice is implemented and verified in the platform-neutral Rust core. The verified core includes `dispatch_hotkey`, exact case-sensitive matching against `&AppConfig`, first-match-wins duplicate behavior, dispatch error classification, and fake-adapter unit tests. Real hotkey registration and platform adapters remain deferred.
