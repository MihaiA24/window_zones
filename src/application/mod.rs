mod backends;
mod cli;
mod notifications;
mod ownership;
mod session;
mod surfaces;

#[cfg(test)]
mod test_support;

use std::process::ExitCode;

use window_zones::logging::Logger;

#[cfg(target_os = "linux")]
use backends::RuntimeBackend;
use backends::{RuntimeWindowSystem, resolve_runtime_backend};
use cli::{Command, ParseStatus, execute_init, parse_args, print_help};
#[cfg(target_os = "linux")]
use session::execute_compositor_dispatch;
use session::{build_app, execute_session};
use surfaces::{execute_dispatch, execute_status};

pub(super) fn run() -> ExitCode {
    let args = match parse_args() {
        ParseStatus::Help => {
            print_help();
            return ExitCode::SUCCESS;
        }
        ParseStatus::Err(message) => {
            window_zones::logging::diagnostic(format_args!("Error: {message}"));
            print_help();
            return ExitCode::FAILURE;
        }
        ParseStatus::Ok(args) => args,
    };
    let logger = args.log_file_path.as_ref().map(|path| {
        let session = match &args.command {
            Command::Status => "status",
            Command::Dispatch { .. } => "dispatch",
            Command::Run => "run",
            Command::Tui => "tui",
            Command::Init => "init",
        };
        Logger::new(path, session)
    });

    if matches!(args.command, Command::Init) {
        return execute_init(args.config_path, args.chord.as_deref(), args.force);
    }

    let backend = match resolve_runtime_backend(args.backend) {
        Ok(backend) => backend,
        Err(error) => {
            #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
            window_zones::logging::diagnostic(format_args!("Backend selection failed: {error}"));
            #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
            window_zones::logging::diagnostic(format_args!("{error}"));
            return ExitCode::FAILURE;
        }
    };

    if matches!(args.command, Command::Run | Command::Tui) {
        return execute_session(
            args.config_path,
            backend,
            &args.command,
            args.show_tray,
            args.perf_log_path.as_ref(),
            logger,
        );
    }
    #[cfg(target_os = "linux")]
    if let Command::Dispatch { hotkey } = &args.command
        && matches!(backend, RuntimeBackend::Wayland(_))
    {
        return execute_compositor_dispatch(
            args.config_path,
            backend,
            args.perf_log_path.as_ref(),
            hotkey,
            logger.as_ref(),
        );
    }
    let mut app = match build_app(args.config_path.as_ref(), args.perf_log_path.as_ref()) {
        Ok(app) => app,
        Err(error) => {
            window_zones::logging::diagnostic(format_args!("{error}"));
            return ExitCode::FAILURE;
        }
    };
    if let Some(logger) = logger {
        app.enable_logging(logger);
    }

    match &args.command {
        Command::Status => {
            execute_status(&app, &RuntimeWindowSystem::with_backend(backend));
            ExitCode::SUCCESS
        }
        Command::Dispatch { hotkey } => {
            execute_dispatch(app, RuntimeWindowSystem::with_backend(backend), hotkey)
        }
        Command::Run | Command::Tui | Command::Init => {
            unreachable!("handled before one-shot App construction")
        }
    }
}
