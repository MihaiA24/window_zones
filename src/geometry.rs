use serde::{Deserialize, Serialize};

/// Integer pixel rectangle in global desktop coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(self) -> i32 {
        self.x.saturating_add_unsigned(self.width)
    }

    pub fn bottom(self) -> i32 {
        self.y.saturating_add_unsigned(self.height)
    }

    pub fn nearly_equals(self, other: Self, tolerance_px: u32) -> bool {
        self.x.abs_diff(other.x) <= tolerance_px
            && self.y.abs_diff(other.y) <= tolerance_px
            && self.right().abs_diff(other.right()) <= tolerance_px
            && self.bottom().abs_diff(other.bottom()) <= tolerance_px
    }
}

/// Display geometry known to the core. `usable_area` excludes taskbars, panels,
/// docks, and similar reserved regions when the platform adapter can provide it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisplayGeometry {
    pub id: String,
    pub usable_area: Rect,
}

impl DisplayGeometry {
    pub fn new(id: impl Into<String>, usable_area: Rect) -> Self {
        Self {
            id: id.into(),
            usable_area,
        }
    }
}
