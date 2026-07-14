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
pub mod runtime;
pub mod window_system;
pub mod zones;

pub use actions::{Action, Binding};
pub use config::{
    AppConfig, BindingValidationError, ConfigError, normalize_hotkey, parse_config,
    validate_and_normalize_bindings,
};
pub use dispatcher::{DispatchHotkeyError, dispatch_hotkey};
pub use display_movement::move_window_to_display;
pub use executor::{ExecuteActionError, execute_action};
pub use geometry::{DisplayGeometry, Rect};
pub use hotkey_system::{HotkeyEvent, HotkeySystem, HotkeySystemError};
pub use runtime::{
    App, ConfigLoadError, ConfigPathError, ConfigState, DispatchState, HotkeyRegistrationState,
    default_config_path,
};
pub use window_system::{FocusedWindow, WindowMove, WindowSystem, WindowSystemError};
pub use zones::{BuiltInZone, rect_for_zone};
