#[cfg(target_os = "linux")]
use std::env;

#[cfg(target_os = "linux")]
use window_zones::{ScriptedCompositor, WaylandBackend};

use super::{BackendPreference, RuntimeBackend};

pub(in crate::application) fn resolve_runtime_backend(
    preference: BackendPreference,
) -> Result<RuntimeBackend, String> {
    #[cfg(target_os = "linux")]
    {
        resolve_linux_backend(preference)
    }

    #[cfg(target_os = "windows")]
    {
        Ok(match preference {
            BackendPreference::DryRun => RuntimeBackend::DryRun,
            BackendPreference::Windows | BackendPreference::Auto => RuntimeBackend::Windows,
        })
    }

    #[cfg(target_os = "macos")]
    {
        Ok(match preference {
            BackendPreference::DryRun => RuntimeBackend::DryRun,
            BackendPreference::MacOS | BackendPreference::Auto => RuntimeBackend::MacOS,
        })
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        match preference {
            BackendPreference::DryRun => Ok(RuntimeBackend::DryRun),
            BackendPreference::Auto => Ok(RuntimeBackend::Unsupported),
        }
    }
}

#[cfg(target_os = "linux")]
fn resolve_linux_backend(preference: BackendPreference) -> Result<RuntimeBackend, String> {
    let session_type = env::var_os("XDG_SESSION_TYPE");
    let wayland_display = env::var_os("WAYLAND_DISPLAY").is_some();
    let identity = session_identity(
        session_type.as_deref(),
        wayland_display,
        env::var_os("DISPLAY").is_some(),
    );
    let wayland_indicated = wayland_display || session_type.as_deref() == Some("wayland".as_ref());
    resolve_linux_backend_in_session(preference, identity, wayland_indicated)
}

/// `wayland_indicated`: any Wayland signal is present, even one the session
/// identity could not reconcile. X11 is never used then: inside a Wayland
/// session it would only reach XWayland windows.
#[cfg(target_os = "linux")]
fn resolve_linux_backend_in_session(
    preference: BackendPreference,
    identity: SessionIdentity,
    wayland_indicated: bool,
) -> Result<RuntimeBackend, String> {
    match preference {
        BackendPreference::DryRun => Ok(RuntimeBackend::DryRun),
        BackendPreference::X11 => {
            if wayland_indicated {
                Err("X11 backend is unavailable while WAYLAND_DISPLAY or XDG_SESSION_TYPE=wayland is set; use the native Wayland compositor integration".to_string())
            } else {
                Ok(RuntimeBackend::X11)
            }
        }
        BackendPreference::Wayland => {
            if matches!(identity, SessionIdentity::X11) {
                return Err(
                    "Wayland backend requires XDG_SESSION_TYPE=wayland or WAYLAND_DISPLAY"
                        .to_string(),
                );
            }
            resolve_wayland_runtime_backend()
        }
        BackendPreference::Auto => match identity {
            SessionIdentity::Wayland => resolve_wayland_runtime_backend(),
            SessionIdentity::X11 => Ok(RuntimeBackend::X11),
            SessionIdentity::Unresolved(reason) => Err(format!(
                "cannot resolve desktop session from XDG_SESSION_TYPE, WAYLAND_DISPLAY, and DISPLAY: {reason}; use --backend x11|wayland"
            )),
        },
    }
}

#[cfg(target_os = "linux")]
fn resolve_wayland_runtime_backend() -> Result<RuntimeBackend, String> {
    match window_zones::resolve_wayland_backend().map_err(|error| error.to_string())? {
        WaylandBackend::Gnome => Ok(RuntimeBackend::Gnome),
        WaylandBackend::Kde => Ok(RuntimeBackend::Kde),
        WaylandBackend::Sway => Ok(RuntimeBackend::Wayland(ScriptedCompositor::Sway)),
        WaylandBackend::Hyprland => Ok(RuntimeBackend::Wayland(ScriptedCompositor::Hyprland)),
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
enum SessionIdentity {
    X11,
    Wayland,
    Unresolved(String),
}

#[cfg(target_os = "linux")]
fn session_identity(
    session_type: Option<&std::ffi::OsStr>,
    wayland_display: bool,
    display: bool,
) -> SessionIdentity {
    match session_type {
        Some(value) if value == "wayland" => SessionIdentity::Wayland,
        Some(value) if value == "x11" => {
            if wayland_display {
                SessionIdentity::Unresolved(
                    "conflicting XDG_SESSION_TYPE=x11 and WAYLAND_DISPLAY".to_string(),
                )
            } else {
                SessionIdentity::X11
            }
        }
        Some(value) => {
            SessionIdentity::Unresolved(format!("unrecognized XDG_SESSION_TYPE={value:?}"))
        }
        None => match (wayland_display, display) {
            (true, false) => SessionIdentity::Wayland,
            (false, true) => SessionIdentity::X11,
            (true, true) => SessionIdentity::Unresolved(
                "conflicting WAYLAND_DISPLAY and DISPLAY without XDG_SESSION_TYPE".to_string(),
            ),
            (false, false) => SessionIdentity::Unresolved(
                "XDG_SESSION_TYPE, WAYLAND_DISPLAY, and DISPLAY are unset".to_string(),
            ),
        },
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::{
        BackendPreference, RuntimeBackend, SessionIdentity, resolve_linux_backend_in_session,
        session_identity,
    };

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_backend_routing_never_crosses_session_protocols() {
        assert_eq!(
            resolve_linux_backend_in_session(BackendPreference::Auto, SessionIdentity::X11, false),
            Ok(RuntimeBackend::X11)
        );
        assert_eq!(
            resolve_linux_backend_in_session(BackendPreference::X11, SessionIdentity::X11, false),
            Ok(RuntimeBackend::X11)
        );
        assert!(
            resolve_linux_backend_in_session(
                BackendPreference::X11,
                SessionIdentity::Wayland,
                true
            )
            .is_err()
        );
        assert!(
            resolve_linux_backend_in_session(
                BackendPreference::Wayland,
                SessionIdentity::X11,
                false
            )
            .is_err()
        );
        assert_eq!(
            resolve_linux_backend_in_session(
                BackendPreference::DryRun,
                SessionIdentity::Wayland,
                true
            ),
            Ok(RuntimeBackend::DryRun)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn session_identity_uses_unambiguous_protocol_signals() {
        for (session_type, wayland, display, expected) in [
            (None, true, false, SessionIdentity::Wayland),
            (None, false, true, SessionIdentity::X11),
            (Some("wayland"), false, true, SessionIdentity::Wayland),
            (Some("wayland"), true, true, SessionIdentity::Wayland),
            (Some("x11"), false, false, SessionIdentity::X11),
        ] {
            assert_eq!(
                session_identity(session_type.map(std::ffi::OsStr::new), wayland, display),
                expected,
                "{session_type:?}, WAYLAND_DISPLAY={wayland}, DISPLAY={display}"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unresolved_sessions_require_explicit_backend_and_x11_needs_no_wayland_signal() {
        for (session_type, wayland, display) in [
            (None, false, false),
            (Some("x11"), true, false),
            (Some("x11"), true, true),
            (None, true, true),
            (Some("tty"), false, true),
            (Some(""), true, false),
        ] {
            let identity =
                || session_identity(session_type.map(std::ffi::OsStr::new), wayland, display);
            assert!(matches!(identity(), SessionIdentity::Unresolved(_)));
            let error =
                resolve_linux_backend_in_session(BackendPreference::Auto, identity(), wayland)
                    .unwrap_err();
            for hint in [
                "XDG_SESSION_TYPE",
                "WAYLAND_DISPLAY",
                "DISPLAY",
                "--backend x11|wayland",
            ] {
                assert!(error.contains(hint), "{error}");
            }
            let explicit_x11 =
                resolve_linux_backend_in_session(BackendPreference::X11, identity(), wayland);
            assert_eq!(
                explicit_x11.is_ok(),
                !wayland,
                "{session_type:?} {wayland} {display}"
            );
        }
    }
}
