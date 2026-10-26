# ADR 0008: The executor correlates windows to displays; Sway and Hyprland dispatch through the compositor

Date: 2026-09-12

Status: accepted; amends ADR 0001 (`FocusedWindow.display_id` seam) and ADR 0003 (Sway/Hyprland as "constrained backends", Sway/Hyprland/macOS gate wording).

The original decision made adapters report only frame geometry and the executor correlate by frame-center containment, failing as `FocusedWindowOffDisplay` when no display contained it. It retained the `(bsiiuu)` companion signature with the display string ignored. ADR 0009 supersedes those details: correlation falls back to greatest overlap, then nearest display, and stable window identity uses `Gnome2` and KWin protocol 3. Hotkey strings still have one canonicalizer, `config::normalize_hotkey`; native listeners and Companions accept canonical strings only. Sway and Hyprland have no global hotkey capability in the App: users bind `window_zones dispatch <hotkey>` in the compositor config (Compositor-bound dispatch), forwarding into the running App's session and history, with `dispatch` exiting non-zero on failure.

## Considered Options

- Keep per-adapter display correlation with a `display_id` on `FocusedWindow`: rejected because every adapter reimplemented the containment rule differently (KWin by output name, GNOME by monitor index, X11 by client origin, macOS by a `display-0` fallback) and two of them were wrong.
- Give Sway and Hyprland a global hotkey path through `rdev`/evdev: rejected because neither compositor delivers accelerators to an evdev listener without root, and both already own keybinding configuration.

## Consequences

- Sway, Hyprland, and macOS are Ungated integrations: implemented and built in CI.
- The automated seated GNOME gate requires authorized writable `/dev/uinput` and fails, rather than falling back to XTEST or physical input, when it is absent.
- X11 placement uses the frame (`TranslateCoordinates` + `_NET_FRAME_EXTENTS`) and restores maximization before configuring. ADR 0009 amends the original full-monitor usable areas: `_NET_WM_STRUT(_PARTIAL)` panel reservations are now subtracted per monitor.
