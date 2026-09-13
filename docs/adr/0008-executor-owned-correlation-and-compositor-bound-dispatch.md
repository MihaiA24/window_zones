# ADR 0008: The executor correlates windows to displays; Sway and Hyprland dispatch through the compositor

Date: 2026-09-12

Status: accepted; amends ADR 0001 (`FocusedWindow.display_id` seam) and ADR 0003 (Sway/Hyprland as "constrained backends", Sway/Hyprland/macOS gate wording).

Adapters report the Focused window's frame geometry only, and the executor resolves its Display as the Display whose Usable area contains the frame center, failing explicitly (`FocusedWindowOffDisplay`) when none does; the D-Bus companions keep the `(bsiiuu)` focused-window signature, with the string now ignored by the App, so no protocol major bump is needed. Hotkey strings have one canonicalizer, `config::normalize_hotkey`; native listeners and Companions accept canonical strings only. Sway and Hyprland have no global hotkey capability in the App: `run` reports hotkeys unavailable there and users bind `window_zones dispatch <hotkey>` in the compositor config (Compositor-bound dispatch), with `dispatch` exiting non-zero on failure.

## Considered Options

- Keep per-adapter display correlation with a `display_id` on `FocusedWindow`: rejected because every adapter reimplemented the containment rule differently (KWin by output name, GNOME by monitor index, X11 by client origin, macOS by a `display-0` fallback) and two of them were wrong.
- Give Sway and Hyprland a global hotkey path through `rdev`/evdev: rejected because neither compositor delivers accelerators to an evdev listener without root, and both already own keybinding configuration.

## Consequences

- Sway, Hyprland, and macOS are Ungated integrations: implemented, built in CI, no Smoke run on record.
- The seated GNOME gate requires a writable `/dev/uinput` and fails, rather than falling back to XTEST, when it is absent.
- X11 placement uses the frame (`TranslateCoordinates` + `_NET_FRAME_EXTENTS`) and unmaximizes before configuring; `_NET_WM_STRUT` panels are not subtracted from the RandR monitor bounds.
