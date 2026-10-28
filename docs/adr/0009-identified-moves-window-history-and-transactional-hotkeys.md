# ADR 0009: Preserve window identity and replace bindings transactionally

A move addresses the window captured during discovery, not whichever window has focus when the request arrives. Re-checking focus only narrows the race; it cannot prevent moving the wrong window. Closed windows fail explicitly. The GNOME and KWin protocols carry this identity and require matching companion upgrades.

The executor owns display correlation and per-window history. Adapters report frame geometry and usable areas rather than independently deciding which display owns a window. History retains the original restore point across partial moves; a user-moved window starts new history. Sway/Hyprland dispatch reaches the running app so it shares that history.

Configuration and the complete registered hotkey set change together. A rejected replacement retains both previous values; unavailable capture is retried without pretending bindings are registered. Applying valid TOML before confirming capture would silently disable working bindings on a desktop-reserved shortcut.

Action semantics and verification commands belong in [README](../../README.md) and the [runbooks](../runbooks/testing.md), not historical implementation-slice records.
