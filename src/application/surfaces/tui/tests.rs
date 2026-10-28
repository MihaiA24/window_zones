use std::fs;
use std::time::Instant;

use window_zones::{App, HotkeyEvent, HotkeySystem, HotkeySystemError};

use super::{
    CAPABILITY_REFRESH_INTERVAL, Report, Surface, TuiCapabilityStatus, TuiSurface,
    tui_capability_status_from, tui_last_error,
};
use crate::application::backends::{RuntimeBackend, RuntimeWindowSystem};
use crate::application::session::Runtime;
use crate::application::test_support::temp_path;

#[test]
fn tui_skips_idle_frames_and_redraws_runtime_events() {
    let runtime = Runtime::new(
        Some(temp_path("idle_tui_missing_config.toml")),
        RuntimeBackend::DryRun,
        None,
    )
    .unwrap();
    let now = Instant::now();
    let mut surface = TuiSurface::new();
    surface.snapshot = "untouched".to_string();
    surface.dirty = false;
    surface.capabilities_due = now + CAPABILITY_REFRESH_INTERVAL;
    surface.refresh(&runtime, now, false);
    assert_eq!(surface.snapshot, "untouched");
    surface.report(&runtime, Report::Event(runtime.app.startup_event()));
    surface.refresh(&runtime, now, false);
    assert!(surface.snapshot.contains("Window Zones TUI"));
    assert!(!surface.dirty);
}

#[test]
fn tui_capability_status_distinguishes_complete_degraded_and_unavailable_companions() {
    let complete = tui_capability_status_from(Some(Ok(vec![
        "focused-window".to_string(),
        "displays".to_string(),
        "move-resize".to_string(),
        "hotkeys".to_string(),
    ])));
    assert_eq!(complete.window, "focused-window, displays, move-resize");
    assert_eq!(complete.hotkey, "hotkeys");
    assert_eq!(complete.diagnostic, None);

    let degraded = tui_capability_status_from(Some(Ok(vec!["focused-window".to_string()])));
    assert_eq!(degraded.window, "focused-window");
    assert_eq!(degraded.hotkey, "<none>");

    let unavailable =
        tui_capability_status_from(Some(Err("GNOME companion unavailable".to_string())));
    assert_eq!(unavailable.window, "unavailable");
    assert_eq!(unavailable.hotkey, "unavailable");
    assert_eq!(
        unavailable.diagnostic.as_deref(),
        Some("GNOME companion unavailable")
    );
}

#[test]
fn tui_error_precedence_prefers_runtime_errors_over_companion_diagnostics() {
    struct RefusingHotkeys;
    impl HotkeySystem for RefusingHotkeys {
        fn register_hotkeys(&mut self, _: &[String]) -> Result<(), HotkeySystemError> {
            Err(HotkeySystemError::Rejected("taken".to_string()))
        }
        fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
            Ok(None)
        }
    }
    let capability_status = TuiCapabilityStatus {
        window: "unavailable".to_string(),
        hotkey: "unavailable".to_string(),
        diagnostic: Some("companion unavailable".to_string()),
    };

    let missing_app = App::start_at(temp_path("missing.toml"));
    let config_path = temp_path("error_precedence.toml");
    fs::write(&config_path, "bindings = [").unwrap();
    let mut app = App::start_at(&config_path);
    let config_error = tui_last_error(&app, &capability_status);
    app.tick(&mut RefusingHotkeys, Instant::now());
    let hotkey_error = tui_last_error(&app, &capability_status);
    app.dispatch_hotkey(
        "Ctrl+Alt+Left",
        &mut RuntimeWindowSystem::with_backend(RuntimeBackend::DryRun),
    );
    let dispatch_error = tui_last_error(&app, &capability_status);
    fs::remove_file(config_path).unwrap();

    assert_eq!(
        tui_last_error(&missing_app, &capability_status),
        "companion unavailable"
    );
    assert!(config_error.starts_with("config: "), "{config_error}");
    assert!(
        hotkey_error.starts_with("hotkey registration: "),
        "{hotkey_error}"
    );
    assert!(dispatch_error.starts_with("dispatch: "), "{dispatch_error}");
}
