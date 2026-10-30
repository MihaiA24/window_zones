mod cli;
mod status;
#[cfg(any(target_os = "linux", target_os = "windows"))]
mod tray;
mod tui;

pub(super) use cli::CliSurface;
pub(super) use status::{execute_dispatch, execute_status, print_dispatch_state, print_status};
#[cfg(any(target_os = "linux", all(test, unix)))]
pub(super) use status::{runtime_status_lines, write_dispatch_state};
#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(super) use tray::TraySurface;
pub(super) use tui::TuiSurface;

use std::time::Instant;

use window_zones::{DispatchState, RuntimeEvent};

use super::session::Runtime;

/// A hotkey press and what dispatching it did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DispatchReport {
    pub(super) hotkey: String,
    pub(super) state: DispatchState,
}

/// What a runtime surface is told to show.
pub(super) enum Report {
    Event(RuntimeEvent),
    HotkeyDispatch(DispatchReport),
    ManualDispatch(DispatchState),
    Status,
    Help,
    Unknown(String),
    ReloadRequested,
    Restarted,
    InputClosed,
}

pub(super) trait Surface {
    fn start(&mut self, runtime: &Runtime);
    fn report(&mut self, runtime: &Runtime, report: Report);
    /// Called once per loop iteration after all reports; `input` is whether an
    /// instruction was handled in this iteration.
    fn refresh(&mut self, runtime: &Runtime, now: Instant, input: bool);
    fn finish(&mut self);
}
