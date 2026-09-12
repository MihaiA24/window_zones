use serde::{Deserialize, Serialize};

/// Platform-neutral action requested by a binding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Action {
    MoveToZone { zone: String },
    MoveToNextDisplay,
    MoveToPreviousDisplay,
}

/// Configured mapping from a hotkey string to an action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub hotkey: String,
    pub action: Action,
}
