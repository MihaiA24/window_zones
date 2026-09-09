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
