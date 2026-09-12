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
- **KDE integration**: The compositor integration for a KDE Plasma Wayland session, consisting of the KWin script, Rust companion service, and their App-facing D-Bus connection. _Avoid_: generic Wayland backend.

- **Capability**: A specific operation a compositor integration can provide to the App, such as focused-window discovery, display enumeration, move/resize, or hotkey registration.
- **Degraded integration**: A present compositor integration that lacks one or more capabilities; actions relying on missing capabilities fail explicitly while available operations remain usable. _Avoid_: unavailable integration.
- **Compositor-bound dispatch**: Running `window_zones dispatch <hotkey>` from the compositor's own keybinding configuration when the App's hotkey capability is unavailable there (Sway, Hyprland); the compositor owns capture and the App owns the action. _Avoid_: hotkey fallback, external hotkey.

- **Registered hotkey set**: The complete set of bindings currently accepted by a companion for event delivery; replacement is atomic, and an empty set means no registered hotkeys. _Avoid_: incremental registration.
- **Hotkey event**: A notification carrying only the canonical binding string when a registered hotkey is activated; the App resolves its action and focused Window. _Avoid_: raw key event.

- **Display identity**: An opaque per-session label a Display carries in status and diagnostics; it never decides which Display a Window is on and is not persistent across companion restarts or monitor changes. _Avoid_: connector identity, monitor index.


- **Window state**: A compositor condition that can constrain a move, such as maximized, fullscreen, tiled, or non-resizable; a rejected move does not change that state.
- **Session identity**: The resolved desktop-session protocol and compositor represented by normalized session signals; unknown or conflicting signals mean the identity is unresolved. _Avoid_: environment heuristic.

- **Integration diagnostic**: An action-oriented report of companion state and required user remediation, distinguishing absence, incompatibility, denial, unavailability, and operation failure.

- **Protocol compatibility**: The App and Companion are compatible when they share the same interface major and the Companion provides the required capabilities; optional behavior is selected by capability, not by speculative version branching.

- **Companion availability**: Whether the current desktop session has a reachable, compatible Companion; it may change independently of the App's configuration and runtime state.

- **Hotkey vocabulary**: The App's canonical binding-string form, produced only by the App's config normalizer: modifiers in the order `alt`, `ctrl`, `shift`, `cmd`, then one key name, lowercase and `+`-joined; Companions and native listeners accept canonical strings only and reject anything else. _Avoid_: key alias, accelerator string.

- **Companion-unavailable state**: An App runtime state in which the resolved GNOME or KDE Companion cannot be reached or accepted; configuration remains valid while operations fail explicitly and recovery is retried.

- **Focused window**: The existing application Window currently eligible for an Action; desktop, overview, lock-screen, and other Shell surfaces are not focused windows.

- **Usable area**: The global rectangle available for zones after compositor-reserved panels, docks, and similar regions are excluded; it is not the full monitor bounds.

- **Window/display correlation**: The App's executor resolves the Focused window's Display as the Display whose Usable area contains the Window frame's center; adapters report geometry only, and an unmatched center is an explicit error, never a guess. _Avoid_: adapter display id.

- **Window frame**: The full global rectangle used for placement, including compositor-managed decoration space where available; it is distinct from client content geometry.

- **Native Wayland support**: An App operation performed through the current compositor's integration; an X11 or XWayland path is not equivalent support.

- **Active controller**: The single App instance currently authorized to own a Companion's registered hotkey set; other App instances may inspect but cannot replace it until disconnect.

- **Release gate**: A platform configuration whose passing Smoke run is required before a release; a gate is _blocking_ when the release cannot ship without that pass, or _deferred_ when the release ships with the integration implemented but unverified. _Avoid_: supported platform.

- **Smoke run**: One execution of the fixed end-to-end check sequence for a Release gate against a single configuration, yielding a pass or a named failure.

- **Verified configuration**: The exact desktop session, compositor, and operating system versions a Smoke run passed against; behavior outside it is unverified rather than assumed.

- **Synthetic session**: A desktop session created solely for verification, with virtual displays and no physical seat; it exercises the same compositor as a user session but cannot evidence physical display or input hardware behavior. _Avoid_: simulated desktop, mock compositor.

- **Gate status**: The recorded outcome of a Release gate: _pending_ when no Smoke run is on record, _blocking - pass_, _blocking - fail_, or _deferred - unverified_; an integration that does not build is not deferred, it is unimplemented.

- **Ungated integration**: An integration that builds and ships but has no Release gate and no Smoke run on record (Sway, Hyprland, macOS); its behavior is unverified, and unlike a deferred gate no trigger closes it. _Avoid_: supported, best-effort.

- **Seated session**: A desktop session attached to a login seat with real input devices and a screen; focus, placement, and accelerator capture can only be observed there, which is what a Synthetic session cannot provide. _Avoid_: real session, physical session.