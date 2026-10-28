#[cfg(any(target_os = "windows", target_os = "macos"))]
use window_zones::RdevHotkeySystem;
#[cfg(target_os = "linux")]
use window_zones::{GnomeHotkeySystem, KwinHotkeySystem, X11HotkeySystem};
use window_zones::{HotkeyEvent, HotkeySystem, HotkeySystemError};

use super::RuntimeBackend;

#[derive(Debug, Default)]
pub(in crate::application) struct NoopHotkeySystem;

impl HotkeySystem for NoopHotkeySystem {
    fn register_hotkeys(&mut self, _hotkeys: &[String]) -> Result<(), HotkeySystemError> {
        Ok(())
    }

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
        Ok(None)
    }
}

pub(in crate::application) enum RuntimeHotkeySystem {
    Noop(NoopHotkeySystem),
    #[cfg(target_os = "linux")]
    X11(X11HotkeySystem),
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    Global(RdevHotkeySystem),
    #[cfg(target_os = "linux")]
    Gnome(GnomeHotkeySystem),
    #[cfg(target_os = "linux")]
    Kde(KwinHotkeySystem),
}

/// Runs `$body` with `$system` bound to whichever hotkey system is active.
macro_rules! with_hotkey_system {
    ($runtime_system:expr, $system:ident => $body:expr) => {
        match $runtime_system {
            RuntimeHotkeySystem::Noop($system) => $body,
            #[cfg(target_os = "linux")]
            RuntimeHotkeySystem::X11($system) => $body,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            RuntimeHotkeySystem::Global($system) => $body,
            #[cfg(target_os = "linux")]
            RuntimeHotkeySystem::Gnome($system) => $body,
            #[cfg(target_os = "linux")]
            RuntimeHotkeySystem::Kde($system) => $body,
        }
    };
}

impl RuntimeHotkeySystem {
    pub(in crate::application) fn new(backend: RuntimeBackend) -> Self {
        match backend {
            RuntimeBackend::DryRun => Self::Noop(NoopHotkeySystem),
            #[cfg(target_os = "linux")]
            RuntimeBackend::Wayland(_) => Self::Noop(NoopHotkeySystem),
            #[cfg(target_os = "linux")]
            RuntimeBackend::X11 => Self::X11(X11HotkeySystem::new()),
            #[cfg(target_os = "linux")]
            RuntimeBackend::Gnome => Self::Gnome(GnomeHotkeySystem::new()),
            #[cfg(target_os = "linux")]
            RuntimeBackend::Kde => Self::Kde(KwinHotkeySystem::new()),
            #[cfg(target_os = "windows")]
            RuntimeBackend::Windows => Self::Global(RdevHotkeySystem::new()),
            #[cfg(target_os = "macos")]
            RuntimeBackend::MacOS => Self::Global(RdevHotkeySystem::new()),
            #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
            RuntimeBackend::Unsupported => Self::Noop(NoopHotkeySystem),
        }
    }
}

impl HotkeySystem for RuntimeHotkeySystem {
    fn register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), HotkeySystemError> {
        with_hotkey_system!(self, system => system.register_hotkeys(hotkeys))
    }

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
        with_hotkey_system!(self, system => system.next_hotkey())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use window_zones::{HotkeySystem, ScriptedCompositor};

    use super::{RuntimeBackend, RuntimeHotkeySystem};

    #[cfg(target_os = "linux")]
    #[test]
    fn sway_hyprland_accept_compositor_owned_hotkeys() {
        let mut hotkeys =
            RuntimeHotkeySystem::new(RuntimeBackend::Wayland(ScriptedCompositor::Hyprland));
        assert_eq!(hotkeys.register_hotkeys(&[]), Ok(()));
        assert_eq!(hotkeys.register_hotkeys(&["ctrl+a".to_string()]), Ok(()));
        assert_eq!(hotkeys.next_hotkey(), Ok(None));
    }
}
