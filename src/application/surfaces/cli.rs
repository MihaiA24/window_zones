use std::io::{self, Write};
use std::time::Instant;

use window_zones::DispatchState;

use super::{DispatchReport, Report, Surface, print_dispatch_state, print_status};
use crate::application::cli::print_help;
use crate::application::notifications::Notifier;
use crate::application::session::Runtime;

/// Line-oriented session for terminals and background services.
pub(in crate::application) struct CliSurface {
    pub(in crate::application) prompt: bool,
    pub(in crate::application) notifier: Option<Notifier>,
}

impl Surface for CliSurface {
    fn start(&mut self, runtime: &Runtime) {
        println!("Window backend: {}", runtime.window_system.name());
        println!("Interactive session started. type `help` for commands.");
        if self.prompt {
            print_prompt();
        }
    }

    fn report(&mut self, runtime: &Runtime, report: Report) {
        match report {
            Report::Event(event) => {
                println!("{event}");
                if let Some(notifier) = &mut self.notifier {
                    notifier.event(&event, Instant::now());
                }
            }
            Report::HotkeyDispatch(DispatchReport {
                hotkey,
                state: DispatchState::Error(error),
            }) => println!("Dispatch failed for {hotkey}: {error}"),
            Report::HotkeyDispatch(_) => {}
            Report::ManualDispatch(state) => print_dispatch_state(&state, &runtime.window_system),
            Report::Status => print_status(&runtime.app),
            Report::Help => print_help(),
            Report::Unknown(message) => {
                println!("Unknown command: {message}");
                println!("type `help` for usage.");
            }
            Report::ReloadRequested => println!("Reload requested."),
            Report::Restarted => println!("Runtime restarted."),
            Report::InputClosed => println!("Input stream closed."),
        }
        let _ = io::stdout().flush();
    }

    fn refresh(&mut self, _runtime: &Runtime, now: Instant, input: bool) {
        if let Some(notifier) = &mut self.notifier {
            notifier.tick(now);
        }
        if input && self.prompt {
            print_prompt();
        }
    }

    fn finish(&mut self) {
        println!("Session closed.");
    }
}

fn print_prompt() {
    print!("window-zones> ");
    let _ = io::stdout().flush();
}
