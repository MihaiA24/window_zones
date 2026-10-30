use thiserror::Error;

use crate::config::{AppConfig, normalize_hotkey};
use crate::executor::{ExecuteActionError, WindowHistory, execute_action};
use crate::window_system::WindowSystem;

/// Errors produced while dispatching an already-received hotkey string.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DispatchHotkeyError {
    #[error("no binding configured for hotkey: {hotkey}")]
    NoBindingForHotkey { hotkey: String },
    #[error(transparent)]
    ExecuteAction(#[from] ExecuteActionError),
}

/// Dispatches a received hotkey string after normalization and canonicalization.
/// Matching is deterministic across modifier/key case, spacing, and token order.
pub fn dispatch_hotkey<W: WindowSystem>(
    config: &AppConfig,
    hotkey: &str,
    history: &mut WindowHistory,
    window_system: &mut W,
) -> Result<(), DispatchHotkeyError> {
    let normalized_hotkey =
        normalize_hotkey(hotkey).map_err(|_| DispatchHotkeyError::NoBindingForHotkey {
            hotkey: hotkey.to_string(),
        })?;

    let binding = config
        .bindings
        .iter()
        .find(|binding| binding.hotkey == normalized_hotkey)
        .ok_or_else(|| DispatchHotkeyError::NoBindingForHotkey {
            hotkey: hotkey.to_string(),
        })?;

    execute_action(&binding.action, &config.zones, history, window_system)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ZoneDefinition;
    use crate::actions::{Action, Binding};
    use crate::geometry::Rect;
    use crate::window_system::{FakeWindowSystem, WindowId, WindowMove, WindowSystemError};
    use std::collections::BTreeMap;

    fn config(bindings: Vec<Binding>, zones: BTreeMap<String, ZoneDefinition>) -> AppConfig {
        AppConfig { bindings, zones }
    }

    fn binding(hotkey: &str, action: Action) -> Binding {
        Binding {
            hotkey: hotkey.to_string(),
            action,
        }
    }

    fn move_to_zone(zone: &str) -> Action {
        Action::MoveToZone {
            zone: zone.to_string(),
        }
    }

    fn window() -> WindowId {
        WindowId::new("window")
    }

    fn dispatch(
        config: &AppConfig,
        hotkey: &str,
        fake: &mut FakeWindowSystem,
    ) -> Result<(), DispatchHotkeyError> {
        dispatch_hotkey(config, hotkey, &mut WindowHistory::default(), fake)
    }

    #[test]
    fn dispatches_known_hotkey_to_zone_movement() {
        let config = config(
            vec![binding("alt+ctrl+left", move_to_zone("left-half"))],
            BTreeMap::new(),
        );
        let mut fake = FakeWindowSystem::with_focus(Rect::new(200, 200, 800, 600));

        dispatch(&config, "Ctrl+Alt+Left", &mut fake).unwrap();

        assert_eq!(
            fake.moves,
            vec![WindowMove::new(window(), Rect::new(0, 0, 960, 1080))]
        );
    }

    #[test]
    fn dispatches_custom_zone_from_config() {
        let mut zones = BTreeMap::new();
        zones.insert(
            "side".to_string(),
            ZoneDefinition {
                x: 0,
                y: 50,
                width: 50,
                height: 50,
            },
        );

        let config = config(vec![binding("alt+ctrl+left", move_to_zone("side"))], zones);
        let mut fake = FakeWindowSystem::with_focus(Rect::new(0, 0, 1920, 1080));

        dispatch(&config, "alt+ctrl+left", &mut fake).unwrap();

        assert_eq!(
            fake.moves,
            vec![WindowMove::new(window(), Rect::new(0, 540, 960, 540))]
        );
    }

    #[test]
    fn dispatches_known_hotkey_to_display_movement() {
        let config = config(
            vec![binding(
                "alt+ctrl+shift+right",
                Action::MoveToNextDisplay {},
            )],
            BTreeMap::new(),
        );
        let mut fake = FakeWindowSystem::with_focus(Rect::new(0, 0, 960, 1080));

        dispatch(&config, "alt+shift+ctrl+right", &mut fake).unwrap();

        assert_eq!(
            fake.moves,
            vec![WindowMove::new(window(), Rect::new(1920, 0, 1280, 1440))]
        );
    }

    #[test]
    fn unknown_hotkey_returns_error_without_moving() {
        let config = config(
            vec![binding("alt+ctrl+left", move_to_zone("left-half"))],
            BTreeMap::new(),
        );
        let mut fake = FakeWindowSystem::with_focus(Rect::new(200, 200, 800, 600));

        let err = dispatch(&config, "ctrl+alt+left+right", &mut fake).unwrap_err();

        assert_eq!(
            err,
            DispatchHotkeyError::NoBindingForHotkey {
                hotkey: "ctrl+alt+left+right".to_string()
            }
        );
        assert!(fake.moves.is_empty());
    }

    #[test]
    fn duplicate_hotkeys_use_first_match() {
        let config = config(
            vec![
                binding("alt+ctrl+x", move_to_zone("left-half")),
                binding("alt+ctrl+x", Action::MoveToNextDisplay {}),
            ],
            BTreeMap::new(),
        );
        let mut fake = FakeWindowSystem::with_focus(Rect::new(200, 200, 800, 600));

        dispatch(&config, "alt+ctrl+x", &mut fake).unwrap();

        assert_eq!(
            fake.moves,
            vec![WindowMove::new(window(), Rect::new(0, 0, 960, 1080))]
        );
    }

    #[test]
    fn executor_errors_are_propagated() {
        let mut fake = FakeWindowSystem::with_focus(Rect::new(200, 200, 800, 600));
        fake.move_error = Some(WindowSystemError::Platform("denied".to_string()));

        let config = config(
            vec![binding("alt+ctrl+right", move_to_zone("right-half"))],
            BTreeMap::new(),
        );

        let err = dispatch(&config, "alt+ctrl+right", &mut fake).unwrap_err();

        assert_eq!(
            err,
            DispatchHotkeyError::ExecuteAction(ExecuteActionError::WindowSystem(
                WindowSystemError::Platform("denied".to_string())
            ))
        );
    }
}
