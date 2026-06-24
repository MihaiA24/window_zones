use thiserror::Error;

use crate::geometry::{DisplayGeometry, Rect};

/// Focused window state as observed by a platform adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusedWindow {
    /// Display that currently contains the focused window, using an adapter-provided id.
    pub display_id: String,
    /// Current window geometry in global desktop coordinates.
    pub geometry: Rect,
}

impl FocusedWindow {
    pub fn new(display_id: impl Into<String>, geometry: Rect) -> Self {
        Self {
            display_id: display_id.into(),
            geometry,
        }
    }
}

/// Requested move/resize target for the focused window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowMove {
    pub target: Rect,
}

impl WindowMove {
    pub const fn new(target: Rect) -> Self {
        Self { target }
    }
}

/// Errors produced by platform-specific window integrations.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WindowSystemError {
    #[error("platform window-system error: {0}")]
    Platform(String),
}

/// Platform adapter contract used by the core executor.
///
/// Implementations own all OS-specific details: focused-window discovery,
/// display enumeration/order, and applying the requested move to the focused
/// window. Returning `Ok(None)` from `focused_window` means there is no focused
/// window for the app to move.
pub trait WindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError>;
    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError>;
    fn move_focused_window(&mut self, window_move: WindowMove) -> Result<(), WindowSystemError>;
}
