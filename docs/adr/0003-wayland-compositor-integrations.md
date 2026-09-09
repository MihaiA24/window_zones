# Native Wayland support uses compositor integrations

Status: accepted

## Decision

Wayland session detection is routing, not a generic window-control implementation. Native KDE and GNOME support uses compositor-owned integrations; routing native windows through X11 is not equivalent support.

### Routing

- `--backend auto` selects X11 only for X11 sessions.
- A Wayland session never silently falls back to X11.
- Auto-detection uses normalized desktop-session environment signals.
- Unknown, conflicting, or unavailable compositor signals fail explicitly.
- The CLI keeps one generic `--backend wayland`; compositor selection stays inside the resolver.

### Integration contract

- Companions live in this repository as optional, versioned packages.
- The companion owns its well-known service name on the user's D-Bus session bus.
- The Rust App remains authoritative for config parsing, normalized bindings, reload state, and action dispatch.
- D-Bus mirrors adapter primitives: capabilities, focused-window state, displays, move/resize, and hotkey registration/events.
- Companions do not duplicate zone or action semantics.
- Hotkey registration is atomic; an unsupported binding rejects the update and retains the last valid set.
- A present companion may expose degraded capabilities; status and reload remain available while unsupported actions fail explicitly.

### Companion form and release gates

- GNOME uses a GNOME Shell extension; KDE uses a KWin script or plugin.
- Companion installation is opt-in through explicit install flags or commands.
- GNOME is implemented first, then the TUI, then KDE.
- Merge-ready V1 gates are Windows, Linux X11, KDE Wayland, and GNOME Wayland.
- macOS remains source-compatible but non-blocking; Sway and Hyprland remain constrained backends.
- V1 validation covers one representative stable GNOME Shell release and one representative stable KDE Plasma/KWin release, with exact versions documented.
