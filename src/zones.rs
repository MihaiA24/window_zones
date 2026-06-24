use serde::{Deserialize, Serialize};

use crate::geometry::Rect;

/// Built-in zones available in v1 config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BuiltInZone {
    LeftHalf,
    RightHalf,
    LeftThird,
    CenterThird,
    RightThird,
    LeftTwoThirds,
    RightTwoThirds,
    Maximize,
}

pub const ALL_BUILT_IN_ZONES: [BuiltInZone; 8] = [
    BuiltInZone::LeftHalf,
    BuiltInZone::RightHalf,
    BuiltInZone::LeftThird,
    BuiltInZone::CenterThird,
    BuiltInZone::RightThird,
    BuiltInZone::LeftTwoThirds,
    BuiltInZone::RightTwoThirds,
    BuiltInZone::Maximize,
];

/// Calculates a zone rectangle inside a display usable area.
///
/// Rounding is deterministic and edge-preserving: right/bottom anchored zones
/// end exactly on the usable area's outer edge, and maximize equals usable area.
pub fn rect_for_zone(zone: BuiltInZone, usable_area: Rect) -> Rect {
    match zone {
        BuiltInZone::LeftHalf => left_fraction(usable_area, 2, 1),
        BuiltInZone::RightHalf => right_fraction(usable_area, 2, 1),
        BuiltInZone::LeftThird => left_fraction(usable_area, 3, 1),
        BuiltInZone::CenterThird => center_third(usable_area),
        BuiltInZone::RightThird => right_fraction(usable_area, 3, 1),
        BuiltInZone::LeftTwoThirds => left_two_thirds(usable_area),
        BuiltInZone::RightTwoThirds => right_two_thirds(usable_area),
        BuiltInZone::Maximize => usable_area,
    }
}

fn left_fraction(area: Rect, denominator: u32, numerator: u32) -> Rect {
    Rect::new(
        area.x,
        area.y,
        area.width.saturating_mul(numerator) / denominator,
        area.height,
    )
}

fn right_fraction(area: Rect, denominator: u32, numerator: u32) -> Rect {
    let width = area.width.saturating_mul(numerator) / denominator;
    Rect::new(area.right() - width as i32, area.y, width, area.height)
}

fn center_third(area: Rect) -> Rect {
    let left_width = area.width / 3;
    let right_width = area.width / 3;
    let center_width = area.width - left_width - right_width;

    Rect::new(
        area.x + left_width as i32,
        area.y,
        center_width,
        area.height,
    )
}

fn left_two_thirds(area: Rect) -> Rect {
    let right_third_width = area.width / 3;
    Rect::new(area.x, area.y, area.width - right_third_width, area.height)
}

fn right_two_thirds(area: Rect) -> Rect {
    let left_third_width = area.width / 3;
    Rect::new(
        area.x + left_third_width as i32,
        area.y,
        area.width - left_third_width,
        area.height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calculates_basic_halves_and_maximize() {
        let area = Rect::new(10, 20, 1920, 1080);

        assert_eq!(
            rect_for_zone(BuiltInZone::LeftHalf, area),
            Rect::new(10, 20, 960, 1080)
        );
        assert_eq!(
            rect_for_zone(BuiltInZone::RightHalf, area),
            Rect::new(970, 20, 960, 1080)
        );
        assert_eq!(rect_for_zone(BuiltInZone::Maximize, area), area);
    }

    #[test]
    fn preserves_edges_for_uneven_thirds() {
        let area = Rect::new(0, 0, 100, 50);

        assert_eq!(
            rect_for_zone(BuiltInZone::LeftThird, area),
            Rect::new(0, 0, 33, 50)
        );
        assert_eq!(
            rect_for_zone(BuiltInZone::CenterThird, area),
            Rect::new(33, 0, 34, 50)
        );
        assert_eq!(
            rect_for_zone(BuiltInZone::RightThird, area),
            Rect::new(67, 0, 33, 50)
        );
        assert_eq!(rect_for_zone(BuiltInZone::RightThird, area).right(), 100);
    }

    #[test]
    fn preserves_outer_edges_for_two_thirds() {
        let area = Rect::new(5, 0, 100, 50);

        let left = rect_for_zone(BuiltInZone::LeftTwoThirds, area);
        let right = rect_for_zone(BuiltInZone::RightTwoThirds, area);

        assert_eq!(left, Rect::new(5, 0, 67, 50));
        assert_eq!(right, Rect::new(38, 0, 67, 50));
        assert_eq!(left.x, area.x);
        assert_eq!(right.right(), area.right());
    }
}
