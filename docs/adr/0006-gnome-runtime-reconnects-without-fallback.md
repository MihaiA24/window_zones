# GNOME runtime reconnects without X11 fallback

When Wayland session resolution selects GNOME but its companion is absent or temporarily unavailable, the App starts in an explicit unavailable state, keeps configuration and status usable, retries on the existing runtime loop, and re-registers the current hotkeys after recovery; it never substitutes X11. Diagnostics are emitted on state changes, not every retry.
