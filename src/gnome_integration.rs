use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use dbus::arg::{AppendAll, ReadAll};
use dbus::blocking::Connection;
use dbus::channel::MatchingReceiver;
use dbus::message::MatchRule;
use parking_lot::Mutex;
use thiserror::Error;

use crate::window_system::TransitionMetrics;
use crate::{
    DisplayGeometry, FocusedWindow, HotkeyEvent, HotkeySystem, HotkeySystemError, Rect, WindowId,
    WindowMove, WindowSystem, WindowSystemError,
};

pub const GNOME_SERVICE_NAME: &str = "org.window_zones.Gnome";
pub const GNOME_OBJECT_PATH: &str = "/org/window_zones/Gnome";
pub const GNOME_INTERFACE: &str = "org.window_zones.Gnome2";
const HOTKEY_SIGNAL: &str = "HotkeyPressed";
const CALL_TIMEOUT: Duration = Duration::from_secs(1);

const FOCUSED_WINDOW_CAPABILITY: &str = "focused-window";
const DISPLAYS_CAPABILITY: &str = "displays";
const MOVE_RESIZE_CAPABILITY: &str = "move-resize";
const HOTKEYS_CAPABILITY: &str = "hotkeys";
const TRANSITION_METRICS_CAPABILITY: &str = "transition-metrics";

type DisplayPayload = (String, i32, i32, u32, u32);

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GnomeIntegrationError {
    #[error(
        "GNOME companion is unavailable: {message}; install and enable it (`scripts/install.sh --gnome-extension`), then log out and back in so GNOME Shell loads it"
    )]
    Unavailable { message: String },
    #[error(
        "GNOME companion is incompatible: {message}; upgrade the GNOME Shell companion for org.window_zones.Gnome2"
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
    #[error("GNOME companion window no longer exists: {message}; focus another window and retry")]
    WindowGone { message: String },
    #[error("GNOME companion operation failed: {message}; retry the operation")]
    Operation { message: String },
}

impl GnomeIntegrationError {
    fn should_reconnect(&self) -> bool {
        matches!(self, Self::Unavailable { .. } | Self::Incompatible { .. })
    }

    /// A refused set leaves the companion's previous set registered; every other
    /// failure means nothing is known to be registered.
    fn into_hotkey_error(self) -> HotkeySystemError {
        match self {
            Self::Unsupported { .. } | Self::Invalid { .. } => {
                HotkeySystemError::Rejected(self.to_string())
            }
            _ => HotkeySystemError::Unavailable(self.to_string()),
        }
    }
}

struct WindowConnection {
    connection: Connection,
    capabilities: Option<Vec<String>>,
    has_transition: bool,
}

#[derive(Clone, Default)]
pub struct GnomeWindowSystem {
    bus_address: Option<String>,
    cache: Arc<Mutex<Option<WindowConnection>>>,
}

impl fmt::Debug for GnomeWindowSystem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GnomeWindowSystem")
            .field("bus_address", &self.bus_address)
            .field("connection_established", &self.cache.lock().is_some())
            .finish()
    }
}

impl GnomeWindowSystem {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn capabilities(&self) -> Result<Vec<String>, GnomeIntegrationError> {
        self.with_connection(true, |cached| {
            Ok(cached.capabilities.as_ref().unwrap().clone())
        })
    }

    #[cfg(test)]
    fn with_bus_address(bus_address: impl Into<String>) -> Self {
        Self {
            bus_address: Some(bus_address.into()),
            cache: Arc::new(Mutex::new(None)),
        }
    }

    fn with_connection<R>(
        &self,
        refresh_capabilities: bool,
        call: impl FnOnce(&mut WindowConnection) -> Result<R, GnomeIntegrationError>,
    ) -> Result<R, GnomeIntegrationError> {
        let mut cache = self.cache.lock();
        let result = (|| {
            if cache.is_none() {
                *cache = Some(WindowConnection {
                    connection: connect(self.bus_address.as_deref())?,
                    capabilities: None,
                    has_transition: false,
                });
            }
            let cached = cache.as_mut().unwrap();
            if refresh_capabilities || cached.capabilities.is_none() {
                cached.capabilities = None;
                cached.capabilities = Some(get_capabilities(&cached.connection)?);
            }
            call(cached)
        })();
        if result
            .as_ref()
            .is_err_and(GnomeIntegrationError::should_reconnect)
        {
            *cache = None;
        }
        result
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
        self.with_connection(false, |cached| {
            cached.has_transition = false;
            require_capability(cached.capabilities.as_ref().unwrap(), capability)?;
            let proxy =
                cached
                    .connection
                    .with_proxy(GNOME_SERVICE_NAME, GNOME_OBJECT_PATH, CALL_TIMEOUT);
            proxy
                .method_call(GNOME_INTERFACE, method, args)
                .map_err(classify_dbus_error)
        })
    }
}

impl WindowSystem for GnomeWindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
        let result: Result<(bool, u64, i32, i32, u32, u32), GnomeIntegrationError> =
            self.call_with_capability(FOCUSED_WINDOW_CAPABILITY, "GetFocusedWindow", ());

        result
            .map(|(present, id, x, y, width, height)| {
                present.then(|| {
                    FocusedWindow::new(
                        WindowId::new(id.to_string()),
                        Rect::new(x, y, width, height),
                    )
                })
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

    fn move_window(&mut self, window_move: &WindowMove) -> Result<(), WindowSystemError> {
        let id = window_move
            .window
            .as_str()
            .parse::<u64>()
            .map_err(|error| {
                if let Some(cached) = self.cache.lock().as_mut() {
                    cached.has_transition = false;
                }
                WindowSystemError::Platform(format!(
                    "invalid GNOME window id '{}': {error}; refocus the window and retry",
                    window_move.window.as_str()
                ))
            })?;
        let target = window_move.target;
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct PartialMovePayload {
            window_id: String,
            observed: (i32, i32, u32, u32),
            message: String,
        }
        let mut partial_move = None;
        let result: Result<(), GnomeIntegrationError> = self.with_connection(false, |cached| {
            cached.has_transition = false;
            require_capability(
                cached.capabilities.as_ref().unwrap(),
                MOVE_RESIZE_CAPABILITY,
            )?;
            let proxy =
                cached
                    .connection
                    .with_proxy(GNOME_SERVICE_NAME, GNOME_OBJECT_PATH, CALL_TIMEOUT);
            proxy
                .method_call::<(), _, _, _>(
                    GNOME_INTERFACE,
                    "MoveWindow",
                    (id, target.x, target.y, target.width, target.height),
                )
                .map_err(|error| {
                    if error.name() == Some("org.window_zones.Gnome.Error.PartialMove")
                        && let Some(payload) = error.message().and_then(|message| {
                            serde_json::from_str::<PartialMovePayload>(message).ok()
                        })
                        && payload.window_id == id.to_string()
                        && payload.observed.2 > 0
                        && payload.observed.3 > 0
                    {
                        let (x, y, width, height) = payload.observed;
                        partial_move = Some((Rect::new(x, y, width, height), payload.message));
                    }
                    classify_dbus_error(error)
                })?;
            cached.has_transition = true;
            Ok(())
        });

        result.map_err(|error| match (error, partial_move) {
            (GnomeIntegrationError::WindowGone { .. }, _) => {
                WindowSystemError::WindowGone(window_move.window.clone())
            }
            (_, Some((observed, message))) => WindowSystemError::PartialMove {
                observed,
                message: GnomeIntegrationError::Operation { message }.to_string(),
            },
            (error, None) => WindowSystemError::Platform(error.to_string()),
        })
    }

    fn transition_metrics(&self) -> Result<Option<TransitionMetrics>, WindowSystemError> {
        self.with_connection(false, |cached| {
            if !std::mem::take(&mut cached.has_transition)
                || !cached
                    .capabilities
                    .as_ref()
                    .unwrap()
                    .iter()
                    .any(|value| value == TRANSITION_METRICS_CAPABILITY)
            {
                return Ok(None);
            }
            let proxy =
                cached
                    .connection
                    .with_proxy(GNOME_SERVICE_NAME, GNOME_OBJECT_PATH, CALL_TIMEOUT);
            let (request_ms, completion_ms): (f64, f64) = proxy
                .method_call(GNOME_INTERFACE, "GetLastTransitionMetrics", ())
                .map_err(classify_dbus_error)?;
            Ok(Some(TransitionMetrics {
                request_ms,
                completion_ms,
            }))
        })
        .map_err(|error| WindowSystemError::Platform(error.to_string()))
    }
}

#[derive(Default)]
pub struct GnomeHotkeySystem {
    bus_address: Option<String>,
    connection: Option<Connection>,
    capabilities: Option<Vec<String>>,
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
        Self::default()
    }

    #[cfg(test)]
    fn with_bus_address(bus_address: impl Into<String>) -> Self {
        Self {
            bus_address: Some(bus_address.into()),
            ..Self::new()
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

        let owner_rule = MatchRule::new_signal("org.freedesktop.DBus", "NameOwnerChanged")
            .with_sender("org.freedesktop.DBus")
            .with_path("/org/freedesktop/DBus");
        // dbus 0.9 MatchRule only supports headers; arg0 must be added to
        // the bus-side rule separately from the local callback filter.
        connection
            .add_match_no_cb(&format!(
                "{},arg0='{GNOME_SERVICE_NAME}'",
                owner_rule.match_str()
            ))
            .map_err(classify_dbus_error)?;
        connection.start_receive(
            owner_rule,
            Box::new(move |message, _| {
                if let Ok((name, old_owner, new_owner)) = message.read3::<String, String, String>()
                    && name == GNOME_SERVICE_NAME
                    && (new_owner.is_empty() || (!old_owner.is_empty() && old_owner != new_owner))
                {
                    service_lost.store(true, Ordering::Relaxed);
                }
                true
            }),
        );

        self.service_lost.store(false, Ordering::Relaxed);
        self.connection = Some(connection);
        Ok(())
    }

    fn ensure_connection(&mut self) -> Result<&Connection, GnomeIntegrationError> {
        if self.connection.is_none() {
            self.connect()?;
        }

        Ok(self.connection.as_ref().unwrap())
    }

    fn reset_connection(&mut self) {
        self.connection = None;
        self.capabilities = None;
        self.events.lock().clear();
    }
}

impl HotkeySystem for GnomeHotkeySystem {
    fn register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), HotkeySystemError> {
        let result = (|| {
            self.ensure_connection()?;
            if self.capabilities.is_none() {
                self.capabilities = Some(get_capabilities(self.connection.as_ref().unwrap())?);
            }
            require_capability(self.capabilities.as_ref().unwrap(), HOTKEYS_CAPABILITY)?;
            let connection = self.connection.as_ref().unwrap();

            let proxy = connection.with_proxy(GNOME_SERVICE_NAME, GNOME_OBJECT_PATH, CALL_TIMEOUT);
            proxy
                .method_call::<(), _, _, _>(
                    GNOME_INTERFACE,
                    "RegisterHotkeys",
                    (dbus::arg::Array::new(hotkeys),),
                )
                .map_err(classify_dbus_error)
        })();

        if let Err(error) = &result {
            if error.should_reconnect() {
                self.reset_connection();
            } else {
                self.events.lock().clear();
            }
        }

        result.map_err(GnomeIntegrationError::into_hotkey_error)
    }

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
        let process_result = {
            let connection = self
                .ensure_connection()
                .map_err(GnomeIntegrationError::into_hotkey_error)?;
            loop {
                match connection.process(Duration::ZERO) {
                    Ok(true) => continue,
                    result => break result,
                }
            }
        };

        if let Err(error) = process_result {
            self.reset_connection();
            return Err(HotkeySystemError::Unavailable(
                GnomeIntegrationError::Unavailable {
                    message: format!("failed to process D-Bus events: {error}"),
                }
                .to_string(),
            ));
        }

        if self.service_lost.load(Ordering::Relaxed) {
            self.reset_connection();
            return Err(HotkeySystemError::Unavailable(
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
        | "org.freedesktop.DBus.Error.Disconnected"
        | "org.freedesktop.DBus.Error.UnknownObject"
        | "org.window_zones.Gnome.Error.Unavailable" => {
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
        "org.window_zones.Gnome.Error.WindowGone" => GnomeIntegrationError::WindowGone { message },
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
        reply_capabilities: bool,
        capabilities_calls: usize,
        window_senders: Vec<String>,
        focused: Option<(u64, i32, i32, u32, u32)>,
        displays: Vec<(String, i32, i32, u32, u32)>,
        moves: Vec<(u64, i32, i32, u32, u32)>,
        move_error: Option<(&'static str, String)>,
        move_observed: Option<(i32, i32, u32, u32)>,
        transition_metrics: Option<(String, f64, f64)>,
        registered_hotkeys: Vec<String>,
        register_error: Option<(&'static str, String)>,
        signal_hotkeys: Vec<String>,
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
                reply_capabilities: true,
                capabilities_calls: 0,
                window_senders: Vec::new(),
                focused: Some((4294967301, -300, -20, 801, 602)),
                displays: vec![
                    ("monitor-0".to_string(), -1920, 0, 1920, 1080),
                    ("monitor-1".to_string(), 0, -50, 1280, 1024),
                ],
                moves: Vec::new(),
                move_error: None,
                move_observed: None,
                transition_metrics: None,
                registered_hotkeys: Vec::new(),
                register_error: None,
                signal_hotkeys: vec!["ctrl+cmd+left".to_string()],
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
                let metrics_state = Arc::clone(&state);
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
                                        let mut state = capabilities_state.lock();
                                        if !state.reply_capabilities {
                                            return Ok(Vec::new());
                                        }
                                        state.capabilities_calls += 1;
                                        state
                                            .window_senders
                                            .push(method.msg.sender().unwrap().to_string());
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
                                        let mut state = focused_state.lock();
                                        state
                                            .window_senders
                                            .push(method.msg.sender().unwrap().to_string());
                                        match &state.focused {
                                            Some((id, x, y, width, height)) => reply
                                                .append_all((true, *id, *x, *y, *width, *height)),
                                            None => reply.append_all((
                                                false, 0_u64, 0_i32, 0_i32, 0_u32, 0_u32,
                                            )),
                                        }
                                        Ok(vec![reply])
                                    }))
                                    .add_m(factory.method("GetDisplays", (), move |method| {
                                        let mut state = displays_state.lock();
                                        state
                                            .window_senders
                                            .push(method.msg.sender().unwrap().to_string());
                                        let reply = method
                                            .msg
                                            .method_return()
                                            .append1(state.displays.clone());
                                        Ok(vec![reply])
                                    }))
                                    .add_m(factory.method("MoveWindow", (), move |method| {
                                        let payload: (u64, i32, i32, u32, u32) =
                                            method.msg.read5()?;
                                        let mut state = move_state.lock();
                                        state
                                            .window_senders
                                            .push(method.msg.sender().unwrap().to_string());
                                        if let Some((x, y, width, height)) = state.move_observed
                                            && state
                                                .focused
                                                .as_ref()
                                                .is_some_and(|window| window.0 == payload.0)
                                        {
                                            state.focused = Some((payload.0, x, y, width, height));
                                        }
                                        if let Some((name, message)) = state.move_error.clone() {
                                            return Err((name, message).into());
                                        }
                                        state.moves.push(payload);
                                        if state
                                            .focused
                                            .as_ref()
                                            .is_some_and(|window| window.0 == payload.0)
                                        {
                                            state.focused = Some(payload);
                                        }
                                        state.transition_metrics = Some((
                                            method.msg.sender().unwrap().to_string(),
                                            3.0,
                                            250.0,
                                        ));
                                        Ok(vec![method.msg.method_return()])
                                    }))
                                    .add_m(factory.method(
                                        "GetLastTransitionMetrics",
                                        (),
                                        move |method| {
                                            let mut state = metrics_state.lock();
                                            let sender = method.msg.sender().unwrap().to_string();
                                            state.window_senders.push(sender.clone());
                                            match &state.transition_metrics {
                                            Some((owner, request_ms, completion_ms))
                                                if owner == &sender =>
                                            {
                                                Ok(vec![
                                                    method
                                                        .msg
                                                        .method_return()
                                                        .append2(*request_ms, *completion_ms),
                                                ])
                                            }
                                            _ => Err((
                                                "org.window_zones.Gnome.Error.InvalidWindowState",
                                                "caller has no completed transition",
                                            )
                                                .into()),
                                        }
                                        },
                                    ))
                                    .add_m(factory.method("RegisterHotkeys", (), move |method| {
                                        let hotkeys: Vec<String> = method.msg.read1()?;
                                        let (error, signal_hotkeys) = {
                                            let mut state = register_state.lock();
                                            let error = state.register_error.clone();
                                            if error.is_none() {
                                                state.registered_hotkeys = hotkeys;
                                            }
                                            (error, state.signal_hotkeys.clone())
                                        };
                                        if let Some((name, message)) = error {
                                            return Err(MethodErr::from((name, message)));
                                        }

                                        let mut replies = vec![method.msg.method_return()];
                                        for hotkey in signal_hotkeys {
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
            Some(FocusedWindow::new(
                WindowId::new("4294967301"),
                Rect::new(-300, -20, 801, 602)
            ))
        );
        assert_eq!(
            window_system.displays().unwrap(),
            vec![
                DisplayGeometry::new("monitor-0", Rect::new(-1920, 0, 1920, 1080)),
                DisplayGeometry::new("monitor-1", Rect::new(0, -50, 1280, 1024)),
            ]
        );

        window_system
            .move_window(&WindowMove::new(
                WindowId::new("4294967301"),
                Rect::new(-123, 456, 777, 888),
            ))
            .unwrap();
        bus.update_state(|state| {
            assert_eq!(state.moves, vec![(4294967301, -123, 456, 777, 888)]);
            state.focused = None;
        });
        assert_eq!(window_system.focused_window().unwrap(), None);

        let mut hotkey_system = GnomeHotkeySystem::with_bus_address(bus.address.clone());
        hotkey_system
            .register_hotkeys(&["ctrl+cmd+left".to_string(), "cmd+1".to_string()])
            .unwrap();
        bus.update_state(|state| {
            assert_eq!(
                state.registered_hotkeys,
                vec!["ctrl+cmd+left".to_string(), "cmd+1".to_string()]
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
                hotkey: "ctrl+cmd+left".to_string()
            })
        );
    }

    #[test]
    fn registration_rejection_keeps_previous_set_and_busy_is_actionable() {
        let bus = FakeBus::new(FakeServiceState::complete());
        let mut hotkey_system = GnomeHotkeySystem::with_bus_address(bus.address.clone());
        hotkey_system
            .register_hotkeys(&["ctrl+cmd+left".to_string()])
            .unwrap();
        let _ = hotkey_system.next_hotkey();

        bus.update_state(|state| {
            state.register_error = Some((
                "org.window_zones.Gnome.Error.Busy",
                "another controller owns hotkey registration".to_string(),
            ));
        });
        let error = hotkey_system
            .register_hotkeys(&["ctrl+cmd+right".to_string()])
            .unwrap_err();
        assert!(matches!(error, HotkeySystemError::Unavailable(_)));
        assert!(error.to_string().contains("busy"));
        bus.update_state(|state| {
            assert_eq!(state.registered_hotkeys, vec!["ctrl+cmd+left".to_string()]);
            state.register_error = None;
        });

        hotkey_system
            .register_hotkeys(&["ctrl+cmd+right".to_string()])
            .unwrap();
        bus.update_state(|state| {
            assert_eq!(state.registered_hotkeys, vec!["ctrl+cmd+right".to_string()]);
        });
    }

    #[test]
    fn inspection_connections_do_not_delay_hotkey_delivery() {
        let mut state = FakeServiceState::complete();
        state.signal_hotkeys.clear();
        let bus = FakeBus::new(state);
        let mut hotkey_system = GnomeHotkeySystem::with_bus_address(bus.address.clone());
        hotkey_system
            .register_hotkeys(&["ctrl+alt+left".to_string()])
            .unwrap();
        let connection = hotkey_system.connection.as_ref().unwrap();
        get_capabilities(connection).unwrap();
        while connection.process(Duration::from_millis(0)).unwrap() {}

        let window_system = GnomeWindowSystem::with_bus_address(bus.address.clone());
        for _ in 0..8 {
            window_system.capabilities().unwrap();
        }
        bus.update_state(|state| {
            state.signal_hotkeys = vec!["ctrl+alt+left".to_string()];
        });
        hotkey_system
            .register_hotkeys(&["ctrl+alt+left".to_string()])
            .unwrap();
        // The reply follows the signal on this connection, making the poll deterministic.
        get_capabilities(hotkey_system.connection.as_ref().unwrap()).unwrap();

        assert_eq!(
            hotkey_system.next_hotkey().unwrap(),
            Some(HotkeyEvent::Pressed {
                hotkey: "ctrl+alt+left".to_string()
            })
        );
    }

    #[test]
    fn companion_disconnect_reconnects_and_re_registers() {
        let mut bus = FakeBus::new(FakeServiceState::complete());
        let mut hotkey_system = GnomeHotkeySystem::with_bus_address(bus.address.clone());
        hotkey_system
            .register_hotkeys(&["ctrl+cmd+left".to_string()])
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
            .register_hotkeys(&["ctrl+cmd+left".to_string()])
            .unwrap();
        bus.update_state(|state| {
            assert_eq!(state.registered_hotkeys, vec!["ctrl+cmd+left".to_string()]);
        });
    }

    #[test]
    fn connection_reset_discards_queued_events() {
        let mut hotkey_system = GnomeHotkeySystem::with_bus_address("unused");
        hotkey_system.events.lock().push_back(HotkeyEvent::Pressed {
            hotkey: "ctrl+cmd+left".to_string(),
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
                "org.window_zones.Gnome.Error.Unavailable",
                GnomeIntegrationError::Unavailable {
                    message: "companion disabled during transition".to_string(),
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
            state.reply_capabilities = false;
        });

        let result = window_system.capabilities();
        assert!(
            matches!(result, Err(GnomeIntegrationError::Unavailable { .. })),
            "{result:?}"
        );
    }

    #[test]
    fn incompatible_and_denied_service_responses_are_exposed() {
        let bus = FakeBus::new(FakeServiceState::complete());
        let window_system = GnomeWindowSystem::with_bus_address(bus.address.clone());

        bus.update_state(|state| {
            state.capabilities_error = Some((
                "org.freedesktop.DBus.Error.UnknownInterface",
                "GNOME2 is not available".to_string(),
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

    #[test]
    fn capability_refresh_observes_live_errors_replacement_and_recovery() {
        let mut bus = FakeBus::new(FakeServiceState::complete());
        let mut system = GnomeWindowSystem::with_bus_address(bus.address.clone());
        assert_eq!(
            system.capabilities().unwrap(),
            FakeServiceState::complete().capabilities
        );
        bus.update_state(|state| {
            assert_eq!(
                state.capabilities_calls, 1,
                "initial refresh makes one request"
            );
            state.capabilities_error = Some((
                "org.freedesktop.DBus.Error.AccessDenied",
                "session policy denied inspection".to_string(),
            ));
        });
        // Action paths retain the established handshake until an explicit refresh.
        let focused = system.focused_window().unwrap().unwrap();
        system.displays().unwrap();
        let window_move = WindowMove::new(focused.id, Rect::new(1, 2, 300, 400));
        system.move_window(&window_move).unwrap();
        assert!(matches!(
            system.capabilities(),
            Err(GnomeIntegrationError::Denied { .. })
        ));

        bus.update_state(|state| {
            state.capabilities_error = None;
        });
        assert_eq!(
            system.capabilities().unwrap(),
            FakeServiceState::complete().capabilities
        );
        bus.stop_companion();
        assert!(matches!(
            system.capabilities(),
            Err(GnomeIntegrationError::Unavailable { .. })
        ));
        bus.start_companion();
        assert_eq!(
            system.capabilities().unwrap(),
            FakeServiceState::complete().capabilities
        );

        // A replacement must also be observed without probing during its outage.
        bus.stop_companion();
        bus.update_state(|state| {
            state.capabilities = vec![DISPLAYS_CAPABILITY.to_string()];
        });
        bus.start_companion();
        assert_eq!(
            system.capabilities().unwrap(),
            vec![DISPLAYS_CAPABILITY.to_string()]
        );
        assert!(
            system
                .move_window(&window_move)
                .unwrap_err()
                .to_string()
                .contains("does not support move-resize")
        );
        assert_eq!(system.displays().unwrap().len(), 2);
    }

    #[test]
    fn transition_metrics_use_the_cached_move_sender_once_and_clear_after_failure() {
        let mut state = FakeServiceState::complete();
        state
            .capabilities
            .push(TRANSITION_METRICS_CAPABILITY.to_string());
        let bus = FakeBus::new(state);
        let mut system = GnomeWindowSystem::with_bus_address(bus.address.clone());
        assert!(system.transition_metrics().unwrap().is_none());
        let focused = system.focused_window().unwrap().unwrap();
        let window_move = WindowMove::new(focused.id.clone(), Rect::new(1, 2, 300, 400));
        system.move_window(&window_move).unwrap();
        let metrics = system.transition_metrics().unwrap().unwrap();
        assert_eq!((metrics.request_ms, metrics.completion_ms), (3.0, 250.0));
        assert!(system.transition_metrics().unwrap().is_none());
        system.move_window(&window_move).unwrap();
        let invalid_move = WindowMove::new(WindowId::new("not-a-number"), window_move.target);
        assert!(system.move_window(&invalid_move).is_err());
        assert!(system.transition_metrics().unwrap().is_none());
        system.move_window(&window_move).unwrap();
        bus.update_state(|state| {
            assert!(
                state
                    .window_senders
                    .iter()
                    .all(|sender| sender == &state.window_senders[0])
            );
            state.move_error = Some((
                "org.window_zones.Gnome.Error.WindowGone",
                "window was closed".to_string(),
            ));
        });
        assert_eq!(
            system.move_window(&window_move),
            Err(WindowSystemError::WindowGone(focused.id))
        );
        assert!(system.transition_metrics().unwrap().is_none());
    }

    #[test]
    fn next_and_previous_display_noops_discard_unread_previous_metrics() {
        let mut state = FakeServiceState::complete();
        state
            .capabilities
            .push(TRANSITION_METRICS_CAPABILITY.to_string());
        state.displays.truncate(1);
        let bus = FakeBus::new(state);
        let mut system = GnomeWindowSystem::with_bus_address(bus.address.clone());
        let window_move =
            WindowMove::new(WindowId::new("4294967301"), Rect::new(-1900, 20, 300, 400));
        let mut history = crate::WindowHistory::default();
        for action in [
            crate::Action::MoveToNextDisplay {},
            crate::Action::MoveToPreviousDisplay {},
        ] {
            system.move_window(&window_move).unwrap();
            crate::execute_action(
                &action,
                &std::collections::BTreeMap::new(),
                &mut history,
                &mut system,
            )
            .unwrap();
            assert!(system.transition_metrics().unwrap().is_none());
        }
        bus.update_state(|state| {
            assert_eq!(
                state.moves,
                vec![
                    (4294967301, -1900, 20, 300, 400),
                    (4294967301, -1900, 20, 300, 400),
                ],
                "single-display actions do not move the window"
            );
        });
    }

    #[test]
    fn window_gone_maps_to_requested_identity_without_resetting_cache() {
        let bus = FakeBus::new(FakeServiceState::complete());
        let mut system = GnomeWindowSystem::with_bus_address(bus.address.clone());
        let focused = system.focused_window().unwrap().unwrap();
        bus.update_state(|state| {
            state.move_error = Some((
                "org.window_zones.Gnome.Error.WindowGone",
                "window was closed".to_string(),
            ));
        });
        assert_eq!(
            system.move_window(&WindowMove::new(
                focused.id.clone(),
                Rect::new(1, 2, 300, 400)
            )),
            Err(WindowSystemError::WindowGone(focused.id)),
        );
        system.displays().unwrap();
        bus.update_state(|state| {
            assert_eq!(state.capabilities_calls, 1);
            assert!(state.moves.is_empty());
            assert!(
                state
                    .window_senders
                    .iter()
                    .all(|sender| sender == &state.window_senders[0])
            );
        });
    }

    fn move_test_app() -> crate::App {
        let path = std::env::temp_dir().join(format!(
            "window-zones-gnome-moves-{}-{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(
            &path,
            r#"
[[bindings]]
hotkey = "ctrl+cmd+left"
action = { type = "move-to-zone", zone = "left-half" }
[[bindings]]
hotkey = "ctrl+cmd+r"
action = { type = "restore" }
"#,
        )
        .unwrap();
        let app = crate::App::start_at(&path);
        std::fs::remove_file(path).unwrap();
        assert!(matches!(app.config_state(), crate::ConfigState::Loaded));
        app
    }

    #[test]
    fn app_preserves_restore_after_partial_placement_and_partial_restore() {
        use crate::{DispatchHotkeyError, DispatchState, ExecuteActionError};

        for initial_failure in [true, false] {
            let bus = FakeBus::new(FakeServiceState::complete());
            let mut system = GnomeWindowSystem::with_bus_address(bus.address.clone());
            let mut app = move_test_app();
            let original = system.focused_window().unwrap().unwrap();
            let constrained = Rect::new(0, -50, 900, 1024);
            if initial_failure {
                bus.update_state(|state| {
                    state.move_observed = Some((constrained.x, constrained.y, constrained.width, constrained.height));
                    state.move_error = Some((
                        "org.window_zones.Gnome.Error.PartialMove",
                        serde_json::json!({
                            "window_id": original.id.as_str(),
                            "observed": [constrained.x, constrained.y, constrained.width as i64, constrained.height as i64],
                            "message": "minimum width constrained placement",
                        }).to_string(),
                    ));
                });
            }
            let dispatch = app.dispatch_hotkey("ctrl+cmd+left", &mut system);
            if initial_failure {
                assert!(matches!(
                    dispatch,
                    DispatchState::Error(DispatchHotkeyError::ExecuteAction(
                        ExecuteActionError::WindowSystem(WindowSystemError::PartialMove { observed, .. })
                    )) if *observed == constrained
                ));
            } else {
                assert_eq!(dispatch, &DispatchState::Succeeded);
            }

            let partial_restore = Rect::new(-250, -10, 850, 650);
            bus.update_state(|state| {
                state.move_observed = Some((partial_restore.x, partial_restore.y, partial_restore.width, partial_restore.height));
                state.move_error = Some((
                    "org.window_zones.Gnome.Error.PartialMove",
                    serde_json::json!({
                        "window_id": original.id.as_str(),
                        "observed": [partial_restore.x, partial_restore.y, partial_restore.width as i64, partial_restore.height as i64],
                        "message": "restore interrupted",
                    }).to_string(),
                ));
            });
            assert!(matches!(
                app.dispatch_hotkey("ctrl+cmd+r", &mut system),
                DispatchState::Error(DispatchHotkeyError::ExecuteAction(
                    ExecuteActionError::WindowSystem(WindowSystemError::PartialMove { observed, .. })
                )) if *observed == partial_restore
            ));
            bus.update_state(|state| {
                state.move_error = None;
                state.move_observed = None;
            });
            assert_eq!(
                app.dispatch_hotkey("ctrl+cmd+r", &mut system),
                &DispatchState::Succeeded
            );
            assert_eq!(system.focused_window().unwrap().unwrap(), original);
            assert!(matches!(
                app.dispatch_hotkey("ctrl+cmd+r", &mut system),
                DispatchState::Error(DispatchHotkeyError::ExecuteAction(
                    ExecuteActionError::NothingToRestore
                ))
            ));
        }
    }

    #[test]
    fn app_does_not_record_refusal_and_discards_closed_window_history() {
        use crate::{DispatchHotkeyError, DispatchState, ExecuteActionError};

        let bus = FakeBus::new(FakeServiceState::complete());
        let mut system = GnomeWindowSystem::with_bus_address(bus.address.clone());
        let mut app = move_test_app();
        let original = system.focused_window().unwrap().unwrap();
        bus.update_state(|state| {
            state.move_error = Some((
                "org.window_zones.Gnome.Error.InvalidWindowState",
                "window refused placement without changing frame".into(),
            ));
        });
        assert!(matches!(
            app.dispatch_hotkey("ctrl+cmd+left", &mut system),
            DispatchState::Error(DispatchHotkeyError::ExecuteAction(
                ExecuteActionError::WindowSystem(WindowSystemError::Platform(_))
            ))
        ));
        assert_eq!(system.focused_window().unwrap().unwrap(), original);
        assert!(matches!(
            app.dispatch_hotkey("ctrl+cmd+r", &mut system),
            DispatchState::Error(DispatchHotkeyError::ExecuteAction(
                ExecuteActionError::NothingToRestore
            ))
        ));
        bus.update_state(|state| state.move_error = None);
        assert_eq!(
            app.dispatch_hotkey("ctrl+cmd+left", &mut system),
            &DispatchState::Succeeded
        );
        bus.update_state(|state| {
            state.move_error = Some((
                "org.window_zones.Gnome.Error.WindowGone",
                "window closed during restore".into(),
            ));
        });
        assert!(matches!(
            app.dispatch_hotkey("ctrl+cmd+r", &mut system),
            DispatchState::Error(DispatchHotkeyError::ExecuteAction(
                ExecuteActionError::WindowSystem(WindowSystemError::WindowGone(id))
            )) if id == &original.id
        ));
        bus.update_state(|state| state.move_error = None);
        assert!(matches!(
            app.dispatch_hotkey("ctrl+cmd+r", &mut system),
            DispatchState::Error(DispatchHotkeyError::ExecuteAction(
                ExecuteActionError::NothingToRestore
            ))
        ));
    }

    #[test]
    fn partial_move_payload_is_validated_on_the_private_bus() {
        let bus = FakeBus::new(FakeServiceState::complete());
        let mut system = GnomeWindowSystem::with_bus_address(bus.address.clone());
        let requested = WindowMove::new(WindowId::new("4294967301"), Rect::new(10, 20, 600, 700));
        let valid = serde_json::json!({
            "window_id": "4294967301",
            "observed": [-100, -20, 900, 700],
            "message": "placement interrupted",
        });
        bus.update_state(|state| {
            state.focused = Some((99, 1, 2, 3, 4));
            state.move_error = Some((
                "org.window_zones.Gnome.Error.PartialMove",
                valid.to_string(),
            ));
        });
        let error = system.move_window(&requested).unwrap_err();
        assert!(matches!(
            &error,
            WindowSystemError::PartialMove { observed, .. } if *observed == Rect::new(-100, -20, 900, 700)
        ));
        assert!(error.to_string().contains("placement interrupted"));
        assert!(system.transition_metrics().unwrap().is_none());

        let mut malformed = vec!["not JSON".to_string(), "{}".to_string()];
        for (field, value) in [
            ("window_id", serde_json::json!("99")),
            ("window_id", serde_json::json!(4294967301_u64)),
            ("observed", serde_json::json!([-100, -20, 0, 700])),
            ("observed", serde_json::json!([-100, -20, -1, 700])),
            (
                "observed",
                serde_json::json!([-100, -20, 4294967296_u64, 700]),
            ),
            (
                "observed",
                serde_json::json!([2147483648_i64, -20, 900, 700]),
            ),
            ("observed", serde_json::json!([-100.5, -20, 900, 700])),
            ("observed", serde_json::json!([-100, -20, 900])),
            ("observed", serde_json::json!([-100, -20, 900, 700, 1])),
            ("message", serde_json::Value::Null),
            ("unexpected", serde_json::json!(true)),
        ] {
            let mut payload = valid.clone();
            payload[field] = value;
            malformed.push(payload.to_string());
        }
        for payload in malformed {
            bus.update_state(|state| {
                state.move_error =
                    Some(("org.window_zones.Gnome.Error.PartialMove", payload.clone()));
            });
            assert!(
                matches!(
                    system.move_window(&requested),
                    Err(WindowSystemError::Platform(_))
                ),
                "malformed or foreign-window payload must not invent geometry: {payload}"
            );
        }
        bus.update_state(|state| {
            state.move_error = Some(("org.window_zones.Gnome.Error.WindowGone", valid.to_string()));
        });
        assert_eq!(
            system.move_window(&requested),
            Err(WindowSystemError::WindowGone(requested.window))
        );
    }

    #[test]
    fn window_adapter_reconnects_after_service_loss() {
        let mut bus = FakeBus::new(FakeServiceState::complete());
        let system = GnomeWindowSystem::with_bus_address(bus.address.clone());
        system.focused_window().unwrap();
        bus.stop_companion();
        assert!(system.displays().is_err());
        bus.start_companion();
        system.focused_window().unwrap();
        bus.update_state(|state| {
            assert_eq!(state.capabilities_calls, 2);
            assert_ne!(
                state.window_senders[0],
                state.window_senders.last().unwrap().as_str()
            );
        });
    }

    #[test]
    fn hotkey_adapter_caches_capabilities_and_drains_pending_bus_events() {
        let bus = FakeBus::new(FakeServiceState::complete());
        let mut system = GnomeHotkeySystem::with_bus_address(bus.address.clone());
        bus.update_state(|state| {
            state.signal_hotkeys = vec!["ctrl+cmd+left".to_string(), "ctrl+cmd+right".to_string()];
        });
        system
            .register_hotkeys(&["ctrl+cmd+left".to_string(), "ctrl+cmd+right".to_string()])
            .unwrap();
        thread::sleep(Duration::from_millis(25));
        assert_eq!(
            system.next_hotkey().unwrap(),
            Some(HotkeyEvent::Pressed {
                hotkey: "ctrl+cmd+left".to_string(),
            })
        );
        assert_eq!(
            system.events.lock().len(),
            1,
            "one call drains transport before popping"
        );
        assert_eq!(
            system.next_hotkey().unwrap(),
            Some(HotkeyEvent::Pressed {
                hotkey: "ctrl+cmd+right".to_string(),
            })
        );
        assert_eq!(system.next_hotkey().unwrap(), None);
        system.register_hotkeys(&[]).unwrap();
        bus.update_state(|state| assert_eq!(state.capabilities_calls, 1));
    }

    #[test]
    fn unsupported_hotkey_set_is_rejected_and_previous_set_survives() {
        let bus = FakeBus::new(FakeServiceState::complete());
        let mut system = GnomeHotkeySystem::with_bus_address(bus.address.clone());
        system
            .register_hotkeys(&["ctrl+cmd+left".to_string()])
            .unwrap();
        bus.update_state(|state| {
            state.register_error = Some((
                "org.window_zones.Gnome.Error.Unsupported",
                "non-canonical hotkey 'Ctrl+Left'".to_string(),
            ));
        });
        let error = system
            .register_hotkeys(&["Ctrl+Left".to_string()])
            .unwrap_err();
        assert!(matches!(error, HotkeySystemError::Rejected(_)));
        bus.update_state(|state| {
            assert_eq!(state.registered_hotkeys, vec!["ctrl+cmd+left".to_string()]);
            assert_eq!(state.capabilities_calls, 1);
        });
    }
}
