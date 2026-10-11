use std::collections::BTreeMap;

use thiserror::Error;

use crate::actions::Action;
use crate::display_movement::{KNOWN_ZONE_MATCH_TOLERANCE_PX, move_window_to_display};
use crate::geometry::DisplayGeometry;
use crate::window_system::{WindowMove, WindowSystem, WindowSystemError};
use crate::zones::{BuiltInZone, ZoneDefinition, rect_for_built_in_zone, rect_for_zone};

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
    #[error("focused window display is missing: {display_id}")]
    FocusedWindowDisplayMissing { display_id: String },
    #[error("unknown zone: {zone}")]
    UnknownZone { zone: String },
}

/// Executes an action by asking the provided window system for current state,
/// calculating a platform-neutral target rectangle, and applying the move.
pub fn execute_action<W: WindowSystem>(
    action: &Action,
    custom_zones: &BTreeMap<String, ZoneDefinition>,
    window_system: &mut W,
) -> Result<(), ExecuteActionError> {
    let focused = window_system.focused_window()?;
    let focused = focused.ok_or(ExecuteActionError::NoFocusedWindow)?;

    let mut displays = window_system.displays()?;

    displays.retain(|display| display.usable_area.width > 0 && display.usable_area.height > 0);

    if displays.is_empty() {
        return Err(ExecuteActionError::NoDisplays);
    }

    let current_display_index = displays
        .iter()
        .position(|display| display.id == focused.display_id)
        .ok_or_else(|| ExecuteActionError::FocusedWindowDisplayMissing {
            display_id: focused.display_id.clone(),
        })?;

    let target = match action {
        Action::MoveToZone { zone } => {
            let target = rect_for_zone(
                zone,
                displays[current_display_index].usable_area,
                custom_zones,
            )
            .ok_or_else(|| ExecuteActionError::UnknownZone {
                zone: zone.to_string(),
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
                && let Some(neighbour) =
                    horizontal_neighbour(&displays, current_display_index, left)
            {
                rect_for_built_in_zone(landing_zone, neighbour.usable_area)
            } else {
                target
            }
        }
        Action::MoveToNextDisplay => {
            let target_display = next_display(&displays, current_display_index);
            move_window_to_display(
                focused.geometry,
                displays[current_display_index].usable_area,
                target_display.usable_area,
            )
        }
        Action::MoveToPreviousDisplay => {
            let target_display = previous_display(&displays, current_display_index);
            move_window_to_display(
                focused.geometry,
                displays[current_display_index].usable_area,
                target_display.usable_area,
            )
        }
    };

    window_system.move_focused_window(WindowMove::new(target))?;
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

fn next_display(displays: &[DisplayGeometry], current_index: usize) -> &DisplayGeometry {
    &displays[(current_index + 1) % displays.len()]
}

fn previous_display(displays: &[DisplayGeometry], current_index: usize) -> &DisplayGeometry {
    if current_index == 0 {
        &displays[displays.len() - 1]
    } else {
        &displays[current_index - 1]
    }
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

        fn move_focused_window(
            &mut self,
            window_move: WindowMove,
        ) -> Result<(), WindowSystemError> {
            if let Some(error) = self.move_error.clone() {
                return Err(error);
            }

            self.moves.push(window_move);
            Ok(())
        }
    }

    fn fake_with_focus(display_id: &str, geometry: Rect) -> FakeWindowSystem {
        FakeWindowSystem {
            focused_window: Ok(Some(FocusedWindow::new(display_id, geometry))),
            displays: Ok(vec![
                DisplayGeometry::new("left", Rect::new(0, 0, 1920, 1080)),
                DisplayGeometry::new("right", Rect::new(1920, 0, 2560, 1440)),
            ]),
            moves: Vec::new(),
            move_error: None,
        }
    }

    fn move_to_left_half_action() -> (String, BTreeMap<String, ZoneDefinition>) {
        ("left-half".to_string(), BTreeMap::new())
    }

    #[test]
    fn moves_focused_window_to_zone_on_current_display() {
        for (zone, geometry, expected) in [
            (
                "left-half",
                Rect::new(100, 100, 800, 600),
                Rect::new(0, 0, 960, 1080),
            ),
            (
                "right-half",
                Rect::new(100, 100, 800, 600),
                Rect::new(960, 0, 960, 1080),
            ),
            (
                "left-half",
                Rect::new(960, 0, 960, 1080),
                Rect::new(0, 0, 960, 1080),
            ),
            (
                "right-half",
                Rect::new(0, 0, 960, 1080),
                Rect::new(960, 0, 960, 1080),
            ),
        ] {
            let mut fake = fake_with_focus("left", geometry);
            execute_action(
                &Action::MoveToZone {
                    zone: zone.to_string(),
                },
                &BTreeMap::new(),
                &mut fake,
            )
            .unwrap();

            assert_eq!(fake.moves, vec![WindowMove::new(expected)], "{zone}");
        }
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
            let mut fake = fake_with_focus("current", geometry);
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
                &mut fake,
            )
            .unwrap();

            assert_eq!(fake.moves, vec![WindowMove::new(expected)], "{zone}");
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
                let mut fake = fake_with_focus("current", Rect::new(960, 0, 960, 1080));
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
                    &mut fake,
                )
                .unwrap();

                assert_eq!(
                    fake.moves,
                    vec![WindowMove::new(expected)],
                    "reverse={reverse}"
                );
            }
        }
    }

    #[test]
    fn repeated_halves_stay_at_outer_edges_and_on_a_single_display() {
        for (zone, display_id, geometry, single_display) in [
            ("left-half", "left", Rect::new(0, 0, 960, 1080), false),
            ("right-half", "right", Rect::new(3200, 0, 1280, 1440), false),
            ("left-half", "left", Rect::new(0, 0, 960, 1080), true),
            ("right-half", "left", Rect::new(960, 0, 960, 1080), true),
        ] {
            let mut fake = fake_with_focus(display_id, geometry);
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
                &mut fake,
            )
            .unwrap();

            assert_eq!(
                fake.moves,
                vec![WindowMove::new(geometry)],
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
            let mut fake = fake_with_focus("left", geometry);
            execute_action(
                &Action::MoveToZone {
                    zone: "right-half".to_string(),
                },
                &BTreeMap::new(),
                &mut fake,
            )
            .unwrap();

            assert_eq!(fake.moves, vec![WindowMove::new(expected)], "{geometry:?}");
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
            let mut fake = fake_with_focus("right", expected);
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
                &mut fake,
            )
            .unwrap();

            assert_eq!(fake.moves, vec![WindowMove::new(expected)], "{zone}");
        }
    }

    #[test]
    fn moves_to_custom_zone_from_map() {
        let mut fake = fake_with_focus("left", Rect::new(200, 200, 800, 600));

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
            &crate::Action::MoveToZone {
                zone: "side".to_string(),
            },
            &zones,
            &mut fake,
        )
        .unwrap();

        assert_eq!(
            fake.moves,
            vec![WindowMove::new(Rect::new(960, 0, 960, 1080))]
        );
    }

    #[test]
    fn moves_to_next_display_preserving_recognized_zone() {
        let mut fake = fake_with_focus("left", Rect::new(0, 0, 960, 1080));

        execute_action(
            &crate::Action::MoveToNextDisplay,
            &BTreeMap::new(),
            &mut fake,
        )
        .unwrap();

        assert_eq!(
            fake.moves,
            vec![WindowMove::new(Rect::new(1920, 0, 1280, 1440))]
        );
    }

    #[test]
    fn wraps_next_display_from_last_to_first() {
        let mut fake = fake_with_focus("right", Rect::new(1920, 0, 1280, 1440));

        execute_action(
            &crate::Action::MoveToNextDisplay,
            &BTreeMap::new(),
            &mut fake,
        )
        .unwrap();

        assert_eq!(
            fake.moves,
            vec![WindowMove::new(Rect::new(0, 0, 960, 1080))]
        );
    }

    #[test]
    fn wraps_previous_display_from_first_to_last() {
        let mut fake = fake_with_focus("left", Rect::new(0, 0, 960, 1080));

        execute_action(
            &crate::Action::MoveToPreviousDisplay,
            &BTreeMap::new(),
            &mut fake,
        )
        .unwrap();

        assert_eq!(
            fake.moves,
            vec![WindowMove::new(Rect::new(1920, 0, 1280, 1440))]
        );
    }

    #[test]
    fn returns_no_focused_window_without_moving() {
        let mut fake = fake_with_focus("left", Rect::new(0, 0, 960, 1080));
        fake.focused_window = Ok(None);

        let (zone, zones) = move_to_left_half_action();
        let err =
            execute_action(&crate::Action::MoveToZone { zone }, &zones, &mut fake).unwrap_err();

        assert_eq!(err, ExecuteActionError::NoFocusedWindow);
        assert!(fake.moves.is_empty());
    }

    #[test]
    fn returns_no_displays_without_moving() {
        let mut fake = fake_with_focus("left", Rect::new(0, 0, 960, 1080));
        fake.displays = Ok(Vec::new());

        let (zone, zones) = move_to_left_half_action();
        let err =
            execute_action(&crate::Action::MoveToZone { zone }, &zones, &mut fake).unwrap_err();

        assert_eq!(err, ExecuteActionError::NoDisplays);
        assert!(fake.moves.is_empty());
    }
    #[test]
    fn returns_no_displays_when_only_zero_sized_displays_without_moving() {
        let mut fake = fake_with_focus("left", Rect::new(0, 0, 960, 1080));
        fake.displays = Ok(vec![
            DisplayGeometry::new("zero-width", Rect::new(0, 0, 0, 1080)),
            DisplayGeometry::new("zero-height", Rect::new(10, 10, 640, 0)),
        ]);

        let (zone, zones) = move_to_left_half_action();
        let err =
            execute_action(&crate::Action::MoveToZone { zone }, &zones, &mut fake).unwrap_err();

        assert_eq!(err, ExecuteActionError::NoDisplays);
        assert!(fake.moves.is_empty());
    }

    #[test]
    fn returns_missing_focused_display_without_moving() {
        let mut fake = fake_with_focus("missing", Rect::new(0, 0, 960, 1080));

        let (zone, zones) = move_to_left_half_action();
        let err =
            execute_action(&crate::Action::MoveToZone { zone }, &zones, &mut fake).unwrap_err();

        assert_eq!(
            err,
            ExecuteActionError::FocusedWindowDisplayMissing {
                display_id: "missing".to_string()
            }
        );
        assert!(fake.moves.is_empty());
    }

    #[test]
    fn returns_unknown_zone_without_moving() {
        let mut fake = fake_with_focus("left", Rect::new(0, 0, 960, 1080));

        let err = execute_action(
            &crate::Action::MoveToZone {
                zone: "missing-zone".to_string(),
            },
            &BTreeMap::new(),
            &mut fake,
        )
        .unwrap_err();

        assert_eq!(
            err,
            ExecuteActionError::UnknownZone {
                zone: "missing-zone".to_string()
            }
        );
    }

    #[test]
    fn wraps_platform_errors() {
        let mut fake = fake_with_focus("left", Rect::new(0, 0, 960, 1080));
        fake.move_error = Some(WindowSystemError::Platform("denied".to_string()));

        let (zone, zones) = move_to_left_half_action();
        let err =
            execute_action(&crate::Action::MoveToZone { zone }, &zones, &mut fake).unwrap_err();

        assert_eq!(
            err,
            ExecuteActionError::WindowSystem(WindowSystemError::Platform("denied".to_string()))
        );
    }
}
