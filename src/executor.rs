use std::collections::{BTreeMap, HashMap};

use thiserror::Error;

use crate::actions::{Action, Direction};
use crate::display_movement::move_window_to_display;
use crate::geometry::{DisplayGeometry, Rect};
use crate::window_system::{FocusedWindow, WindowId, WindowMove, WindowSystem, WindowSystemError};
use crate::zones::{ZoneDefinition, rect_for_zone};

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

/// Per-window memory of the App's own moves: the geometry to restore and the
/// position in a repeated-half cycle.
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
    /// Cycling zone action and step of the last move, if it was one.
    cycle: Option<(String, usize)>,
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

    fn remember(&mut self, window: &FocusedWindow, applied: Rect, cycle: Option<(String, usize)>) {
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
                cycle,
                sequence,
            },
        );
    }

    fn forget(&mut self, window: &WindowId) {
        self.records.remove(window);
    }
}

/// Zones a repeated half action steps through, Rectangle-style.
fn zone_cycle(zone: &str) -> Option<[&'static str; 3]> {
    match zone {
        "left-half" => Some(["left-half", "left-two-thirds", "left-third"]),
        "right-half" => Some(["right-half", "right-two-thirds", "right-third"]),
        _ => None,
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

    let mut cycle = None;
    let target = match action {
        Action::MoveToZone { zone } => {
            let step_zone = match zone_cycle(zone) {
                Some(steps) => {
                    let step = match history
                        .placed_record(&focused)
                        .and_then(|record| record.cycle.as_ref())
                    {
                        Some((cycled_zone, step)) if cycled_zone == zone => {
                            (step + 1) % steps.len()
                        }
                        _ => 0,
                    };
                    cycle = Some((zone.clone(), step));
                    steps[step]
                }
                None => zone.as_str(),
            };
            rect_for_zone(step_zone, current_area, custom_zones).ok_or_else(|| {
                ExecuteActionError::UnknownZone {
                    zone: zone.to_string(),
                }
            })?
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
            move_window(window_system, history, &focused.id, restore)?;
            history.forget(&focused.id);
            return Ok(());
        }
    };

    move_window(window_system, history, &focused.id, target)?;
    history.remember(&focused, target, cycle);
    Ok(())
}

fn move_window<W: WindowSystem>(
    window_system: &mut W,
    history: &mut WindowHistory,
    window: &WindowId,
    target: Rect,
) -> Result<(), ExecuteActionError> {
    window_system
        .move_window(&WindowMove::new(window.clone(), target))
        .map_err(|error| {
            if matches!(error, WindowSystemError::WindowGone(_)) {
                history.forget(window);
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
    use crate::window_system::FocusedWindow;
    use crate::{DisplayGeometry, ZoneDefinition};

    #[derive(Debug)]
    struct FakeWindowSystem {
        focused_window: Result<Option<FocusedWindow>, WindowSystemError>,
        displays: Result<Vec<DisplayGeometry>, WindowSystemError>,
        moves: Vec<WindowMove>,
        move_error: Option<WindowSystemError>,
    }

    impl WindowSystem for FakeWindowSystem {
        fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
            self.focused_window.clone()
        }

        fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
            self.displays.clone()
        }

        fn move_window(&mut self, window_move: &WindowMove) -> Result<(), WindowSystemError> {
            if let Some(error) = self.move_error.clone() {
                return Err(error);
            }

            self.moves.push(window_move.clone());
            // The fake compositor applies moves immediately.
            if let Ok(Some(focused)) = &mut self.focused_window
                && focused.id == window_move.window
            {
                focused.geometry = window_move.target;
            }
            Ok(())
        }
    }

    fn window_id() -> WindowId {
        WindowId::new("window-a")
    }

    fn fake_with_focus(geometry: Rect) -> FakeWindowSystem {
        FakeWindowSystem {
            focused_window: Ok(Some(FocusedWindow::new(window_id(), geometry))),
            displays: Ok(vec![
                DisplayGeometry::new("left", Rect::new(0, 0, 1920, 1080)),
                DisplayGeometry::new("right", Rect::new(1920, 0, 2560, 1440)),
            ]),
            moves: Vec::new(),
            move_error: None,
        }
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
        let mut fake = fake_with_focus(Rect::new(100, 100, 800, 600));

        execute(&mut fake, &zone("left-half")).unwrap();

        assert_eq!(fake.moves, vec![moved_to(Rect::new(0, 0, 960, 1080))]);
    }

    #[test]
    fn uses_second_display_when_overlapping_window_center_is_on_shared_edge() {
        let mut fake = fake_with_focus(Rect::new(1520, 100, 800, 600));

        execute(&mut fake, &zone("left-half")).unwrap();

        assert_eq!(fake.moves, vec![moved_to(Rect::new(1920, 0, 1280, 1440))]);
    }

    #[test]
    fn moves_to_custom_zone_from_map() {
        let mut fake = fake_with_focus(Rect::new(200, 200, 800, 600));

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
        let mut fake = fake_with_focus(Rect::new(0, 0, 960, 1080));

        execute(&mut fake, &Action::MoveToNextDisplay {}).unwrap();

        assert_eq!(fake.moves, vec![moved_to(Rect::new(1920, 0, 1280, 1440))]);
    }

    #[test]
    fn wraps_next_display_from_last_to_first() {
        let mut fake = fake_with_focus(Rect::new(1920, 0, 1280, 1440));

        execute(&mut fake, &Action::MoveToNextDisplay {}).unwrap();

        assert_eq!(fake.moves, vec![moved_to(Rect::new(0, 0, 960, 1080))]);
    }

    #[test]
    fn wraps_previous_display_from_first_to_last() {
        let mut fake = fake_with_focus(Rect::new(0, 0, 960, 1080));

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
    fn repeated_half_action_cycles_half_two_thirds_third() {
        let mut fake = fake_with_focus(Rect::new(100, 100, 800, 600));
        let mut history = WindowHistory::default();
        let left_half = zone("left-half");

        for _ in 0..4 {
            execute_action(&left_half, &BTreeMap::new(), &mut history, &mut fake).unwrap();
        }

        assert_eq!(
            targets(&fake),
            vec![
                Rect::new(0, 0, 960, 1080),
                Rect::new(0, 0, 1280, 1080),
                Rect::new(0, 0, 640, 1080),
                Rect::new(0, 0, 960, 1080),
            ]
        );
    }

    #[test]
    fn half_cycle_restarts_after_another_action_or_a_manual_move() {
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
        let mut fake = fake_with_focus(Rect::new(0, 0, 960, 1080));
        fake.focused_window = Ok(None);

        let err = execute(&mut fake, &zone("left-half")).unwrap_err();

        assert_eq!(err, ExecuteActionError::NoFocusedWindow);
        assert!(fake.moves.is_empty());
    }

    #[test]
    fn returns_no_displays_without_moving() {
        let mut fake = fake_with_focus(Rect::new(0, 0, 960, 1080));
        fake.displays = Ok(Vec::new());

        let err = execute(&mut fake, &zone("left-half")).unwrap_err();

        assert_eq!(err, ExecuteActionError::NoDisplays);
        assert!(fake.moves.is_empty());
    }

    #[test]
    fn returns_no_displays_when_only_zero_sized_displays_without_moving() {
        let mut fake = fake_with_focus(Rect::new(0, 0, 960, 1080));
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
        let mut fake = fake_with_focus(Rect::new(0, 0, 960, 1080));

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
        let mut fake = fake_with_focus(Rect::new(0, 0, 960, 1080));
        fake.move_error = Some(WindowSystemError::Platform("denied".to_string()));

        let err = execute(&mut fake, &zone("left-half")).unwrap_err();

        assert_eq!(
            err,
            ExecuteActionError::WindowSystem(WindowSystemError::Platform("denied".to_string()))
        );
    }
}
