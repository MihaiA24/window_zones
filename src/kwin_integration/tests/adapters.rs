use super::super::protocol::{
    DISPLAYS_CAPABILITY, KwinIntegrationError, MOVE_RESIZE_CAPABILITY, classify_dbus_error,
};
use super::super::test_bus::{FakeBus, FakeServiceState};
use super::{KwinHotkeySystem, KwinWindowSystem, require_capability};
use crate::{
    DisplayGeometry, FocusedWindow, HotkeyEvent, HotkeySystem, HotkeySystemError, Rect, WindowId,
    WindowMove, WindowSystem, WindowSystemError,
};

#[test]
fn display_requests_refresh_before_reading_the_cached_work_area() {
    let bus = FakeBus::new(FakeServiceState::complete());
    let window_system = KwinWindowSystem::with_bus_address(bus.address.clone());
    bus.update_state(|state| {
        state.next_displays = Some(vec![("monitor-0".to_string(), 0, 50, 1200, 850)]);
    });
    assert_eq!(
        window_system.displays().unwrap(),
        vec![DisplayGeometry::new(
            "monitor-0",
            Rect::new(0, 50, 1200, 850)
        )]
    );
    bus.update_state(|state| {
        assert!(
            state.next_displays.is_none(),
            "refresh must complete before reading"
        );
        state.request_error = Some((
            "org.window_zones.KWin.Error.Unavailable",
            "refresh failed".to_string(),
        ));
    });
    assert!(
        window_system
            .displays()
            .unwrap_err()
            .to_string()
            .contains("refresh failed")
    );
}

#[test]
fn private_service_round_trips_kde_window_and_hotkey_contract() {
    let bus = FakeBus::new(FakeServiceState::complete());
    let mut window_system = KwinWindowSystem::with_bus_address(bus.address.clone());

    assert_eq!(
        window_system.focused_window().unwrap(),
        Some(FocusedWindow::new(
            WindowId::new("window-1"),
            Rect::new(-300, -20, 801, 602)
        ))
    );
    assert_eq!(
        window_system.displays().unwrap(),
        vec![
            DisplayGeometry::new("monitor-0", Rect::new(-1920, 0, 1920, 1080)),
            DisplayGeometry::new("monitor-1", Rect::new(0, -50, 1280, 1024)),
        ]
    );

    window_system
        .move_window(&WindowMove::new(
            WindowId::new("window-1"),
            Rect::new(-123, 456, 777, 888),
        ))
        .unwrap();
    window_system
        .move_window(&WindowMove::new(
            WindowId::new("window-1"),
            Rect::new(12, -34, 500, 400),
        ))
        .unwrap();
    bus.update_state(|state| {
        assert_eq!(
            state.moves,
            vec![
                ("window-1".to_string(), -123, 456, 777, 888),
                ("window-1".to_string(), 12, -34, 500, 400),
            ]
        );
    });

    let mut hotkey_system = KwinHotkeySystem::with_bus_address(bus.address.clone());
    hotkey_system
        .register_hotkeys(&["alt+ctrl+left".to_string(), "cmd+1".to_string()])
        .unwrap();
    bus.update_state(|state| {
        assert_eq!(
            state.registered_hotkeys,
            vec!["alt+ctrl+left".to_string(), "cmd+1".to_string()]
        );
    });
    assert_eq!(
        hotkey_system.next_hotkey().unwrap(),
        Some(HotkeyEvent::Pressed {
            hotkey: "alt+ctrl+left".to_string()
        })
    );
    assert_eq!(hotkey_system.next_hotkey().unwrap(), None);
}

#[test]
fn move_keeps_observed_identity_after_focus_switches() {
    let bus = FakeBus::new(FakeServiceState::complete());
    let mut system = KwinWindowSystem::with_bus_address(bus.address.clone());
    let focused = system.focused_window().unwrap().unwrap();
    bus.update_state(|state| {
        state.focused = Some(("window-2".to_string(), 0, 0, 640, 480));
    });
    let target = Rect::new(-20, 30, 900, 600);
    system
        .move_window(&WindowMove::new(focused.id, target))
        .unwrap();
    bus.update_state(|state| {
        assert_eq!(
            state.moves,
            vec![("window-1".to_string(), -20, 30, 900, 600)]
        );
    });
}

#[test]
fn closed_window_maps_to_window_gone() {
    let mut state = FakeServiceState::complete();
    state.request_error = Some((
        "org.window_zones.KWin.Error.WindowGone",
        "window-1".to_string(),
    ));
    let bus = FakeBus::new(state);
    let mut system = KwinWindowSystem::with_bus_address(bus.address.clone());
    let movement = WindowMove::new(WindowId::new("window-1"), Rect::new(0, 0, 900, 600));
    assert_eq!(
        system.move_window(&movement),
        Err(WindowSystemError::WindowGone(WindowId::new("window-1")))
    );
    bus.update_state(|state| assert!(state.moves.is_empty()));
}

#[test]
fn constrained_moves_and_interrupted_restore_keep_the_original_restore_point() {
    use crate::{Action, ExecuteActionError, WindowHistory, execute_action};
    let original = Rect::new(90, 100, 700, 500);
    let constrained = Rect::new(0, 40, 800, 860);
    let interrupted_restore = Rect::new(20, 50, 650, 700);
    let mut state = FakeServiceState::complete();
    state.focused = Some(("window-1".to_string(), 90, 100, 700, 500));
    state.displays = vec![("output-1".to_string(), 0, 40, 1200, 860)];
    let bus = FakeBus::new(state);
    let mut system = KwinWindowSystem::with_bus_address(bus.address.clone());
    let mut history = WindowHistory::default();
    let zones = std::collections::BTreeMap::new();
    for (action, observed) in [
        (
            Action::MoveToZone {
                zone: "left-half".to_string(),
            },
            constrained,
        ),
        (Action::Restore {}, interrupted_restore),
    ] {
        bus.update_state(|state| {
            state.move_observed = Some(observed);
            state.move_error = Some((
                "org.window_zones.KWin.Error.PartialMove",
                serde_json::json!({
                    "window_id": "window-1",
                    "observed": [observed.x, observed.y, observed.width as i64, observed.height as i64],
                    "message": "client constrained the frame",
                }).to_string(),
            ));
        });
        assert!(matches!(
            execute_action(&action, &zones, &mut history, &mut system),
            Err(ExecuteActionError::WindowSystem(WindowSystemError::PartialMove {
                observed: actual, ..
            })) if actual == observed
        ));
    }
    bus.update_state(|state| {
        state.move_error = None;
        state.move_observed = Some(original);
    });
    execute_action(&Action::Restore {}, &zones, &mut history, &mut system).unwrap();
    bus.update_state(|state| {
        assert_eq!(
            state.moves,
            vec![("window-1".to_string(), 90, 100, 700, 500)]
        );
    });
    assert_eq!(
        execute_action(&Action::Restore {}, &zones, &mut history, &mut system),
        Err(ExecuteActionError::NothingToRestore)
    );
}

#[test]
fn partial_move_payloads_require_the_captured_identity_and_valid_frame() {
    let bus = FakeBus::new(FakeServiceState::complete());
    let mut system = KwinWindowSystem::with_bus_address(bus.address.clone());
    let movement = WindowMove::new(WindowId::new("window-1"), Rect::new(0, 40, 600, 860));
    let valid = serde_json::json!({
        "window_id": "window-1",
        "observed": [0, 40, 800, 860],
        "message": "client minimum width",
    });
    bus.update_state(|state| {
        state.focused = Some(("window-2".to_string(), 900, 100, 200, 300));
        state.move_error = Some(("org.window_zones.KWin.Error.PartialMove", valid.to_string()));
    });
    assert!(matches!(
        system.move_window(&movement),
        Err(WindowSystemError::PartialMove { observed, .. })
            if observed == Rect::new(0, 40, 800, 860)
    ));
    for (field, value) in [
        ("window_id", serde_json::json!("window-2")),
        ("window_id", serde_json::json!(1)),
        ("observed", serde_json::json!([0, 40, 0, 860])),
        ("observed", serde_json::json!([0, 40, 4294967296_u64, 860])),
        ("observed", serde_json::json!([0.5, 40, 800, 860])),
        ("message", serde_json::Value::Null),
        ("unexpected", serde_json::json!(true)),
    ] {
        let mut payload = valid.clone();
        payload[field] = value;
        bus.update_state(|state| {
            state.move_error = Some((
                "org.window_zones.KWin.Error.PartialMove",
                payload.to_string(),
            ));
        });
        assert!(
            matches!(
                system.move_window(&movement),
                Err(WindowSystemError::Platform(_))
            ),
            "invalid payload must not change history: {payload}"
        );
    }
}

#[test]
fn hotkey_conflict_is_rejected_and_previous_set_survives() {
    let bus = FakeBus::new(FakeServiceState::complete());
    let mut system = KwinHotkeySystem::with_bus_address(bus.address.clone());
    system
        .register_hotkeys(&["alt+ctrl+left".to_string()])
        .unwrap();
    bus.update_state(|state| {
        state.request_error = Some((
            "org.window_zones.KWin.Error.Unsupported",
            "accelerator is already held; choose a different binding".to_string(),
        ));
    });
    assert!(matches!(
        system.register_hotkeys(&["alt+ctrl+right".to_string()]),
        Err(HotkeySystemError::Rejected(_))
    ));
    bus.update_state(|state| {
        assert_eq!(state.registered_hotkeys, vec!["alt+ctrl+left"]);
        state.request_error = None;
    });
    drop(system);
    bus.update_state(|state| assert!(state.registered_hotkeys.is_empty()));
}

#[test]
fn unavailable_kde_companion_is_actionable() {
    let window_system = KwinWindowSystem::with_bus_address("unix:path=/window-zones-no-such-bus");
    let error = window_system
        .capabilities()
        .expect_err("missing KWin companion must fail");

    assert!(matches!(error, KwinIntegrationError::Unavailable { .. }));
    assert!(error.to_string().contains("KWin companion"));
}

#[test]
fn incompatible_kde_companion_is_classified() {
    let mut state = FakeServiceState::complete();
    state.capabilities_error = Some((
        "org.window_zones.KWin.Error.Incompatible",
        "protocol mismatch".to_string(),
    ));
    let bus = FakeBus::new(state);
    let window_system = KwinWindowSystem::with_bus_address(bus.address.clone());

    let error = window_system
        .capabilities()
        .expect_err("incompatible KWin companion must fail");
    assert!(matches!(error, KwinIntegrationError::Incompatible { .. }));
    assert!(error.to_string().contains("upgrade"));
}

#[test]
fn missing_capability_is_explicit() {
    let error = require_capability(&[DISPLAYS_CAPABILITY.to_string()], MOVE_RESIZE_CAPABILITY)
        .expect_err("missing capability should fail");

    assert!(matches!(error, KwinIntegrationError::Unsupported { .. }));
    assert!(error.to_string().contains("move-resize"));
}

#[test]
fn lifecycle_errors_are_classified() {
    let error = dbus::Error::new_custom("org.window_zones.KWin.Error.Incompatible", "old script");
    assert!(matches!(
        classify_dbus_error(error),
        KwinIntegrationError::Incompatible { .. }
    ));
}
