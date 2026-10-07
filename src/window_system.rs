use std::fmt;

use thiserror::Error;

use crate::geometry::{DisplayGeometry, Rect};

/// Opaque identity of a Window, scoped to one adapter and stable while that
/// window exists. The executor carries it from focused-window discovery to the
/// move so a focus change in between cannot redirect the move to another window.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WindowId(String);

impl WindowId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WindowId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Focused window state as observed by a platform adapter.
///
/// The executor correlates the window to a display using its frame geometry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FocusedWindow {
    pub id: WindowId,
    /// Current window frame geometry in global desktop coordinates.
    pub geometry: Rect,
}

impl FocusedWindow {
    pub fn new(id: WindowId, geometry: Rect) -> Self {
        Self { id, geometry }
    }
}

/// Requested move/resize target for one identified window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowMove {
    pub window: WindowId,
    pub target: Rect,
}

impl WindowMove {
    pub fn new(window: WindowId, target: Rect) -> Self {
        Self { window, target }
    }
}

/// Errors produced by platform-specific window integrations.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum WindowSystemError {
    #[error("platform window-system error: {0}")]
    Platform(String),
    #[error("window {0} no longer exists")]
    WindowGone(WindowId),
}

/// Platform adapter contract used by the core executor.
///
/// Implementations own all OS-specific details: focused-window discovery,
/// display enumeration, and applying a move to the identified window.
/// Returning `Ok(None)` from `focused_window` means there is no focused window
/// for the app to move. `move_window` moves the window named by
/// `WindowMove::window` whether or not it still has focus, restoring a
/// maximized or tiled state first, and fails with `WindowGone` when the window
/// has closed.
pub trait WindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError>;
    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError>;
    fn move_window(&mut self, window_move: &WindowMove) -> Result<(), WindowSystemError>;
}
