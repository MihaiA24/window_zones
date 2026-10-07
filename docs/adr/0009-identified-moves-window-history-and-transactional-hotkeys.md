# ADR 0009: Moves target an identified window; config and hotkeys change together

Date: 2026-10-06

Status: accepted; amends ADR 0001 (`WindowSystem` move seam, built-in zones, action set), ADR 0002 (reload API and what "atomic" covers), ADR 0004/0005 (GNOME interface major), and ADR 0008 (correlation fallback).

## Decision

**Window identity.** `FocusedWindow` carries an opaque adapter-scoped `WindowId`, and `WindowSystem::move_window(&WindowMove)` moves the window that id names, not whatever has focus when the move arrives. A closed window fails as `WindowSystemError::WindowGone`. Adapters restore a maximized or tiled window before placing it and reject fullscreen windows. The GNOME companion interface becomes `org.window_zones.Gnome2` (`GetFocusedWindow -> (btiiuu)` with the Mutter window id, `MoveWindow(tiiuu)`); the KWin protocol major moves to 2 for the same reason.

**Correlation.** A window belongs to the display whose usable area contains its frame center, else the display its frame overlaps most, else the nearest display. Any visible window can be acted on, including one that straddles displays or sits over a panel. Next/previous display follow the left-to-right, top-to-bottom layout, and do nothing when there is one display.

**Actions.** Built-in zones add `top-half`, `bottom-half`, `top-left`, `top-right`, `bottom-left`, `bottom-right`. New actions: `move-to-display` (`direction = left|right|up|down`), `center` (keep size), `restore`. Repeating `left-half`/`right-half` on the same window cycles half → two-thirds → third. The executor keeps a per-window history (restore point and cycle step) for one run. A window whose geometry no longer matches the App's last placement is treated as moved by the user and starts over. Field-less actions are empty struct variants, so unknown fields on them are rejected.

**Transactional config and hotkeys.** `App::tick`/`App::reload` apply a changed config only if the hotkey system accepts its complete set. `HotkeySystemError::Rejected` means the set was refused and the previous set stays registered. The previous config then stays active, and the config state reports the refusal. `Unavailable` means nothing is known to be registered, so registration is retried every second. Runtime events are emitted once per transition. `dispatch_next_hotkey` returns `None` when the queue is empty, so one press is reported once, and the runtime drains every queued press each 50 ms poll.

**Runtime.** One engine serves the CLI, TUI, and tray. `run`/`tui` take a single-instance lock and exit 75 when it is held. Without a terminal, problem events become desktop notifications. `window_zones init` writes a starter config whose chord avoids the desktop's own bindings (`Ctrl+Super` on GNOME). X11 hotkeys use exclusive `XGrabKey` grabs. Windows and macOS use a process-wide grabbing `rdev` hook, so a bound key never reaches the focused application.

## Considered options

- Re-check focus in the companion before moving: rejected. It narrows the race but still moves a different window when focus changes between the query and the move, and delayed hotkey events make that window arbitrary.
- Keep rejecting maximized/tiled windows: rejected. With GNOME edge tiling, ordinary snapping made every later action fail until the user restored the window by hand.
- Apply the config and report the hotkey refusal separately: rejected. Valid TOML with one desktop-reserved shortcut silently disabled every working binding.

## Consequences

- The GNOME extension and KWin script must be upgraded together with the binary; an older companion fails as incompatible.
- `dispatch` runs one action per process, so `restore` and half cycling only remember windows within a `run`/`tui` session.
- X11 usable areas now subtract `_NET_WM_STRUT(_PARTIAL)` per monitor (amends ADR 0008's consequence).
