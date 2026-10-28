#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::io::{self, IsTerminal};
#[cfg(target_os = "macos")]
use std::thread;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::time::Duration;
use std::time::Instant;

use window_zones::RuntimeEvent;

use super::backends::RuntimeBackend;

/// How long hotkeys may stay unavailable before a notification is shown. Short
/// outages fix themselves: at login the App can start before GNOME Shell has
/// loaded the companion, and a Shell or companion restart reconnects within seconds.
#[cfg(any(target_os = "linux", target_os = "macos"))]
const HOTKEY_OUTAGE_GRACE: Duration = Duration::from_secs(20);

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProblemKind {
    Config,
    Hotkeys,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Debug, PartialEq, Eq)]
enum Notification {
    Problem(String),
    Resolved(String),
}

/// Decides which runtime events become desktop notifications. Config problems
/// and refused hotkeys are shown at once; unavailable hotkeys only once the
/// outage outlasts `HOTKEY_OUTAGE_GRACE`. A resolution is shown only for the
/// problem currently on screen.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Debug, Default)]
struct NotificationPolicy {
    pending_outage: Option<(Instant, String)>,
    shown: Option<ProblemKind>,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl NotificationPolicy {
    fn event(&mut self, event: &RuntimeEvent, now: Instant) -> Option<Notification> {
        let resolved = match event {
            RuntimeEvent::HotkeysUnavailable { .. } => {
                let since = self
                    .pending_outage
                    .as_ref()
                    .map_or(now, |(since, _)| *since);
                self.pending_outage = Some((since, event.to_string()));
                return None;
            }
            RuntimeEvent::ConfigFailed { .. } | RuntimeEvent::ConfigRejected { .. } => {
                return Some(self.problem(ProblemKind::Config, event));
            }
            RuntimeEvent::HotkeysRejected { .. } => {
                self.pending_outage = None;
                return Some(self.problem(ProblemKind::Hotkeys, event));
            }
            RuntimeEvent::ConfigLoaded { .. } => ProblemKind::Config,
            RuntimeEvent::HotkeysRegistered { .. } | RuntimeEvent::HotkeysRecovered { .. } => {
                self.pending_outage = None;
                ProblemKind::Hotkeys
            }
            RuntimeEvent::ConfigMissing { .. } => return None,
        };
        if self.shown == Some(resolved) {
            self.shown = None;
            return Some(Notification::Resolved(event.to_string()));
        }
        None
    }

    /// Shows an outage that has lasted past the grace period.
    fn tick(&mut self, now: Instant) -> Option<Notification> {
        let (since, _) = self.pending_outage.as_ref()?;
        if now.duration_since(*since) < HOTKEY_OUTAGE_GRACE {
            return None;
        }
        let (_, message) = self.pending_outage.take()?;
        self.shown = Some(ProblemKind::Hotkeys);
        Some(Notification::Problem(message))
    }

    fn problem(&mut self, kind: ProblemKind, event: &RuntimeEvent) -> Notification {
        self.shown = Some(kind);
        Notification::Problem(event.to_string())
    }
}

/// Desktop notifications for problems while no terminal is attached.
/// Linux updates one notification in place; macOS uses Notification Center.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) struct Notifier {
    policy: NotificationPolicy,
    #[cfg(target_os = "linux")]
    connection: Option<dbus::blocking::Connection>,
    #[cfg(target_os = "linux")]
    notification_id: u32,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl Notifier {
    pub(super) fn new(backend: RuntimeBackend) -> Option<Self> {
        if io::stdout().is_terminal() || matches!(backend, RuntimeBackend::DryRun) {
            return None;
        }
        Some(Self {
            policy: NotificationPolicy::default(),
            #[cfg(target_os = "linux")]
            connection: None,
            #[cfg(target_os = "linux")]
            notification_id: 0,
        })
    }

    pub(super) fn event(&mut self, event: &RuntimeEvent, now: Instant) {
        if let Some(notification) = self.policy.event(event, now) {
            self.show(notification);
        }
    }

    pub(super) fn tick(&mut self, now: Instant) {
        if let Some(notification) = self.policy.tick(now) {
            self.show(notification);
        }
    }

    #[cfg(target_os = "linux")]
    fn show(&mut self, notification: Notification) {
        let (summary, body) = match &notification {
            Notification::Problem(body) => ("Window Zones needs attention", body),
            Notification::Resolved(body) => ("Window Zones", body),
        };
        if self.connection.is_none() {
            self.connection = dbus::blocking::Connection::new_session().ok();
        }
        let Some(connection) = &self.connection else {
            return;
        };
        let proxy = connection.with_proxy(
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            Duration::from_millis(500),
        );
        let hints = dbus::arg::PropMap::new();
        let result: Result<(u32,), dbus::Error> = proxy.method_call(
            "org.freedesktop.Notifications",
            "Notify",
            (
                "Window Zones",
                self.notification_id,
                "preferences-system-windows",
                summary,
                body.as_str(),
                Vec::<String>::new(),
                hints,
                -1_i32,
            ),
        );
        match result {
            Ok((id,)) => self.notification_id = id,
            Err(error) => {
                window_zones::logging::diagnostic(format_args!(
                    "Desktop notification failed: {error}"
                ));
                self.connection = None;
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn show(&mut self, notification: Notification) {
        let (summary, body) = match notification {
            Notification::Problem(body) => ("Window Zones needs attention", body),
            Notification::Resolved(body) => ("Window Zones", body),
        };
        // argv transports user/config text without interpreting it as a script.
        // Delivery runs off the hotkey thread and its child is always reaped.
        thread::spawn(move || {
            let result = std::process::Command::new("osascript")
                .args([
                    "-l", "JavaScript", "-e",
                    "function run(argv) { const app = Application.currentApplication(); app.includeStandardAdditions = true; app.displayNotification(argv[1], {withTitle: argv[0]}); }",
                    "--", summary, &body,
                ])
                .status();
            match result {
                Ok(status) if status.success() => {}
                Ok(status) => window_zones::logging::diagnostic(format_args!(
                    "Desktop notification failed: {status}"
                )),
                Err(error) => window_zones::logging::diagnostic(format_args!(
                    "Desktop notification failed: {error}"
                )),
            }
        });
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) struct Notifier;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
impl Notifier {
    pub(super) fn new(_backend: RuntimeBackend) -> Option<Self> {
        None
    }

    pub(super) fn event(&mut self, _event: &RuntimeEvent, _now: Instant) {}

    pub(super) fn tick(&mut self, _now: Instant) {}
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use std::time::{Duration, Instant};

    use window_zones::RuntimeEvent;

    use super::{HOTKEY_OUTAGE_GRACE, Notification, NotificationPolicy};

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn short_hotkey_outages_never_notify_and_long_ones_notify_once() {
        let unavailable = RuntimeEvent::HotkeysUnavailable {
            message: "companion absent".to_string(),
        };
        let recovered = RuntimeEvent::HotkeysRecovered { count: 18 };
        let start = Instant::now();

        // Login race: the companion appears a few seconds after the App.
        let mut login = NotificationPolicy::default();
        let login_notifications = [
            login.event(&RuntimeEvent::ConfigLoaded { bindings: 18 }, start),
            login.event(&unavailable, start),
            login.tick(start + Duration::from_secs(3)),
            login.event(&recovered, start + Duration::from_secs(4)),
            login.tick(start + HOTKEY_OUTAGE_GRACE * 2),
        ];

        // The companion stays away: one notification, then its resolution.
        let mut outage = NotificationPolicy::default();
        outage.event(&unavailable, start);
        let before_grace = outage.tick(start + HOTKEY_OUTAGE_GRACE - Duration::from_secs(1));
        let after_grace = outage.tick(start + HOTKEY_OUTAGE_GRACE);
        let again = outage.tick(start + HOTKEY_OUTAGE_GRACE * 3);
        let resolution = outage.event(&recovered, start + HOTKEY_OUTAGE_GRACE * 4);

        assert_eq!(login_notifications, [None, None, None, None, None]);
        assert_eq!(before_grace, None);
        assert_eq!(
            after_grace,
            Some(Notification::Problem(unavailable.to_string()))
        );
        assert_eq!(again, None);
        assert_eq!(
            resolution,
            Some(Notification::Resolved(recovered.to_string()))
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn changing_outage_diagnostics_do_not_restart_the_grace_period() {
        let mut policy = NotificationPolicy::default();
        let start = Instant::now();
        policy.event(
            &RuntimeEvent::HotkeysUnavailable {
                message: "absent".into(),
            },
            start,
        );
        let changed = RuntimeEvent::HotkeysUnavailable {
            message: "reconnecting".into(),
        };
        policy.event(
            &changed,
            start + HOTKEY_OUTAGE_GRACE - Duration::from_secs(1),
        );

        assert_eq!(
            policy.tick(start + HOTKEY_OUTAGE_GRACE),
            Some(Notification::Problem(changed.to_string()))
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn config_problems_notify_at_once_and_only_their_own_fix_resolves_them() {
        let start = Instant::now();
        let failed = RuntimeEvent::ConfigFailed {
            message: "bad toml".to_string(),
        };
        let refused = RuntimeEvent::HotkeysRejected {
            message: "ctrl+super+c is taken".to_string(),
        };
        let loaded = RuntimeEvent::ConfigLoaded { bindings: 18 };
        let mut policy = NotificationPolicy::default();

        let config_problem = policy.event(&failed, start);
        let config_fixed = policy.event(&loaded, start);
        let hotkey_problem = policy.event(&refused, start);
        // A config reload alone does not claim the refused hotkeys are fixed.
        let unrelated_reload = policy.event(&loaded, start);
        let hotkeys_fixed = policy.event(&RuntimeEvent::HotkeysRegistered { count: 18 }, start);

        assert_eq!(
            config_problem,
            Some(Notification::Problem(failed.to_string()))
        );
        assert_eq!(
            config_fixed,
            Some(Notification::Resolved(loaded.to_string()))
        );
        assert_eq!(
            hotkey_problem,
            Some(Notification::Problem(refused.to_string()))
        );
        assert_eq!(unrelated_reload, None);
        assert!(matches!(hotkeys_fixed, Some(Notification::Resolved(_))));
    }
}
