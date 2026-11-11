#[cfg(any(target_os = "linux", all(test, unix)))]
use std::io::Write as _;
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(any(target_os = "linux", all(test, unix)))]
use window_zones::DispatchState;

#[cfg(any(target_os = "linux", all(test, unix)))]
use super::local_dispatch;
use super::{Runtime, RuntimeInstruction, RuntimeRequest};
use crate::application::surfaces::{Report, Surface};
#[cfg(any(target_os = "linux", all(test, unix)))]
use crate::application::surfaces::{runtime_status_lines, write_dispatch_state};

/// How often queued hotkey presses are read; bounds keypress-to-move latency.
const HOTKEY_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Drives a surface until Quit. With `quit_on_input_end`, the end of the
/// instruction stream ends the session (interactive terminals); otherwise the
/// runtime keeps serving hotkeys (services with stdin closed).
pub(super) fn run_runtime(
    runtime: &mut Runtime,
    surface: &mut impl Surface,
    instructions: mpsc::Receiver<RuntimeRequest>,
    quit_on_input_end: bool,
) -> ExitCode {
    surface.start(runtime);
    surface.report(runtime, Report::Event(runtime.app.startup_event()));

    let mut input_open = true;
    let exit_code = loop {
        let (events, dispatches) = runtime.poll(Instant::now());
        for event in events {
            surface.report(runtime, Report::Event(event));
        }
        for dispatch in dispatches {
            surface.report(runtime, Report::HotkeyDispatch(dispatch));
        }
        let instruction = if input_open {
            match instructions.recv_timeout(HOTKEY_POLL_INTERVAL) {
                Ok(instruction) => Some(instruction),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    input_open = false;
                    if quit_on_input_end {
                        surface.report(runtime, Report::InputClosed);
                        break ExitCode::SUCCESS;
                    }
                    None
                }
            }
        } else {
            thread::sleep(HOTKEY_POLL_INTERVAL);
            None
        };

        let now = Instant::now();
        let handled_input = instruction.is_some();
        if let Some(request) = instruction {
            #[cfg(any(target_os = "linux", all(test, unix)))]
            if let Some(reply) = &request.reply {
                let rejection = if now >= reply.deadline {
                    Some("Error: dispatch expired before execution; no action was run\n")
                } else if !reply.identity.compositor_is_current() {
                    Some(
                        "Error: compositor instance changed before execution; no action was run; restart the session\n",
                    )
                } else {
                    None
                };
                if let Some(output) = rejection {
                    let _ = reply.sender.try_send(local_dispatch::Response {
                        success: false,
                        output: output.to_string(),
                    });
                    continue;
                }
            }
            #[cfg(any(target_os = "linux", all(test, unix)))]
            if matches!(request.instruction, RuntimeInstruction::Dispatch(_))
                && runtime
                    .compositor_identity
                    .as_ref()
                    .is_some_and(|identity| !identity.compositor_is_current())
            {
                window_zones::logging::diagnostic(format_args!(
                    "Compositor instance changed before execution; no action was run; close and relaunch the session"
                ));
                break ExitCode::FAILURE;
            }
            match request.instruction {
                RuntimeInstruction::Quit => break ExitCode::SUCCESS,
                #[cfg(target_os = "linux")]
                RuntimeInstruction::SurfaceFailed(message) => {
                    window_zones::logging::diagnostic(format_args!(
                        "Runtime surface failed: {message}"
                    ));
                    break ExitCode::FAILURE;
                }
                RuntimeInstruction::InputClosed if quit_on_input_end => {
                    surface.report(runtime, Report::InputClosed);
                    break ExitCode::SUCCESS;
                }
                RuntimeInstruction::InputClosed | RuntimeInstruction::Empty => {}
                RuntimeInstruction::Status => surface.report(runtime, Report::Status),
                RuntimeInstruction::Help => surface.report(runtime, Report::Help),
                RuntimeInstruction::Unknown(message) => {
                    surface.report(runtime, Report::Unknown(message));
                }
                RuntimeInstruction::Reload => {
                    surface.report(runtime, Report::ReloadRequested);
                    for event in runtime.app.reload(&mut runtime.hotkey_system, now) {
                        surface.report(runtime, Report::Event(event));
                    }
                }
                RuntimeInstruction::Restart => {
                    let events = runtime.restart(now);
                    surface.report(runtime, Report::Restarted);
                    for event in events {
                        surface.report(runtime, Report::Event(event));
                    }
                }
                RuntimeInstruction::Dispatch(hotkey) => {
                    #[cfg(any(target_os = "linux", all(test, unix)))]
                    let mut output = request.reply.as_ref().map(|_| {
                        let mut output = Vec::new();
                        let _ = writeln!(
                            output,
                            "Using runtime window backend: {}",
                            runtime.window_system.name()
                        );
                        for line in runtime_status_lines(&runtime.app) {
                            let _ = writeln!(output, "{line}");
                        }
                        output
                    });
                    let state = runtime.dispatch(&hotkey);
                    #[cfg(any(target_os = "linux", all(test, unix)))]
                    if let Some(reply) = request.reply {
                        let output = output.as_mut().expect("remote report initialized");
                        let _ = write_dispatch_state(output, &state, &runtime.window_system);
                        let _ = reply.sender.try_send(local_dispatch::Response {
                            success: !matches!(state, DispatchState::Error(_)),
                            output: String::from_utf8(std::mem::take(output))
                                .expect("formatted report is UTF-8"),
                        });
                    }
                    surface.report(runtime, Report::ManualDispatch(state));
                }
            }
        }

        surface.refresh(runtime, now, handled_input);
    };

    surface.finish();
    exit_code
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Instant;

    use super::run_runtime;
    use crate::application::backends::RuntimeBackend;
    use crate::application::session::Runtime;
    use crate::application::surfaces::{Report, Surface};
    use crate::application::test_support::temp_path;

    #[test]
    fn interactive_runtime_exits_on_input_end() {
        struct InputEndSurface {
            closed: bool,
            finished: bool,
        }
        impl Surface for InputEndSurface {
            fn start(&mut self, _: &Runtime) {}
            fn report(&mut self, _: &Runtime, report: Report) {
                self.closed |= matches!(report, Report::InputClosed);
            }
            fn refresh(&mut self, _: &Runtime, _: Instant, _: bool) {
                panic!("interactive EOF must exit before polling");
            }
            fn finish(&mut self) {
                self.finished = true;
            }
        }
        let mut runtime = Runtime::new(
            Some(temp_path("eof_missing_config.toml")),
            RuntimeBackend::DryRun,
            None,
        )
        .unwrap();
        let (tx, instructions) = mpsc::channel();
        drop(tx);
        let mut surface = InputEndSurface {
            closed: false,
            finished: false,
        };
        run_runtime(&mut runtime, &mut surface, instructions, true);
        assert!(surface.closed);
        assert!(surface.finished);
    }
}
