use crate::geometry::Rect;
use crate::zones::{ALL_BUILT_IN_ZONES, rect_for_built_in_zone};
pub const KNOWN_ZONE_MATCH_TOLERANCE_PX: i32 = 2;

/// Moves a window from one display usable area to another.
///
/// If the current window nearly matches a built-in zone on the source display,
/// the same zone is applied on the target display. Otherwise, the window is
/// mapped proportionally from source usable area to target usable area.
pub fn move_window_to_display(
    current: Rect,
    source_usable_area: Rect,
    target_usable_area: Rect,
) -> Rect {
    if let Some(zone) = ALL_BUILT_IN_ZONES.into_iter().find(|zone| {
        current.nearly_equals(
            rect_for_built_in_zone(*zone, source_usable_area),
            KNOWN_ZONE_MATCH_TOLERANCE_PX,
        )
    }) {
        return rect_for_built_in_zone(zone, target_usable_area);
    }

    map_proportionally(current, source_usable_area, target_usable_area)
}

fn map_proportionally(current: Rect, source: Rect, target: Rect) -> Rect {
    if source.width == 0 || source.height == 0 {
        return target;
    }

    let width = scale_length(current.width, source.width, target.width).min(target.width);
    let height = scale_length(current.height, source.height, target.height).min(target.height);

    let rel_x = (current.x - source.x) as f64 / source.width as f64;
    let rel_y = (current.y - source.y) as f64 / source.height as f64;

    let unclamped_x = target.x + (rel_x * target.width as f64).round() as i32;
    let unclamped_y = target.y + (rel_y * target.height as f64).round() as i32;

    let x = clamp_position(unclamped_x, target.x, target.right() - width as i32);
    let y = clamp_position(unclamped_y, target.y, target.bottom() - height as i32);

    Rect::new(x, y, width, height)
}

fn scale_length(length: u32, source_length: u32, target_length: u32) -> u32 {
    ((length as f64 / source_length as f64) * target_length as f64).round() as u32
}

fn clamp_position(value: i32, min: i32, max: i32) -> i32 {
    value.clamp(min, max.max(min))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zones::{BuiltInZone, rect_for_built_in_zone};

    #[test]
    fn preserves_recognized_zone_when_moving_between_displays() {
        let source = Rect::new(0, 0, 1920, 1080);
        let target = Rect::new(1920, 0, 2560, 1440);
        let current = rect_for_built_in_zone(BuiltInZone::LeftHalf, source);

        assert_eq!(
            move_window_to_display(current, source, target),
            rect_for_built_in_zone(BuiltInZone::LeftHalf, target)
        );
    }

    #[test]
    fn preserves_nearly_recognized_zone_when_moving_between_displays() {
        let source = Rect::new(0, 0, 1920, 1080);
        let target = Rect::new(1920, 0, 2560, 1440);
        let current = Rect::new(1, 1, 960, 1079);

        assert_eq!(
            move_window_to_display(current, source, target),
            rect_for_built_in_zone(BuiltInZone::LeftHalf, target)
        );
    }

    #[test]
    fn maps_unrecognized_windows_proportionally() {
        let source = Rect::new(0, 0, 1000, 1000);
        let target = Rect::new(2000, 100, 2000, 1500);
        let current = Rect::new(100, 200, 500, 400);

        assert_eq!(
            move_window_to_display(current, source, target),
            Rect::new(2200, 400, 1000, 600)
        );
    }

    #[test]
    fn clamps_proportional_windows_to_target_usable_area() {
        let source = Rect::new(0, 0, 1000, 1000);
        let target = Rect::new(0, 0, 500, 500);
        let current = Rect::new(900, 900, 300, 300);

        assert_eq!(
            move_window_to_display(current, source, target),
            Rect::new(350, 350, 150, 150)
        );
    }
}
