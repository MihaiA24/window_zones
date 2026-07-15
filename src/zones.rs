use std::collections::BTreeMap;

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

/// User-defined zone geometry expressed in percentages of a display usable area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ZoneDefinition {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl ZoneDefinition {
    pub fn rect_in_area(&self, usable_area: Rect) -> Rect {
        let x = percent_to_pixel(self.x, usable_area.width);
        let y = percent_to_pixel(self.y, usable_area.height);
        let right = percent_to_pixel(self.x + self.width, usable_area.width);
        let bottom = percent_to_pixel(self.y + self.height, usable_area.height);

        let left = clamp_i32(usable_area.x + x as i32, usable_area.x, usable_area.right());
        let top = clamp_i32(
            usable_area.y + y as i32,
            usable_area.y,
            usable_area.bottom(),
        );
        let right = clamp_i32(usable_area.x + right as i32, left, usable_area.right());
        let bottom = clamp_i32(usable_area.y + bottom as i32, top, usable_area.bottom());

        Rect::new(left, top, (right - left) as u32, (bottom - top) as u32)
    }
}

fn percent_to_pixel(percent: u32, dimension: u32) -> u32 {
    dimension.saturating_mul(percent) / 100
}

fn clamp_i32(value: i32, min: i32, max: i32) -> i32 {
    value.clamp(min, max)
}

pub fn built_in_zone_from_name(name: &str) -> Option<BuiltInZone> {
    match name {
        "left-half" => Some(BuiltInZone::LeftHalf),
        "right-half" => Some(BuiltInZone::RightHalf),
        "left-third" => Some(BuiltInZone::LeftThird),
        "center-third" => Some(BuiltInZone::CenterThird),
        "right-third" => Some(BuiltInZone::RightThird),
        "left-two-thirds" => Some(BuiltInZone::LeftTwoThirds),
        "right-two-thirds" => Some(BuiltInZone::RightTwoThirds),
        "maximize" => Some(BuiltInZone::Maximize),
        _ => None,
    }
}

pub fn is_built_in_zone_name(name: &str) -> bool {
    built_in_zone_from_name(name).is_some()
}

/// Calculates a zone rectangle inside a display usable area.
///
/// Rounding is deterministic and edge-preserving: right/bottom anchored zones
/// end exactly on the usable area's outer edge, and maximize equals usable area.
pub fn rect_for_zone(
    zone: &str,
    usable_area: Rect,
    custom_zones: &BTreeMap<String, ZoneDefinition>,
) -> Option<Rect> {
    if let Some(builtin) = built_in_zone_from_name(zone) {
        return Some(rect_for_built_in_zone(builtin, usable_area));
    }

    custom_zones
        .get(zone)
        .map(|definition| definition.rect_in_area(usable_area))
}

pub fn rect_for_built_in_zone(zone: BuiltInZone, usable_area: Rect) -> Rect {
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
    fn resolves_built_in_and_custom_zones() {
        let area = Rect::new(0, 0, 100, 100);
        let mut custom = BTreeMap::new();
        custom.insert(
            "left-top-quarter".to_string(),
            ZoneDefinition {
                x: 0,
                y: 0,
                width: 25,
                height: 25,
            },
        );

        assert_eq!(
            rect_for_zone("left-half", area, &custom),
            Some(Rect::new(0, 0, 50, 100))
        );
        assert_eq!(
            rect_for_zone("left-top-quarter", area, &custom),
            Some(Rect::new(0, 0, 25, 25))
        );
        assert_eq!(rect_for_zone("missing", area, &custom), None);
    }

    #[test]
    fn custom_zones_are_clamped_to_usable_area_boundaries() {
        let area = Rect::new(10, 20, 100, 40);
        let zone = ZoneDefinition {
            x: 10,
            y: 12,
            width: 55,
            height: 50,
        };

        assert_eq!(zone.rect_in_area(area), Rect::new(20, 24, 55, 20));
    }

    #[test]
    fn calculates_basic_halves_and_maximize() {
        let area = Rect::new(10, 20, 1920, 1080);

        assert_eq!(
            rect_for_zone("left-half", area, &BTreeMap::new()),
            Some(Rect::new(10, 20, 960, 1080))
        );
        assert_eq!(
            rect_for_zone("right-half", area, &BTreeMap::new()),
            Some(Rect::new(970, 20, 960, 1080))
        );
        assert_eq!(
            rect_for_zone("maximize", area, &BTreeMap::new()),
            Some(area)
        );
    }

    #[test]
    fn preserves_edges_for_uneven_thirds() {
        let area = Rect::new(0, 0, 100, 50);

        assert_eq!(
            rect_for_zone("left-third", area, &BTreeMap::new()),
            Some(Rect::new(0, 0, 33, 50))
        );
        assert_eq!(
            rect_for_zone("center-third", area, &BTreeMap::new()),
            Some(Rect::new(33, 0, 34, 50))
        );
        assert_eq!(
            rect_for_zone("right-third", area, &BTreeMap::new()),
            Some(Rect::new(67, 0, 33, 50))
        );
        assert_eq!(
            rect_for_zone("right-third", area, &BTreeMap::new())
                .unwrap()
                .right(),
            100
        );
    }

    #[test]
    fn preserves_outer_edges_for_two_thirds() {
        let area = Rect::new(5, 0, 100, 50);

        let left = rect_for_zone("left-two-thirds", area, &BTreeMap::new()).unwrap();
        let right = rect_for_zone("right-two-thirds", area, &BTreeMap::new()).unwrap();

        assert_eq!(left, Rect::new(5, 0, 67, 50));
        assert_eq!(right, Rect::new(38, 0, 67, 50));
        assert_eq!(left.x, area.x);
        assert_eq!(right.right(), area.right());
    }
}
