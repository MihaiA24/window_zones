# ADR 0003: Native Wayland control belongs to the compositor

Wayland has no generic interface for this app's window-control operations. GNOME and KDE therefore use repository-owned, opt-in companions; Sway and Hyprland use their compositor interfaces. COSMIC requires an opt-in bridge compiled into its compositor: its public management protocol has no arbitrary placement request or loadable window-management extension. Sway, Hyprland, and COSMIC use compositor-bound dispatch into the running App. A Wayland session never silently falls back to X11: XWayland cannot control native Wayland windows.

The Rust app owns configuration, canonical hotkeys, zones, actions, and window/display correlation. Companions expose capabilities and native adapter primitives over the user-session D-Bus connection, not a second implementation of placement policy.

Protocol majors must match; capabilities select optional operations. Missing or incompatible companions fail explicitly and retain configuration. Hotkey-capture outages recover through the runtime retry loop. The app does not launch or replace the GNOME service owner.

For companions offering hotkeys, one active controller owns the complete hotkey set; replacement is atomic and disconnect releases ownership. Inspection remains available to other clients. This avoids competing app instances silently replacing each other's bindings.

The pinned COSMIC patch and source overlay live in `cosmic-comp/`. Its versioned user-session D-Bus interface exposes only window primitives; native custom-command shortcuts remain COSMIC-owned. The App pins the bridge's unique bus owner and Wayland socket identity so a replacement compositor cannot inherit captured window IDs or Restore history. Building the patch does not install or replace the login compositor.

