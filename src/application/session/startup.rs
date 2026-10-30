#[cfg(target_os = "linux")]
use std::ffi::OsString;
use std::io::{self, IsTerminal};
#[cfg(target_os = "linux")]
use std::path::Path;
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
                compositor_dispatch_identity(backend, path, |name| std::env::var_os(name))?;
            runtime.compositor_identity = Some(identity.clone());
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
        return run_runtime(&mut runtime, &mut TuiSurface::new(), instructions, true);
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
            return run_runtime(&mut runtime, &mut surface, instructions, true);
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
    run_runtime(&mut runtime, &mut surface, instructions, interactive)
}

#[cfg(target_os = "linux")]
fn compositor_dispatch_identity(
    backend: RuntimeBackend,
    config_path: &Path,
    get_env: impl Fn(&str) -> Option<OsString>,
) -> io::Result<local_dispatch::Identity> {
    let required = |name| {
        get_env(name)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{name} is required to identify the compositor instance"),
                )
            })
    };
    let (backend, socket) = match backend {
        RuntimeBackend::Wayland(ScriptedCompositor::Sway) => {
            ("sway", PathBuf::from(required("SWAYSOCK")?))
        }
        RuntimeBackend::Wayland(ScriptedCompositor::Hyprland) => {
            let signature = required("HYPRLAND_INSTANCE_SIGNATURE")?;
            let runtime_dir = get_env("XDG_RUNTIME_DIR")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    PathBuf::from(format!(
                        "/run/user/{}",
                        crate::application::ownership::current_uid()
                    ))
                });
            let socket = runtime_dir
                .join("hypr")
                .join(&signature)
                .join(".socket.sock");
            let legacy = PathBuf::from("/tmp/hypr")
                .join(signature)
                .join(".socket.sock");
            (
                "hyprland",
                if !socket.exists() && legacy.exists() {
                    legacy
                } else {
                    socket
                },
            )
        }
        _ => unreachable!("only compositor-bound backends use local dispatch"),
    };
    local_dispatch::Identity::new(backend, &socket, config_path)
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
        let identity = compositor_dispatch_identity(backend, &path, |name| std::env::var_os(name))
            .map_err(|error| format!("cannot identify dispatch session: {error}"))?;
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    use window_zones::ScriptedCompositor;

    use super::compositor_dispatch_identity;
    use crate::application::backends::RuntimeBackend;

    #[test]
    fn dispatch_identity_requires_and_distinguishes_compositor_connections() {
        let config = Path::new("/nonexistent/window-zones-dispatch.toml");
        for (compositor, variable, first, second) in [
            (
                ScriptedCompositor::Sway,
                "SWAYSOCK",
                "/nonexistent/sway-a.sock",
                "/nonexistent/sway-b.sock",
            ),
            (
                ScriptedCompositor::Hyprland,
                "HYPRLAND_INSTANCE_SIGNATURE",
                "window-zones-a",
                "window-zones-b",
            ),
        ] {
            let backend = RuntimeBackend::Wayland(compositor);
            assert!(compositor_dispatch_identity(backend, config, |_| None).is_err());
            assert!(
                compositor_dispatch_identity(backend, config, |_| Some(OsString::new())).is_err()
            );
            let identity = |instance| {
                compositor_dispatch_identity(backend, config, |name| match name {
                    name if name == variable => Some(OsString::from(instance)),
                    "XDG_RUNTIME_DIR" => Some(OsString::from("/nonexistent/window-zones-runtime")),
                    _ => None,
                })
                .unwrap()
            };
            assert_eq!(identity(first), identity(first));
            assert_ne!(identity(first), identity(second));
        }
        let hyprland = RuntimeBackend::Wayland(ScriptedCompositor::Hyprland);
        let identity = |runtime_dir| {
            compositor_dispatch_identity(hyprland, config, |name| match name {
                "HYPRLAND_INSTANCE_SIGNATURE" => {
                    Some(OsString::from("window-zones-same-signature"))
                }
                "XDG_RUNTIME_DIR" => Some(OsString::from(runtime_dir)),
                _ => None,
            })
            .unwrap()
        };
        assert_ne!(
            identity("/nonexistent/runtime-a"),
            identity("/nonexistent/runtime-b")
        );
    }
}
