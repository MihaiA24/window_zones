use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;

use window_zones::logging::Logger;
#[cfg(target_os = "linux")]
use window_zones::{ScriptedCompositor, default_config_path};

use super::Runtime;
use super::instructions::{parse_runtime_input, parse_tui_input, spawn_stdin_reader};
#[cfg(target_os = "linux")]
use super::local_dispatch;
use super::runner::run_runtime;
use crate::application::backends::RuntimeBackend;
use crate::application::cli::Command;
use crate::application::notifications::Notifier;
use crate::application::ownership::{
    EXIT_ALREADY_RUNNING, InstanceLockError, acquire_instance_lock, instance_lock_path,
};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use crate::application::surfaces::TraySurface;
use crate::application::surfaces::{CliSurface, TuiSurface};

pub(in crate::application) fn execute_session(
    config_path: Option<PathBuf>,
    backend: RuntimeBackend,
    command: &Command,
    show_tray: bool,
    perf_log_path: Option<&PathBuf>,
    logger: Option<Logger>,
) -> ExitCode {
    let _lock = if matches!(backend, RuntimeBackend::DryRun) {
        None
    } else {
        match instance_lock_path()
            .map_err(InstanceLockError::Io)
            .and_then(|path| acquire_instance_lock(&path))
        {
            Ok(lock) => Some(lock),
            Err(error) => {
                let (message, code) = match error {
                    InstanceLockError::AlreadyRunning(message) => {
                        (message, ExitCode::from(EXIT_ALREADY_RUNNING))
                    }
                    InstanceLockError::Io(message) => (message, ExitCode::FAILURE),
                };
                window_zones::logging::diagnostic(format_args!("Error: {message}"));
                return code;
            }
        }
    };

    let mut runtime = match Runtime::new(config_path, backend, perf_log_path) {
        Ok(runtime) => runtime,
        Err(error) => {
            window_zones::logging::diagnostic(format_args!("{error}"));
            return ExitCode::FAILURE;
        }
    };
    if let Some(logger) = logger {
        runtime.app.enable_logging(logger);
    }
    let (tx, instructions) = mpsc::channel();
    #[cfg(target_os = "linux")]
    let _dispatch_server = if matches!(backend, RuntimeBackend::Wayland(_)) {
        let server = (|| {
            let path = runtime.app.config_path().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "cannot resolve session config path",
                )
            })?;
            let identity =
                local_dispatch::Identity::new(compositor_dispatch_backend(backend), path)?;
            local_dispatch::Server::start(
                _lock.as_ref().expect("live session holds instance lock"),
                identity,
                tx.clone(),
            )
        })();
        match server {
            Ok(server) => Some(server),
            Err(error) => {
                window_zones::logging::diagnostic(format_args!(
                    "Cannot start compositor-bound dispatch: {error}"
                ));
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };
    let interactive = io::stdin().is_terminal();

    if matches!(command, Command::Tui) {
        spawn_stdin_reader(parse_tui_input, tx);
        run_runtime(&mut runtime, &mut TuiSurface::new(), instructions, true);
        return ExitCode::SUCCESS;
    }

    if show_tray {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        {
            let mut surface = match TraySurface::new(&runtime, &tx) {
                Ok(surface) => surface,
                Err(error) => {
                    window_zones::logging::diagnostic(format_args!(
                        "Failed to start tray surface: {error}"
                    ));
                    return ExitCode::FAILURE;
                }
            };
            drop(tx);
            run_runtime(&mut runtime, &mut surface, instructions, true);
            return ExitCode::SUCCESS;
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        window_zones::logging::diagnostic(format_args!(
            "Tray mode is not supported on this platform yet; continuing without it."
        ));
    }

    let mut surface = CliSurface {
        prompt: interactive,
        // Without a terminal nobody reads stdout live: surface problems on the desktop.
        notifier: Notifier::new(backend),
    };
    spawn_stdin_reader(parse_runtime_input, tx);
    run_runtime(&mut runtime, &mut surface, instructions, interactive);
    ExitCode::SUCCESS
}

#[cfg(target_os = "linux")]
fn compositor_dispatch_backend(backend: RuntimeBackend) -> &'static str {
    match backend {
        RuntimeBackend::Wayland(ScriptedCompositor::Sway) => "sway",
        RuntimeBackend::Wayland(ScriptedCompositor::Hyprland) => "hyprland",
        _ => unreachable!("only compositor-bound backends use local dispatch"),
    }
}

#[cfg(target_os = "linux")]
pub(in crate::application) fn execute_compositor_dispatch(
    config_path: Option<PathBuf>,
    backend: RuntimeBackend,
    perf_log_path: Option<&PathBuf>,
    hotkey: &str,
    logger: Option<&Logger>,
) -> ExitCode {
    if perf_log_path.is_some() {
        window_zones::logging::diagnostic(format_args!(
            "Error: compositor-bound dispatch uses session-owned performance logging; set --perf-log on `window_zones run` or `tui`, not on dispatch clients"
        ));
        return ExitCode::FAILURE;
    }
    let response = (|| {
        let path = config_path
            .map_or_else(default_config_path, Ok)
            .map_err(|error| error.to_string())?;
        let identity = local_dispatch::Identity::new(compositor_dispatch_backend(backend), &path)
            .map_err(|error| format!("cannot resolve dispatch config path: {error}"))?;
        local_dispatch::dispatch(&instance_lock_path()?, identity, hotkey)
    })();
    match response {
        Ok(response) => {
            if let Some(logger) = logger {
                logger.log(format_args!(
                    "compositor dispatch {hotkey}: {}",
                    response.output
                ));
            }
            print!("{}", response.output);
            if response.success {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            if let Some(logger) = logger {
                logger.log(format_args!("compositor dispatch {hotkey} failed: {error}"));
            }
            window_zones::logging::diagnostic(format_args!("Error: {error}"));
            ExitCode::FAILURE
        }
    }
}
