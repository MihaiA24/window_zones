# ADR 0009: Moves target an identified window; config and hotkeys change together

Date: 2026-10-06

Status: accepted; amends ADR 0001 (`WindowSystem` move seam, built-in zones, action set), ADR 0002 (reload API and what "atomic" covers), ADR 0004/0005 (GNOME interface major), and ADR 0008 (correlation fallback).

## Decision

**Window identity.** `FocusedWindow` carries an opaque adapter-scoped `WindowId`, and `WindowSystem::move_window(&WindowMove)` moves the window that id names, not whatever has focus when the move arrives. A closed window fails as `WindowSystemError::WindowGone`. Adapters restore a maximized or tiled window before placing it and reject fullscreen windows. The GNOME companion interface is `org.window_zones.Gnome2` (`GetFocusedWindow -> (btiiuu)` with the Mutter window id, `MoveWindow(tiiuu)`). KWin protocol 3 retains identified moves and adds fresh usable-area requests and native accelerator release; matching script and companion upgrades are required.

**Correlation.** A window belongs to the display whose usable area contains its frame center, else the display its frame overlaps most, else the nearest display. Any visible window can be acted on, including one that straddles displays or sits over a panel. Next/previous display follow the left-to-right, top-to-bottom layout, and do nothing when there is one display.

**Actions.** Built-in zones add `top-half`, `bottom-half`, `top-left`, `top-right`, `bottom-left`, `bottom-right`. New actions: `move-to-display` (`direction = left|right|up|down`), `center` (keep size, shrink to fit), `restore`. Left/right-half placement snaps within the current display first. Repeating a matching half crosses to the nearest physically adjacent display in that horizontal direction, provided its usable area overlaps vertically, and lands on the opposite, source-facing half. There is no timing threshold or directional wrap; no neighbour leaves the window in place. This combined contract supersedes the incoming half → two-thirds → third width cycle. The executor keeps a per-window restore point and last placement for one run; a window whose geometry no longer matches the App's last placement is treated as moved by the user and starts over. Field-less actions are empty struct variants, so unknown fields on them are rejected.

**Partial placement.** A failed move that changes the identified window reports `WindowSystemError::PartialMove` with its observed frame. The executor remembers that frame as its last placement without losing the original Restore point, including when Restore itself partially fails. Failed readback never invents geometry, and `WindowGone` still discards history.

**Transactional config and hotkeys.** `App::tick`/`App::reload` apply a changed config only if the hotkey system accepts its complete set. `HotkeySystemError::Rejected` means the set was refused and the previous set stays registered. The previous config then stays active, and the config state reports the refusal. `Unavailable` means nothing is known to be registered, so registration is retried every second. Runtime events are emitted once per transition. `dispatch_next_hotkey` returns `None` when the queue is empty, so one press is reported once, and the runtime drains every queued press each 50 ms poll.

**Runtime.** One engine serves the CLI, TUI, and tray. `run`/`tui` take a single-instance lock and exit 75 when it is held. Without a terminal, problem events become desktop notifications. `window_zones init` writes a starter config whose chord avoids the desktop's own bindings (`Ctrl+Super` on GNOME). X11 hotkeys use exclusive `XGrabKey` grabs. Windows uses a process-wide grabbing `rdev` hook; macOS owns a native keyboard event tap that reconciles held keys and re-enables itself after disablement. Both consume configured keys before the focused application receives them.

## Considered options

- Re-check focus in the companion before moving: rejected. It narrows the race but still moves a different window when focus changes between the query and the move, and delayed hotkey events make that window arbitrary.
- Keep rejecting maximized/tiled windows: rejected. With GNOME edge tiling, ordinary snapping made every later action fail until the user restored the window by hand.
- Apply the config and report the hotkey refusal separately: rejected. Valid TOML with one desktop-reserved shortcut silently disabled every working binding.

## Consequences

- The GNOME extension and KWin script must be upgraded together with the binary; an older companion fails as incompatible.
- On Sway/Hyprland, compositor-bound `dispatch` forwards to the existing `run`/`tui` session, sharing its config and Restore history; no running session is an actionable error. Other backends retain one-shot dispatch. Repeated-half crossing uses the current frame.
- X11 usable areas now subtract `_NET_WM_STRUT(_PARTIAL)` per monitor (amends ADR 0008's consequence).
