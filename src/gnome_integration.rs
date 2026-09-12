use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dbus::arg::{AppendAll, ReadAll};
use dbus::blocking::Connection;
use dbus::message::MatchRule;
use parking_lot::Mutex;
use thiserror::Error;

use crate::{
    DisplayGeometry, FocusedWindow, HotkeyEvent, HotkeySystem, HotkeySystemError, Rect, WindowMove,
    WindowSystem, WindowSystemError,
};

pub const GNOME_SERVICE_NAME: &str = "org.window_zones.Gnome";
pub const GNOME_OBJECT_PATH: &str = "/org/window_zones/Gnome";
pub const GNOME_INTERFACE: &str = "org.window_zones.Gnome1";
const HOTKEY_SIGNAL: &str = "HotkeyPressed";
const CALL_TIMEOUT: Duration = Duration::from_secs(1);

const FOCUSED_WINDOW_CAPABILITY: &str = "focused-window";
const DISPLAYS_CAPABILITY: &str = "displays";
const MOVE_RESIZE_CAPABILITY: &str = "move-resize";
const HOTKEYS_CAPABILITY: &str = "hotkeys";

type DisplayPayload = (String, i32, i32, u32, u32);

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GnomeIntegrationError {
    #[error(
        "GNOME companion is unavailable: {message}; install and enable the GNOME Shell companion, then retry"
    )]
    Unavailable { message: String },
    #[error(
        "GNOME companion is incompatible: {message}; upgrade the GNOME Shell companion for org.window_zones.Gnome1"
    )]
    Incompatible { message: String },
    #[error(
        "GNOME companion denied the request: {message}; check session permissions and companion access"
    )]
    Denied { message: String },
    #[error(
        "GNOME companion does not support {capability}: {message}; use a companion exposing that capability"
    )]
    Unsupported { capability: String, message: String },
    #[error("GNOME companion is busy: {message}; release the other App controller before retrying")]
    Busy { message: String },
    #[error(
        "GNOME companion rejected the request: {message}; restore an eligible focused window or correct the request"
    )]
    Invalid { message: String },
    #[error("GNOME companion operation failed: {message}; retry the operation")]
    Operation { message: String },
}

impl GnomeIntegrationError {
    fn should_reconnect(&self) -> bool {
        matches!(self, Self::Unavailable { .. } | Self::Incompatible { .. })
    }
}

#[derive(Debug, Clone)]
pub struct GnomeWindowSystem {
    bus_address: Option<String>,
}

impl GnomeWindowSystem {
    pub fn new() -> Self {
        Self { bus_address: None }
    }

    pub fn capabilities(&self) -> Result<Vec<String>, GnomeIntegrationError> {
        let connection = connect(self.bus_address.as_deref())?;
        get_capabilities(&connection)
    }

    #[cfg(test)]
    fn with_bus_address(bus_address: impl Into<String>) -> Self {
        Self {
            bus_address: Some(bus_address.into()),
        }
    }

    fn call_with_capability<R, A>(
        &self,
        capability: &str,
        method: &str,
        args: A,
    ) -> Result<R, GnomeIntegrationError>
    where
        R: ReadAll,
        A: AppendAll,
    {
        let connection = connect(self.bus_address.as_deref())?;
        let capabilities = get_capabilities(&connection)?;
        require_capability(&capabilities, capability)?;

        let proxy = connection.with_proxy(GNOME_SERVICE_NAME, GNOME_OBJECT_PATH, CALL_TIMEOUT);
        proxy
            .method_call(GNOME_INTERFACE, method, args)
            .map_err(classify_dbus_error)
    }
}

impl Default for GnomeWindowSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowSystem for GnomeWindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
        let result: Result<(bool, String, i32, i32, u32, u32), GnomeIntegrationError> =
            self.call_with_capability(FOCUSED_WINDOW_CAPABILITY, "GetFocusedWindow", ());

        result
            .map(|(present, _display_id, x, y, width, height)| {
                present.then(|| FocusedWindow::new(Rect::new(x, y, width, height)))
            })
            .map_err(|error| WindowSystemError::Platform(error.to_string()))
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        let result: Result<(Vec<DisplayPayload>,), GnomeIntegrationError> =
            self.call_with_capability(DISPLAYS_CAPABILITY, "GetDisplays", ());

        result
            .map(|(displays,)| {
                displays
                    .into_iter()
                    .map(|(id, x, y, width, height)| {
                        DisplayGeometry::new(id, Rect::new(x, y, width, height))
                    })
                    .collect()
            })
            .map_err(|error| WindowSystemError::Platform(error.to_string()))
    }

    fn move_focused_window(&mut self, window_move: WindowMove) -> Result<(), WindowSystemError> {
        let target = window_move.target;
        let result: Result<(), GnomeIntegrationError> = self.call_with_capability(
            MOVE_RESIZE_CAPABILITY,
            "MoveFocusedWindow",
            (target.x, target.y, target.width, target.height),
        );

        result.map_err(|error| WindowSystemError::Platform(error.to_string()))
    }
}

pub struct GnomeHotkeySystem {
    bus_address: Option<String>,
    connection: Option<Connection>,
    events: Arc<Mutex<VecDeque<HotkeyEvent>>>,
    service_lost: Arc<AtomicBool>,
}

impl fmt::Debug for GnomeHotkeySystem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GnomeHotkeySystem")
            .field("bus_address", &self.bus_address)
            .field("connection_established", &self.connection.is_some())
            .field("events", &self.events)
            .field("service_lost", &self.service_lost.load(Ordering::Relaxed))
            .finish()
    }
}

impl GnomeHotkeySystem {
    pub fn new() -> Self {
        Self {
            bus_address: None,
            connection: None,
            events: Arc::new(Mutex::new(VecDeque::new())),
            service_lost: Arc::new(AtomicBool::new(false)),
        }
    }

    #[cfg(test)]
    fn with_bus_address(bus_address: impl Into<String>) -> Self {
        Self {
            bus_address: Some(bus_address.into()),
            connection: None,
            events: Arc::new(Mutex::new(VecDeque::new())),
            service_lost: Arc::new(AtomicBool::new(false)),
        }
    }

    fn connect(&mut self) -> Result<(), GnomeIntegrationError> {
        self.events.lock().clear();
        let connection = connect(self.bus_address.as_deref())?;

        let events = Arc::clone(&self.events);
        let service_lost = Arc::clone(&self.service_lost);
        let match_rule = MatchRule::new_signal(GNOME_INTERFACE, HOTKEY_SIGNAL)
            .with_path(GNOME_OBJECT_PATH)
            .with_sender(GNOME_SERVICE_NAME);

        connection
            .add_match(match_rule, move |(hotkey,): (String,), _, _| {
                events.lock().push_back(HotkeyEvent::Pressed { hotkey });
                true
            })
            .map_err(classify_dbus_error)?;

        let owner_rule = MatchRule::new_signal("org.freedesktop.DBus", "NameOwnerChanged");
        connection
            .add_match(
                owner_rule,
                move |(name, old_owner, new_owner): (String, String, String), _, _| {
                    if name == GNOME_SERVICE_NAME
                        && (new_owner.is_empty()
                            || (!old_owner.is_empty() && old_owner != new_owner))
                    {
                        service_lost.store(true, Ordering::Relaxed);
                    }
                    true
                },
            )
            .map_err(classify_dbus_error)?;

        self.service_lost.store(false, Ordering::Relaxed);
        self.connection = Some(connection);
        Ok(())
    }

    fn ensure_connection(&mut self) -> Result<&Connection, GnomeIntegrationError> {
        if self.connection.is_none() {
            self.connect()?;
        }

        self.connection
            .as_ref()
            .ok_or_else(|| GnomeIntegrationError::Unavailable {
                message: "D-Bus connection was not established".to_string(),
            })
    }

    fn call_register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), GnomeIntegrationError> {
        let result = (|| {
            let connection = self.ensure_connection()?;
            let capabilities = get_capabilities(connection)?;
            require_capability(&capabilities, HOTKEYS_CAPABILITY)?;

            let proxy = connection.with_proxy(GNOME_SERVICE_NAME, GNOME_OBJECT_PATH, CALL_TIMEOUT);
            proxy
                .method_call(GNOME_INTERFACE, "RegisterHotkeys", (hotkeys.to_vec(),))
                .map(|(): ()| ())
                .map_err(classify_dbus_error)
        })();

        if let Err(error) = &result {
            if error.should_reconnect() {
                self.reset_connection();
            } else {
                self.events.lock().clear();
            }
        }

        result
    }
    fn reset_connection(&mut self) {
        self.connection = None;
        self.events.lock().clear();
    }
}

impl Default for GnomeHotkeySystem {
    fn default() -> Self {
        Self::new()
    }
}

impl HotkeySystem for GnomeHotkeySystem {
    fn register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), HotkeySystemError> {
        self.call_register_hotkeys(hotkeys)
            .map_err(|error| HotkeySystemError::Platform(error.to_string()))
    }

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
        let process_result = {
            let connection = self
                .ensure_connection()
                .map_err(|error| HotkeySystemError::Platform(error.to_string()))?;
            connection.process(Duration::from_millis(0))
        };

        if let Err(error) = process_result {
            self.reset_connection();
            return Err(HotkeySystemError::Platform(
                GnomeIntegrationError::Unavailable {
                    message: format!("failed to process D-Bus events: {error}"),
                }
                .to_string(),
            ));
        }

        if self.service_lost.load(Ordering::Relaxed) {
            self.reset_connection();
            return Err(HotkeySystemError::Platform(
                GnomeIntegrationError::Unavailable {
                    message: "GNOME companion service disappeared from the session bus".to_string(),
                }
                .to_string(),
            ));
        }

        Ok(self.events.lock().pop_front())
    }
}

fn connect(bus_address: Option<&str>) -> Result<Connection, GnomeIntegrationError> {
    let result = match bus_address {
        Some(address) => Connection::new_address(address),
        None => Connection::new_session(),
    };

    result.map_err(|error| GnomeIntegrationError::Unavailable {
        message: format!("failed to connect to the user session bus: {error}"),
    })
}

fn get_capabilities(connection: &Connection) -> Result<Vec<String>, GnomeIntegrationError> {
    let proxy = connection.with_proxy(GNOME_SERVICE_NAME, GNOME_OBJECT_PATH, CALL_TIMEOUT);
    proxy
        .method_call(GNOME_INTERFACE, "GetCapabilities", ())
        .map(|(capabilities,): (Vec<String>,)| capabilities)
        .map_err(classify_dbus_error)
}

fn require_capability(
    capabilities: &[String],
    capability: &str,
) -> Result<(), GnomeIntegrationError> {
    if capabilities.iter().any(|value| value == capability) {
        return Ok(());
    }

    Err(GnomeIntegrationError::Unsupported {
        capability: capability.to_string(),
        message: "install a companion version exposing the required capability".to_string(),
    })
}

fn classify_dbus_error(error: dbus::Error) -> GnomeIntegrationError {
    let name = error.name().unwrap_or("org.freedesktop.DBus.Error.Failed");
    let message = error
        .message()
        .unwrap_or("D-Bus returned an unspecified error")
        .to_string();

    match name {
        "org.freedesktop.DBus.Error.ServiceUnknown"
        | "org.freedesktop.DBus.Error.NameHasNoOwner"
        | "org.freedesktop.DBus.Error.NoReply"
        | "org.freedesktop.DBus.Error.UnknownObject" => {
            GnomeIntegrationError::Unavailable { message }
        }
        "org.freedesktop.DBus.Error.UnknownInterface"
        | "org.freedesktop.DBus.Error.UnknownMethod" => {
            GnomeIntegrationError::Incompatible { message }
        }
        "org.freedesktop.DBus.Error.AccessDenied" => GnomeIntegrationError::Denied { message },
        "org.freedesktop.DBus.Error.InvalidArgs" => GnomeIntegrationError::Invalid { message },
        "org.window_zones.Gnome.Error.Unsupported" => GnomeIntegrationError::Unsupported {
            capability: "requested operation".to_string(),
            message,
        },
        "org.window_zones.Gnome.Error.Busy" => GnomeIntegrationError::Busy { message },
        "org.window_zones.Gnome.Error.InvalidWindowState" => {
            GnomeIntegrationError::Invalid { message }
        }
        _ => GnomeIntegrationError::Operation { message },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use dbus::blocking::LocalConnection;
    use dbus_tree::{Factory, MethodErr};
    use parking_lot::Mutex;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread::{self, JoinHandle};

    #[derive(Debug)]
    struct FakeServiceState {
        capabilities: Vec<String>,
        capabilities_error: Option<(&'static str, String)>,
        capabilities_delay: Option<Duration>,
        focused: Option<(String, i32, i32, u32, u32)>,
        displays: Vec<(String, i32, i32, u32, u32)>,
        moves: Vec<(i32, i32, u32, u32)>,
        registered_hotkeys: Vec<String>,
        register_error: Option<(&'static str, String)>,
        signal_hotkey: Option<String>,
    }

    impl FakeServiceState {
        fn complete() -> Self {
            Self {
                capabilities: vec![
                    FOCUSED_WINDOW_CAPABILITY.to_string(),
                    DISPLAYS_CAPABILITY.to_string(),
                    MOVE_RESIZE_CAPABILITY.to_string(),
                    HOTKEYS_CAPABILITY.to_string(),
                ],
                capabilities_error: None,
                capabilities_delay: None,
                focused: Some(("monitor-1".to_string(), -300, -20, 801, 602)),
                displays: vec![
                    ("monitor-0".to_string(), -1920, 0, 1920, 1080),
                    ("monitor-1".to_string(), 0, -50, 1280, 1024),
                ],
                moves: Vec::new(),
                registered_hotkeys: Vec::new(),
                register_error: None,
                signal_hotkey: Some("ctrl+alt+left".to_string()),
            }
        }
    }

    struct FakeBus {
        address: String,
        state: Arc<Mutex<FakeServiceState>>,
        stop: Arc<AtomicBool>,
        service_thread: Option<JoinHandle<()>>,
        daemon: Child,
    }

    impl FakeBus {
        fn new(state: FakeServiceState) -> Self {
            let mut daemon = Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address=1"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("dbus-daemon must be installed for GNOME contract tests");
            let stdout = daemon.stdout.take().expect("dbus-daemon stdout");
            let mut reader = BufReader::new(stdout);
            let mut address = String::new();
            reader
                .read_line(&mut address)
                .expect("read private D-Bus address");
            let address = address.trim().to_string();
            assert!(
                !address.is_empty(),
                "private D-Bus address must not be empty"
            );

            let mut bus = Self {
                address,
                state: Arc::new(Mutex::new(state)),
                stop: Arc::new(AtomicBool::new(false)),
                service_thread: None,
                daemon,
            };
            bus.start_companion();
            bus
        }

        fn start_companion(&mut self) {
            assert!(
                self.service_thread.is_none(),
                "fake companion must not already be running"
            );
            self.stop.store(false, Ordering::Relaxed);

            let address = self.address.clone();
            let state = Arc::clone(&self.state);
            let stop = Arc::clone(&self.stop);
            let (ready_tx, ready_rx) = mpsc::channel();

            self.service_thread = Some(thread::spawn(move || {
                let connection = match LocalConnection::new_address(&address) {
                    Ok(connection) => connection,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                        return;
                    }
                };
                if let Err(error) = connection.request_name(GNOME_SERVICE_NAME, false, true, false)
                {
                    let _ = ready_tx.send(Err(error.to_string()));
                    return;
                }

                let factory = Factory::new_fn::<()>();
                let signal = Arc::new(factory.signal(HOTKEY_SIGNAL, ()).sarg::<&str, _>("hotkey"));
                let signal_for_method = Arc::clone(&signal);

                let capabilities_state = Arc::clone(&state);
                let focused_state = Arc::clone(&state);
                let displays_state = Arc::clone(&state);
                let move_state = Arc::clone(&state);
                let register_state = Arc::clone(&state);

                let tree = factory
                    .tree(())
                    .add(
                        factory
                            .object_path(GNOME_OBJECT_PATH, ())
                            .introspectable()
                            .add(
                                factory
                                    .interface(GNOME_INTERFACE, ())
                                    .add_m(factory.method("GetCapabilities", (), move |method| {
                                        let delay = capabilities_state.lock().capabilities_delay;
                                        if let Some(delay) = delay {
                                            thread::sleep(delay);
                                        }
                                        let state = capabilities_state.lock();
                                        if let Some((name, message)) =
                                            state.capabilities_error.clone()
                                        {
                                            return Err((name, message).into());
                                        }
                                        let reply = method
                                            .msg
                                            .method_return()
                                            .append1(state.capabilities.clone());
                                        Ok(vec![reply])
                                    }))
                                    .add_m(factory.method("GetFocusedWindow", (), move |method| {
                                        let mut reply = method.msg.method_return();
                                        let state = focused_state.lock();
                                        match &state.focused {
                                            Some((id, x, y, width, height)) => reply.append_all((
                                                true,
                                                id.clone(),
                                                *x,
                                                *y,
                                                *width,
                                                *height,
                                            )),
                                            None => reply.append_all((
                                                false,
                                                String::new(),
                                                0_i32,
                                                0_i32,
                                                0_u32,
                                                0_u32,
                                            )),
                                        }
                                        Ok(vec![reply])
                                    }))
                                    .add_m(factory.method("GetDisplays", (), move |method| {
                                        let state = displays_state.lock();
                                        let reply = method
                                            .msg
                                            .method_return()
                                            .append1(state.displays.clone());
                                        Ok(vec![reply])
                                    }))
                                    .add_m(factory.method("MoveFocusedWindow", (), move |method| {
                                        let (x, y, width, height): (i32, i32, u32, u32) =
                                            method.msg.read4()?;
                                        move_state.lock().moves.push((x, y, width, height));
                                        Ok(vec![method.msg.method_return()])
                                    }))
                                    .add_m(factory.method("RegisterHotkeys", (), move |method| {
                                        let hotkeys: Vec<String> = method.msg.read1()?;
                                        let (error, signal_hotkey) = {
                                            let mut state = register_state.lock();
                                            let error = state.register_error.clone();
                                            if error.is_none() {
                                                state.registered_hotkeys = hotkeys;
                                            }
                                            (error, state.signal_hotkey.clone())
                                        };
                                        if let Some((name, message)) = error {
                                            return Err(MethodErr::from((name, message)));
                                        }

                                        let mut replies = vec![method.msg.method_return()];
                                        if let Some(hotkey) = signal_hotkey {
                                            replies.push(
                                                signal_for_method
                                                    .msg(
                                                        method.path.get_name(),
                                                        method.iface.get_name(),
                                                    )
                                                    .append1(hotkey),
                                            );
                                        }
                                        Ok(replies)
                                    }))
                                    .add_s(signal),
                            ),
                    )
                    .add(factory.object_path("/", ()).introspectable());
                tree.start_receive(&connection);
                ready_tx
                    .send(Ok(()))
                    .expect("report fake companion readiness");

                while !stop.load(Ordering::Relaxed) {
                    if connection.process(Duration::from_millis(25)).is_err() {
                        break;
                    }
                }
            }));

            match ready_rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Ok(())) => {}
                Ok(Err(error)) => panic!("start fake companion: {error}"),
                Err(error) => panic!("fake companion did not start: {error}"),
            }
        }

        fn stop_companion(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = self.service_thread.take() {
                thread.join().expect("fake companion thread");
            }
        }

        fn update_state(&self, update: impl FnOnce(&mut FakeServiceState)) {
            update(&mut self.state.lock());
        }
    }

    impl Drop for FakeBus {
        fn drop(&mut self) {
            self.stop_companion();
            let _ = self.daemon.kill();
            let _ = self.daemon.wait();
        }
    }

    #[test]
    fn missing_capability_is_explicit() {
        let error = require_capability(&["displays".to_string()], MOVE_RESIZE_CAPABILITY)
            .expect_err("missing capability should fail");

        assert!(matches!(error, GnomeIntegrationError::Unsupported { .. }));
        assert!(error.to_string().contains("move-resize"));
    }

    #[test]
    fn required_capability_is_accepted() {
        let capabilities = vec![FOCUSED_WINDOW_CAPABILITY.to_string()];

        assert!(require_capability(&capabilities, FOCUSED_WINDOW_CAPABILITY).is_ok());
    }

    #[test]
    fn private_service_round_trips_window_and_hotkey_contract() {
        let bus = FakeBus::new(FakeServiceState::complete());
        let mut window_system = GnomeWindowSystem::with_bus_address(bus.address.clone());

        assert_eq!(
            window_system.focused_window().unwrap(),
            Some(FocusedWindow::new(Rect::new(-300, -20, 801, 602)))
        );
        assert_eq!(
            window_system.displays().unwrap(),
            vec![
                DisplayGeometry::new("monitor-0", Rect::new(-1920, 0, 1920, 1080)),
                DisplayGeometry::new("monitor-1", Rect::new(0, -50, 1280, 1024)),
            ]
        );

        window_system
            .move_focused_window(WindowMove::new(Rect::new(-123, 456, 777, 888)))
            .unwrap();
        bus.update_state(|state| {
            assert_eq!(state.moves, vec![(-123, 456, 777, 888)]);
            state.focused = None;
        });
        assert_eq!(window_system.focused_window().unwrap(), None);

        let mut hotkey_system = GnomeHotkeySystem::with_bus_address(bus.address.clone());
        hotkey_system
            .register_hotkeys(&["ctrl+alt+left".to_string(), "super+1".to_string()])
            .unwrap();
        bus.update_state(|state| {
            assert_eq!(
                state.registered_hotkeys,
                vec!["ctrl+alt+left".to_string(), "super+1".to_string()]
            );
        });
        let mut event = None;
        for _ in 0..100 {
            event = hotkey_system.next_hotkey().unwrap();
            if event.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            event,
            Some(HotkeyEvent::Pressed {
                hotkey: "ctrl+alt+left".to_string()
            })
        );
    }

    #[test]
    fn registration_rejection_keeps_previous_set_and_busy_is_actionable() {
        let bus = FakeBus::new(FakeServiceState::complete());
        let mut hotkey_system = GnomeHotkeySystem::with_bus_address(bus.address.clone());
        hotkey_system
            .register_hotkeys(&["ctrl+alt+left".to_string()])
            .unwrap();
        let _ = hotkey_system.next_hotkey();

        bus.update_state(|state| {
            state.register_error = Some((
                "org.window_zones.Gnome.Error.Busy",
                "another controller owns hotkey registration".to_string(),
            ));
        });
        let error = hotkey_system
            .register_hotkeys(&["ctrl+alt+right".to_string()])
            .unwrap_err();
        assert!(error.to_string().contains("busy"));
        bus.update_state(|state| {
            assert_eq!(state.registered_hotkeys, vec!["ctrl+alt+left".to_string()]);
            state.register_error = None;
        });

        hotkey_system
            .register_hotkeys(&["ctrl+alt+right".to_string()])
            .unwrap();
        bus.update_state(|state| {
            assert_eq!(state.registered_hotkeys, vec!["ctrl+alt+right".to_string()]);
        });
    }

    #[test]
    fn companion_disconnect_reconnects_and_re_registers() {
        let mut bus = FakeBus::new(FakeServiceState::complete());
        let mut hotkey_system = GnomeHotkeySystem::with_bus_address(bus.address.clone());
        hotkey_system
            .register_hotkeys(&["ctrl+alt+left".to_string()])
            .unwrap();
        let _ = hotkey_system.next_hotkey();

        bus.stop_companion();
        let mut disconnected = false;
        for _ in 0..100 {
            if hotkey_system.next_hotkey().is_err() {
                disconnected = true;
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        assert!(disconnected, "client should report companion disconnect");
        let window_system = GnomeWindowSystem::with_bus_address(bus.address.clone());
        assert!(matches!(
            window_system.capabilities(),
            Err(GnomeIntegrationError::Unavailable { .. })
        ));

        bus.start_companion();
        hotkey_system
            .register_hotkeys(&["ctrl+alt+left".to_string()])
            .unwrap();
        bus.update_state(|state| {
            assert_eq!(state.registered_hotkeys, vec!["ctrl+alt+left".to_string()]);
        });
    }

    #[test]
    fn connection_reset_discards_queued_events() {
        let mut hotkey_system = GnomeHotkeySystem::with_bus_address("unused");
        hotkey_system.events.lock().push_back(HotkeyEvent::Pressed {
            hotkey: "ctrl+alt+left".to_string(),
        });

        hotkey_system.reset_connection();

        assert!(hotkey_system.events.lock().is_empty());
    }

    #[test]
    fn lifecycle_dbus_errors_are_classified() {
        let cases = [
            (
                "org.freedesktop.DBus.Error.ServiceUnknown",
                GnomeIntegrationError::Unavailable {
                    message: "missing".to_string(),
                },
            ),
            (
                "org.freedesktop.DBus.Error.UnknownObject",
                GnomeIntegrationError::Unavailable {
                    message: "object removed".to_string(),
                },
            ),
            (
                "org.freedesktop.DBus.Error.UnknownInterface",
                GnomeIntegrationError::Incompatible {
                    message: "old".to_string(),
                },
            ),
            (
                "org.freedesktop.DBus.Error.AccessDenied",
                GnomeIntegrationError::Denied {
                    message: "denied".to_string(),
                },
            ),
            (
                "org.freedesktop.DBus.Error.NoReply",
                GnomeIntegrationError::Unavailable {
                    message: "timeout".to_string(),
                },
            ),
        ];
        for (name, expected) in cases {
            let actual = classify_dbus_error(dbus::Error::new_custom(name, &expected.to_string()));
            match expected {
                GnomeIntegrationError::Unavailable { .. } => {
                    assert!(matches!(actual, GnomeIntegrationError::Unavailable { .. }));
                }
                GnomeIntegrationError::Incompatible { .. } => {
                    assert!(matches!(actual, GnomeIntegrationError::Incompatible { .. }));
                }
                GnomeIntegrationError::Denied { .. } => {
                    assert!(matches!(actual, GnomeIntegrationError::Denied { .. }));
                }
                _ => unreachable!("only lifecycle classifications are tested"),
            }
        }
    }

    #[test]
    fn service_timeout_is_reported_as_unavailable() {
        let bus = FakeBus::new(FakeServiceState::complete());
        let window_system = GnomeWindowSystem::with_bus_address(bus.address.clone());
        bus.update_state(|state| {
            state.capabilities_delay = Some(CALL_TIMEOUT + Duration::from_millis(100));
        });

        assert!(matches!(
            window_system.capabilities(),
            Err(GnomeIntegrationError::Unavailable { .. })
        ));
    }

    #[test]
    fn incompatible_and_denied_service_responses_are_exposed() {
        let bus = FakeBus::new(FakeServiceState::complete());
        let window_system = GnomeWindowSystem::with_bus_address(bus.address.clone());

        bus.update_state(|state| {
            state.capabilities_error = Some((
                "org.freedesktop.DBus.Error.UnknownInterface",
                "GNOME1 is not available".to_string(),
            ));
        });
        assert!(matches!(
            window_system.capabilities(),
            Err(GnomeIntegrationError::Incompatible { .. })
        ));

        bus.update_state(|state| {
            state.capabilities_error = Some((
                "org.freedesktop.DBus.Error.AccessDenied",
                "user session policy denied access".to_string(),
            ));
        });
        assert!(matches!(
            window_system.capabilities(),
            Err(GnomeIntegrationError::Denied { .. })
        ));
    }
}
