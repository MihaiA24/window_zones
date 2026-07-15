//! Linux Wayland adapter for `WindowSystem`.
//!
//! This adapter is intentionally capability-limited. Generic window discovery
//! and movement are not implemented because cross-compositor Wayland support
//! requires protocol- and runtime-specific integrations.
//!
//! Use this adapter to surface explicit diagnostics:
//! - if the session is not Wayland, it reports session mismatch;
//! - if it is Wayland, it reports unsupported protocol capability.

use crate::{DisplayGeometry, FocusedWindow, Rect, WindowMove, WindowSystem, WindowSystemError};

use std::ffi::OsString;

/// Adapter handle for Linux Wayland sessions.
#[derive(Debug, Default)]
pub struct WaylandWindowSystem;

impl WaylandWindowSystem {
    pub fn new() -> Self {
        Self
    }

    fn session_error() -> WindowSystemError {
        if is_wayland_session() {
            WindowSystemError::Platform(
                "Wayland adapter is currently unsupported for focused-window discovery and movement".to_string(),
            )
        } else {
            WindowSystemError::Platform(
                "Wayland adapter can only be used when XDG_SESSION_TYPE=wayland or WAYLAND_DISPLAY is set".to_string(),
            )
        }
    }
}

fn is_wayland_session() -> bool {
    is_wayland_session_with_env(|name| std::env::var_os(name))
}

fn is_wayland_session_with_env(get_env: impl Fn(&str) -> Option<OsString>) -> bool {
    get_env("XDG_SESSION_TYPE")
        .and_then(|value| value.to_str().map(|value| value.to_ascii_lowercase()))
        .is_some_and(|value| value == "wayland")
        || get_env("WAYLAND_DISPLAY").is_some()
}

impl WindowSystem for WaylandWindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
        Err(Self::session_error())
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        Err(Self::session_error())
    }

    fn move_focused_window(&mut self, _window_move: WindowMove) -> Result<(), WindowSystemError> {
        Err(Self::session_error())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WindowSystemError;
    use std::env;

    fn set_env(key: &str, value: Option<&str>) {
        if let Some(value) = value {
            env::set_var(key, value);
        } else {
            env::remove_var(key);
        }
    }

    fn restore_env(vars: &[(String, Option<OsString>)]) {
        for (key, value) in vars {
            match value {
                Some(value) => env::set_var(key, value),
                None => env::remove_var(key),
            }
        }
    }

    #[test]
    fn non_wayland_session_returns_session_error() {
        let backups = [
            (
                "XDG_SESSION_TYPE".to_string(),
                env::var_os("XDG_SESSION_TYPE"),
            ),
            (
                "WAYLAND_DISPLAY".to_string(),
                env::var_os("WAYLAND_DISPLAY"),
            ),
        ];

        set_env("XDG_SESSION_TYPE", Some("x11"));
        set_env("WAYLAND_DISPLAY", None);

        let error = WaylandWindowSystem::new().displays().unwrap_err();
        assert!(matches!(
            error,
            WindowSystemError::Platform(message)
                if message.contains("can only be used when XDG_SESSION_TYPE=wayland")
        ));

        restore_env(&backups);
    }

    #[test]
    fn wayland_session_reports_unsupported_capability() {
        let backups = [
            (
                "XDG_SESSION_TYPE".to_string(),
                env::var_os("XDG_SESSION_TYPE"),
            ),
            (
                "WAYLAND_DISPLAY".to_string(),
                env::var_os("WAYLAND_DISPLAY"),
            ),
        ];

        set_env("XDG_SESSION_TYPE", Some("wayland"));
        set_env("WAYLAND_DISPLAY", Some("wayland-0"));

        let error = WaylandWindowSystem::new().displays().unwrap_err();
        assert!(matches!(
            error,
            WindowSystemError::Platform(message)
                if message.contains("currently unsupported for focused-window discovery and movement")
        ));

        restore_env(&backups);
    }
}
