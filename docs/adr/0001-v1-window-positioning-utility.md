# ADR 0001: Position windows; do not replace the window manager

Window Zones is a background utility that moves existing OS-managed windows in response to configured bindings. Focus policy, stacking, workspaces, full tiling-layout ownership, snapping previews, and persistent session restore remain outside its scope.

Geometry, zones, actions, config, and window history stay platform-neutral. Native adapters own window discovery, usable areas, identified moves, and hotkey capture; CLI, TUI, and tray share the same runtime.

Becoming a window manager would require owning platform-specific desktop policy rather than integrating with the user's existing desktop.
