mod instructions;
#[cfg(any(target_os = "linux", all(test, unix)))]
mod local_dispatch;
mod runner;
mod startup;

pub(super) use instructions::{RuntimeInstruction, RuntimeRequest};
#[cfg(target_os = "linux")]
pub(super) use startup::execute_compositor_dispatch;
pub(super) use startup::execute_session;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use window_zones::{App, DispatchState, RuntimeEvent};

use super::backends::{RuntimeBackend, RuntimeHotkeySystem, RuntimeWindowSystem};
use super::surfaces::DispatchReport;

/// How often the config file and hotkey registration are reconciled.
const RUNTIME_TICK_INTERVAL: Duration = Duration::from_millis(250);
/// Upper bound on presses handled in one poll, so input stays responsive.
const MAX_HOTKEYS_PER_POLL: usize = 32;

pub(super) fn build_app(
    config_path: Option<&PathBuf>,
    perf_log_path: Option<&PathBuf>,
) -> Result<App, String> {
    let mut app = match config_path {
        Some(path) => App::start_at(path),
        None => App::start(),
    };
    if let Some(path) = perf_log_path {
        app.enable_perf_log(path).map_err(|error| {
            format!("Cannot open performance log at {}: {error}", path.display())
        })?;
    }
    Ok(app)
}

/// The running App with its adapters: one engine shared by the CLI, TUI, and
/// tray surfaces.
pub(super) struct Runtime {
    pub(super) app: App,
    pub(super) backend: RuntimeBackend,
    pub(super) window_system: RuntimeWindowSystem,
    hotkey_system: RuntimeHotkeySystem,
    next_tick: Instant,
}

impl Runtime {
    pub(super) fn new(
        config_path: Option<PathBuf>,
        backend: RuntimeBackend,
        perf_log_path: Option<&PathBuf>,
    ) -> Result<Self, String> {
        Ok(Self {
            app: build_app(config_path.as_ref(), perf_log_path)?,
            backend,
            window_system: RuntimeWindowSystem::with_backend(backend),
            hotkey_system: RuntimeHotkeySystem::new(backend),
            next_tick: Instant::now(),
        })
    }

    /// Reconciles config and registration when due, then dispatches every
    /// queued hotkey press.
    fn poll(&mut self, now: Instant) -> (Vec<RuntimeEvent>, Vec<DispatchReport>) {
        let mut events = Vec::new();
        if now >= self.next_tick {
            events = self.app.tick(&mut self.hotkey_system, now);
            self.next_tick = now + RUNTIME_TICK_INTERVAL;
        }

        let mut dispatches = Vec::new();
        if !self.app.hotkeys_registered() {
            return (events, dispatches);
        }
        for _ in 0..MAX_HOTKEYS_PER_POLL {
            match self
                .app
                .dispatch_next_hotkey(&mut self.hotkey_system, &mut self.window_system)
            {
                Ok(Some(state)) => {
                    let state = state.clone();
                    dispatches.push(DispatchReport {
                        hotkey: self
                            .app
                            .last_dispatch_hotkey()
                            .unwrap_or_default()
                            .to_string(),
                        state,
                    });
                }
                Ok(None) => break,
                Err(error) => {
                    events.extend(self.app.hotkeys_lost(error, now));
                    self.next_tick = now;
                    break;
                }
            }
        }
        (events, dispatches)
    }

    /// Rebuilds the App and adapters from scratch, releasing the old grabs first.
    fn restart(&mut self, now: Instant) -> Vec<RuntimeEvent> {
        self.hotkey_system = RuntimeHotkeySystem::Noop;
        self.app.restart();
        self.window_system = RuntimeWindowSystem::with_backend(self.backend);
        self.hotkey_system = RuntimeHotkeySystem::new(self.backend);
        self.next_tick = now + RUNTIME_TICK_INTERVAL;
        let mut events = vec![self.app.startup_event()];
        events.extend(self.app.tick(&mut self.hotkey_system, now));
        events
    }

    fn dispatch(&mut self, hotkey: &str) -> DispatchState {
        self.app
            .dispatch_hotkey(hotkey, &mut self.window_system)
            .clone()
    }
}

#[cfg(test)]
mod runtime_tests;
