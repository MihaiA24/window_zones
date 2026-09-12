use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use dbus::arg::{AppendAll, ReadAll};
use dbus::blocking::{Connection, LocalConnection};
use dbus::message::MatchRule;
use dbus_tree::{Factory, MethodErr};
use parking_lot::Mutex;
use thiserror::Error;

use crate::{
    DisplayGeometry, FocusedWindow, HotkeyEvent, HotkeySystem, HotkeySystemError, Rect, WindowMove,
    WindowSystem, WindowSystemError,
};

pub const KWIN_SERVICE_NAME: &str = "org.window_zones.KWin";
pub const KWIN_OBJECT_PATH: &str = "/org/window_zones/KWin";
pub const KWIN_INTERFACE: &str = "org.window_zones.KWin1";
pub const KWIN_PROTOCOL_MAJOR: u32 = 1;

const CALL_TIMEOUT: Duration = Duration::from_secs(1);
const COMPANION_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);
const FOCUSED_WINDOW_CAPABILITY: &str = "focused-window";
const DISPLAYS_CAPABILITY: &str = "displays";
const MOVE_RESIZE_CAPABILITY: &str = "move-resize";
const HOTKEYS_CAPABILITY: &str = "hotkeys";
const REQUEST_MOVE: &str = "move";
const REQUEST_REGISTER_HOTKEYS: &str = "register-hotkeys";
// KWin hosts KGlobalAccel in-process and registers script actions under the "kwin" component.
const KGLOBALACCEL_SERVICE: &str = "org.kde.kglobalaccel";
const KGLOBALACCEL_PATH: &str = "/kglobalaccel";
const KGLOBALACCEL_INTERFACE: &str = "org.kde.KGlobalAccel";
const KGLOBALACCEL_COMPONENT: &str = "kwin";
const ACCELERATOR_SETTLE_SAMPLES: u32 = 6;
const ACCELERATOR_SETTLE_INTERVAL: Duration = Duration::from_millis(120);

type DisplayPayload = (String, i32, i32, u32, u32);
type NextRequestPayload = (u32, String, i32, i32, u32, u32, String);

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum KwinIntegrationError {
    #[error(
        "KWin companion is unavailable: {message}; install and enable the KWin script and start the companion service, then retry"
    )]
    Unavailable { message: String },
    #[error(
        "KWin companion is incompatible: {message}; upgrade the KWin script and companion service together"
    )]
    Incompatible { message: String },
    #[error(
        "KWin companion denied the request: {message}; check the user-session D-Bus and KWin script permissions"
    )]
    Denied { message: String },
    #[error(
        "KWin companion does not support {capability}: {message}; install a companion exposing that capability"
    )]
    Unsupported { capability: String, message: String },
    #[error("KWin companion is busy: {message}; release the other App controller before retrying")]
    Busy { message: String },
    #[error(
        "KWin companion rejected the request: {message}; restore an eligible focused window or correct the request"
    )]
    Invalid { message: String },
    #[error("KWin companion operation failed: {message}; retry the operation")]
    Operation { message: String },
}

impl KwinIntegrationError {
    fn should_reconnect(&self) -> bool {
        matches!(self, Self::Unavailable { .. } | Self::Incompatible { .. })
    }
}

pub struct KwinWindowSystem {
    bus_address: Option<String>,
    connection: Mutex<Option<Connection>>,
}

impl fmt::Debug for KwinWindowSystem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KwinWindowSystem")
            .field("bus_address", &self.bus_address)
            .field("connection_established", &self.connection.lock().is_some())
            .finish()
    }
}

impl KwinWindowSystem {
    pub fn new() -> Self {
        Self {
            bus_address: None,
            connection: Mutex::new(None),
        }
    }

    pub fn capabilities(&self) -> Result<Vec<String>, KwinIntegrationError> {
        let mut connection_slot = self.connection.lock();
        if connection_slot.is_none() {
            *connection_slot = Some(connect(self.bus_address.as_deref())?);
        }
        let connection =
            connection_slot
                .as_ref()
                .ok_or_else(|| KwinIntegrationError::Unavailable {
                    message: "D-Bus connection was not established".to_string(),
                })?;
        let result = get_capabilities(connection);
        if let Err(error) = &result
            && error.should_reconnect()
        {
            *connection_slot = None;
        }
        result
    }

    #[cfg(test)]
    fn with_bus_address(bus_address: impl Into<String>) -> Self {
        Self {
            bus_address: Some(bus_address.into()),
            connection: Mutex::new(None),
        }
    }

    fn call_with_capability<R, A>(
        &self,
        capability: &str,
        method: &str,
        args: A,
    ) -> Result<R, KwinIntegrationError>
    where
        R: ReadAll,
        A: AppendAll,
    {
        let mut connection_slot = self.connection.lock();
        if connection_slot.is_none() {
            *connection_slot = Some(connect(self.bus_address.as_deref())?);
        }
        let connection =
            connection_slot
                .as_ref()
                .ok_or_else(|| KwinIntegrationError::Unavailable {
                    message: "D-Bus connection was not established".to_string(),
                })?;
        let result = (|| {
            let capabilities = get_capabilities(connection)?;
            require_capability(&capabilities, capability)?;

            let proxy = connection.with_proxy(KWIN_SERVICE_NAME, KWIN_OBJECT_PATH, CALL_TIMEOUT);
            proxy
                .method_call(KWIN_INTERFACE, method, args)
                .map_err(classify_dbus_error)
        })();
        if let Err(error) = &result
            && error.should_reconnect()
        {
            *connection_slot = None;
        }
        result
    }
}

impl Default for KwinWindowSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowSystem for KwinWindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
        let result: Result<(bool, String, i32, i32, u32, u32), KwinIntegrationError> =
            self.call_with_capability(FOCUSED_WINDOW_CAPABILITY, "GetFocusedWindow", ());

        result
            .map(|(present, _display_id, x, y, width, height)| {
                present.then(|| FocusedWindow::new(Rect::new(x, y, width, height)))
            })
            .map_err(|error| WindowSystemError::Platform(error.to_string()))
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        let result: Result<(Vec<DisplayPayload>,), KwinIntegrationError> =
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
        let result: Result<(), KwinIntegrationError> = self.request_and_wait(
            MOVE_RESIZE_CAPABILITY,
            "MoveFocusedWindow",
            (target.x, target.y, target.width, target.height),
        );

        result.map_err(|error| WindowSystemError::Platform(error.to_string()))
    }
}

impl KwinWindowSystem {
    fn request_and_wait<A>(
        &self,
        capability: &str,
        method: &str,
        args: A,
    ) -> Result<(), KwinIntegrationError>
    where
        A: AppendAll,
    {
        let (request_id,): (u32,) = self.call_with_capability(capability, method, args)?;

        let mut connection_slot = self.connection.lock();
        let connection =
            connection_slot
                .as_ref()
                .ok_or_else(|| KwinIntegrationError::Unavailable {
                    message: "D-Bus connection was not established".to_string(),
                })?;
        let result = wait_for_request(connection, request_id);
        if let Err(error) = &result
            && error.should_reconnect()
        {
            *connection_slot = None;
        }
        result
    }
}

pub struct KwinHotkeySystem {
    bus_address: Option<String>,
    connection: Option<Connection>,
}

impl fmt::Debug for KwinHotkeySystem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("KwinHotkeySystem")
            .field("bus_address", &self.bus_address)
            .field("connection_established", &self.connection.is_some())
            .finish()
    }
}

impl KwinHotkeySystem {
    pub fn new() -> Self {
        Self {
            bus_address: None,
            connection: None,
        }
    }

    #[cfg(test)]
    fn with_bus_address(bus_address: impl Into<String>) -> Self {
        Self {
            bus_address: Some(bus_address.into()),
            connection: None,
        }
    }

    fn connect(&mut self) -> Result<(), KwinIntegrationError> {
        self.connection = Some(connect(self.bus_address.as_deref())?);
        Ok(())
    }

    fn ensure_connection(&mut self) -> Result<&Connection, KwinIntegrationError> {
        if self.connection.is_none() {
            self.connect()?;
        }

        self.connection
            .as_ref()
            .ok_or_else(|| KwinIntegrationError::Unavailable {
                message: "D-Bus connection was not established".to_string(),
            })
    }

    fn reset_connection(&mut self) {
        self.connection = None;
    }

    fn call_register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), KwinIntegrationError> {
        let result = (|| {
            let connection = self.ensure_connection()?;
            let capabilities = get_capabilities(connection)?;
            require_capability(&capabilities, HOTKEYS_CAPABILITY)?;

            let payload =
                serde_json::to_string(hotkeys).map_err(|_| KwinIntegrationError::Invalid {
                    message: "failed to encode hotkey request payload".to_string(),
                })?;
            let proxy = connection.with_proxy(KWIN_SERVICE_NAME, KWIN_OBJECT_PATH, CALL_TIMEOUT);
            let (request_id,): (u32,) = proxy
                .method_call(KWIN_INTERFACE, "RegisterHotkeys", (payload,))
                .map_err(classify_dbus_error)?;
            wait_for_request(connection, request_id)?;
            // The script preflights replacements before committing; this post-check only reads.
            verify_accelerators_assigned(connection, hotkeys)
        })();

        if let Err(error) = &result
            && error.should_reconnect()
        {
            self.reset_connection();
        }

        result
    }
}

impl Default for KwinHotkeySystem {
    fn default() -> Self {
        Self::new()
    }
}

impl HotkeySystem for KwinHotkeySystem {
    fn register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), HotkeySystemError> {
        self.call_register_hotkeys(hotkeys)
            .map_err(|error| HotkeySystemError::Platform(error.to_string()))
    }

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
        let result = (|| {
            let connection = self
                .ensure_connection()
                .map_err(|error| HotkeySystemError::Platform(error.to_string()))?;
            let proxy = connection.with_proxy(KWIN_SERVICE_NAME, KWIN_OBJECT_PATH, CALL_TIMEOUT);
            let (present, hotkey): (bool, String) = proxy
                .method_call(KWIN_INTERFACE, "GetNextHotkey", ())
                .map_err(|error| {
                    let classified = classify_dbus_error(error);
                    HotkeySystemError::Platform(classified.to_string())
                })?;
            Ok(present.then_some(HotkeyEvent::Pressed { hotkey }))
        })();

        if result.is_err() {
            self.reset_connection();
        }

        result
    }
}

fn connect(bus_address: Option<&str>) -> Result<Connection, KwinIntegrationError> {
    let result = match bus_address {
        Some(address) => Connection::new_address(address),
        None => Connection::new_session(),
    };

    result.map_err(|error| KwinIntegrationError::Unavailable {
        message: format!("failed to connect to the user session bus: {error}"),
    })
}

fn get_capabilities(connection: &Connection) -> Result<Vec<String>, KwinIntegrationError> {
    let proxy = connection.with_proxy(KWIN_SERVICE_NAME, KWIN_OBJECT_PATH, CALL_TIMEOUT);
    proxy
        .method_call(KWIN_INTERFACE, "GetCapabilities", ())
        .map(|(capabilities,): (Vec<String>,)| capabilities)
        .map_err(classify_dbus_error)
}

fn require_capability(
    capabilities: &[String],
    capability: &str,
) -> Result<(), KwinIntegrationError> {
    if capabilities.iter().any(|value| value == capability) {
        return Ok(());
    }

    Err(KwinIntegrationError::Unsupported {
        capability: capability.to_string(),
        message: "install a companion version exposing the required capability".to_string(),
    })
}

fn wait_for_request(connection: &Connection, request_id: u32) -> Result<(), KwinIntegrationError> {
    let deadline = Instant::now() + CALL_TIMEOUT;
    let proxy = connection.with_proxy(KWIN_SERVICE_NAME, KWIN_OBJECT_PATH, CALL_TIMEOUT);

    loop {
        let (complete, message): (bool, String) = proxy
            .method_call(
                KWIN_INTERFACE,
                "GetRequestResult",
                (request_id.to_string(),),
            )
            .map_err(classify_dbus_error)?;
        if complete {
            return if message.is_empty() {
                Ok(())
            } else {
                Err(KwinIntegrationError::Operation { message })
            };
        }

        if Instant::now() >= deadline {
            return Err(KwinIntegrationError::Unavailable {
                message: format!("timed out waiting for KWin request {request_id}"),
            });
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// KWin's scripting `registerShortcut` reports success for any callable handler: it hands the
/// sequence to KGlobalAccel and discards the result, so an accelerator another action already owns
/// is accepted and then never fires. Ask KGlobalAccel which keys our actions actually hold.
fn verify_accelerators_assigned(
    connection: &Connection,
    hotkeys: &[String],
) -> Result<(), KwinIntegrationError> {
    if hotkeys.is_empty() {
        return Ok(());
    }

    let accel = connection.with_proxy(KGLOBALACCEL_SERVICE, KGLOBALACCEL_PATH, CALL_TIMEOUT);
    // Without a reachable KGlobalAccel nothing can be verified and the Companion's own result
    // stands; with one, an action it does not know about is an accelerator that was not assigned.
    if accel
        .method_call::<(Vec<Vec<String>>,), _, _, _>(
            KGLOBALACCEL_INTERFACE,
            "allMainComponents",
            (),
        )
        .is_err()
    {
        return Ok(());
    }

    for hotkey in hotkeys {
        let action = format!("Window Zones Hotkey {hotkey}");
        let action_id = vec![
            KGLOBALACCEL_COMPONENT.to_string(),
            action.clone(),
            String::new(),
            String::new(),
        ];
        // KGlobalAccel answers with requested keys while registration is still in flight.
        // Its action query selects the dispatch winner by registration serial, unlike the
        // unordered getGlobalShortcutsByKey list. A later competitor does not steal our binding.
        let mut competitor = None;
        for attempt in 0..ACCELERATOR_SETTLE_SAMPLES {
            if attempt > 0 {
                thread::sleep(ACCELERATOR_SETTLE_INTERVAL);
            }
            let keys = accel
                .method_call::<(Vec<(Vec<i32>,)>,), _, _, _>(
                    KGLOBALACCEL_INTERFACE,
                    "shortcutKeys",
                    (action_id.clone(),),
                )
                .map(|(keys,)| keys)
                .unwrap_or_default();
            let key = keys
                .iter()
                .flat_map(|(sequence,)| sequence.iter())
                .copied()
                .find(|key| *key != 0);
            let Some(key) = key else {
                competitor = Some(String::new());
                break;
            };
            let winner = accel
                .method_call::<(Vec<String>,), _, _, _>(KGLOBALACCEL_INTERFACE, "action", (key,))
                .map(|(winner,)| winner)
                .unwrap_or_default();
            if winner.first().map(String::as_str) != Some(KGLOBALACCEL_COMPONENT)
                || winner.get(1) != Some(&action)
            {
                competitor = Some(winner.get(1).cloned().unwrap_or_default());
                break;
            }
        }
        if let Some(competitor) = competitor {
            return Err(KwinIntegrationError::Operation {
                message: if competitor.is_empty() {
                    format!(
                        "KWin accepted '{hotkey}' but KGlobalAccel assigned it no keys; another shortcut already owns that accelerator"
                    )
                } else {
                    format!(
                        "KWin accepted '{hotkey}' but the accelerator is also held by '{competitor}', which receives it instead; choose a different binding"
                    )
                },
            });
        }
    }

    Ok(())
}

fn classify_dbus_error(error: dbus::Error) -> KwinIntegrationError {
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
            KwinIntegrationError::Unavailable { message }
        }
        "org.freedesktop.DBus.Error.UnknownInterface"
        | "org.freedesktop.DBus.Error.UnknownMethod"
        | "org.window_zones.KWin.Error.Incompatible" => {
            KwinIntegrationError::Incompatible { message }
        }
        "org.freedesktop.DBus.Error.AccessDenied" | "org.window_zones.KWin.Error.Denied" => {
            KwinIntegrationError::Denied { message }
        }
        "org.freedesktop.DBus.Error.InvalidArgs" | "org.window_zones.KWin.Error.Invalid" => {
            KwinIntegrationError::Invalid { message }
        }
        "org.window_zones.KWin.Error.Unavailable" => KwinIntegrationError::Unavailable { message },
        "org.window_zones.KWin.Error.Unsupported" => KwinIntegrationError::Unsupported {
            capability: "requested operation".to_string(),
            message,
        },
        "org.window_zones.KWin.Error.Busy" => KwinIntegrationError::Busy { message },
        _ => KwinIntegrationError::Operation { message },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CompanionRequest {
    Move {
        id: u32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
    RegisterHotkeys {
        id: u32,
        hotkeys: Vec<String>,
    },
}

impl CompanionRequest {
    fn id(&self) -> u32 {
        match self {
            Self::Move { id, .. } | Self::RegisterHotkeys { id, .. } => *id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestResult {
    complete: bool,
    message: String,
}

#[derive(Debug)]
struct CompanionState {
    script_ready: bool,
    script_sender: Option<String>,
    controller_sender: Option<String>,
    last_seen: Option<Instant>,
    focused: Option<(String, i32, i32, u32, u32)>,
    displays: Vec<DisplayPayload>,
    requests: VecDeque<CompanionRequest>,
    claimed_requests: HashSet<u32>,
    results: HashMap<u32, RequestResult>,
    next_request_id: u32,
    registered_hotkeys: Vec<String>,
    events: VecDeque<String>,
}

impl Default for CompanionState {
    fn default() -> Self {
        Self {
            script_ready: false,
            script_sender: None,
            controller_sender: None,
            last_seen: None,
            focused: None,
            displays: Vec::new(),
            requests: VecDeque::new(),
            claimed_requests: HashSet::new(),
            results: HashMap::new(),
            next_request_id: 1,
            registered_hotkeys: Vec::new(),
            events: VecDeque::new(),
        }
    }
}

impl CompanionState {
    fn mark_ready(
        &mut self,
        protocol_major: u32,
        script_sender: String,
        reset_state: bool,
    ) -> Result<(), MethodErr> {
        if protocol_major != KWIN_PROTOCOL_MAJOR {
            return Err(method_error(
                "org.window_zones.KWin.Error.Incompatible",
                "unsupported KWin companion protocol major",
            ));
        }
        if !reset_state
            && self.script_ready
            && self.script_sender.as_deref() == Some(script_sender.as_str())
        {
            self.touch();
            return Ok(());
        }

        let previous_controller = self.controller_sender.clone();
        let previous_hotkeys = self.registered_hotkeys.clone();
        self.script_ready = true;
        self.script_sender = Some(script_sender);
        self.controller_sender = previous_controller;
        self.last_seen = Some(Instant::now());
        self.focused = None;
        self.displays.clear();
        self.requests.clear();
        self.claimed_requests.clear();
        self.results.clear();
        self.registered_hotkeys.clear();
        self.events.clear();
        if !previous_hotkeys.is_empty() {
            self.enqueue_hotkeys(previous_hotkeys);
        }
        Ok(())
    }

    fn ensure_ready(&mut self) -> Result<(), MethodErr> {
        if self.script_ready
            && self
                .last_seen
                .is_some_and(|last_seen| last_seen.elapsed() <= COMPANION_HEARTBEAT_TIMEOUT)
        {
            return Ok(());
        }

        self.script_ready = false;
        self.script_sender = None;
        self.requests.clear();
        self.claimed_requests.clear();
        self.results.clear();
        self.registered_hotkeys.clear();
        self.events.clear();
        Err(method_error(
            "org.window_zones.KWin.Error.Unavailable",
            "KWin script is not connected; enable the Window Zones KWin script",
        ))
    }

    fn ensure_script(&mut self, sender: &str) -> Result<(), MethodErr> {
        self.ensure_ready()?;
        if self.script_sender.as_deref() == Some(sender) {
            return Ok(());
        }

        Err(method_error(
            "org.window_zones.KWin.Error.Denied",
            "request sender is not the connected KWin script",
        ))
    }

    fn authorize_controller(&mut self, sender: String) -> Result<(), MethodErr> {
        match self.controller_sender.as_deref() {
            Some(owner) if owner != sender => Err(method_error(
                "org.window_zones.KWin.Error.Busy",
                "another Window Zones process owns the KWin controller",
            )),
            Some(_) => Ok(()),
            None => {
                self.controller_sender = Some(sender);
                Ok(())
            }
        }
    }

    fn owner_lost(&mut self, owner: &str) {
        if self.script_sender.as_deref() == Some(owner) {
            self.script_ready = false;
            self.script_sender = None;
            self.requests.clear();
            self.claimed_requests.clear();
            self.results.clear();
            self.registered_hotkeys.clear();
            self.events.clear();
        }
        if self.controller_sender.as_deref() == Some(owner) {
            self.controller_sender = None;
            self.requests.clear();
            self.claimed_requests.clear();
            self.results.clear();
            self.registered_hotkeys.clear();
            self.events.clear();
        }
    }

    fn touch(&mut self) {
        self.last_seen = Some(Instant::now());
    }

    fn next_request_id(&mut self) -> u32 {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        id
    }
    fn enqueue_move(&mut self, x: i32, y: i32, width: u32, height: u32) -> u32 {
        let id = self.next_request_id();
        self.requests.push_back(CompanionRequest::Move {
            id,
            x,
            y,
            width,
            height,
        });
        id
    }

    fn enqueue_hotkeys(&mut self, hotkeys: Vec<String>) -> u32 {
        let id = self.next_request_id();
        self.requests
            .push_back(CompanionRequest::RegisterHotkeys { id, hotkeys });
        id
    }

    fn next_request(&mut self, sender: &str) -> Result<NextRequestPayload, MethodErr> {
        self.ensure_script(sender)?;
        self.touch();

        let Some(request) = self
            .requests
            .iter()
            .find(|request| !self.claimed_requests.contains(&request.id()))
            .cloned()
        else {
            return Ok((0, String::new(), 0, 0, 0, 0, "[]".to_string()));
        };
        self.claimed_requests.insert(request.id());

        Ok(match request {
            CompanionRequest::Move {
                id,
                x,
                y,
                width,
                height,
            } => (
                id,
                REQUEST_MOVE.to_string(),
                x,
                y,
                width,
                height,
                "[]".to_string(),
            ),
            CompanionRequest::RegisterHotkeys { id, hotkeys } => (
                id,
                REQUEST_REGISTER_HOTKEYS.to_string(),
                0,
                0,
                0,
                0,
                serde_json::to_string(&hotkeys).map_err(|_| {
                    method_error(
                        "org.window_zones.KWin.Error.Operation",
                        "failed to encode hotkey request payload",
                    )
                })?,
            ),
        })
    }

    fn complete_request(&mut self, id: u32, ok: bool, message: String) -> Result<(), MethodErr> {
        let Some(index) = self.requests.iter().position(|request| request.id() == id) else {
            return Err(method_error(
                "org.window_zones.KWin.Error.Invalid",
                "unknown KWin request id",
            ));
        };
        if !self.claimed_requests.remove(&id) {
            return Err(method_error(
                "org.window_zones.KWin.Error.Invalid",
                "KWin request was not claimed by the script",
            ));
        }

        let request = self.requests.remove(index).expect("request index exists");
        if ok && let CompanionRequest::RegisterHotkeys { hotkeys, .. } = request {
            self.registered_hotkeys = hotkeys;
            if self.registered_hotkeys.is_empty() {
                self.controller_sender = None;
            }
        }
        self.results.insert(
            id,
            RequestResult {
                complete: true,
                message: if ok { String::new() } else { message },
            },
        );
        Ok(())
    }

    fn request_result(&mut self, id: u32) -> (bool, String) {
        if let Some(result) = self.results.remove(&id) {
            return (result.complete, result.message);
        }
        if self.requests.iter().any(|request| request.id() == id) {
            return (false, String::new());
        }
        (true, "unknown KWin request id".to_string())
    }
}

fn method_error(name: &'static str, message: &'static str) -> MethodErr {
    MethodErr::from((name, message))
}

fn parse_hotkeys(raw: &str) -> Result<Vec<String>, MethodErr> {
    serde_json::from_str(raw).map_err(|_| {
        method_error(
            "org.window_zones.KWin.Error.Invalid",
            "hotkey request payload is not valid JSON",
        )
    })
}

fn parse_displays(raw: &str) -> Result<Vec<DisplayPayload>, MethodErr> {
    serde_json::from_str(raw).map_err(|_| {
        method_error(
            "org.window_zones.KWin.Error.Invalid",
            "display update payload is not valid JSON",
        )
    })
}
fn parse_u32_argument(raw: &str, message: &'static str) -> Result<u32, MethodErr> {
    raw.parse()
        .map_err(|_| method_error("org.window_zones.KWin.Error.Invalid", message))
}

fn sender_name(message: &dbus::Message) -> Result<String, MethodErr> {
    message
        .sender()
        .map(|sender| sender.to_string())
        .ok_or_else(|| {
            method_error(
                "org.window_zones.KWin.Error.Denied",
                "D-Bus request has no sender identity",
            )
        })
}

pub fn run_kwin_companion_service() -> Result<(), String> {
    let connection = LocalConnection::new_session()
        .map_err(|error| format!("connect to the user session bus: {error}"))?;
    connection
        .request_name(KWIN_SERVICE_NAME, false, true, false)
        .map_err(|error| format!("own {KWIN_SERVICE_NAME}: {error}"))?;

    let state = Arc::new(Mutex::new(CompanionState::default()));
    let owner_state = Arc::clone(&state);
    let owner_rule = MatchRule::new_signal("org.freedesktop.DBus", "NameOwnerChanged");
    connection
        .add_match(
            owner_rule,
            move |(name, _old_owner, new_owner): (String, String, String), _, _| {
                if new_owner.is_empty() {
                    owner_state.lock().owner_lost(&name);
                }
                true
            },
        )
        .map_err(|error| format!("watch D-Bus owner lifecycle: {error}"))?;

    let factory = Factory::new_fn::<()>();
    let register_state = Arc::clone(&state);
    let capabilities_state = Arc::clone(&state);
    let focused_state = Arc::clone(&state);
    let displays_state = Arc::clone(&state);
    let move_state = Arc::clone(&state);
    let register_hotkeys_state = Arc::clone(&state);
    let next_request_state = Arc::clone(&state);
    let complete_request_state = Arc::clone(&state);
    let request_result_state = Arc::clone(&state);
    let update_focused_state = Arc::clone(&state);
    let update_displays_state = Arc::clone(&state);
    let hotkey_state = Arc::clone(&state);
    let next_hotkey_state = Arc::clone(&state);

    let tree = factory
        .tree(())
        .add(
            factory
                .object_path(KWIN_OBJECT_PATH, ())
                .introspectable()
                .add(
                    factory
                        .interface(KWIN_INTERFACE, ())
                        .add_m(factory.method("RegisterCompanion", (), move |method| {
                            let (protocol_raw, reset_state): (String, bool) = method.msg.read2()?;
                            let protocol_major = parse_u32_argument(
                                &protocol_raw,
                                "KWin companion protocol major must be an unsigned integer",
                            )?;
                            let sender = sender_name(method.msg)?;
                            register_state.lock().mark_ready(
                                protocol_major,
                                sender,
                                reset_state,
                            )?;
                            Ok(vec![method.msg.method_return()])
                        }))
                        .add_m(factory.method("GetCapabilities", (), move |method| {
                            let mut state = capabilities_state.lock();
                            state.ensure_ready()?;
                            let reply = method.msg.method_return().append1(vec![
                                FOCUSED_WINDOW_CAPABILITY,
                                DISPLAYS_CAPABILITY,
                                MOVE_RESIZE_CAPABILITY,
                                HOTKEYS_CAPABILITY,
                            ]);
                            Ok(vec![reply])
                        }))
                        .add_m(factory.method("GetFocusedWindow", (), move |method| {
                            let mut state = focused_state.lock();
                            state.ensure_ready()?;
                            let mut reply = method.msg.method_return();
                            match &state.focused {
                                Some((id, x, y, width, height)) => {
                                    reply.append_all((true, id.clone(), *x, *y, *width, *height))
                                }
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
                            let mut state = displays_state.lock();
                            state.ensure_ready()?;
                            let reply = method.msg.method_return().append1(state.displays.clone());
                            Ok(vec![reply])
                        }))
                        .add_m(factory.method("MoveFocusedWindow", (), move |method| {
                            let (x, y, width, height): (i32, i32, u32, u32) = method.msg.read4()?;
                            if width == 0 || height == 0 {
                                return Err(method_error(
                                    "org.window_zones.KWin.Error.Invalid",
                                    "window target must have positive dimensions",
                                ));
                            }
                            let mut state = move_state.lock();
                            state.ensure_ready()?;
                            let request_id = state.enqueue_move(x, y, width, height);
                            Ok(vec![method.msg.method_return().append1(request_id)])
                        }))
                        .add_m(factory.method("RegisterHotkeys", (), move |method| {
                            let payload: String = method.msg.read1()?;
                            let hotkeys = parse_hotkeys(&payload)?;
                            let sender = sender_name(method.msg)?;
                            let mut state = register_hotkeys_state.lock();
                            state.ensure_ready()?;
                            state.authorize_controller(sender)?;
                            let request_id = state.enqueue_hotkeys(hotkeys);
                            Ok(vec![method.msg.method_return().append1(request_id)])
                        }))
                        .add_m(factory.method("NextRequest", (), move |method| {
                            let sender = sender_name(method.msg)?;
                            let request = next_request_state.lock().next_request(&sender)?;
                            let mut reply = method.msg.method_return();
                            reply.append_all(request);
                            Ok(vec![reply])
                        }))
                        .add_m(factory.method("CompleteRequest", (), move |method| {
                            let (id_raw, ok, message): (String, bool, String) =
                                method.msg.read3()?;
                            let id = parse_u32_argument(
                                &id_raw,
                                "KWin request id must be an unsigned integer",
                            )?;
                            let sender = sender_name(method.msg)?;
                            let mut state = complete_request_state.lock();
                            state.ensure_script(&sender)?;
                            state.complete_request(id, ok, message)?;
                            Ok(vec![method.msg.method_return()])
                        }))
                        .add_m(factory.method("GetRequestResult", (), move |method| {
                            let request_id_raw: String = method.msg.read1()?;
                            let request_id = parse_u32_argument(
                                &request_id_raw,
                                "KWin request id must be an unsigned integer",
                            )?;
                            let mut state = request_result_state.lock();
                            state.ensure_ready()?;
                            let result = state.request_result(request_id);
                            let mut reply = method.msg.method_return();
                            reply.append_all(result);
                            Ok(vec![reply])
                        }))
                        .add_m(factory.method("UpdateFocusedWindow", (), move |method| {
                            let payload_raw: String = method.msg.read1()?;
                            let payload: (bool, String, i32, i32, u32, u32) =
                                serde_json::from_str(&payload_raw).map_err(|_| {
                                    method_error(
                                        "org.window_zones.KWin.Error.Invalid",
                                        "focused-window update payload is not valid JSON",
                                    )
                                })?;
                            let sender = sender_name(method.msg)?;
                            let mut state = update_focused_state.lock();
                            state.ensure_script(&sender)?;
                            state.touch();
                            state.focused = payload
                                .0
                                .then_some((payload.1, payload.2, payload.3, payload.4, payload.5));
                            Ok(vec![method.msg.method_return()])
                        }))
                        .add_m(factory.method("UpdateDisplays", (), move |method| {
                            let payload: String = method.msg.read1()?;
                            let displays = parse_displays(&payload)?;
                            let sender = sender_name(method.msg)?;
                            let mut state = update_displays_state.lock();
                            state.ensure_script(&sender)?;
                            state.touch();
                            state.displays = displays;
                            Ok(vec![method.msg.method_return()])
                        }))
                        .add_m(factory.method("HotkeyPressed", (), move |method| {
                            let hotkey: String = method.msg.read1()?;
                            let sender = sender_name(method.msg)?;
                            let mut state = hotkey_state.lock();
                            state.ensure_script(&sender)?;
                            state.touch();
                            if state
                                .registered_hotkeys
                                .iter()
                                .any(|registered| registered == &hotkey)
                            {
                                state.events.push_back(hotkey);
                            }
                            Ok(vec![method.msg.method_return()])
                        }))
                        .add_m(factory.method("GetNextHotkey", (), move |method| {
                            let sender = sender_name(method.msg)?;
                            let mut state = next_hotkey_state.lock();
                            state.ensure_ready()?;
                            state.authorize_controller(sender)?;
                            let mut reply = method.msg.method_return();
                            match state.events.pop_front() {
                                Some(hotkey) => reply.append_all((true, hotkey)),
                                None => reply.append_all((false, String::new())),
                            }
                            Ok(vec![reply])
                        })),
                ),
        )
        .add(factory.object_path("/", ()).introspectable());
    tree.start_receive(&connection);

    println!("KWin companion service started for the Window Zones KWin script.");
    loop {
        connection
            .process(Duration::from_millis(100))
            .map_err(|error| format!("process KWin companion D-Bus requests: {error}"))?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use dbus::blocking::LocalConnection;
    use dbus_tree::Factory;
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread::JoinHandle;

    #[derive(Debug)]
    enum FakeRequest {
        Move {
            x: i32,
            y: i32,
            width: u32,
            height: u32,
            seen: bool,
        },
        RegisterHotkeys {
            hotkeys: Vec<String>,
            seen: bool,
        },
    }

    #[derive(Debug)]
    struct FakeServiceState {
        capabilities: Vec<String>,
        capabilities_error: Option<(&'static str, String)>,
        focused: Option<(String, i32, i32, u32, u32)>,
        displays: Vec<DisplayPayload>,
        moves: Vec<(i32, i32, u32, u32)>,
        registered_hotkeys: Vec<String>,
        pending: HashMap<u32, FakeRequest>,
        events: VecDeque<String>,
        next_request_id: u32,
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
                focused: Some(("monitor-1".to_string(), -300, -20, 801, 602)),
                displays: vec![
                    ("monitor-0".to_string(), -1920, 0, 1920, 1080),
                    ("monitor-1".to_string(), 0, -50, 1280, 1024),
                ],
                moves: Vec::new(),
                registered_hotkeys: Vec::new(),
                pending: HashMap::new(),
                events: VecDeque::from(["alt+ctrl+left".to_string()]),
                next_request_id: 1,
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
                .expect("dbus-daemon must be installed for KDE contract tests");
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

            self.service_thread = Some(std::thread::spawn(move || {
                let connection = match LocalConnection::new_address(&address) {
                    Ok(connection) => connection,
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.to_string()));
                        return;
                    }
                };
                if let Err(error) = connection.request_name(KWIN_SERVICE_NAME, false, true, false) {
                    let _ = ready_tx.send(Err(error.to_string()));
                    return;
                }

                let factory = Factory::new_fn::<()>();
                let capabilities_state = Arc::clone(&state);
                let focused_state = Arc::clone(&state);
                let displays_state = Arc::clone(&state);
                let move_state = Arc::clone(&state);
                let register_state = Arc::clone(&state);
                let request_result_state = Arc::clone(&state);
                let hotkey_state = Arc::clone(&state);

                let tree = factory
                    .tree(())
                    .add(
                        factory
                            .object_path(KWIN_OBJECT_PATH, ())
                            .introspectable()
                            .add(
                                factory
                                    .interface(KWIN_INTERFACE, ())
                                    .add_m(factory.method("GetCapabilities", (), move |method| {
                                        let state = capabilities_state.lock();
                                        if let Some((name, message)) =
                                            state.capabilities_error.clone()
                                        {
                                            return Err((name, message).into());
                                        }
                                        Ok(vec![
                                            method
                                                .msg
                                                .method_return()
                                                .append1(state.capabilities.clone()),
                                        ])
                                    }))
                                    .add_m(factory.method("GetFocusedWindow", (), move |method| {
                                        let state = focused_state.lock();
                                        let mut reply = method.msg.method_return();
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
                                        Ok(vec![
                                            method
                                                .msg
                                                .method_return()
                                                .append1(state.displays.clone()),
                                        ])
                                    }))
                                    .add_m(factory.method("MoveFocusedWindow", (), move |method| {
                                        let (x, y, width, height): (i32, i32, u32, u32) =
                                            method.msg.read4()?;
                                        let mut state = move_state.lock();
                                        let request_id = state.next_request_id;
                                        state.next_request_id += 1;
                                        state.pending.insert(
                                            request_id,
                                            FakeRequest::Move {
                                                x,
                                                y,
                                                width,
                                                height,
                                                seen: false,
                                            },
                                        );
                                        Ok(vec![method.msg.method_return().append1(request_id)])
                                    }))
                                    .add_m(factory.method("RegisterHotkeys", (), move |method| {
                                        let payload: String = method.msg.read1()?;
                                        let hotkeys = parse_hotkeys(&payload)?;
                                        let mut state = register_state.lock();
                                        let request_id = state.next_request_id;
                                        state.next_request_id += 1;
                                        state.pending.insert(
                                            request_id,
                                            FakeRequest::RegisterHotkeys {
                                                hotkeys,
                                                seen: false,
                                            },
                                        );
                                        Ok(vec![method.msg.method_return().append1(request_id)])
                                    }))
                                    .add_m(factory.method("GetRequestResult", (), move |method| {
                                        let request_id_raw: String = method.msg.read1()?;
                                        let request_id = parse_u32_argument(
                                            &request_id_raw,
                                            "KWin request id must be an unsigned integer",
                                        )?;
                                        let mut state = request_result_state.lock();
                                        let Some(request) = state.pending.get_mut(&request_id)
                                        else {
                                            let mut reply = method.msg.method_return();
                                            reply.append_all((
                                                true,
                                                "unknown KWin request id".to_string(),
                                            ));
                                            return Ok(vec![reply]);
                                        };
                                        let seen = match request {
                                            FakeRequest::Move { seen, .. }
                                            | FakeRequest::RegisterHotkeys { seen, .. } => {
                                                let previous = *seen;
                                                *seen = true;
                                                previous
                                            }
                                        };
                                        if !seen {
                                            let mut reply = method.msg.method_return();
                                            reply.append_all((false, String::new()));
                                            return Ok(vec![reply]);
                                        }

                                        let request = state
                                            .pending
                                            .remove(&request_id)
                                            .expect("pending KWin request exists");
                                        match request {
                                            FakeRequest::Move {
                                                x,
                                                y,
                                                width,
                                                height,
                                                ..
                                            } => state.moves.push((x, y, width, height)),
                                            FakeRequest::RegisterHotkeys { hotkeys, .. } => {
                                                state.registered_hotkeys = hotkeys
                                            }
                                        }
                                        let mut reply = method.msg.method_return();
                                        reply.append_all((true, String::new()));
                                        Ok(vec![reply])
                                    }))
                                    .add_m(factory.method("GetNextHotkey", (), move |method| {
                                        let mut state = hotkey_state.lock();
                                        let mut reply = method.msg.method_return();
                                        match state.events.pop_front() {
                                            Some(hotkey) => reply.append_all((true, hotkey)),
                                            None => reply.append_all((false, String::new())),
                                        }
                                        Ok(vec![reply])
                                    })),
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
    fn private_service_round_trips_kde_window_and_hotkey_contract() {
        let bus = FakeBus::new(FakeServiceState::complete());
        let mut window_system = KwinWindowSystem::with_bus_address(bus.address.clone());

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
        window_system
            .move_focused_window(WindowMove::new(Rect::new(12, -34, 500, 400)))
            .unwrap();
        bus.update_state(|state| {
            assert_eq!(
                state.moves,
                vec![(-123, 456, 777, 888), (12, -34, 500, 400)]
            );
        });

        let mut hotkey_system = KwinHotkeySystem::with_bus_address(bus.address.clone());
        hotkey_system
            .register_hotkeys(&["alt+ctrl+left".to_string(), "cmd+1".to_string()])
            .unwrap();
        bus.update_state(|state| {
            assert_eq!(
                state.registered_hotkeys,
                vec!["alt+ctrl+left".to_string(), "cmd+1".to_string()]
            );
        });
        assert_eq!(
            hotkey_system.next_hotkey().unwrap(),
            Some(HotkeyEvent::Pressed {
                hotkey: "alt+ctrl+left".to_string()
            })
        );
        assert_eq!(hotkey_system.next_hotkey().unwrap(), None);
    }

    #[test]
    fn unavailable_kde_companion_is_actionable() {
        let window_system =
            KwinWindowSystem::with_bus_address("unix:path=/window-zones-no-such-bus");
        let error = window_system
            .capabilities()
            .expect_err("missing KWin companion must fail");

        assert!(matches!(error, KwinIntegrationError::Unavailable { .. }));
        assert!(error.to_string().contains("KWin companion"));
    }

    #[test]
    fn incompatible_kde_companion_is_classified() {
        let mut state = FakeServiceState::complete();
        state.capabilities_error = Some((
            "org.window_zones.KWin.Error.Incompatible",
            "protocol mismatch".to_string(),
        ));
        let bus = FakeBus::new(state);
        let window_system = KwinWindowSystem::with_bus_address(bus.address.clone());

        let error = window_system
            .capabilities()
            .expect_err("incompatible KWin companion must fail");
        assert!(matches!(error, KwinIntegrationError::Incompatible { .. }));
        assert!(error.to_string().contains("upgrade"));
    }

    #[test]
    fn missing_capability_is_explicit() {
        let error = require_capability(&[DISPLAYS_CAPABILITY.to_string()], MOVE_RESIZE_CAPABILITY)
            .expect_err("missing capability should fail");

        assert!(matches!(error, KwinIntegrationError::Unsupported { .. }));
        assert!(error.to_string().contains("move-resize"));
    }

    #[test]
    fn request_payloads_are_claimed_and_complete_atomically() {
        let mut state = CompanionState::default();
        state
            .mark_ready(KWIN_PROTOCOL_MAJOR, "kwin-script".to_string(), true)
            .unwrap();
        let request_id = state.enqueue_move(-123, 456, 777, 888);

        assert_eq!(
            state.next_request("kwin-script").unwrap(),
            (
                request_id,
                REQUEST_MOVE.to_string(),
                -123,
                456,
                777,
                888,
                "[]".to_string()
            )
        );
        assert_eq!(state.request_result(request_id), (false, String::new()));
        state
            .complete_request(request_id, true, String::new())
            .unwrap();
        assert_eq!(state.request_result(request_id), (true, String::new()));
    }

    #[test]
    fn rejected_hotkey_registration_keeps_previous_set() {
        let mut state = CompanionState::default();
        state
            .mark_ready(KWIN_PROTOCOL_MAJOR, "kwin-script".to_string(), true)
            .unwrap();
        let first = state.enqueue_hotkeys(vec!["alt+ctrl+left".to_string()]);
        let _ = state.next_request("kwin-script").unwrap();
        state.complete_request(first, true, String::new()).unwrap();
        let _ = state.request_result(first);
        assert_eq!(state.registered_hotkeys, vec!["alt+ctrl+left"]);

        let second = state.enqueue_hotkeys(vec!["alt+ctrl+right".to_string()]);
        let _ = state.next_request("kwin-script").unwrap();
        state
            .complete_request(second, false, "KWin rejected shortcut".to_string())
            .unwrap();
        assert_eq!(state.request_result(second).1, "KWin rejected shortcut");
        assert_eq!(state.registered_hotkeys, vec!["alt+ctrl+left"]);
    }

    #[test]
    fn empty_registration_releases_controller_after_completion() {
        let mut state = CompanionState::default();
        state
            .mark_ready(KWIN_PROTOCOL_MAJOR, "kwin-script".to_string(), true)
            .unwrap();
        state
            .authorize_controller("window-zones-1".to_string())
            .unwrap();
        let request_id = state.enqueue_hotkeys(Vec::new());
        let _ = state.next_request("kwin-script").unwrap();
        assert!(
            state
                .authorize_controller("window-zones-2".to_string())
                .is_err()
        );
        state
            .complete_request(request_id, true, String::new())
            .unwrap();

        state
            .authorize_controller("window-zones-2".to_string())
            .unwrap();
        let request_id = state.enqueue_hotkeys(vec!["alt+ctrl+left".to_string()]);
        let _ = state.next_request("kwin-script").unwrap();
        state
            .complete_request(request_id, true, String::new())
            .unwrap();
        assert_eq!(state.registered_hotkeys, vec!["alt+ctrl+left"]);
    }

    #[test]
    fn script_reset_requeues_registered_hotkeys_once() {
        let mut state = CompanionState::default();
        state
            .mark_ready(KWIN_PROTOCOL_MAJOR, "kwin-script".to_string(), true)
            .unwrap();
        state
            .authorize_controller("window-zones-1".to_string())
            .unwrap();

        let first = state.enqueue_hotkeys(vec!["alt+ctrl+left".to_string()]);
        let _ = state.next_request("kwin-script").unwrap();
        state.complete_request(first, true, String::new()).unwrap();
        let _ = state.request_result(first);

        state
            .mark_ready(KWIN_PROTOCOL_MAJOR, "kwin-script".to_string(), true)
            .unwrap();
        assert_eq!(state.controller_sender.as_deref(), Some("window-zones-1"));
        let (request_id, kind, _, _, _, _, hotkeys) = state.next_request("kwin-script").unwrap();
        assert_ne!(request_id, 0);
        assert_eq!(kind, REQUEST_REGISTER_HOTKEYS);
        assert_eq!(hotkeys, r#"["alt+ctrl+left"]"#);
        state
            .complete_request(request_id, true, String::new())
            .unwrap();
        let _ = state.request_result(request_id);

        state
            .mark_ready(KWIN_PROTOCOL_MAJOR, "kwin-script".to_string(), false)
            .unwrap();
        assert_eq!(
            state.next_request("kwin-script").unwrap(),
            (0, String::new(), 0, 0, 0, 0, "[]".to_string())
        );
    }
    #[test]
    fn sender_ownership_is_enforced_and_released() {
        let mut state = CompanionState::default();
        state
            .mark_ready(KWIN_PROTOCOL_MAJOR, "kwin-script".to_string(), true)
            .unwrap();
        state
            .authorize_controller("window-zones-1".to_string())
            .unwrap();

        let busy = state
            .authorize_controller("window-zones-2".to_string())
            .expect_err("a second controller must be rejected");
        assert_eq!(
            busy.errorname().to_string(),
            "org.window_zones.KWin.Error.Busy"
        );
        assert_eq!(
            state
                .ensure_script("other-script")
                .unwrap_err()
                .errorname()
                .to_string(),
            "org.window_zones.KWin.Error.Denied"
        );

        state.owner_lost("window-zones-1");
        state
            .authorize_controller("window-zones-2".to_string())
            .unwrap();
        state.owner_lost("kwin-script");
        assert_eq!(
            state.ensure_ready().unwrap_err().errorname().to_string(),
            "org.window_zones.KWin.Error.Unavailable"
        );
    }

    #[test]
    fn incompatible_companion_protocol_is_rejected() {
        let mut state = CompanionState::default();
        let error = state
            .mark_ready(KWIN_PROTOCOL_MAJOR + 1, "kwin-script".to_string(), true)
            .expect_err("unknown protocol must be rejected");
        assert_eq!(
            error.errorname().to_string(),
            "org.window_zones.KWin.Error.Incompatible"
        );
        assert!(!state.script_ready);
    }

    #[test]
    fn lifecycle_errors_are_classified() {
        let error =
            dbus::Error::new_custom("org.window_zones.KWin.Error.Incompatible", "old script");
        assert!(matches!(
            classify_dbus_error(error),
            KwinIntegrationError::Incompatible { .. }
        ));
    }
}
