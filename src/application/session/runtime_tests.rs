use std::fs;
use std::sync::mpsc;

use window_zones::{DispatchState, Rect, starter_config};

use super::runner::run_runtime;
use super::{Runtime, RuntimeInstruction};
use crate::application::backends::RuntimeBackend;
use crate::application::surfaces::CliSurface;
use crate::application::test_support::temp_path;

#[test]
fn unified_runtime_keeps_performance_logging_through_restart() {
    let config_path = temp_path("runtime_perf_config.toml");
    let log_path = temp_path("runtime_perf.jsonl");
    fs::write(&config_path, starter_config("Ctrl+Alt")).unwrap();
    let mut runtime = Runtime::new(
        Some(config_path.clone()),
        RuntimeBackend::DryRun,
        Some(&log_path),
    )
    .unwrap();
    let (tx, instructions) = mpsc::channel();
    for instruction in [
        RuntimeInstruction::Dispatch("Ctrl+Alt+Right".to_string()),
        RuntimeInstruction::Restart,
        RuntimeInstruction::Dispatch("Ctrl+Alt+Left".to_string()),
        RuntimeInstruction::Quit,
    ] {
        tx.send(instruction.into()).unwrap();
    }
    drop(tx);
    run_runtime(
        &mut runtime,
        &mut CliSurface {
            prompt: false,
            notifier: None,
        },
        instructions,
        true,
    );
    let rows: Vec<serde_json::Value> = fs::read_to_string(&log_path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["event"], "session");
    assert_eq!(rows[1]["hotkey"], "alt+ctrl+right");
    assert_eq!(rows[2]["hotkey"], "alt+ctrl+left");
    assert_eq!(rows[1]["result"], "ok");
    assert_eq!(rows[2]["result"], "ok");
    assert!(runtime.app.hotkeys_registered());
    assert!(runtime.app.perf_log_error().is_none());
    drop(runtime);
    fs::remove_file(config_path).unwrap();
    fs::remove_file(log_path).unwrap();
}

#[test]
fn runtime_reports_performance_log_open_failure_before_dispatch() {
    let log_path = temp_path("runtime_perf_directory");
    fs::create_dir_all(&log_path).unwrap();
    let result = Runtime::new(
        Some(temp_path("runtime_perf_missing_config.toml")),
        RuntimeBackend::DryRun,
        Some(&log_path),
    );
    assert!(matches!(result, Err(error) if error.contains("Cannot open performance log")));
    fs::remove_dir(log_path).unwrap();
}

#[test]
fn dry_run_runtime_crosses_displays_and_restores_the_simulated_window() {
    let config_path = temp_path("dry_run_crossing.toml");
    fs::write(&config_path, starter_config("Ctrl+Alt")).unwrap();
    let mut runtime =
        Runtime::new(Some(config_path.clone()), RuntimeBackend::DryRun, None).unwrap();

    let mut targets = Vec::new();
    for hotkey in [
        "ctrl+alt+right",
        "ctrl+alt+right",
        "ctrl+alt+left",
        "ctrl+alt+backspace",
    ] {
        assert_eq!(
            runtime.dispatch(hotkey),
            DispatchState::Succeeded,
            "{hotkey}"
        );
        targets.push(runtime.window_system.last_move().unwrap().target);
    }
    fs::remove_file(config_path).unwrap();

    assert_eq!(
        targets,
        vec![
            Rect::new(960, 0, 960, 1080),
            Rect::new(1920, 0, 640, 1024),
            Rect::new(960, 0, 960, 1080),
            Rect::new(40, 40, 640, 480),
        ]
    );
}
