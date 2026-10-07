use std::fmt;

use serde::{Deserialize, Serialize};

/// Platform-neutral action requested by a binding.
///
/// Field-less actions are empty struct variants rather than unit variants:
/// serde's internally tagged unit variants accept and ignore unknown fields,
/// which would let `{ type = "move-to-next-display", zone = "left-half" }` load.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Action {
    /// Moves the window into a named zone. Repeating `left-half` or
    /// `right-half` on the same window cycles through half, two-thirds, and
    /// one third.
    MoveToZone {
        zone: String,
    },
    MoveToNextDisplay {},
    MoveToPreviousDisplay {},
    /// Moves the window to the nearest display in a direction.
    MoveToDisplay {
        direction: Direction,
    },
    /// Centers the window in its display's usable area, keeping its size.
    Center {},
    /// Returns the window to its geometry before the first App move.
    Restore {},
}

/// Spatial direction between displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl fmt::Display for Direction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Direction::Left => "left",
            Direction::Right => "right",
            Direction::Up => "up",
            Direction::Down => "down",
        })
    }
}

/// Configured mapping from a hotkey string to an action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub hotkey: String,
    pub action: Action,
}
