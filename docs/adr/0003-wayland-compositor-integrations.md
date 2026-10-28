# ADR 0003: Native Wayland control belongs to the compositor

Wayland has no generic interface for this app's window-control operations. GNOME and KDE therefore use repository-owned, opt-in companions; Sway and Hyprland use their compositor interfaces and compositor-bound dispatch into the running app. A Wayland session never silently falls back to X11: XWayland cannot control native Wayland windows.

The Rust app owns configuration, canonical hotkeys, zones, actions, and window/display correlation. Companions expose capabilities and native adapter primitives over the user-session D-Bus connection, not a second implementation of placement policy.

Protocol majors must match; capabilities select optional operations. Missing or incompatible companions fail explicitly, retain configuration, and recover through the runtime retry loop. The app does not launch or replace the GNOME service owner.

One active controller owns a companion's complete hotkey set; replacement is atomic and disconnect releases ownership. Inspection remains available to other clients. This avoids competing app instances silently replacing each other's bindings.
