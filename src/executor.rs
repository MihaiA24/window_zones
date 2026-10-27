use std::collections::{BTreeMap, HashMap};

use thiserror::Error;

use crate::actions::{Action, Direction};
use crate::display_movement::{KNOWN_ZONE_MATCH_TOLERANCE_PX, move_window_to_display};
use crate::geometry::{DisplayGeometry, Rect};
use crate::window_system::{FocusedWindow, WindowId, WindowMove, WindowSystem, WindowSystemError};
use crate::zones::{BuiltInZone, ZoneDefinition, rect_for_built_in_zone, rect_for_zone};

/// A window whose frame is within this many pixels of the geometry the App last
/// applied is still "where the App put it": compositors round sizes to client
/// constraints (terminal cells, minimum sizes), so an exact match is too strict.
const APPLIED_GEOMETRY_TOLERANCE_PX: u32 = 16;
/// Bound on remembered windows; the oldest record is dropped beyond it.
const HISTORY_CAPACITY: usize = 256;

/// Errors the platform-neutral executor can classify before or after calling
/// the platform adapter.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ExecuteActionError {
    #[error(transparent)]
    WindowSystem(#[from] WindowSystemError),
    #[error("no focused window")]
    NoFocusedWindow,
    #[error("no displays available")]
    NoDisplays,
    #[error("no display {direction} of the focused window's display")]
    NoDisplayInDirection { direction: Direction },
    #[error("nothing to restore: the App has not moved the focused window")]
    NothingToRestore,
    #[error("unknown zone: {zone}")]
    UnknownZone { zone: String },
}

/// Per-window memory of the App's own moves and the geometry to restore.
#[derive(Debug, Default)]
pub struct WindowHistory {
    records: HashMap<WindowId, WindowRecord>,
    next_sequence: u64,
}

#[derive(Debug, Clone)]
struct WindowRecord {
    /// Geometry before the first App move since the user last placed the window.
    restore: Rect,
    /// Geometry the App last applied.
    applied: Rect,
    sequence: u64,
}

impl WindowHistory {
    fn record(&self, window: &WindowId) -> Option<&WindowRecord> {
        self.records.get(window)
    }

    /// The record for a window that is still where the App last put it.
    fn placed_record(&self, window: &FocusedWindow) -> Option<&WindowRecord> {
        self.record(&window.id).filter(|record| {
            record
                .applied
                .nearly_equals(window.geometry, APPLIED_GEOMETRY_TOLERANCE_PX)
        })
    }

    fn remember(&mut self, window: &FocusedWindow, applied: Rect) {
        let restore = self
            .placed_record(window)
            .map_or(window.geometry, |record| record.restore);

        if !self.records.contains_key(&window.id) && self.records.len() >= HISTORY_CAPACITY {
            let oldest = self
                .records
                .iter()
                .min_by_key(|(_, record)| record.sequence)
                .map(|(id, _)| id.clone());
            if let Some(oldest) = oldest {
                self.records.remove(&oldest);
            }
        }

        let sequence = self.next_sequence;
        self.next_sequence += 1;
        self.records.insert(
            window.id.clone(),
            WindowRecord {
                restore,
                applied,
                sequence,
            },
        );
    }

    fn forget(&mut self, window: &WindowId) {
        self.records.remove(window);
    }
}

/// Executes an action by asking the provided window system for current state,
/// calculating a platform-neutral target rectangle, and moving the window that
/// was focused when the action started.
pub fn execute_action<W: WindowSystem>(
    action: &Action,
    custom_zones: &BTreeMap<String, ZoneDefinition>,
    history: &mut WindowHistory,
    window_system: &mut W,
) -> Result<(), ExecuteActionError> {
    let focused = window_system
        .focused_window()?
        .ok_or(ExecuteActionError::NoFocusedWindow)?;
    if let Some(record) = history.record(&focused.id)
        && !record
            .applied
            .nearly_equals(focused.geometry, APPLIED_GEOMETRY_TOLERANCE_PX)
    {
        history.forget(&focused.id);
    }

    let mut displays = window_system.displays()?;
    displays.retain(|display| display.usable_area.width > 0 && display.usable_area.height > 0);
    if displays.is_empty() {
        return Err(ExecuteActionError::NoDisplays);
    }
    // Next/previous follow the desktop's spatial layout, left to right then top
    // to bottom, not the order an adapter happens to enumerate displays in.
    displays.sort_by_key(|display| (display.usable_area.x, display.usable_area.y));

    let current_index = display_index_for_window(&displays, focused.geometry);
    let current_area = displays[current_index].usable_area;

    let target = match action {
        Action::MoveToZone { zone } => {
            let target = rect_for_zone(zone, current_area, custom_zones).ok_or_else(|| {
                ExecuteActionError::UnknownZone {
                    zone: zone.to_string(),
                }
            })?;
            let direction = match zone.as_str() {
                "left-half" => Some((true, BuiltInZone::RightHalf)),
                "right-half" => Some((false, BuiltInZone::LeftHalf)),
                _ => None,
            };
            if let Some((left, landing_zone)) = direction
                && focused
                    .geometry
                    .nearly_equals(target, KNOWN_ZONE_MATCH_TOLERANCE_PX)
                && let Some(neighbour) = horizontal_neighbour(&displays, current_index, left)
            {
                rect_for_built_in_zone(landing_zone, neighbour.usable_area)
            } else {
                target
            }
        }
        Action::MoveToNextDisplay {} | Action::MoveToPreviousDisplay {} => {
            if displays.len() == 1 {
                return Ok(());
            }
            let target_index = if matches!(action, Action::MoveToNextDisplay {}) {
                (current_index + 1) % displays.len()
            } else {
                (current_index + displays.len() - 1) % displays.len()
            };
            move_window_to_display(
                focused.geometry,
                current_area,
                displays[target_index].usable_area,
            )
        }
        Action::MoveToDisplay { direction } => {
            let target_index = display_in_direction(&displays, current_index, *direction).ok_or(
                ExecuteActionError::NoDisplayInDirection {
                    direction: *direction,
                },
            )?;
            move_window_to_display(
                focused.geometry,
                current_area,
                displays[target_index].usable_area,
            )
        }
        Action::Center {} => centered(focused.geometry, current_area),
        Action::Restore {} => {
            let restore = history
                .record(&focused.id)
                .map(|record| record.restore)
                .ok_or(ExecuteActionError::NothingToRestore)?;
            move_window(window_system, history, &focused, restore)?;
            history.forget(&focused.id);
            return Ok(());
        }
    };

    move_window(window_system, history, &focused, target)?;
    history.remember(&focused, target);
    Ok(())
}

fn horizontal_neighbour(
    displays: &[DisplayGeometry],
    current_index: usize,
    left: bool,
) -> Option<&DisplayGeometry> {
    let current = &displays[current_index];
    let source = current.usable_area;
    let source_right = i64::from(source.x) + i64::from(source.width);
    let source_bottom = i64::from(source.y) + i64::from(source.height);
    let source_center_y = 2 * i64::from(source.y) + i64::from(source.height);

    displays
        .iter()
        .filter(|display| display.id != current.id)
        .filter_map(|display| {
            let area = display.usable_area;
            let right = i64::from(area.x) + i64::from(area.width);
            let bottom = i64::from(area.y) + i64::from(area.height);
            if source_bottom.min(bottom) <= i64::from(source.y).max(i64::from(area.y)) {
                return None;
            }
            let gap = if left {
                i64::from(source.x) - right
            } else {
                i64::from(area.x) - source_right
            };
            if gap < 0 {
                return None;
            }
            let alignment =
                (source_center_y - (2 * i64::from(area.y) + i64::from(area.height))).abs();
            Some((
                (
                    gap,
                    alignment,
                    i64::from(area.y),
                    i64::from(area.x),
                    &display.id,
                ),
                display,
            ))
        })
        .min_by(|(key, _), (other_key, _)| key.cmp(other_key))
        .map(|(_, display)| display)
}

fn move_window<W: WindowSystem>(
    window_system: &mut W,
    history: &mut WindowHistory,
    window: &FocusedWindow,
    target: Rect,
) -> Result<(), ExecuteActionError> {
    window_system
        .move_window(&WindowMove::new(window.id.clone(), target))
        .map_err(|error| {
            match &error {
                WindowSystemError::WindowGone(_) => history.forget(&window.id),
                WindowSystemError::PartialMove { observed, .. } => {
                    history.remember(window, *observed);
                }
                WindowSystemError::Platform(_) => {}
            }
            ExecuteActionError::from(error)
        })
}

/// The display that owns a window: the one whose usable area contains the frame
/// center, else the one the frame overlaps most, else the nearest one. A window
/// straddling displays, centered over a panel, or partly off-screen still
/// belongs somewhere, so every action can rescue it.
fn display_index_for_window(displays: &[DisplayGeometry], geometry: Rect) -> usize {
    let (center_x, center_y) = center(geometry);

    if let Some(index) = displays
        .iter()
        .position(|display| contains(display.usable_area, center_x, center_y))
    {
        return index;
    }

    let overlaps = displays
        .iter()
        .map(|display| overlap_area(display.usable_area, geometry));
    if let Some((index, overlap)) = overlaps
        .enumerate()
        .max_by_key(|(index, overlap)| (*overlap, std::cmp::Reverse(*index)))
        && overlap > 0
    {
        return index;
    }

    displays
        .iter()
        .enumerate()
        .min_by_key(|(_, display)| distance_squared(display.usable_area, center_x, center_y))
        .map_or(0, |(index, _)| index)
}

fn center(rect: Rect) -> (i64, i64) {
    (
        i64::from(rect.x) + i64::from(rect.width / 2),
        i64::from(rect.y) + i64::from(rect.height / 2),
    )
}

fn contains(area: Rect, x: i64, y: i64) -> bool {
    i64::from(area.x) <= x
        && x < i64::from(area.x) + i64::from(area.width)
        && i64::from(area.y) <= y
        && y < i64::from(area.y) + i64::from(area.height)
}

fn overlap_area(a: Rect, b: Rect) -> i64 {
    let span = |a_start: i32, a_len: u32, b_start: i32, b_len: u32| {
        let start = i64::from(a_start).max(i64::from(b_start));
        let end =
            (i64::from(a_start) + i64::from(a_len)).min(i64::from(b_start) + i64::from(b_len));
        (end - start).max(0)
    };
    span(a.x, a.width, b.x, b.width) * span(a.y, a.height, b.y, b.height)
}

fn distance_squared(area: Rect, x: i64, y: i64) -> i64 {
    let axis = |value: i64, start: i32, length: u32| {
        let start = i64::from(start);
        let end = start + i64::from(length);
        if value < start {
            start - value
        } else if value >= end {
            value - end + 1
        } else {
            0
        }
    };
    let dx = axis(x, area.x, area.width);
    let dy = axis(y, area.y, area.height);
    dx * dx + dy * dy
}

/// The nearest display whose center lies in `direction` from the current
/// display's center, preferring displays that share the perpendicular span.
fn display_in_direction(
    displays: &[DisplayGeometry],
    current_index: usize,
    direction: Direction,
) -> Option<usize> {
    let current = displays[current_index].usable_area;
    let (current_x, current_y) = center(current);

    displays
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != current_index)
        .filter_map(|(index, display)| {
            let area = display.usable_area;
            let (x, y) = center(area);
            let (primary, perpendicular, shares_span) = match direction {
                Direction::Left => (
                    current_x - x,
                    (y - current_y).abs(),
                    spans_overlap_y(current, area),
                ),
                Direction::Right => (
                    x - current_x,
                    (y - current_y).abs(),
                    spans_overlap_y(current, area),
                ),
                Direction::Up => (
                    current_y - y,
                    (x - current_x).abs(),
                    spans_overlap_x(current, area),
                ),
                Direction::Down => (
                    y - current_y,
                    (x - current_x).abs(),
                    spans_overlap_x(current, area),
                ),
            };
            (primary > 0).then_some((index, (!shares_span, primary, perpendicular)))
        })
        .min_by_key(|(_, score)| *score)
        .map(|(index, _)| index)
}

fn spans_overlap_x(a: Rect, b: Rect) -> bool {
    i64::from(a.x) < i64::from(b.x) + i64::from(b.width)
        && i64::from(b.x) < i64::from(a.x) + i64::from(a.width)
}

fn spans_overlap_y(a: Rect, b: Rect) -> bool {
    i64::from(a.y) < i64::from(b.y) + i64::from(b.height)
        && i64::from(b.y) < i64::from(a.y) + i64::from(a.height)
}

/// The window's size, shrunk to fit if needed, centered in the usable area.
fn centered(window: Rect, area: Rect) -> Rect {
    let width = window.width.min(area.width);
    let height = window.height.min(area.height);
    Rect::new(
        area.x + ((area.width - width) / 2) as i32,
        area.y + ((area.height - height) / 2) as i32,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use crate::geometry::Rect;
    use crate::window_system::FakeWindowSystem;
    use crate::{DisplayGeometry, ZoneDefinition};

    fn move_to_left_half_action() -> (String, BTreeMap<String, ZoneDefinition>) {
        ("left-half".to_string(), BTreeMap::new())
    }

    fn window_id() -> WindowId {
        WindowId::new("window")
    }

    fn fake_with_focus(geometry: Rect) -> FakeWindowSystem {
        FakeWindowSystem::with_focus(geometry)
    }

    fn moved_to(target: Rect) -> WindowMove {
        WindowMove::new(window_id(), target)
    }

    fn zone(name: &str) -> Action {
        Action::MoveToZone {
            zone: name.to_string(),
        }
    }

    fn execute(fake: &mut FakeWindowSystem, action: &Action) -> Result<(), ExecuteActionError> {
        execute_action(
            action,
            &BTreeMap::new(),
            &mut WindowHistory::default(),
            fake,
        )
    }

    fn targets(fake: &FakeWindowSystem) -> Vec<Rect> {
        fake.moves
            .iter()
            .map(|window_move| window_move.target)
            .collect()
    }

    #[test]
    fn moves_focused_window_to_zone_on_current_display() {
        let mut fake = FakeWindowSystem::with_focus(Rect::new(100, 100, 800, 600));

        let (zone, zones) = move_to_left_half_action();
        execute_action(
            &Action::MoveToZone { zone },
            &zones,
            &mut WindowHistory::default(),
            &mut fake,
        )
        .unwrap();

        assert_eq!(fake.moves, vec![moved_to(Rect::new(0, 0, 960, 1080))]);
    }

    #[test]
    fn repeated_halves_land_on_nearest_edges_of_physical_neighbours() {
        for (zone, geometry, expected) in [
            (
                "left-half",
                Rect::new(0, 0, 960, 1080),
                Rect::new(-800, -200, 800, 1200),
            ),
            (
                "right-half",
                Rect::new(960, 0, 960, 1080),
                Rect::new(1920, 80, 1280, 1440),
            ),
        ] {
            let mut fake = fake_with_focus(geometry);
            fake.displays = Ok(vec![
                DisplayGeometry::new("far-right", Rect::new(5000, 0, 1600, 900)),
                DisplayGeometry::new("left", Rect::new(-1600, -200, 1600, 1200)),
                DisplayGeometry::new("right", Rect::new(1920, 80, 2560, 1440)),
                DisplayGeometry::new("current", Rect::new(0, 0, 1920, 1080)),
                DisplayGeometry::new("far-left", Rect::new(i32::MIN, 0, 1600, 900)),
            ]);
            execute_action(
                &Action::MoveToZone {
                    zone: zone.to_string(),
                },
                &BTreeMap::new(),
                &mut WindowHistory::default(),
                &mut fake,
            )
            .unwrap();

            assert_eq!(fake.moves, vec![moved_to(expected)], "{zone}");
        }
    }

    #[test]
    fn neighbours_require_vertical_overlap_then_rank_gap_and_alignment_without_list_order() {
        for (neighbours, expected) in [
            (
                vec![
                    DisplayGeometry::new("below", Rect::new(1920, 1080, 1280, 720)),
                    DisplayGeometry::new("above", Rect::new(1920, -720, 1280, 720)),
                    DisplayGeometry::new("overlapping", Rect::new(2200, 200, 1600, 1000)),
                ],
                Rect::new(2200, 200, 800, 1000),
            ),
            (
                vec![
                    DisplayGeometry::new("far-aligned", Rect::new(2200, 0, 1600, 1080)),
                    DisplayGeometry::new("near-offset", Rect::new(1920, 700, 1280, 1080)),
                ],
                Rect::new(1920, 700, 640, 1080),
            ),
            (
                vec![
                    DisplayGeometry::new("offset", Rect::new(1920, 400, 1280, 1080)),
                    DisplayGeometry::new("aligned", Rect::new(1920, -100, 1600, 1280)),
                ],
                Rect::new(1920, -100, 800, 1280),
            ),
            (
                vec![
                    DisplayGeometry::new("lower", Rect::new(1920, 200, 1280, 1080)),
                    DisplayGeometry::new("upper", Rect::new(1920, -200, 1280, 1080)),
                ],
                Rect::new(1920, -200, 640, 1080),
            ),
        ] {
            for reverse in [false, true] {
                let mut fake = fake_with_focus(Rect::new(960, 0, 960, 1080));
                let mut displays = neighbours.clone();
                displays.push(DisplayGeometry::new("current", Rect::new(0, 0, 1920, 1080)));
                if reverse {
                    displays.reverse();
                }
                fake.displays = Ok(displays);
                execute_action(
                    &Action::MoveToZone {
                        zone: "right-half".to_string(),
                    },
                    &BTreeMap::new(),
                    &mut WindowHistory::default(),
                    &mut fake,
                )
                .unwrap();

                assert_eq!(fake.moves, vec![moved_to(expected)], "reverse={reverse}");
            }
        }
    }

    #[test]
    fn repeated_halves_stay_at_outer_edges_and_on_a_single_display() {
        for (zone, geometry, single_display) in [
            ("left-half", Rect::new(0, 0, 960, 1080), false),
            ("right-half", Rect::new(3200, 0, 1280, 1440), false),
            ("left-half", Rect::new(0, 0, 960, 1080), true),
            ("right-half", Rect::new(960, 0, 960, 1080), true),
        ] {
            let mut fake = fake_with_focus(geometry);
            if single_display {
                fake.displays = Ok(vec![DisplayGeometry::new(
                    "left",
                    Rect::new(0, 0, 1920, 1080),
                )]);
            }
            execute_action(
                &Action::MoveToZone {
                    zone: zone.to_string(),
                },
                &BTreeMap::new(),
                &mut WindowHistory::default(),
                &mut fake,
            )
            .unwrap();

            assert_eq!(
                fake.moves,
                vec![moved_to(geometry)],
                "{zone}, single={single_display}"
            );
        }
    }

    #[test]
    fn repeated_half_matching_includes_two_pixels_but_not_three_on_any_edge() {
        for (geometry, expected) in [
            (Rect::new(962, 2, 960, 1080), Rect::new(1920, 0, 1280, 1440)),
            (Rect::new(963, 0, 957, 1080), Rect::new(960, 0, 960, 1080)),
            (Rect::new(960, -3, 960, 1083), Rect::new(960, 0, 960, 1080)),
            (Rect::new(960, 0, 963, 1080), Rect::new(960, 0, 960, 1080)),
            (Rect::new(960, 0, 960, 1083), Rect::new(960, 0, 960, 1080)),
        ] {
            let mut fake = fake_with_focus(geometry);
            execute_action(
                &Action::MoveToZone {
                    zone: "right-half".to_string(),
                },
                &BTreeMap::new(),
                &mut WindowHistory::default(),
                &mut fake,
            )
            .unwrap();

            assert_eq!(fake.moves, vec![moved_to(expected)], "{geometry:?}");
        }
    }

    #[test]
    fn repeated_non_half_zones_remain_on_current_display() {
        for (zone, expected) in [
            ("side", Rect::new(1920, 0, 1280, 1440)),
            ("left-third", Rect::new(1920, 0, 853, 1440)),
            ("center-third", Rect::new(2773, 0, 854, 1440)),
            ("right-third", Rect::new(3627, 0, 853, 1440)),
            ("left-two-thirds", Rect::new(1920, 0, 1707, 1440)),
            ("right-two-thirds", Rect::new(2773, 0, 1707, 1440)),
            ("maximize", Rect::new(1920, 0, 2560, 1440)),
        ] {
            let mut fake = fake_with_focus(expected);
            let zones = BTreeMap::from([(
                "side".to_string(),
                ZoneDefinition {
                    x: 0,
                    y: 0,
                    width: 50,
                    height: 100,
                },
            )]);
            execute_action(
                &Action::MoveToZone {
                    zone: zone.to_string(),
                },
                &zones,
                &mut WindowHistory::default(),
                &mut fake,
            )
            .unwrap();

            assert_eq!(fake.moves, vec![moved_to(expected)], "{zone}");
        }
    }

    #[test]
    fn uses_second_display_when_overlapping_window_center_is_on_shared_edge() {
        let mut fake = fake_with_focus(Rect::new(1520, 100, 800, 600));

        execute(&mut fake, &zone("left-half")).unwrap();

        assert_eq!(fake.moves, vec![moved_to(Rect::new(1920, 0, 1280, 1440))]);
    }

    #[test]
    fn moves_to_custom_zone_from_map() {
        let mut fake = FakeWindowSystem::with_focus(Rect::new(200, 200, 800, 600));

        let mut zones = BTreeMap::new();
        zones.insert(
            "side".to_string(),
            ZoneDefinition {
                x: 50,
                y: 0,
                width: 50,
                height: 100,
            },
        );

        execute_action(
            &zone("side"),
            &zones,
            &mut WindowHistory::default(),
            &mut fake,
        )
        .unwrap();

        assert_eq!(fake.moves, vec![moved_to(Rect::new(960, 0, 960, 1080))]);
    }

    #[test]
    fn moves_to_next_display_preserving_recognized_zone() {
        let mut fake = FakeWindowSystem::with_focus(Rect::new(0, 0, 960, 1080));

        execute(&mut fake, &Action::MoveToNextDisplay {}).unwrap();

        assert_eq!(fake.moves, vec![moved_to(Rect::new(1920, 0, 1280, 1440))]);
    }

    #[test]
    fn wraps_next_display_from_last_to_first() {
        let mut fake = FakeWindowSystem::with_focus(Rect::new(1920, 0, 1280, 1440));

        execute(&mut fake, &Action::MoveToNextDisplay {}).unwrap();

        assert_eq!(fake.moves, vec![moved_to(Rect::new(0, 0, 960, 1080))]);
    }

    #[test]
    fn wraps_previous_display_from_first_to_last() {
        let mut fake = FakeWindowSystem::with_focus(Rect::new(0, 0, 960, 1080));

        execute(&mut fake, &Action::MoveToPreviousDisplay {}).unwrap();

        assert_eq!(fake.moves, vec![moved_to(Rect::new(1920, 0, 1280, 1440))]);
    }

    #[test]
    fn next_display_follows_spatial_order_not_adapter_order() {
        let mut fake = fake_with_focus(Rect::new(0, 0, 960, 1080));
        fake.displays = Ok(vec![
            DisplayGeometry::new("far-right", Rect::new(4480, 0, 1920, 1080)),
            DisplayGeometry::new("left", Rect::new(0, 0, 1920, 1080)),
            DisplayGeometry::new("middle", Rect::new(1920, 0, 2560, 1440)),
        ]);

        execute(&mut fake, &Action::MoveToNextDisplay {}).unwrap();

        assert_eq!(fake.moves, vec![moved_to(Rect::new(1920, 0, 1280, 1440))]);
    }

    #[test]
    fn display_actions_do_nothing_with_a_single_display() {
        let mut fake = fake_with_focus(Rect::new(-100, 50, 2400, 600));
        fake.displays = Ok(vec![DisplayGeometry::new(
            "laptop",
            Rect::new(0, 0, 1920, 1080),
        )]);

        execute(&mut fake, &Action::MoveToNextDisplay {}).unwrap();
        execute(&mut fake, &Action::MoveToPreviousDisplay {}).unwrap();

        assert!(fake.moves.is_empty());
    }

    #[test]
    fn window_centered_off_every_display_belongs_to_the_one_it_overlaps_most() {
        // Center (1880, 1200) is below both usable areas; the frame overlaps
        // 16000px² of the left display and 32000px² of the right one.
        let mut fake = fake_with_focus(Rect::new(1520, 1000, 720, 400));
        fake.displays = Ok(vec![
            DisplayGeometry::new("left", Rect::new(0, 0, 1920, 1040)),
            DisplayGeometry::new("right", Rect::new(1920, 0, 2560, 1100)),
        ]);

        execute(&mut fake, &zone("maximize")).unwrap();

        assert_eq!(fake.moves, vec![moved_to(Rect::new(1920, 0, 2560, 1100))]);
    }

    #[test]
    fn window_entirely_off_screen_belongs_to_the_nearest_display() {
        let mut fake = fake_with_focus(Rect::new(5000, -3000, 960, 1080));

        execute(&mut fake, &zone("left-half")).unwrap();

        assert_eq!(fake.moves, vec![moved_to(Rect::new(1920, 0, 1280, 1440))]);
    }

    #[test]
    fn moves_to_the_display_in_a_direction() {
        // Layout: left and right side by side, a third display above the right one.
        let displays = vec![
            DisplayGeometry::new("left", Rect::new(0, 0, 1920, 1080)),
            DisplayGeometry::new("right", Rect::new(1920, 0, 1920, 1080)),
            DisplayGeometry::new("top", Rect::new(1920, -1080, 1920, 1080)),
        ];
        let mut fake = fake_with_focus(Rect::new(1920, 0, 960, 1080));
        fake.displays = Ok(displays);

        execute(
            &mut fake,
            &Action::MoveToDisplay {
                direction: Direction::Up,
            },
        )
        .unwrap();
        execute(
            &mut fake,
            &Action::MoveToDisplay {
                direction: Direction::Down,
            },
        )
        .unwrap();
        execute(
            &mut fake,
            &Action::MoveToDisplay {
                direction: Direction::Left,
            },
        )
        .unwrap();
        let error = execute(
            &mut fake,
            &Action::MoveToDisplay {
                direction: Direction::Left,
            },
        )
        .unwrap_err();

        assert_eq!(
            targets(&fake),
            vec![
                Rect::new(1920, -1080, 960, 1080),
                Rect::new(1920, 0, 960, 1080),
                Rect::new(0, 0, 960, 1080),
            ]
        );
        assert_eq!(
            error,
            ExecuteActionError::NoDisplayInDirection {
                direction: Direction::Left
            }
        );
    }

    #[test]
    fn centers_keeping_size_and_shrinks_oversized_windows() {
        let mut fake = fake_with_focus(Rect::new(10, 20, 801, 600));
        execute(&mut fake, &Action::Center {}).unwrap();

        let mut oversized = fake_with_focus(Rect::new(10, 20, 3000, 600));
        execute(&mut oversized, &Action::Center {}).unwrap();

        assert_eq!(targets(&fake), vec![Rect::new(559, 240, 801, 600)]);
        assert_eq!(targets(&oversized), vec![Rect::new(0, 240, 1920, 600)]);
    }

    #[test]
    fn repeated_halves_cross_displays_and_preserve_the_restore_point() {
        let original = Rect::new(100, 100, 800, 600);
        let mut fake = fake_with_focus(original);
        let mut history = WindowHistory::default();

        for _ in 0..4 {
            execute_action(
                &zone("right-half"),
                &BTreeMap::new(),
                &mut history,
                &mut fake,
            )
            .unwrap();
        }
        execute_action(
            &Action::Restore {},
            &BTreeMap::new(),
            &mut history,
            &mut fake,
        )
        .unwrap();

        assert_eq!(
            targets(&fake),
            vec![
                Rect::new(960, 0, 960, 1080),
                Rect::new(1920, 0, 1280, 1440),
                Rect::new(3200, 0, 1280, 1440),
                Rect::new(3200, 0, 1280, 1440),
                original,
            ]
        );
    }

    #[test]
    fn half_placement_restarts_after_another_action_or_a_manual_move() {
        let mut fake = fake_with_focus(Rect::new(100, 100, 800, 600));
        let mut history = WindowHistory::default();
        let left_half = zone("left-half");

        execute_action(&left_half, &BTreeMap::new(), &mut history, &mut fake).unwrap();
        execute_action(&zone("maximize"), &BTreeMap::new(), &mut history, &mut fake).unwrap();
        execute_action(&left_half, &BTreeMap::new(), &mut history, &mut fake).unwrap();
        // The user drags the window away; the next press starts at half again.
        fake.focused_window = Ok(Some(FocusedWindow::new(
            window_id(),
            Rect::new(300, 300, 960, 1080),
        )));
        execute_action(&left_half, &BTreeMap::new(), &mut history, &mut fake).unwrap();

        assert_eq!(
            targets(&fake),
            vec![
                Rect::new(0, 0, 960, 1080),
                Rect::new(0, 0, 1920, 1080),
                Rect::new(0, 0, 960, 1080),
                Rect::new(0, 0, 960, 1080),
            ]
        );
    }

    #[test]
    fn restore_returns_to_geometry_before_the_first_app_move() {
        let original = Rect::new(100, 100, 800, 600);
        let mut fake = fake_with_focus(original);
        let mut history = WindowHistory::default();

        execute_action(
            &zone("left-half"),
            &BTreeMap::new(),
            &mut history,
            &mut fake,
        )
        .unwrap();
        execute_action(
            &Action::MoveToNextDisplay {},
            &BTreeMap::new(),
            &mut history,
            &mut fake,
        )
        .unwrap();
        execute_action(
            &Action::Restore {},
            &BTreeMap::new(),
            &mut history,
            &mut fake,
        )
        .unwrap();
        let second_restore = execute_action(
            &Action::Restore {},
            &BTreeMap::new(),
            &mut history,
            &mut fake,
        )
        .unwrap_err();

        assert_eq!(targets(&fake).last(), Some(&original));
        assert_eq!(second_restore, ExecuteActionError::NothingToRestore);
    }

    #[test]
    fn failed_partial_move_keeps_the_original_restore_point() {
        for already_moved in [false, true] {
            let original = Rect::new(100, 100, 800, 600);
            let mut fake = fake_with_focus(original);
            let mut history = WindowHistory::default();
            if already_moved {
                execute_action(
                    &zone("left-half"),
                    &BTreeMap::new(),
                    &mut history,
                    &mut fake,
                )
                .unwrap();
            }
            fake.move_error = Some(WindowSystemError::PartialMove {
                observed: Rect::new(960, 0, 1200, 1080),
                message: "minimum width constrained placement".into(),
            });
            let failed = execute_action(
                &zone("right-half"),
                &BTreeMap::new(),
                &mut history,
                &mut fake,
            );
            assert!(matches!(
                failed,
                Err(ExecuteActionError::WindowSystem(
                    WindowSystemError::PartialMove { .. }
                ))
            ));
            fake.move_error = None;
            execute_action(
                &Action::Restore {},
                &BTreeMap::new(),
                &mut history,
                &mut fake,
            )
            .unwrap();
            assert_eq!(fake.focused_window().unwrap().unwrap().geometry, original);
        }
    }

    #[test]
    fn partially_failed_restore_can_be_retried() {
        let original = Rect::new(100, 100, 800, 600);
        let mut fake = fake_with_focus(original);
        let mut history = WindowHistory::default();
        execute_action(
            &zone("left-half"),
            &BTreeMap::new(),
            &mut history,
            &mut fake,
        )
        .unwrap();
        fake.move_error = Some(WindowSystemError::PartialMove {
            observed: Rect::new(100, 100, 900, 700),
            message: "constrained restore".into(),
        });
        let failed = execute_action(
            &Action::Restore {},
            &BTreeMap::new(),
            &mut history,
            &mut fake,
        );
        assert!(matches!(
            failed,
            Err(ExecuteActionError::WindowSystem(
                WindowSystemError::PartialMove { .. }
            ))
        ));
        fake.move_error = None;
        execute_action(
            &Action::Restore {},
            &BTreeMap::new(),
            &mut history,
            &mut fake,
        )
        .unwrap();
        assert_eq!(fake.focused_window().unwrap().unwrap().geometry, original);
    }

    #[test]
    fn restore_does_not_override_a_manual_move() {
        let mut fake = fake_with_focus(Rect::new(100, 100, 800, 600));
        let mut history = WindowHistory::default();
        execute_action(
            &zone("left-half"),
            &BTreeMap::new(),
            &mut history,
            &mut fake,
        )
        .unwrap();
        let user_placed = Rect::new(400, 300, 700, 500);
        fake.focused_window = Ok(Some(FocusedWindow::new(window_id(), user_placed)));
        assert_eq!(
            execute_action(
                &Action::Restore {},
                &BTreeMap::new(),
                &mut history,
                &mut fake
            ),
            Err(ExecuteActionError::NothingToRestore)
        );
        assert_eq!(
            fake.focused_window().unwrap().unwrap().geometry,
            user_placed
        );
        fake.focused_window = Ok(Some(FocusedWindow::new(
            window_id(),
            Rect::new(0, 0, 960, 1080),
        )));
        assert_eq!(
            execute_action(
                &Action::Restore {},
                &BTreeMap::new(),
                &mut history,
                &mut fake
            ),
            Err(ExecuteActionError::NothingToRestore)
        );
    }

    #[test]
    fn restore_point_moves_to_where_the_user_last_placed_the_window() {
        let mut fake = fake_with_focus(Rect::new(100, 100, 800, 600));
        let mut history = WindowHistory::default();

        execute_action(
            &zone("left-half"),
            &BTreeMap::new(),
            &mut history,
            &mut fake,
        )
        .unwrap();
        let user_placed = Rect::new(400, 300, 700, 500);
        fake.focused_window = Ok(Some(FocusedWindow::new(window_id(), user_placed)));
        execute_action(
            &zone("right-half"),
            &BTreeMap::new(),
            &mut history,
            &mut fake,
        )
        .unwrap();
        execute_action(
            &Action::Restore {},
            &BTreeMap::new(),
            &mut history,
            &mut fake,
        )
        .unwrap();

        assert_eq!(targets(&fake).last(), Some(&user_placed));
    }

    #[test]
    fn restore_is_per_window() {
        let mut fake = fake_with_focus(Rect::new(100, 100, 800, 600));
        let mut history = WindowHistory::default();

        execute_action(
            &zone("left-half"),
            &BTreeMap::new(),
            &mut history,
            &mut fake,
        )
        .unwrap();
        fake.focused_window = Ok(Some(FocusedWindow::new(
            WindowId::new("window-b"),
            Rect::new(0, 0, 960, 1080),
        )));
        let error = execute_action(
            &Action::Restore {},
            &BTreeMap::new(),
            &mut history,
            &mut fake,
        )
        .unwrap_err();

        assert_eq!(error, ExecuteActionError::NothingToRestore);
    }

    #[test]
    fn closed_window_is_forgotten() {
        let mut fake = fake_with_focus(Rect::new(100, 100, 800, 600));
        let mut history = WindowHistory::default();

        execute_action(
            &zone("left-half"),
            &BTreeMap::new(),
            &mut history,
            &mut fake,
        )
        .unwrap();
        fake.move_error = Some(WindowSystemError::WindowGone(window_id()));
        let moved = execute_action(
            &zone("left-half"),
            &BTreeMap::new(),
            &mut history,
            &mut fake,
        );
        fake.move_error = None;
        let restore = execute_action(
            &Action::Restore {},
            &BTreeMap::new(),
            &mut history,
            &mut fake,
        );

        assert_eq!(
            moved,
            Err(ExecuteActionError::WindowSystem(
                WindowSystemError::WindowGone(window_id())
            ))
        );
        assert_eq!(restore, Err(ExecuteActionError::NothingToRestore));
    }

    #[test]
    fn returns_no_focused_window_without_moving() {
        let mut fake = FakeWindowSystem::with_focus(Rect::new(0, 0, 960, 1080));
        fake.focused_window = Ok(None);

        let err = execute(&mut fake, &zone("left-half")).unwrap_err();

        assert_eq!(err, ExecuteActionError::NoFocusedWindow);
        assert!(fake.moves.is_empty());
    }

    #[test]
    fn returns_no_displays_without_moving() {
        let mut fake = FakeWindowSystem::with_focus(Rect::new(0, 0, 960, 1080));
        fake.displays = Ok(Vec::new());

        let err = execute(&mut fake, &zone("left-half")).unwrap_err();

        assert_eq!(err, ExecuteActionError::NoDisplays);
        assert!(fake.moves.is_empty());
    }

    #[test]
    fn returns_no_displays_when_only_zero_sized_displays_without_moving() {
        let mut fake = FakeWindowSystem::with_focus(Rect::new(0, 0, 960, 1080));
        fake.displays = Ok(vec![
            DisplayGeometry::new("zero-width", Rect::new(0, 0, 0, 1080)),
            DisplayGeometry::new("zero-height", Rect::new(10, 10, 640, 0)),
        ]);

        let err = execute(&mut fake, &zone("left-half")).unwrap_err();

        assert_eq!(err, ExecuteActionError::NoDisplays);
        assert!(fake.moves.is_empty());
    }

    #[test]
    fn returns_unknown_zone_without_moving() {
        let mut fake = FakeWindowSystem::with_focus(Rect::new(0, 0, 960, 1080));

        let err = execute(&mut fake, &zone("missing-zone")).unwrap_err();

        assert_eq!(
            err,
            ExecuteActionError::UnknownZone {
                zone: "missing-zone".to_string()
            }
        );
        assert!(fake.moves.is_empty());
    }

    #[test]
    fn wraps_platform_errors() {
        let mut fake = FakeWindowSystem::with_focus(Rect::new(0, 0, 960, 1080));
        fake.move_error = Some(WindowSystemError::Platform("denied".to_string()));

        let err = execute(&mut fake, &zone("left-half")).unwrap_err();

        assert_eq!(
            err,
            ExecuteActionError::WindowSystem(WindowSystemError::Platform("denied".to_string()))
        );
    }
}
