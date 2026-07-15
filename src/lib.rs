//! Platform-neutral core for Window Zones.
//!
//! This crate intentionally contains no OS-specific window handles or APIs.
//! Platform adapters are expected to provide focused-window geometry, display
//! usable areas, hotkey registration, and move/resize execution.

pub mod actions;
pub mod config;
pub mod dispatcher;
pub mod display_movement;
pub mod executor;
pub mod geometry;
pub mod hotkey_system;
#[cfg(target_os = "macos")]
pub mod macos_window_system;
pub mod runtime;
#[cfg(target_os = "linux")]
pub mod wayland_window_system;
pub mod window_system;
#[cfg(target_os = "windows")]
pub mod windows_window_system;
#[cfg(target_os = "linux")]
pub mod x11_window_system;
pub mod zones;

pub use actions::{Action, Binding};
pub use config::{
    AppConfig, BindingValidationError, ConfigError, parse_config,
    validate_and_normalize_app_config, validate_and_normalize_bindings,
};
pub use dispatcher::{DispatchHotkeyError, dispatch_hotkey};
pub use display_movement::move_window_to_display;
pub use executor::{ExecuteActionError, execute_action};
pub use geometry::{DisplayGeometry, Rect};
pub use hotkey_system::{HotkeyEvent, HotkeySystem, HotkeySystemError};
#[cfg(target_os = "macos")]
pub use macos_window_system::MacOSWindowSystem;
pub use runtime::{
    App, ConfigLoadError, ConfigPathError, ConfigState, DispatchState, HotkeyRegistrationState,
    default_config_path,
};
#[cfg(target_os = "linux")]
pub use wayland_window_system::WaylandWindowSystem;
pub use window_system::{FocusedWindow, WindowMove, WindowSystem, WindowSystemError};
#[cfg(target_os = "windows")]
pub use windows_window_system::WindowsWindowSystem;
#[cfg(target_os = "linux")]
pub use x11_window_system::X11WindowSystem;
pub use zones::{
    BuiltInZone, ZoneDefinition, built_in_zone_from_name, is_built_in_zone_name,
    rect_for_built_in_zone, rect_for_zone,
};
