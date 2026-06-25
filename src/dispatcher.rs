use thiserror::Error;

use crate::config::AppConfig;
use crate::executor::{ExecuteActionError, execute_action};
use crate::window_system::WindowSystem;

/// Errors produced while dispatching an already-received hotkey string.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DispatchHotkeyError {
    #[error("no binding configured for hotkey: {hotkey}")]
    NoBindingForHotkey { hotkey: String },
    #[error(transparent)]
    ExecuteAction(#[from] ExecuteActionError),
}

/// Dispatches a received hotkey string using exact, case-sensitive matching
/// against the parsed app config.
///
/// Duplicate hotkeys are first-match-wins for v1. Validation and indexing are
/// intentionally deferred until config reload and platform hotkey syntax are
/// introduced.
pub fn dispatch_hotkey<W: WindowSystem>(
    config: &AppConfig,
    hotkey: &str,
    window_system: &mut W,
) -> Result<(), DispatchHotkeyError> {
    let binding = config
        .bindings
        .iter()
        .find(|binding| binding.hotkey == hotkey)
        .ok_or_else(|| DispatchHotkeyError::NoBindingForHotkey {
            hotkey: hotkey.to_string(),
        })?;

    execute_action(&binding.action, window_system)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actions::{Action, Binding};
    use crate::geometry::Rect;
    use crate::window_system::{FocusedWindow, WindowMove, WindowSystemError};
    use crate::{BuiltInZone, DisplayGeometry};

    #[derive(Debug)]
    struct FakeWindowSystem {
        focused_window: Result<Option<FocusedWindow>, WindowSystemError>,
        displays: Result<Vec<DisplayGeometry>, WindowSystemError>,
        moves: Vec<WindowMove>,
        move_error: Option<WindowSystemError>,
    }

    impl crate::WindowSystem for FakeWindowSystem {
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

    fn config(bindings: Vec<Binding>) -> AppConfig {
        AppConfig { bindings }
    }

    fn binding(hotkey: &str, action: Action) -> Binding {
        Binding {
            hotkey: hotkey.to_string(),
            action,
        }
    }

    fn move_to_zone(zone: BuiltInZone) -> Action {
        Action::MoveToZone { zone }
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

    #[test]
    fn dispatches_known_hotkey_to_zone_movement() {
        let config = config(vec![binding(
            "Ctrl+Alt+Left",
            move_to_zone(BuiltInZone::LeftHalf),
        )]);
        let mut fake = fake_with_focus("left", Rect::new(200, 200, 800, 600));

        dispatch_hotkey(&config, "Ctrl+Alt+Left", &mut fake).unwrap();

        assert_eq!(
            fake.moves,
            vec![WindowMove::new(Rect::new(0, 0, 960, 1080))]
        );
    }

    #[test]
    fn dispatches_known_hotkey_to_display_movement() {
        let config = config(vec![binding(
            "Ctrl+Alt+Shift+Right",
            Action::MoveToNextDisplay,
        )]);
        let mut fake = fake_with_focus("left", Rect::new(0, 0, 960, 1080));

        dispatch_hotkey(&config, "Ctrl+Alt+Shift+Right", &mut fake).unwrap();

        assert_eq!(
            fake.moves,
            vec![WindowMove::new(Rect::new(1920, 0, 1280, 1440))]
        );
    }

    #[test]
    fn unknown_hotkey_returns_error_without_moving() {
        let config = config(vec![binding(
            "Ctrl+Alt+Left",
            move_to_zone(BuiltInZone::LeftHalf),
        )]);
        let mut fake = fake_with_focus("left", Rect::new(200, 200, 800, 600));

        let err = dispatch_hotkey(&config, "ctrl+alt+left", &mut fake).unwrap_err();

        assert_eq!(
            err,
            DispatchHotkeyError::NoBindingForHotkey {
                hotkey: "ctrl+alt+left".to_string()
            }
        );
        assert!(fake.moves.is_empty());
    }

    #[test]
    fn duplicate_hotkeys_use_first_match() {
        let config = config(vec![
            binding("Ctrl+Alt+X", move_to_zone(BuiltInZone::LeftHalf)),
            binding("Ctrl+Alt+X", move_to_zone(BuiltInZone::RightHalf)),
        ]);
        let mut fake = fake_with_focus("left", Rect::new(200, 200, 800, 600));

        dispatch_hotkey(&config, "Ctrl+Alt+X", &mut fake).unwrap();

        assert_eq!(
            fake.moves,
            vec![WindowMove::new(Rect::new(0, 0, 960, 1080))]
        );
    }

    #[test]
    fn executor_errors_are_propagated() {
        let config = config(vec![binding(
            "Ctrl+Alt+Right",
            move_to_zone(BuiltInZone::RightHalf),
        )]);
        let mut fake = fake_with_focus("left", Rect::new(200, 200, 800, 600));
        fake.move_error = Some(WindowSystemError::Platform("denied".to_string()));

        let err = dispatch_hotkey(&config, "Ctrl+Alt+Right", &mut fake).unwrap_err();

        assert_eq!(
            err,
            DispatchHotkeyError::ExecuteAction(ExecuteActionError::WindowSystem(
                WindowSystemError::Platform("denied".to_string())
            ))
        );
    }
}
