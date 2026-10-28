#[cfg(target_os = "macos")]
use window_zones::MacOSWindowSystem;
#[cfg(target_os = "windows")]
use window_zones::WindowsWindowSystem;
use window_zones::{DisplayGeometry, FocusedWindow, WindowMove, WindowSystem, WindowSystemError};
#[cfg(target_os = "linux")]
use window_zones::{GnomeWindowSystem, KwinWindowSystem, WaylandWindowSystem, X11WindowSystem};

use super::RuntimeBackend;
use super::dry_run::DryRunWindowSystem;

#[derive(Debug)]
pub(in crate::application) enum RuntimeWindowSystem {
    DryRun(DryRunWindowSystem),
    #[cfg(target_os = "linux")]
    X11(X11WindowSystem),
    #[cfg(target_os = "linux")]
    Wayland(WaylandWindowSystem),
    #[cfg(target_os = "linux")]
    Gnome(GnomeWindowSystem),
    #[cfg(target_os = "linux")]
    Kde(KwinWindowSystem),
    #[cfg(target_os = "windows")]
    Windows(WindowsWindowSystem),
    #[cfg(target_os = "macos")]
    MacOS(MacOSWindowSystem),
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    Unsupported,
}

/// Runs `$body` with `$system` bound to whichever adapter is active.
macro_rules! with_window_system {
    ($runtime_system:expr, $system:ident => $body:expr) => {
        match $runtime_system {
            RuntimeWindowSystem::DryRun($system) => $body,
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::X11($system) => $body,
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Wayland($system) => $body,
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Gnome($system) => $body,
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Kde($system) => $body,
            #[cfg(target_os = "windows")]
            RuntimeWindowSystem::Windows($system) => $body,
            #[cfg(target_os = "macos")]
            RuntimeWindowSystem::MacOS($system) => $body,
            #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
            RuntimeWindowSystem::Unsupported => Err(WindowSystemError::Platform(
                "window-system adapter is unsupported on this platform".to_string(),
            )),
        }
    };
}

impl RuntimeWindowSystem {
    pub(in crate::application) fn with_backend(backend: RuntimeBackend) -> Self {
        match backend {
            RuntimeBackend::DryRun => Self::DryRun(DryRunWindowSystem::new()),
            #[cfg(target_os = "linux")]
            RuntimeBackend::X11 => Self::X11(X11WindowSystem::new()),
            #[cfg(target_os = "linux")]
            RuntimeBackend::Wayland(compositor) => {
                Self::Wayland(WaylandWindowSystem::new(compositor))
            }
            #[cfg(target_os = "linux")]
            RuntimeBackend::Gnome => Self::Gnome(GnomeWindowSystem::new()),
            #[cfg(target_os = "linux")]
            RuntimeBackend::Kde => Self::Kde(KwinWindowSystem::new()),
            #[cfg(target_os = "windows")]
            RuntimeBackend::Windows => Self::Windows(WindowsWindowSystem::new()),
            #[cfg(target_os = "macos")]
            RuntimeBackend::MacOS => Self::MacOS(MacOSWindowSystem::new()),
            #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
            RuntimeBackend::Unsupported => Self::Unsupported,
        }
    }

    pub(in crate::application) fn last_move(&self) -> Option<&WindowMove> {
        match self {
            Self::DryRun(system) => system.last_move.as_ref(),
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }

    pub(in crate::application) fn name(&self) -> &'static str {
        match self {
            Self::DryRun(_) => "dry-run",
            #[cfg(target_os = "linux")]
            Self::X11(_) => "x11",
            #[cfg(target_os = "linux")]
            Self::Wayland(_) => "wayland",
            #[cfg(target_os = "linux")]
            Self::Gnome(_) => "gnome-wayland",
            #[cfg(target_os = "linux")]
            Self::Kde(_) => "kde-wayland",
            #[cfg(target_os = "windows")]
            Self::Windows(_) => "windows",
            #[cfg(target_os = "macos")]
            Self::MacOS(_) => "macos",
            #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
            Self::Unsupported => "unsupported",
        }
    }

    /// The companion label and its capability report, for companion backends.
    pub(in crate::application) fn companion_capabilities(
        &self,
    ) -> Option<(&'static str, Result<Vec<String>, String>)> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Gnome(system) => Some((
                "GNOME",
                system.capabilities().map_err(|error| error.to_string()),
            )),
            #[cfg(target_os = "linux")]
            Self::Kde(system) => Some((
                "KDE",
                system.capabilities().map_err(|error| error.to_string()),
            )),
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }
}

impl WindowSystem for RuntimeWindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
        with_window_system!(self, system => system.focused_window())
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        with_window_system!(self, system => system.displays())
    }

    fn move_window(&mut self, window_move: &WindowMove) -> Result<(), WindowSystemError> {
        with_window_system!(self, system => system.move_window(window_move))
    }

    fn transition_metrics(
        &self,
    ) -> Result<
        Option<window_zones::window_system::TransitionMetrics>,
        window_zones::WindowSystemError,
    > {
        with_window_system!(self, system => system.transition_metrics())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use window_zones::ScriptedCompositor;

    use super::{RuntimeBackend, RuntimeWindowSystem};

    #[cfg(target_os = "linux")]
    #[test]
    fn runtime_window_backend_names_identify_native_linux_paths() {
        assert_eq!(
            RuntimeWindowSystem::with_backend(RuntimeBackend::X11).name(),
            "x11"
        );
        assert_eq!(
            RuntimeWindowSystem::with_backend(RuntimeBackend::Wayland(ScriptedCompositor::Sway))
                .name(),
            "wayland"
        );
        assert_eq!(
            RuntimeWindowSystem::with_backend(RuntimeBackend::Gnome).name(),
            "gnome-wayland"
        );
        assert_eq!(
            RuntimeWindowSystem::with_backend(RuntimeBackend::Kde).name(),
            "kde-wayland"
        );
    }
}
