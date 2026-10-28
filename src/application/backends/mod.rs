mod dry_run;
mod hotkeys;
mod selection;
mod window_system;

pub(super) use hotkeys::{NoopHotkeySystem, RuntimeHotkeySystem};
pub(super) use selection::resolve_runtime_backend;
pub(super) use window_system::RuntimeWindowSystem;

#[cfg(target_os = "linux")]
use window_zones::ScriptedCompositor;

#[derive(Debug, Clone, Copy)]
pub(super) enum BackendPreference {
    Auto,
    #[cfg(target_os = "linux")]
    X11,
    #[cfg(target_os = "linux")]
    Wayland,
    #[cfg(target_os = "windows")]
    Windows,
    #[cfg(target_os = "macos")]
    MacOS,
    DryRun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RuntimeBackend {
    DryRun,
    #[cfg(target_os = "linux")]
    X11,
    #[cfg(target_os = "linux")]
    Wayland(ScriptedCompositor),
    #[cfg(target_os = "linux")]
    Gnome,
    #[cfg(target_os = "linux")]
    Kde,
    #[cfg(target_os = "windows")]
    Windows,
    #[cfg(target_os = "macos")]
    MacOS,
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    Unsupported,
}
