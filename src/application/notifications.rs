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
const DELIVERY_RETRY: Duration = Duration::from_secs(1);

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProblemKind {
    Config,
    Hotkeys,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(test, derive(Clone))]
enum Notification {
    Problem(String),
    Resolved(String),
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl Notification {
    fn text(&self) -> (&'static str, &str) {
        match self {
            Self::Problem(body) => ("Window Zones needs attention", body),
            Self::Resolved(body) => ("Window Zones", body),
        }
    }
}

/// Decides which runtime events become desktop notifications. Config problems
/// and refused hotkeys are shown at once; unavailable hotkeys only once the
/// outage outlasts `HOTKEY_OUTAGE_GRACE`. A resolution is shown only for the
/// problem currently on screen.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Debug, Default)]
struct NotificationPolicy {
    pending_outage: Option<(Instant, String)>,
    pending_config: Option<(Instant, Notification)>,
    pending_hotkeys: Option<(Instant, Notification)>,
    in_flight: Option<ProblemKind>,
    shown: Option<ProblemKind>,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl NotificationPolicy {
    fn event(&mut self, event: &RuntimeEvent, now: Instant) {
        let resolved = match event {
            RuntimeEvent::HotkeysUnavailable { .. } => {
                let since = self
                    .pending_outage
                    .as_ref()
                    .map_or(now, |(since, _)| *since);
                self.pending_outage = Some((since, event.to_string()));
                return;
            }
            RuntimeEvent::ConfigFailed { .. }
            | RuntimeEvent::ConfigPending { .. }
            | RuntimeEvent::ConfigRejected { .. } => {
                self.pending_config = Some((now, Notification::Problem(event.to_string())));
                return;
            }
            RuntimeEvent::HotkeysRejected { .. } => {
                self.pending_outage = None;
                self.pending_hotkeys = Some((now, Notification::Problem(event.to_string())));
                return;
            }
            RuntimeEvent::ConfigLoaded { .. } => ProblemKind::Config,
            RuntimeEvent::HotkeysRegistered { .. } | RuntimeEvent::HotkeysRecovered { .. } => {
                self.pending_outage = None;
                ProblemKind::Hotkeys
            }
            RuntimeEvent::ConfigMissing { .. } => {
                self.pending_config = None;
                if self.in_flight == Some(ProblemKind::Config) {
                    self.in_flight = None;
                }
                return;
            }
        };
        let relevant = self.shown == Some(resolved) || self.in_flight == Some(resolved);
        *self.pending(resolved) =
            relevant.then(|| (now, Notification::Resolved(event.to_string())));
    }

    fn pending(&mut self, kind: ProblemKind) -> &mut Option<(Instant, Notification)> {
        match kind {
            ProblemKind::Config => &mut self.pending_config,
            ProblemKind::Hotkeys => &mut self.pending_hotkeys,
        }
    }

    fn next(&mut self, now: Instant) -> Option<(ProblemKind, Notification)> {
        let shown = self.shown;
        for kind in [ProblemKind::Config, ProblemKind::Hotkeys] {
            let pending = self.pending(kind);
            if pending.as_ref().is_some_and(|(at, _)| now >= *at)
                && let Some((_, notification)) = pending.take()
                && (matches!(notification, Notification::Problem(_)) || shown == Some(kind))
            {
                self.in_flight = Some(kind);
                return Some((kind, notification));
            }
        }
        let (since, _) = self.pending_outage.as_ref()?;
        if self.pending_hotkeys.is_some() || now.duration_since(*since) < HOTKEY_OUTAGE_GRACE {
            return None;
        }
        let (_, message) = self.pending_outage.take()?;
        self.in_flight = Some(ProblemKind::Hotkeys);
        Some((ProblemKind::Hotkeys, Notification::Problem(message)))
    }

    fn complete(&mut self, kind: ProblemKind, notice: Notification, now: Instant, delivered: bool) {
        let in_flight = self.in_flight.take();
        if delivered {
            self.shown = match notice {
                Notification::Problem(_) => Some(kind),
                Notification::Resolved(_) => None,
            };
        } else if in_flight == Some(kind) {
            // An event received during delivery may have replaced or resolved this problem.
            self.pending(kind)
                .get_or_insert((now + DELIVERY_RETRY, notice));
        }
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
    #[cfg(target_os = "macos")]
    delivery: Option<thread::JoinHandle<(ProblemKind, Notification, bool)>>,
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
            #[cfg(target_os = "macos")]
            delivery: None,
        })
    }

    pub(super) fn event(&mut self, event: &RuntimeEvent, now: Instant) {
        self.policy.event(event, now);
        self.tick(now);
    }

    pub(super) fn tick(&mut self, now: Instant) {
        #[cfg(target_os = "macos")]
        if let Some(delivery) = self.delivery.as_ref() {
            if !delivery.is_finished() {
                return;
            }
            if let Ok((kind, notification, delivered)) = self.delivery.take().unwrap().join() {
                self.policy.complete(kind, notification, now, delivered);
            } else {
                self.policy.in_flight = None;
            }
        }
        if let Some((kind, notification)) = self.policy.next(now) {
            #[cfg(target_os = "linux")]
            {
                let delivered = self.show(&notification);
                self.policy.complete(kind, notification, now, delivered);
            }
            #[cfg(target_os = "macos")]
            {
                // Delivery runs off the hotkey thread and its child is always reaped.
                self.delivery = Some(thread::spawn(move || {
                    let delivered = Self::show(&notification);
                    (kind, notification, delivered)
                }));
            }
        }
    }

    #[cfg(target_os = "linux")]
    fn show(&mut self, notification: &Notification) -> bool {
        let (summary, body) = notification.text();
        if self.connection.is_none() {
            self.connection = dbus::blocking::Connection::new_session().ok();
        }
        let Some(connection) = &self.connection else {
            return false;
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
                body,
                Vec::<String>::new(),
                hints,
                -1_i32,
            ),
        );
        match result {
            Ok((id,)) => {
                self.notification_id = id;
                true
            }
            Err(error) => {
                window_zones::logging::diagnostic(format_args!(
                    "Desktop notification failed: {error}"
                ));
                self.connection = None;
                false
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn show(notification: &Notification) -> bool {
        let (summary, body) = notification.text();
        // argv transports user/config text without interpreting it as a script.
        let result = std::process::Command::new("osascript")
            .args([
                "-l", "JavaScript", "-e",
                "function run(argv) { const app = Application.currentApplication(); app.includeStandardAdditions = true; app.displayNotification(argv[1], {withTitle: argv[0]}); }",
                "--", summary, body,
            ])
            .status();
        match result {
            Ok(status) if status.success() => return true,
            Ok(status) => window_zones::logging::diagnostic(format_args!(
                "Desktop notification failed: {status}"
            )),
            Err(error) => window_zones::logging::diagnostic(format_args!(
                "Desktop notification failed: {error}"
            )),
        }
        false
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

    use super::{DELIVERY_RETRY, HOTKEY_OUTAGE_GRACE, Notification, NotificationPolicy};

    #[derive(Default)]
    struct TestNotifier(NotificationPolicy);

    impl TestNotifier {
        fn event(&mut self, event: &RuntimeEvent, now: Instant) -> Option<Notification> {
            self.0.event(event, now);
            self.tick(now)
        }

        fn tick(&mut self, now: Instant) -> Option<Notification> {
            self.deliver(now, |_| true)
        }

        fn deliver(
            &mut self,
            now: Instant,
            notify: impl FnOnce(&Notification) -> bool,
        ) -> Option<Notification> {
            let (kind, notification) = self.0.next(now)?;
            self.0
                .complete(kind, notification.clone(), now, notify(&notification));
            Some(notification)
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn short_hotkey_outages_never_notify_and_long_ones_notify_once() {
        let unavailable = RuntimeEvent::HotkeysUnavailable {
            message: "companion absent".to_string(),
        };
        let recovered = RuntimeEvent::HotkeysRecovered { count: 18 };
        let start = Instant::now();

        // Login race: the companion appears a few seconds after the App.
        let mut login = TestNotifier::default();
        let login_notifications = [
            login.event(&RuntimeEvent::ConfigLoaded { bindings: 18 }, start),
            login.event(&unavailable, start),
            login.tick(start + Duration::from_secs(3)),
            login.event(&recovered, start + Duration::from_secs(4)),
            login.tick(start + HOTKEY_OUTAGE_GRACE * 2),
        ];

        // The companion stays away: one notification, then its resolution.
        let mut outage = TestNotifier::default();
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
        let mut policy = TestNotifier::default();
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

    #[test]
    fn pending_config_notifies_and_only_acceptance_resolves_it() {
        let mut policy = TestNotifier::default();
        let start = Instant::now();
        policy.event(
            &RuntimeEvent::HotkeysUnavailable {
                message: "companion absent".into(),
            },
            start,
        );
        let pending = RuntimeEvent::ConfigPending {
            message: "hotkeys unavailable: companion absent".into(),
        };
        assert_eq!(
            policy.event(&pending, start + Duration::from_secs(1)),
            Some(Notification::Problem(pending.to_string()))
        );
        assert_eq!(
            policy.event(&RuntimeEvent::HotkeysRecovered { count: 1 }, start),
            None
        );
        let loaded = RuntimeEvent::ConfigLoaded { bindings: 1 };
        assert_eq!(
            policy.event(&loaded, start),
            Some(Notification::Resolved(loaded.to_string()))
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
        let mut policy = TestNotifier::default();

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

    #[test]
    fn undelivered_config_warning_retries_until_shown_or_fixed() {
        let start = Instant::now();
        let failed = RuntimeEvent::ConfigFailed {
            message: "bad toml".into(),
        };
        let problem = Some(Notification::Problem(failed.to_string()));
        let loaded = RuntimeEvent::ConfigLoaded { bindings: 18 };
        let mut notifier = TestNotifier::default();

        notifier.0.event(&failed, start);
        assert_eq!(notifier.deliver(start, |_| false), problem);
        assert_eq!(notifier.tick(start), None);
        assert_eq!(notifier.tick(start + DELIVERY_RETRY), problem);
        assert_eq!(notifier.tick(start + DELIVERY_RETRY * 2), None);
        assert_eq!(
            notifier.event(&loaded, start + DELIVERY_RETRY * 2),
            Some(Notification::Resolved(loaded.to_string()))
        );

        let later = start + DELIVERY_RETRY * 3;
        notifier.0.event(&failed, later);
        assert_eq!(notifier.deliver(later, |_| false), problem);
        assert_eq!(notifier.event(&loaded, later), None);
        assert_eq!(notifier.tick(later + DELIVERY_RETRY), None);
    }
}
