use serde::{Deserialize, Serialize};

/// Platform-neutral action requested by a binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Action {
    MoveToZone { zone: String },
    MoveToNextDisplay,
    MoveToPreviousDisplay,
}

/// Configured mapping from a hotkey string to an action.
///
/// Hotkey strings are intentionally opaque in the core. Platform adapters own
/// validation and registration because syntax support is platform-dependent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    pub hotkey: String,
    pub action: Action,
}
