use std::io;
use std::process::ExitCode;

use window_zones::{App, ConfigState, DispatchState};

use crate::application::backends::RuntimeWindowSystem;

pub(in crate::application) fn runtime_status_lines(app: &App) -> Vec<String> {
    let mut status = vec![
        "Runtime status:".to_string(),
        format!(
            "  config path: {}",
            app.config_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<unresolved>".to_string())
        ),
        format!("  binding count: {}", app.config().bindings.len()),
        format!("  config state: {:?}", app.config_state()),
        format!("  hotkey state: {:?}", app.hotkey_state()),
        format!(
            "  last action: {}",
            app.last_dispatch_hotkey().unwrap_or("<none>")
        ),
        format!("  dispatch state: {:?}", app.dispatch_state()),
    ];

    if let ConfigState::Error(error) = app.config_state() {
        status.push(format!("  config error: {error}"));
    }
    if let DispatchState::Error(error) = app.dispatch_state() {
        status.push(format!("  dispatch error: {error}"));
    }
    if let Some(error) = app.perf_log_error() {
        status.push(format!("  performance log error: {error}"));
    }

    status
}

pub(in crate::application) fn print_status(app: &App) {
    for line in runtime_status_lines(app) {
        println!("{line}");
    }
}

pub(in crate::application) fn print_dispatch_state(
    state: &DispatchState,
    window_system: &RuntimeWindowSystem,
) {
    let _ = write_dispatch_state(&mut io::stdout().lock(), state, window_system);
}

pub(in crate::application) fn write_dispatch_state(
    output: &mut impl io::Write,
    state: &DispatchState,
    window_system: &RuntimeWindowSystem,
) -> io::Result<()> {
    writeln!(output, "Dispatch state: {state:?}")?;
    match state {
        DispatchState::Error(error) => writeln!(output, "  error: {error}")?,
        DispatchState::Succeeded => {
            if let Some(window_move) = window_system.last_move() {
                writeln!(
                    output,
                    "  last move: x={} y={} w={} h={}",
                    window_move.target.x,
                    window_move.target.y,
                    window_move.target.width,
                    window_move.target.height
                )?;
            }
        }
        DispatchState::Idle => {}
    }
    Ok(())
}

pub(in crate::application) fn execute_dispatch(
    mut app: App,
    mut window_system: RuntimeWindowSystem,
    hotkey: &str,
) -> ExitCode {
    println!("Using runtime window backend: {}", window_system.name());
    print_status(&app);

    let state = app.dispatch_hotkey(hotkey, &mut window_system).clone();
    print_dispatch_state(&state, &window_system);
    ExitCode::SUCCESS
}

pub(in crate::application) fn execute_status(app: &App, window_system: &RuntimeWindowSystem) {
    println!("Using runtime window backend: {}", window_system.name());
    match window_system.companion_capabilities() {
        Some((label, Ok(capabilities))) => {
            println!("{label} companion capabilities: {capabilities:?}");
        }
        Some((label, Err(error))) => println!("{label} companion status: {error}"),
        None => {}
    }
    print_status(app);
}
