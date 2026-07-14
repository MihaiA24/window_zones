use thiserror::Error;

/// Event generated when a configured global hotkey is activated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyEvent {
    Pressed { hotkey: String },
}

/// Errors produced by platform hotkey adapters.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HotkeySystemError {
    #[error("platform hotkey error: {0}")]
    Platform(String),
}

/// Platform adapter contract for global hotkey registration and dispatch.
pub trait HotkeySystem {
    fn register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), HotkeySystemError>;

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError>;
}
