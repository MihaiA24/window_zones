use std::sync::mpsc;
use std::time::Instant;

use tray_item::{IconSource, TrayItem};
use window_zones::{ConfigState, DispatchState, HotkeyRegistrationState};

use super::{CliSurface, Report, Surface};
use crate::application::notifications::Notifier;
use crate::application::session::{Runtime, RuntimeInstruction, RuntimeRequest};

/// Fixed tray status lines, updated in place.
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn tray_status_lines(runtime: &Runtime) -> [String; 4] {
    let app = &runtime.app;
    let config = match app.config_state() {
        ConfigState::Loaded => format!("Config: {} bindings", app.config().bindings.len()),
        ConfigState::Missing => "Config: missing (run `window_zones init`)".to_string(),
        ConfigState::Error(error) => format!("Config error: {error}"),
    };
    let hotkeys = match app.hotkey_state() {
        HotkeyRegistrationState::Registered => "Hotkeys: registered".to_string(),
        HotkeyRegistrationState::Unregistered => "Hotkeys: not registered".to_string(),
        HotkeyRegistrationState::Error(error) => format!("Hotkeys: {error}"),
    };
    let last = match (app.last_dispatch_hotkey(), app.dispatch_state()) {
        (Some(hotkey), DispatchState::Error(error)) => format!("Last: {hotkey} failed: {error}"),
        (Some(hotkey), _) => format!("Last: {hotkey}"),
        (None, _) => "Last: <none>".to_string(),
    };
    [
        format!("Backend: {}", runtime.window_system.name()),
        config,
        hotkeys,
        last,
    ]
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
pub(in crate::application) struct TraySurface {
    tray: TrayItem,
    label_ids: Vec<u32>,
    labels: Vec<String>,
    cli: CliSurface,
    dirty: bool,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl TraySurface {
    pub(in crate::application) fn new(
        runtime: &Runtime,
        tx: &mpsc::Sender<RuntimeRequest>,
    ) -> Result<Self, String> {
        #[cfg(target_os = "linux")]
        let icon = IconSource::Resource("view-grid-symbolic");
        #[cfg(target_os = "windows")]
        let icon = IconSource::Resource("");
        let mut tray = TrayItem::new("Window Zones", icon).map_err(|error| error.to_string())?;

        let labels = tray_status_lines(runtime).to_vec();
        let mut label_ids = Vec::with_capacity(labels.len());
        for line in &labels {
            #[cfg(target_os = "linux")]
            let id = {
                let tx = tx.clone();
                tray.inner_mut().add_menu_item_with_id(line, move || {
                    let _ = tx.send(RuntimeInstruction::Status.into());
                })
            };
            #[cfg(target_os = "windows")]
            let id = tray.inner_mut().add_label_with_id(line);
            label_ids.push(id.map_err(|error| error.to_string())?);
        }

        for (label, instruction) in [
            ("Show status", RuntimeInstruction::Status),
            ("Reload", RuntimeInstruction::Reload),
            ("Restart", RuntimeInstruction::Restart),
            ("Quit", RuntimeInstruction::Quit),
        ] {
            let tx = tx.clone();
            tray.add_menu_item(label, move || {
                let _ = tx.send(instruction.clone().into());
            })
            .map_err(|error| error.to_string())?;
        }

        Ok(Self {
            tray,
            label_ids,
            labels,
            dirty: false,
            cli: CliSurface {
                prompt: false,
                notifier: Notifier::new(runtime.backend),
            },
        })
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl Surface for TraySurface {
    fn start(&mut self, runtime: &Runtime) {
        println!("Window backend: {}", runtime.window_system.name());
        println!("Tray menu started. Use tray controls to reload/restart/quit.");
    }

    fn report(&mut self, runtime: &Runtime, report: Report) {
        self.dirty = true;
        self.cli.report(runtime, report);
    }

    fn refresh(&mut self, runtime: &Runtime, now: Instant, _input: bool) {
        self.cli.refresh(runtime, now, false);
        if !self.dirty {
            return;
        }
        self.dirty = false;
        for (index, line) in tray_status_lines(runtime).into_iter().enumerate() {
            if self.labels[index] == line {
                continue;
            }
            #[cfg(target_os = "linux")]
            let updated = self
                .tray
                .inner_mut()
                .set_menu_item_label(&line, self.label_ids[index]);
            #[cfg(target_os = "windows")]
            let updated = self
                .tray
                .inner_mut()
                .set_label(&line, self.label_ids[index]);
            match updated {
                Ok(()) => self.labels[index] = line,
                Err(error) => {
                    self.dirty = true;
                    window_zones::logging::diagnostic(format_args!(
                        "Failed to refresh tray status: {error}"
                    ));
                }
            }
        }
    }

    fn finish(&mut self) {
        self.cli.finish();
    }
}
