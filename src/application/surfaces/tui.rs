use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::time::{Duration, Instant};

use window_zones::{App, ConfigState, DispatchState, HotkeyRegistrationState};

use super::{Report, Surface};
use crate::application::backends::{RuntimeBackend, RuntimeWindowSystem};
use crate::application::session::Runtime;

/// How often the TUI re-reads companion capabilities.
const CAPABILITY_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
struct TuiCapabilityStatus {
    window: String,
    hotkey: String,
    diagnostic: Option<String>,
}

fn tui_capability_status(
    backend: RuntimeBackend,
    window_system: &RuntimeWindowSystem,
) -> TuiCapabilityStatus {
    #[cfg(target_os = "linux")]
    if matches!(backend, RuntimeBackend::Wayland(_)) {
        return TuiCapabilityStatus {
            window: "focused-window, displays, move-resize".to_string(),
            hotkey: "<none>".to_string(),
            diagnostic: None,
        };
    }
    let _ = backend;
    tui_capability_status_from(
        window_system
            .companion_capabilities()
            .map(|(_, capabilities)| capabilities),
    )
}

fn tui_capability_status_from(
    companion_capabilities: Option<Result<Vec<String>, String>>,
) -> TuiCapabilityStatus {
    let mut status = TuiCapabilityStatus {
        window: "focused-window, displays, move-resize".to_string(),
        hotkey: "hotkeys".to_string(),
        diagnostic: None,
    };

    let Some(capabilities) = companion_capabilities else {
        return status;
    };

    match capabilities {
        Ok(capabilities) => {
            let window_capabilities = capabilities
                .iter()
                .filter(|capability| capability.as_str() != "hotkeys")
                .cloned()
                .collect::<Vec<_>>();
            status.window = if window_capabilities.is_empty() {
                "<none>".to_string()
            } else {
                window_capabilities.join(", ")
            };
            status.hotkey = if capabilities
                .iter()
                .any(|capability| capability == "hotkeys")
            {
                "hotkeys".to_string()
            } else {
                "<none>".to_string()
            };
        }
        Err(error) => {
            status.window = "unavailable".to_string();
            status.hotkey = "unavailable".to_string();
            status.diagnostic = Some(error);
        }
    }

    status
}

fn tui_hotkey_mode(backend: RuntimeBackend) -> &'static str {
    match backend {
        RuntimeBackend::DryRun => "dry-run",
        #[cfg(target_os = "linux")]
        RuntimeBackend::Gnome => "gnome-companion",
        #[cfg(target_os = "linux")]
        RuntimeBackend::Kde => "kwin-companion",
        #[cfg(target_os = "linux")]
        RuntimeBackend::Wayland(_) => "compositor-bound dispatch",
        #[allow(unreachable_patterns)]
        _ => "global",
    }
}

fn tui_last_error(app: &App, capabilities: &TuiCapabilityStatus) -> String {
    if let DispatchState::Error(error) = app.dispatch_state() {
        return format!("dispatch: {error}");
    }
    if let HotkeyRegistrationState::Error(error) = app.hotkey_state() {
        return format!("hotkey registration: {error}");
    }
    if let ConfigState::Error(error) = app.config_state() {
        return format!("config: {error}");
    }
    if let Some(error) = app.perf_log_error() {
        return error.to_string();
    }

    capabilities
        .diagnostic
        .as_deref()
        .unwrap_or("none")
        .to_string()
}

fn tui_frame(
    runtime: &Runtime,
    capabilities: &TuiCapabilityStatus,
    last_event: Option<&str>,
) -> String {
    let app = &runtime.app;
    let mut frame = format!(
        "\x1b[2J\x1b[HWindow Zones TUI\n================\n\
         Window backend: {}\nWindow capabilities: {}\nHotkey mode: {}\n\
         Hotkey capabilities: {}\nHotkey state: {:?}\nConfig: {} ({:?})\nBindings:\n",
        runtime.window_system.name(),
        capabilities.window,
        tui_hotkey_mode(runtime.backend),
        capabilities.hotkey,
        app.hotkey_state(),
        app.config_path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<unresolved>".to_string()),
        app.config_state(),
    );
    if app.config().bindings.is_empty() {
        frame.push_str("  <none>\n");
    } else {
        for binding in &app.config().bindings {
            writeln!(frame, "  {} -> {:?}", binding.hotkey, binding.action)
                .expect("write TUI frame");
        }
    }
    write!(
        frame,
        "Last action: {}\nLast error: {}\nLast event: {}\n\n\
         Commands: reload | restart | status/refresh | dispatch HOTKEY | quit\nwindow-zones tui> ",
        app.last_dispatch_hotkey().unwrap_or("<none>"),
        tui_last_error(app, capabilities),
        last_event.unwrap_or("none"),
    )
    .expect("write TUI frame");
    frame
}

/// Full-screen ANSI dashboard redrawn whenever its content changes.
pub(in crate::application) struct TuiSurface {
    snapshot: String,
    capabilities: TuiCapabilityStatus,
    capabilities_due: Instant,
    last_event: Option<String>,
    dirty: bool,
}

impl TuiSurface {
    pub(in crate::application) fn new() -> Self {
        Self {
            snapshot: String::new(),
            capabilities: tui_capability_status_from(None),
            capabilities_due: Instant::now(),
            last_event: None,
            dirty: true,
        }
    }

    fn draw(&mut self, runtime: &Runtime, force: bool) {
        let frame = tui_frame(runtime, &self.capabilities, self.last_event.as_deref());
        if force || frame != self.snapshot {
            print!("{frame}");
            let _ = io::stdout().flush();
            self.snapshot = frame;
        }
        self.dirty = false;
    }
}

impl Surface for TuiSurface {
    fn start(&mut self, runtime: &Runtime) {
        self.capabilities = tui_capability_status(runtime.backend, &runtime.window_system);
        self.capabilities_due = Instant::now() + CAPABILITY_REFRESH_INTERVAL;
        self.draw(runtime, true);
    }

    fn report(&mut self, _runtime: &Runtime, report: Report) {
        self.dirty = true;
        match report {
            Report::Event(event) => self.last_event = Some(event.to_string()),
            Report::Unknown(message) => {
                self.last_event = Some(format!("unknown command: {message}"));
            }
            Report::Status | Report::Restarted => self.capabilities_due = Instant::now(),
            _ => {}
        }
    }

    fn refresh(&mut self, runtime: &Runtime, now: Instant, input: bool) {
        if now >= self.capabilities_due {
            let capabilities = tui_capability_status(runtime.backend, &runtime.window_system);
            self.dirty |= self.capabilities != capabilities;
            self.capabilities = capabilities;
            self.capabilities_due = now + CAPABILITY_REFRESH_INTERVAL;
        }
        if self.dirty || input {
            self.draw(runtime, input);
        }
    }

    fn finish(&mut self) {
        println!("\nSession closed.");
    }
}

#[cfg(test)]
mod tests;
