# Window Zones Context

## Glossary

- **App**: The background utility that listens for configured bindings and asks the operating system to move or resize the focused window.
- **Desktop session**: The graphical host session in which the App runs, identified as X11 or Wayland.
- **Compositor**: The desktop component that owns window placement in a Wayland desktop session, such as KDE's KWin or GNOME Shell.
- **Compositor integration**: A compositor-specific control surface through which the App can observe or change native Wayland windows and register bindings.
- **Window**: An existing application window managed by the host operating system or desktop environment.
- **Display**: A physical or virtual monitor with a usable area where windows can be positioned.
- **Zone**: A named rectangle within a display's usable area, such as left half, right third, or left two-thirds.
- **Action**: A requested operation, such as moving the focused window to a zone or moving it to another display.
- **Binding**: A config entry that maps a hotkey to an action.

- **Companion**: The compositor-owned counterpart to the App that exposes a compositor integration for a desktop session. _Avoid_: helper, backend.
- **GNOME integration**: The compositor integration for a GNOME Wayland session, consisting of the GNOME Shell companion and its App-facing connection. _Avoid_: generic Wayland backend.

- **Capability**: A specific operation a compositor integration can provide to the App, such as focused-window discovery, display enumeration, move/resize, or hotkey registration.
- **Degraded integration**: A present compositor integration that lacks one or more capabilities; actions relying on missing capabilities fail explicitly while available operations remain usable. _Avoid_: unavailable integration.

- **Registered hotkey set**: The complete set of bindings currently accepted by a companion for event delivery; replacement is atomic, and an empty set means no registered hotkeys. _Avoid_: incremental registration.
- **Hotkey event**: A notification carrying only the canonical binding string when a registered hotkey is activated; the App resolves its action and focused Window. _Avoid_: raw key event.

- **Display identity**: An opaque identifier used within one compositor session to correlate a Window with a Display; it is not persistent across companion restarts or monitor changes. _Avoid_: connector identity, monitor index.


- **Window state**: A compositor condition that can constrain a move, such as maximized, fullscreen, tiled, or non-resizable; a rejected move does not change that state.
- **Session identity**: The resolved desktop-session protocol and compositor represented by normalized session signals; unknown or conflicting signals mean the identity is unresolved. _Avoid_: environment heuristic.

- **Integration diagnostic**: An action-oriented report of companion state and required user remediation, distinguishing absence, incompatibility, denial, unavailability, and operation failure.

- **Protocol compatibility**: The App and Companion are compatible when they share the same interface major and the Companion provides the required capabilities; optional behavior is selected by capability, not by speculative version branching.

- **Companion availability**: Whether the current desktop session has a reachable, compatible Companion; it may change independently of the App's configuration and runtime state.

- **Hotkey vocabulary**: The App's canonical binding-string form shared with a Companion; the Companion maps it to local accelerator syntax and may reject unsupported entries.

- **Companion-unavailable state**: An App runtime state in which the session resolves to GNOME but its Companion cannot be reached or accepted; configuration remains valid while operations fail explicitly and recovery is retried.

- **Focused window**: The existing application Window currently eligible for an Action; desktop, overview, lock-screen, and other Shell surfaces are not focused windows.

- **Usable area**: The global rectangle available for zones after compositor-reserved panels, docks, and similar regions are excluded; it is not the full monitor bounds.

- **Window/display correlation**: The Focused window's Display is the active display containing its frame center; an unmatched center remains explicit instead of being guessed.

- **Window frame**: The full global rectangle used for placement, including compositor-managed decoration space where available; it is distinct from client content geometry.

- **Native Wayland support**: An App operation performed through the current compositor's integration; an X11 or XWayland path is not equivalent support.

- **Active controller**: The single App instance currently authorized to own a Companion's registered hotkey set; other App instances may inspect but cannot replace it until disconnect.