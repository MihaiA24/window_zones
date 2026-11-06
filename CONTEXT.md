# Window Zones

Window Zones positions existing windows within the user's desktop; it does not manage the desktop itself.

## Language

**Window**: An existing application window managed by the operating system or compositor.

**Window frame**: The rectangle used for placement, including window decorations where available; not the client-content rectangle.

**Display**: A physical or virtual monitor. Its **usable area** excludes reserved panels and docks.

**Zone**: A named rectangle within a display's usable area.

**Action**: A placement request: move to a zone or display, center, or restore.

**Binding**: A configured association between a hotkey and an action.

**Companion**: The compositor-owned counterpart exposing native window control and, where supported, hotkeys to the App on Wayland.
_Avoid_: generic Wayland backend.

**Capability**: An operation a companion supports. A **degraded integration** is reachable but lacks one or more capabilities.

**Compositor-bound dispatch**: A Sway, Hyprland, or COSMIC keybinding forwarding an action to the running App instead of the App capturing global hotkeys.

**Window identity**: An opaque, adapter-scoped identity that keeps a move addressed to the captured window even if focus changes.

**Window history**: Per-window memory within one app run: the original restore point and last app placement. It is not persistent session restore.

**Release gate**: A platform configuration whose required smoke run blocks release; a **deferred** gate ships implemented but unverified.

**Synthetic session**: A desktop created for verification without a physical login seat; it cannot establish hardware behavior.

**Seated session**: A desktop attached to a login seat with real input devices and a screen.
