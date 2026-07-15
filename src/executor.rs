use std::collections::BTreeMap;

use thiserror::Error;

use crate::actions::Action;
use crate::display_movement::move_window_to_display;
use crate::geometry::DisplayGeometry;
use crate::window_system::{WindowMove, WindowSystem, WindowSystemError};
use crate::zones::{ZoneDefinition, rect_for_zone};

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

    let displays = window_system.displays()?;

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
        Action::MoveToZone { zone } => rect_for_zone(
            zone,
            displays[current_display_index].usable_area,
            custom_zones,
        )
        .ok_or_else(|| ExecuteActionError::UnknownZone {
            zone: zone.to_string(),
        })?,
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
        let mut fake = fake_with_focus("left", Rect::new(100, 100, 800, 600));

        let (zone, zones) = move_to_left_half_action();
        execute_action(&crate::Action::MoveToZone { zone }, &zones, &mut fake).unwrap();

        assert_eq!(
            fake.moves,
            vec![WindowMove::new(Rect::new(0, 0, 960, 1080))]
        );
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
