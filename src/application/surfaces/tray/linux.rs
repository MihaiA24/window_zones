use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use ksni::menu::StandardItem;
use ksni::{Handle, MenuItem, Tray, TrayService};

use crate::application::session::{RuntimeInstruction, RuntimeRequest};

pub(super) struct LinuxTray {
    handle: Handle<Menu>,
    worker: Option<JoinHandle<()>>,
}

impl LinuxTray {
    pub(super) fn new(
        labels: Vec<String>,
        tx: &mpsc::Sender<RuntimeRequest>,
    ) -> Result<Self, String> {
        let (ready_tx, ready_rx) = mpsc::channel();
        let service = TrayService::new(Menu {
            labels,
            tx: tx.clone(),
            ready: ready_tx.clone(),
            registered: Cell::new(false),
        });
        let handle = service.handle();
        // Request an update so readiness is acknowledged from the running worker,
        // after registration and its watcher-loss subscription are installed.
        handle.update(|_| {});
        let tx = tx.clone();
        let worker = thread::Builder::new()
            .name("linux-tray".into())
            .spawn(move || {
                // Unlike spawn(), run() returns bus/registration failures instead of panicking.
                let error = match catch_unwind(AssertUnwindSafe(|| service.run())) {
                    Ok(Ok(())) => return,
                    Ok(Err(error)) => format!(
                        "Linux tray service failed: {error}; check DBUS_SESSION_BUS_ADDRESS and the StatusNotifier/AppIndicator tray host"
                    ),
                    Err(_) => "Linux tray worker panicked; restart --tray after checking the tray host".to_string(),
                };
                let _ = ready_tx.send(Err(error.clone()));
                let _ = tx.send(RuntimeInstruction::SurfaceFailed(error).into());
            })
            .map_err(|error| format!("cannot start Linux tray worker: {error}"))?;
        let tray = Self {
            handle,
            worker: Some(worker),
        };
        ready_rx
            .recv()
            .map_err(|error| format!("Linux tray worker stopped before registering: {error}"))??;
        Ok(tray)
    }

    pub(super) fn update_labels(&mut self, labels: &[String]) {
        self.handle
            .update(|menu| menu.labels.clone_from_slice(labels));
    }
}

impl Drop for LinuxTray {
    fn drop(&mut self) {
        self.handle.shutdown();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Menu {
    labels: Vec<String>,
    tx: mpsc::Sender<RuntimeRequest>,
    ready: mpsc::Sender<Result<(), String>>,
    registered: Cell<bool>,
}

impl Tray for Menu {
    fn id(&self) -> String {
        "window-zones".into()
    }

    fn title(&self) -> String {
        "Window Zones".into()
    }

    fn icon_name(&self) -> String {
        "view-grid-symbolic".into()
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        if self.registered.replace(false) {
            let _ = self.ready.send(Ok(()));
        }
        let mut items = Vec::with_capacity(self.labels.len() + 4);
        for label in &self.labels {
            items.push(
                StandardItem {
                    label: label.clone(),
                    activate: Box::new(|menu: &mut Self| {
                        let _ = menu.tx.send(RuntimeInstruction::Status.into());
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }
        for (label, instruction) in [
            ("Show status", RuntimeInstruction::Status),
            ("Reload", RuntimeInstruction::Reload),
            ("Restart", RuntimeInstruction::Restart),
            ("Quit", RuntimeInstruction::Quit),
        ] {
            items.push(
                StandardItem {
                    label: label.into(),
                    activate: Box::new(move |menu: &mut Self| {
                        let _ = menu.tx.send(instruction.clone().into());
                    }),
                    ..Default::default()
                }
                .into(),
            );
        }
        items
    }

    fn watcher_online(&self) {
        self.registered.set(true);
    }

    fn watcher_offine(&self) -> bool {
        let error = "StatusNotifierWatcher is unavailable; enable a StatusNotifier/AppIndicator tray host and restart --tray".to_string();
        let _ = self.ready.send(Err(error.clone()));
        let _ = self
            .tx
            .send(RuntimeInstruction::SurfaceFailed(error).into());
        false
    }
}
