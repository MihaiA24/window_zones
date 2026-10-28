use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use dbus::arg::{AppendAll, ReadAll};
use dbus::blocking::{Connection, LocalConnection};
use dbus::channel::Sender;
use dbus::message::MatchRule;
use dbus_tree::{Factory, MethodErr};
use parking_lot::Mutex;
use thiserror::Error;

use crate::{
    DisplayGeometry, FocusedWindow, HotkeyEvent, HotkeySystem, HotkeySystemError, Rect, WindowId,
    WindowMove, WindowSystem, WindowSystemError,
};

pub const KWIN_SERVICE_NAME: &str = "org.window_zones.KWin";
pub const KWIN_OBJECT_PATH: &str = "/org/window_zones/KWin";
pub const KWIN_INTERFACE: &str = "org.window_zones.KWin3";
pub const KWIN_PROTOCOL_MAJOR: u32 = 3;

const CALL_TIMEOUT: Duration = Duration::from_secs(1);
const COMPANION_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(5);
const NEXT_REQUEST_WAIT: Duration = Duration::from_secs(1);
const SERVICE_POLL_INTERVAL: Duration = Duration::from_millis(25);
const FOCUSED_WINDOW_CAPABILITY: &str = "focused-window";
const DISPLAYS_CAPABILITY: &str = "displays";
const MOVE_RESIZE_CAPABILITY: &str = "move-resize";
const HOTKEYS_CAPABILITY: &str = "hotkeys";
const REQUEST_MOVE: &str = "move";
const REQUEST_REFRESH_DISPLAYS: &str = "refresh-displays";
const REQUEST_REGISTER_HOTKEYS: &str = "register-hotkeys";
// KWin hosts KGlobalAccel in-process and registers script actions under the "kwin" component.
const KGLOBALACCEL_SERVICE: &str = "org.kde.kglobalaccel";
const KGLOBALACCEL_PATH: &str = "/kglobalaccel";
const KGLOBALACCEL_INTERFACE: &str = "org.kde.KGlobalAccel";
const KGLOBALACCEL_COMPONENT: &str = "kwin";

type DisplayPayload = (String, i32, i32, u32, u32);
type NextRequestPayload = (u32, String, String, i32, i32, u32, u32, String);

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
    #[error("KWin window no longer exists: {window}")]
    WindowGone { window: WindowId },
    #[error("KWin partially moved window {window}: {message}")]
    PartialMove {
        window: WindowId,
        observed: Rect,
        message: String,
    },
    #[error("KWin companion operation failed: {message}; retry the operation")]
    Operation { message: String },
}

impl KwinIntegrationError {
    fn should_reconnect(&self) -> bool {
        matches!(self, Self::Unavailable { .. } | Self::Incompatible { .. })
    }

    fn into_window_error(self) -> WindowSystemError {
        match self {
            Self::WindowGone { window } => WindowSystemError::WindowGone(window),
            _ => WindowSystemError::Platform(self.to_string()),
        }
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
            .map(|(present, window_id, x, y, width, height)| {
                present.then(|| {
                    FocusedWindow::new(WindowId::new(window_id), Rect::new(x, y, width, height))
                })
            })
            .map_err(KwinIntegrationError::into_window_error)
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        self.request_and_wait(DISPLAYS_CAPABILITY, "RefreshDisplays", ())
            .map_err(KwinIntegrationError::into_window_error)?;
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

    fn move_window(&mut self, window_move: &WindowMove) -> Result<(), WindowSystemError> {
        let target = window_move.target;
        let result: Result<(), KwinIntegrationError> = self.request_and_wait(
            MOVE_RESIZE_CAPABILITY,
            "MoveWindow",
            (
                window_move.window.as_str(),
                target.x,
                target.y,
                target.width,
                target.height,
            ),
        );

        result.map_err(|error| match error {
            KwinIntegrationError::PartialMove {
                window,
                observed,
                message,
            } if window == window_move.window => WindowSystemError::PartialMove {
                observed,
                message: KwinIntegrationError::Operation { message }.to_string(),
            },
            error => error.into_window_error(),
        })
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

    fn ensure_connection(&mut self) -> Result<&Connection, KwinIntegrationError> {
        if self.connection.is_none() {
            self.connection = Some(connect(self.bus_address.as_deref())?);
        }

        self.connection
            .as_ref()
            .ok_or_else(|| KwinIntegrationError::Unavailable {
                message: "D-Bus connection was not established".to_string(),
            })
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
            // The script preflights owners and SetShortcut verifies assignment before commit.
            // A later sampler could refuse an already committed set without rolling it back.
            wait_for_request(connection, request_id)
        })();

        if let Err(error) = &result
            && error.should_reconnect()
        {
            self.connection = None;
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
            .map_err(KwinIntegrationError::into_hotkey_error)
    }

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
        let result = (|| {
            let connection = self
                .ensure_connection()
                .map_err(KwinIntegrationError::into_hotkey_error)?;
            let proxy = connection.with_proxy(KWIN_SERVICE_NAME, KWIN_OBJECT_PATH, CALL_TIMEOUT);
            let (present, hotkey): (bool, String) = proxy
                .method_call(KWIN_INTERFACE, "GetNextHotkey", ())
                .map_err(|error| classify_dbus_error(error).into_hotkey_error())?;
            Ok(present.then_some(HotkeyEvent::Pressed { hotkey }))
        })();

        if result.is_err() {
            self.connection = None;
        }

        result
    }
}

impl Drop for KwinHotkeySystem {
    fn drop(&mut self) {
        if self.connection.is_some() {
            let _ = self.call_register_hotkeys(&[]);
        }
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
        let (complete, error_name, message): (bool, String, String) = proxy
            .method_call(
                KWIN_INTERFACE,
                "GetRequestResult",
                (request_id.to_string(),),
            )
            .map_err(classify_dbus_error)?;
        if complete {
            return if error_name.is_empty() && message.is_empty() {
                Ok(())
            } else {
                Err(classify_dbus_error(dbus::Error::new_custom(
                    if error_name.is_empty() {
                        "org.window_zones.KWin.Error.Operation"
                    } else {
                        &error_name
                    },
                    &message,
                )))
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
        "org.window_zones.KWin.Error.WindowGone" => KwinIntegrationError::WindowGone {
            window: WindowId::new(message),
        },
        "org.window_zones.KWin.Error.PartialMove" => {
            #[derive(serde::Deserialize)]
            #[serde(deny_unknown_fields)]
            struct PartialMovePayload {
                window_id: String,
                observed: (i32, i32, u32, u32),
                message: String,
            }
            match serde_json::from_str::<PartialMovePayload>(&message) {
                Ok(payload) if payload.observed.2 > 0 && payload.observed.3 > 0 => {
                    let (x, y, width, height) = payload.observed;
                    KwinIntegrationError::PartialMove {
                        window: WindowId::new(payload.window_id),
                        observed: Rect::new(x, y, width, height),
                        message: payload.message,
                    }
                }
                _ => KwinIntegrationError::Operation {
                    message: format!("invalid KWin partial-move result: {message}"),
                },
            }
        }
        "org.window_zones.KWin.Error.Unsupported" => KwinIntegrationError::Unsupported {
            capability: "requested operation".to_string(),
            message,
        },
        "org.window_zones.KWin.Error.Busy" => KwinIntegrationError::Busy { message },
        _ => KwinIntegrationError::Operation { message },
    }
}

#[derive(Debug, PartialEq, Eq)]
enum CompanionRequest {
    Move {
        id: u32,
        window: String,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
    RefreshDisplays {
        id: u32,
    },
    RegisterHotkeys {
        id: u32,
        hotkeys: Vec<String>,
        controller: Option<String>,
    },
}

impl CompanionRequest {
    fn id(&self) -> u32 {
        match self {
            Self::Move { id, .. }
            | Self::RefreshDisplays { id }
            | Self::RegisterHotkeys { id, .. } => *id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RequestResult {
    error_name: String,
    message: String,
}

#[derive(Debug)]
struct PendingNextRequest {
    call: dbus::Message,
    sender: String,
    deadline: Instant,
}

#[derive(Debug, Default)]
struct CompanionState {
    script_sender: Option<String>,
    controller_sender: Option<String>,
    last_seen: Option<Instant>,
    focused: Option<(String, i32, i32, u32, u32)>,
    displays: Vec<DisplayPayload>,
    requests: VecDeque<CompanionRequest>,
    claimed_requests: HashSet<u32>,
    results: HashMap<u32, RequestResult>,
    last_request_id: u32,
    pending_next_request: Option<PendingNextRequest>,
    registered_hotkeys: Vec<String>,
    events: VecDeque<String>,
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
        if !reset_state && self.script_sender.as_deref() == Some(script_sender.as_str()) {
            self.touch();
            return Ok(());
        }

        let previous_hotkeys = std::mem::take(&mut self.registered_hotkeys);
        self.script_sender = Some(script_sender);
        self.last_seen = Some(Instant::now());
        self.focused = None;
        self.displays.clear();
        self.requests.clear();
        self.claimed_requests.clear();
        self.results.clear();
        self.events.clear();
        // A reconnecting script may still own keys after the companion lost its state.
        if !previous_hotkeys.is_empty() || !reset_state {
            self.enqueue_hotkeys(previous_hotkeys);
        }
        Ok(())
    }

    fn ensure_ready(&mut self) -> Result<(), MethodErr> {
        if self.script_sender.is_some()
            && self
                .last_seen
                .is_some_and(|last_seen| last_seen.elapsed() <= COMPANION_HEARTBEAT_TIMEOUT)
        {
            return Ok(());
        }

        self.script_sender = None;
        self.requests.clear();
        self.claimed_requests.clear();
        self.results.clear();
        self.registered_hotkeys.clear();
        self.events.clear();
        Err(method_error(
            "org.window_zones.KWin.Error.Unavailable",
            "KWin script is not connected; enable or upgrade the Window Zones KWin script to protocol 3",
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
        self.is_controller(&sender)?;
        self.controller_sender = Some(sender);
        Ok(())
    }

    fn is_controller(&self, sender: &str) -> Result<bool, MethodErr> {
        match self.controller_sender.as_deref() {
            Some(owner) if owner != sender => Err(method_error(
                "org.window_zones.KWin.Error.Busy",
                "another Window Zones process owns the KWin controller",
            )),
            Some(_) => Ok(true),
            None => Ok(false),
        }
    }

    fn owner_lost(&mut self, owner: &str) {
        if self.script_sender.as_deref() == Some(owner) {
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
            if self.script_sender.is_some() {
                self.enqueue_hotkeys(Vec::new());
            }
        }
    }

    fn touch(&mut self) {
        self.last_seen = Some(Instant::now());
    }

    fn next_request_id(&mut self) -> u32 {
        self.last_request_id = self.last_request_id.wrapping_add(1).max(1);
        self.last_request_id
    }
    fn enqueue_move(&mut self, window: String, x: i32, y: i32, width: u32, height: u32) -> u32 {
        let id = self.next_request_id();
        self.requests.push_back(CompanionRequest::Move {
            id,
            window,
            x,
            y,
            width,
            height,
        });
        id
    }

    fn enqueue_hotkeys(&mut self, hotkeys: Vec<String>) -> u32 {
        let id = self.next_request_id();
        self.requests.push_back(CompanionRequest::RegisterHotkeys {
            id,
            hotkeys,
            controller: self.controller_sender.clone(),
        });
        id
    }

    fn enqueue_refresh_displays(&mut self) -> u32 {
        let id = self.next_request_id();
        self.requests
            .push_back(CompanionRequest::RefreshDisplays { id });
        id
    }

    fn next_request(&mut self, sender: &str) -> Result<NextRequestPayload, MethodErr> {
        self.ensure_script(sender)?;
        self.touch();

        let Some(request) = self
            .requests
            .iter()
            .find(|request| !self.claimed_requests.contains(&request.id()))
        else {
            return Ok((
                0,
                String::new(),
                String::new(),
                0,
                0,
                0,
                0,
                "[]".to_string(),
            ));
        };
        self.claimed_requests.insert(request.id());

        Ok(match request {
            CompanionRequest::Move {
                id,
                window,
                x,
                y,
                width,
                height,
            } => (
                *id,
                REQUEST_MOVE.to_string(),
                window.clone(),
                *x,
                *y,
                *width,
                *height,
                "[]".to_string(),
            ),
            CompanionRequest::RefreshDisplays { id } => (
                *id,
                REQUEST_REFRESH_DISPLAYS.to_string(),
                String::new(),
                0,
                0,
                0,
                0,
                "[]".to_string(),
            ),
            CompanionRequest::RegisterHotkeys { id, hotkeys, .. } => (
                *id,
                REQUEST_REGISTER_HOTKEYS.to_string(),
                String::new(),
                0,
                0,
                0,
                0,
                serde_json::to_string(hotkeys).map_err(|_| {
                    method_error(
                        "org.window_zones.KWin.Error.Operation",
                        "failed to encode hotkey request payload",
                    )
                })?,
            ),
        })
    }

    fn poll_next_request(&mut self, call: &dbus::Message) -> Result<Vec<dbus::Message>, MethodErr> {
        let sender = sender_name(call)?;
        self.ensure_script(&sender)?;
        if self.pending_next_request.is_some() {
            return Err(method_error(
                "org.window_zones.KWin.Error.Busy",
                "KWin script already has an outstanding NextRequest",
            ));
        }
        let request = self.next_request(&sender)?;
        if request.0 != 0 {
            let mut reply = call.method_return();
            reply.append_all(request);
            return Ok(vec![reply]);
        }
        // dbus_message_copy deliberately resets the serial; a deferred reply must retain it.
        let mut deferred_call = call
            .duplicate()
            .map_err(|error| MethodErr::failed(&error))?;
        deferred_call.set_serial(call.get_serial().ok_or_else(|| {
            method_error(
                "org.window_zones.KWin.Error.Invalid",
                "NextRequest has no D-Bus serial",
            )
        })?);
        self.pending_next_request = Some(PendingNextRequest {
            call: deferred_call,
            sender,
            deadline: Instant::now() + NEXT_REQUEST_WAIT,
        });
        Ok(Vec::new())
    }

    fn take_next_request_reply(&mut self, now: Instant) -> Option<dbus::Message> {
        let pending = self.pending_next_request.take()?;
        match self.next_request(&pending.sender) {
            Ok(request) if request.0 != 0 || now >= pending.deadline => {
                let mut reply = pending.call.method_return();
                reply.append_all(request);
                Some(reply)
            }
            Ok(_) => {
                self.pending_next_request = Some(pending);
                None
            }
            Err(error) => Some(error.to_message(&pending.call)),
        }
    }

    fn complete_request(
        &mut self,
        id: u32,
        ok: bool,
        error_name: String,
        message: String,
    ) -> Result<(), MethodErr> {
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
        if ok
            && let CompanionRequest::RegisterHotkeys {
                hotkeys,
                controller,
                ..
            } = request
        {
            self.registered_hotkeys = hotkeys;
            if self.registered_hotkeys.is_empty() && self.controller_sender == controller {
                self.controller_sender = None;
            }
        }
        self.results.insert(
            id,
            RequestResult {
                error_name: if ok { String::new() } else { error_name },
                message: if ok { String::new() } else { message },
            },
        );
        Ok(())
    }

    fn request_result(&mut self, id: u32) -> (bool, String, String) {
        if let Some(result) = self.results.remove(&id) {
            return (true, result.error_name, result.message);
        }
        if self.requests.iter().any(|request| request.id() == id) {
            return (false, String::new(), String::new());
        }
        (
            true,
            "org.window_zones.KWin.Error.Invalid".to_string(),
            "unknown KWin request id".to_string(),
        )
    }
}

fn method_error(name: &'static str, message: &'static str) -> MethodErr {
    MethodErr::from((name, message))
}

fn parse_hotkeys(raw: &str) -> Result<Vec<String>, MethodErr> {
    let hotkeys: Vec<String> = serde_json::from_str(raw).map_err(|_| {
        method_error(
            "org.window_zones.KWin.Error.Invalid",
            "hotkey request payload is not valid JSON",
        )
    })?;
    for hotkey in &hotkeys {
        if crate::config::normalize_hotkey(hotkey).as_ref() != Ok(hotkey) {
            return Err((
                "org.window_zones.KWin.Error.Unsupported",
                format!(
                    "unsupported non-canonical KWin hotkey '{hotkey}'; use a canonical binding"
                ),
            )
                .into());
        }
    }
    Ok(hotkeys)
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
    // Keep outbound accelerator calls off the D-Bus server connection, and never hold
    // CompanionState while KWin processes them.
    let accelerator_connection = LocalConnection::new_session()
        .map_err(|error| format!("connect to KGlobalAccel on the user session bus: {error}"))?;
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
    let refresh_displays_state = Arc::clone(&state);
    let shortcut_state = Arc::clone(&state);
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
                            let present = state.focused.is_some();
                            let (id, x, y, width, height) = state.focused.clone().unwrap_or_default();
                            reply.append_all((present, id, x, y, width, height));
                            Ok(vec![reply])
                        }))
                        .add_m(factory.method("GetDisplays", (), move |method| {
                            let mut state = displays_state.lock();
                            state.ensure_ready()?;
                            let reply = method.msg.method_return().append1(state.displays.clone());
                            Ok(vec![reply])
                        }))
                        .add_m(factory.method("RefreshDisplays", (), move |method| {
                            let mut state = refresh_displays_state.lock();
                            state.ensure_ready()?;
                            let request_id = state.enqueue_refresh_displays();
                            Ok(vec![method.msg.method_return().append1(request_id)])
                        }))
                        .add_m(factory.method("SetShortcut", (), move |method| {
                            let (hotkey, key): (String, i32) = method.msg.read2()?;
                            let sender = sender_name(method.msg)?;
                            shortcut_state.lock().ensure_script(&sender)?;
                            if crate::config::normalize_hotkey(&hotkey).as_ref() != Ok(&hotkey) {
                                return Err(method_error(
                                    "org.window_zones.KWin.Error.Invalid",
                                    "SetShortcut requires a canonical hotkey",
                                ));
                            }
                            let action_id = vec![
                                KGLOBALACCEL_COMPONENT.to_string(),
                                format!("Window Zones Hotkey {hotkey}"),
                                String::new(),
                                String::new(),
                            ];
                            // QJSEngine callDBus cannot marshal JS arrays as a(ai).
                            // setForeignShortcutKeys also updates KWin's QAction cache, so an
                            // accelerator restart cannot resurrect a removed grab.
                            let keys: Vec<(Vec<i32>,)> = if key == 0 {
                                Vec::new()
                            } else {
                                // QKeySequence's D-Bus form always contains four key slots.
                                vec![(vec![key, 0, 0, 0],)]
                            };
                            let accel = accelerator_connection.with_proxy(
                                KGLOBALACCEL_SERVICE,
                                KGLOBALACCEL_PATH,
                                CALL_TIMEOUT,
                            );
                            accel
                                .method_call::<(), _, _, _>(
                                    KGLOBALACCEL_INTERFACE,
                                    "setForeignShortcutKeys",
                                    (&action_id, &keys),
                                )
                                .map_err(|error| {
                                    MethodErr::from((
                                        "org.window_zones.KWin.Error.Unavailable",
                                        format!(
                                            "KGlobalAccel could not update '{hotkey}': {error}"
                                        ),
                                    ))
                                })?;
                            let (assigned,): (Vec<(Vec<i32>,)>,) = accel
                                .method_call(KGLOBALACCEL_INTERFACE, "shortcutKeys", (&action_id,))
                                .map_err(|error| {
                                    MethodErr::from((
                                        "org.window_zones.KWin.Error.Unavailable",
                                        format!(
                                            "KGlobalAccel could not verify '{hotkey}': {error}"
                                        ),
                                    ))
                                })?;
                            if assigned != keys {
                                return Err(method_error(
                                    "org.window_zones.KWin.Error.Unavailable",
                                    "KGlobalAccel did not apply the requested shortcut",
                                ));
                            }
                            if key != 0 {
                                let (winner,): (Vec<String>,) = accel
                                    .method_call(KGLOBALACCEL_INTERFACE, "action", (key,))
                                    .map_err(|error| {
                                        MethodErr::from((
                                            "org.window_zones.KWin.Error.Unavailable",
                                            format!(
                                                "KGlobalAccel could not verify the owner of '{hotkey}': {error}"
                                            ),
                                        ))
                                    })?;
                                if winner.first() != action_id.first()
                                    || winner.get(1) != action_id.get(1)
                                {
                                    return Err(MethodErr::from((
                                        "org.window_zones.KWin.Error.Unavailable",
                                        format!(
                                            "KGlobalAccel did not assign '{hotkey}' to Window Zones; current owner: {}",
                                            winner.get(1).map(String::as_str).unwrap_or("none")
                                        ),
                                    )));
                                }
                            }
                            Ok(vec![method.msg.method_return()])
                        }))
                        .add_m(factory.method("MoveWindow", (), move |method| {
                            let (window, x, y, width, height): (String, i32, i32, u32, u32) =
                                method.msg.read_all()?;
                            if width == 0 || height == 0 {
                                return Err(method_error(
                                    "org.window_zones.KWin.Error.Invalid",
                                    "window target must have positive dimensions",
                                ));
                            }
                            let mut state = move_state.lock();
                            state.ensure_ready()?;
                            let request_id = state.enqueue_move(window, x, y, width, height);
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
                            next_request_state.lock().poll_next_request(method.msg)
                        }))
                        .add_m(factory.method("CompleteRequest", (), move |method| {
                            let (id_raw, ok, error_name, message): (String, bool, String, String) =
                                method.msg.read4()?;
                            let id = parse_u32_argument(
                                &id_raw,
                                "KWin request id must be an unsigned integer",
                            )?;
                            let sender = sender_name(method.msg)?;
                            let mut state = complete_request_state.lock();
                            state.ensure_script(&sender)?;
                            state.complete_request(id, ok, error_name, message)?;
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
                            let event = if state.is_controller(&sender)? {
                                state.events.pop_front()
                            } else {
                                None
                            };
                            let mut reply = method.msg.method_return();
                            reply.append_all((event.is_some(), event.unwrap_or_default()));
                            Ok(vec![reply])
                        })),
                ),
        )
        .add(factory.object_path("/", ()).introspectable());
    tree.start_receive(&connection);

    println!("KWin companion service started for the Window Zones KWin script.");
    loop {
        if let Some(reply) = state.lock().take_next_request_reply(Instant::now()) {
            connection
                .send(reply)
                .map_err(|_| "send deferred KWin NextRequest reply".to_string())?;
        }
        connection
            .process(SERVICE_POLL_INTERVAL)
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
            window: String,
            x: i32,
            y: i32,
            width: u32,
            height: u32,
            seen: bool,
        },
        RefreshDisplays {
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
        request_error: Option<(&'static str, String)>,
        move_error: Option<(&'static str, String)>,
        move_observed: Option<Rect>,
        expose_stale_accelerator: bool,
        focused: Option<(String, i32, i32, u32, u32)>,
        displays: Vec<DisplayPayload>,
        next_displays: Option<Vec<DisplayPayload>>,
        moves: Vec<(String, i32, i32, u32, u32)>,
        registered_hotkeys: Vec<String>,
        pending: HashMap<u32, FakeRequest>,
        events: VecDeque<String>,
        next_request_id: u32,
        script_state: CompanionState,
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
                request_error: None,
                move_error: None,
                move_observed: None,
                expose_stale_accelerator: false,
                focused: Some(("window-1".to_string(), -300, -20, 801, 602)),
                displays: vec![
                    ("monitor-0".to_string(), -1920, 0, 1920, 1080),
                    ("monitor-1".to_string(), 0, -50, 1280, 1024),
                ],
                next_displays: None,
                moves: Vec::new(),
                registered_hotkeys: Vec::new(),
                pending: HashMap::new(),
                events: VecDeque::from(["alt+ctrl+left".to_string()]),
                next_request_id: 1,
                script_state: CompanionState::default(),
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
                if state.lock().expose_stale_accelerator
                    && let Err(error) =
                        connection.request_name(KGLOBALACCEL_SERVICE, false, true, false)
                {
                    let _ = ready_tx.send(Err(error.to_string()));
                    return;
                }

                let factory = Factory::new_fn::<()>();
                let capabilities_state = Arc::clone(&state);
                let focused_state = Arc::clone(&state);
                let displays_state = Arc::clone(&state);
                let refresh_displays_state = Arc::clone(&state);
                let move_state = Arc::clone(&state);
                let register_state = Arc::clone(&state);
                let request_result_state = Arc::clone(&state);
                let hotkey_state = Arc::clone(&state);
                let companion_state = Arc::clone(&state);
                let next_request_state = Arc::clone(&state);

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
                                        let (protocol, reset): (String, bool) =
                                            method.msg.read2()?;
                                        companion_state.lock().script_state.mark_ready(
                                            protocol.parse().unwrap(),
                                            sender_name(method.msg)?,
                                            reset,
                                        )?;
                                        Ok(vec![method.msg.method_return()])
                                    }))
                                    .add_m(factory.method("NextRequest", (), move |method| {
                                        next_request_state
                                            .lock()
                                            .script_state
                                            .poll_next_request(method.msg)
                                    }))
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
                                    .add_m(factory.method("RefreshDisplays", (), move |method| {
                                        let mut state = refresh_displays_state.lock();
                                        let request_id = state.next_request_id;
                                        state.next_request_id += 1;
                                        state.pending.insert(
                                            request_id,
                                            FakeRequest::RefreshDisplays { seen: false },
                                        );
                                        Ok(vec![method.msg.method_return().append1(request_id)])
                                    }))
                                    .add_m(factory.method("MoveWindow", (), move |method| {
                                        let (window, x, y, width, height): (
                                            String,
                                            i32,
                                            i32,
                                            u32,
                                            u32,
                                        ) = method.msg.read_all()?;
                                        let mut state = move_state.lock();
                                        let request_id = state.next_request_id;
                                        state.next_request_id += 1;
                                        state.pending.insert(
                                            request_id,
                                            FakeRequest::Move {
                                                window,
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
                                                "org.window_zones.KWin.Error.Invalid".to_string(),
                                                "unknown KWin request id".to_string(),
                                            ));
                                            return Ok(vec![reply]);
                                        };
                                        let seen = match request {
                                            FakeRequest::Move { seen, .. }
                                            | FakeRequest::RefreshDisplays { seen }
                                            | FakeRequest::RegisterHotkeys { seen, .. } => {
                                                let previous = *seen;
                                                *seen = true;
                                                previous
                                            }
                                        };
                                        if !seen {
                                            let mut reply = method.msg.method_return();
                                            reply.append_all((false, String::new(), String::new()));
                                            return Ok(vec![reply]);
                                        }

                                        let request = state
                                            .pending
                                            .remove(&request_id)
                                            .expect("pending KWin request exists");
                                        let request_error =
                                            if matches!(request, FakeRequest::Move { .. }) {
                                                state
                                                    .move_error
                                                    .clone()
                                                    .or_else(|| state.request_error.clone())
                                            } else {
                                                state.request_error.clone()
                                            };
                                        if let FakeRequest::Move { window, .. } = &request
                                            && let Some(observed) = state.move_observed
                                            && let Some((id, x, y, width, height)) =
                                                &mut state.focused
                                            && id == window
                                        {
                                            (*x, *y, *width, *height) = (
                                                observed.x,
                                                observed.y,
                                                observed.width,
                                                observed.height,
                                            );
                                        }
                                        if request_error.is_none() {
                                            match request {
                                                FakeRequest::Move {
                                                    window,
                                                    x,
                                                    y,
                                                    width,
                                                    height,
                                                    ..
                                                } => {
                                                    state.moves.push((window, x, y, width, height))
                                                }
                                                FakeRequest::RefreshDisplays { .. } => {
                                                    if let Some(displays) =
                                                        state.next_displays.take()
                                                    {
                                                        state.displays = displays;
                                                    }
                                                }
                                                FakeRequest::RegisterHotkeys {
                                                    hotkeys, ..
                                                } => state.registered_hotkeys = hotkeys,
                                            }
                                        }
                                        let mut reply = method.msg.method_return();
                                        let (error_name, message) = request_error
                                            .map(|(name, message)| (name.to_string(), message))
                                            .unwrap_or_default();
                                        reply.append_all((true, error_name, message));
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
                    .add(
                        factory.object_path(KGLOBALACCEL_PATH, ()).add(
                            factory
                                .interface(KGLOBALACCEL_INTERFACE, ())
                                .add_m(factory.method("allMainComponents", (), move |method| {
                                    Ok(vec![
                                        method.msg.method_return().append1(vec![vec![
                                            KGLOBALACCEL_COMPONENT.to_string(),
                                        ]]),
                                    ])
                                }))
                                .add_m(factory.method("shortcutKeys", (), move |method| {
                                    // A later, stale sampler must not veto a committed request.
                                    let keys: Vec<(Vec<i32>,)> = Vec::new();
                                    Ok(vec![method.msg.method_return().append1(keys)])
                                })),
                        ),
                    )
                    .add(factory.object_path("/", ()).introspectable());
                tree.start_receive(&connection);
                ready_tx
                    .send(Ok(()))
                    .expect("report fake companion readiness");

                while !stop.load(Ordering::Relaxed) {
                    if let Some(reply) = state
                        .lock()
                        .script_state
                        .take_next_request_reply(Instant::now())
                    {
                        connection
                            .send(reply)
                            .expect("send deferred fake request reply");
                    }
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
    fn display_requests_refresh_before_reading_the_cached_work_area() {
        let bus = FakeBus::new(FakeServiceState::complete());
        let window_system = KwinWindowSystem::with_bus_address(bus.address.clone());
        bus.update_state(|state| {
            state.next_displays = Some(vec![("monitor-0".to_string(), 0, 50, 1200, 850)]);
        });
        assert_eq!(
            window_system.displays().unwrap(),
            vec![DisplayGeometry::new(
                "monitor-0",
                Rect::new(0, 50, 1200, 850)
            )]
        );
        bus.update_state(|state| {
            assert!(
                state.next_displays.is_none(),
                "refresh must complete before reading"
            );
            state.request_error = Some((
                "org.window_zones.KWin.Error.Unavailable",
                "refresh failed".to_string(),
            ));
        });
        assert!(
            window_system
                .displays()
                .unwrap_err()
                .to_string()
                .contains("refresh failed")
        );
    }

    #[test]
    fn private_service_round_trips_kde_window_and_hotkey_contract() {
        let bus = FakeBus::new(FakeServiceState::complete());
        let mut window_system = KwinWindowSystem::with_bus_address(bus.address.clone());

        assert_eq!(
            window_system.focused_window().unwrap(),
            Some(FocusedWindow::new(
                WindowId::new("window-1"),
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
                WindowId::new("window-1"),
                Rect::new(-123, 456, 777, 888),
            ))
            .unwrap();
        window_system
            .move_window(&WindowMove::new(
                WindowId::new("window-1"),
                Rect::new(12, -34, 500, 400),
            ))
            .unwrap();
        bus.update_state(|state| {
            assert_eq!(
                state.moves,
                vec![
                    ("window-1".to_string(), -123, 456, 777, 888),
                    ("window-1".to_string(), 12, -34, 500, 400),
                ]
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
    fn move_keeps_observed_identity_after_focus_switches() {
        let bus = FakeBus::new(FakeServiceState::complete());
        let mut system = KwinWindowSystem::with_bus_address(bus.address.clone());
        let focused = system.focused_window().unwrap().unwrap();
        bus.update_state(|state| {
            state.focused = Some(("window-2".to_string(), 0, 0, 640, 480));
        });
        let target = Rect::new(-20, 30, 900, 600);
        system
            .move_window(&WindowMove::new(focused.id, target))
            .unwrap();
        bus.update_state(|state| {
            assert_eq!(
                state.moves,
                vec![("window-1".to_string(), -20, 30, 900, 600)]
            );
        });
    }

    #[test]
    fn closed_window_maps_to_window_gone() {
        let mut state = FakeServiceState::complete();
        state.request_error = Some((
            "org.window_zones.KWin.Error.WindowGone",
            "window-1".to_string(),
        ));
        let bus = FakeBus::new(state);
        let mut system = KwinWindowSystem::with_bus_address(bus.address.clone());
        let movement = WindowMove::new(WindowId::new("window-1"), Rect::new(0, 0, 900, 600));
        assert_eq!(
            system.move_window(&movement),
            Err(WindowSystemError::WindowGone(WindowId::new("window-1")))
        );
        bus.update_state(|state| assert!(state.moves.is_empty()));
    }

    #[test]
    fn constrained_moves_and_interrupted_restore_keep_the_original_restore_point() {
        use crate::{Action, ExecuteActionError, WindowHistory, execute_action};
        let original = Rect::new(90, 100, 700, 500);
        let constrained = Rect::new(0, 40, 800, 860);
        let interrupted_restore = Rect::new(20, 50, 650, 700);
        let mut state = FakeServiceState::complete();
        state.focused = Some(("window-1".to_string(), 90, 100, 700, 500));
        state.displays = vec![("output-1".to_string(), 0, 40, 1200, 860)];
        let bus = FakeBus::new(state);
        let mut system = KwinWindowSystem::with_bus_address(bus.address.clone());
        let mut history = WindowHistory::default();
        let zones = std::collections::BTreeMap::new();
        for (action, observed) in [
            (
                Action::MoveToZone {
                    zone: "left-half".to_string(),
                },
                constrained,
            ),
            (Action::Restore {}, interrupted_restore),
        ] {
            bus.update_state(|state| {
                state.move_observed = Some(observed);
                state.move_error = Some((
                    "org.window_zones.KWin.Error.PartialMove",
                    serde_json::json!({
                        "window_id": "window-1",
                        "observed": [observed.x, observed.y, observed.width as i64, observed.height as i64],
                        "message": "client constrained the frame",
                    }).to_string(),
                ));
            });
            assert!(matches!(
                execute_action(&action, &zones, &mut history, &mut system),
                Err(ExecuteActionError::WindowSystem(WindowSystemError::PartialMove {
                    observed: actual, ..
                })) if actual == observed
            ));
        }
        bus.update_state(|state| {
            state.move_error = None;
            state.move_observed = Some(original);
        });
        execute_action(&Action::Restore {}, &zones, &mut history, &mut system).unwrap();
        bus.update_state(|state| {
            assert_eq!(
                state.moves,
                vec![("window-1".to_string(), 90, 100, 700, 500)]
            );
        });
        assert_eq!(
            execute_action(&Action::Restore {}, &zones, &mut history, &mut system),
            Err(ExecuteActionError::NothingToRestore)
        );
    }

    #[test]
    fn partial_move_payloads_require_the_captured_identity_and_valid_frame() {
        let bus = FakeBus::new(FakeServiceState::complete());
        let mut system = KwinWindowSystem::with_bus_address(bus.address.clone());
        let movement = WindowMove::new(WindowId::new("window-1"), Rect::new(0, 40, 600, 860));
        let valid = serde_json::json!({
            "window_id": "window-1",
            "observed": [0, 40, 800, 860],
            "message": "client minimum width",
        });
        bus.update_state(|state| {
            state.focused = Some(("window-2".to_string(), 900, 100, 200, 300));
            state.move_error = Some(("org.window_zones.KWin.Error.PartialMove", valid.to_string()));
        });
        assert!(matches!(
            system.move_window(&movement),
            Err(WindowSystemError::PartialMove { observed, .. })
                if observed == Rect::new(0, 40, 800, 860)
        ));
        for (field, value) in [
            ("window_id", serde_json::json!("window-2")),
            ("window_id", serde_json::json!(1)),
            ("observed", serde_json::json!([0, 40, 0, 860])),
            ("observed", serde_json::json!([0, 40, 4294967296_u64, 860])),
            ("observed", serde_json::json!([0.5, 40, 800, 860])),
            ("message", serde_json::Value::Null),
            ("unexpected", serde_json::json!(true)),
        ] {
            let mut payload = valid.clone();
            payload[field] = value;
            bus.update_state(|state| {
                state.move_error = Some((
                    "org.window_zones.KWin.Error.PartialMove",
                    payload.to_string(),
                ));
            });
            assert!(
                matches!(
                    system.move_window(&movement),
                    Err(WindowSystemError::Platform(_))
                ),
                "invalid payload must not change history: {payload}"
            );
        }
    }

    #[test]
    fn committed_hotkey_replacement_is_not_vetoed_by_a_late_accelerator_sampler() {
        let mut state = FakeServiceState::complete();
        state.expose_stale_accelerator = true;
        let bus = FakeBus::new(state);
        let mut system = KwinHotkeySystem::with_bus_address(bus.address.clone());
        system
            .register_hotkeys(&["alt+ctrl+left".to_string()])
            .unwrap();
        system
            .register_hotkeys(&["alt+ctrl+right".to_string()])
            .unwrap();
        bus.update_state(|state| {
            assert_eq!(state.registered_hotkeys, vec!["alt+ctrl+right"]);
        });
    }

    #[test]
    fn hotkey_conflict_is_rejected_and_previous_set_survives() {
        let bus = FakeBus::new(FakeServiceState::complete());
        let mut system = KwinHotkeySystem::with_bus_address(bus.address.clone());
        system
            .register_hotkeys(&["alt+ctrl+left".to_string()])
            .unwrap();
        bus.update_state(|state| {
            state.request_error = Some((
                "org.window_zones.KWin.Error.Unsupported",
                "accelerator is already held; choose a different binding".to_string(),
            ));
        });
        assert!(matches!(
            system.register_hotkeys(&["alt+ctrl+right".to_string()]),
            Err(HotkeySystemError::Rejected(_))
        ));
        bus.update_state(|state| {
            assert_eq!(state.registered_hotkeys, vec!["alt+ctrl+left"]);
            state.request_error = None;
        });
        drop(system);
        bus.update_state(|state| assert!(state.registered_hotkeys.is_empty()));
    }

    #[test]
    fn real_service_hotkey_polling_never_claims_controller() {
        const CHILD_ENV: &str = "WINDOW_ZONES_KWIN_SERVICE_TEST_CHILD";
        if std::env::var_os(CHILD_ENV).is_some() {
            run_kwin_companion_service().unwrap();
            return;
        }

        struct Process(Child);
        impl Drop for Process {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let mut daemon = Process(
            Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address=1"])
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("dbus-daemon must be installed for KDE contract tests"),
        );
        let mut address = String::new();
        BufReader::new(daemon.0.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let address = address.trim();
        assert!(!address.is_empty());
        let mut service = Process(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "kwin_integration::tests::real_service_hotkey_polling_never_claims_controller",
                    "--nocapture",
                ])
                .env(CHILD_ENV, "1")
                .env("DBUS_SESSION_BUS_ADDRESS", address)
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let script = Connection::new_address(address).unwrap();
        let bus = script.with_proxy(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            Duration::from_secs(1),
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let (owned,): (bool,) = bus
                .method_call("org.freedesktop.DBus", "NameHasOwner", (KWIN_SERVICE_NAME,))
                .unwrap();
            if owned {
                break;
            }
            assert!(
                service.0.try_wait().unwrap().is_none(),
                "real KWin companion exited before owning its bus name"
            );
            assert!(Instant::now() < deadline, "real KWin service never started");
            thread::sleep(Duration::from_millis(10));
        }
        let proxy = script.with_proxy(KWIN_SERVICE_NAME, KWIN_OBJECT_PATH, CALL_TIMEOUT);
        proxy
            .method_call::<(), _, _, _>(
                KWIN_INTERFACE,
                "RegisterCompanion",
                (KWIN_PROTOCOL_MAJOR.to_string(), true),
            )
            .unwrap();
        let old = Connection::new_address(address).unwrap();
        let new = Connection::new_address(address).unwrap();
        let register = |connection: &Connection, hotkeys: &[&str]| {
            let (id,): (u32,) = connection
                .with_proxy(KWIN_SERVICE_NAME, KWIN_OBJECT_PATH, CALL_TIMEOUT)
                .method_call(
                    KWIN_INTERFACE,
                    "RegisterHotkeys",
                    (serde_json::to_string(hotkeys).unwrap(),),
                )?;
            Ok::<_, dbus::Error>(id)
        };
        let complete_registration = |id: u32, hotkeys: &[&str]| {
            let request: NextRequestPayload = proxy
                .method_call(KWIN_INTERFACE, "NextRequest", ())
                .unwrap();
            assert_eq!(request.0, id);
            assert_eq!(request.1, REQUEST_REGISTER_HOTKEYS);
            assert_eq!(
                serde_json::from_str::<Vec<String>>(&request.7).unwrap(),
                hotkeys
            );
            proxy
                .method_call::<(), _, _, _>(
                    KWIN_INTERFACE,
                    "CompleteRequest",
                    (id.to_string(), true, String::new(), String::new()),
                )
                .unwrap();
        };
        let poll = |connection: &Connection| {
            connection
                .with_proxy(KWIN_SERVICE_NAME, KWIN_OBJECT_PATH, CALL_TIMEOUT)
                .method_call::<(bool, String), _, _, _>(KWIN_INTERFACE, "GetNextHotkey", ())
        };

        complete_registration(
            register(&old, &["alt+ctrl+left"]).unwrap(),
            &["alt+ctrl+left"],
        );
        assert_eq!(
            register(&new, &["alt+ctrl+right"]).unwrap_err().name(),
            Some("org.window_zones.KWin.Error.Busy")
        );
        complete_registration(register(&old, &[]).unwrap(), &[]);
        for _ in 0..2 {
            assert_eq!(poll(&old).unwrap(), (false, String::new()));
            assert_eq!(poll(&new).unwrap(), (false, String::new()));
        }
        for hotkey in ["ctrl+alt+left", "ALT+left", "alt+enter"] {
            assert_eq!(
                register(&old, &[hotkey]).unwrap_err().name(),
                Some("org.window_zones.KWin.Error.Unsupported")
            );
        }
        complete_registration(
            register(&new, &["alt+ctrl+right"]).unwrap(),
            &["alt+ctrl+right"],
        );
        proxy
            .method_call::<(), _, _, _>(
                KWIN_INTERFACE,
                "HotkeyPressed",
                ("alt+ctrl+right".to_string(),),
            )
            .unwrap();
        assert_eq!(
            poll(&old).unwrap_err().name(),
            Some("org.window_zones.KWin.Error.Busy")
        );
        assert_eq!(poll(&new).unwrap(), (true, "alt+ctrl+right".to_string()));
        assert_eq!(poll(&new).unwrap(), (false, String::new()));
    }

    #[test]
    fn idle_next_request_waits_but_queued_move_wakes_it() {
        let bus = FakeBus::new(FakeServiceState::complete());
        let script = Connection::new_address(&bus.address).unwrap();
        let proxy = script.with_proxy(KWIN_SERVICE_NAME, KWIN_OBJECT_PATH, Duration::from_secs(3));
        proxy
            .method_call::<(), _, _, _>(
                KWIN_INTERFACE,
                "RegisterCompanion",
                (KWIN_PROTOCOL_MAJOR.to_string(), true),
            )
            .unwrap();
        for _ in 0..2 {
            let started = Instant::now();
            let reply: NextRequestPayload = proxy
                .method_call(KWIN_INTERFACE, "NextRequest", ())
                .unwrap();
            assert_eq!(reply.0, 0);
            assert!(
                started.elapsed() >= NEXT_REQUEST_WAIT,
                "idle replies must not busy-poll"
            );
            assert!(started.elapsed() < NEXT_REQUEST_WAIT + Duration::from_secs(1));
        }
        let state = Arc::clone(&bus.state);
        let producer = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let mut state = state.lock();
                if state.script_state.pending_next_request.is_some() {
                    return state.script_state.enqueue_move(
                        "window-1".to_string(),
                        -1,
                        2,
                        800,
                        600,
                    );
                }
                drop(state);
                assert!(
                    Instant::now() < deadline,
                    "script long poll was never deferred"
                );
                thread::sleep(Duration::from_millis(1));
            }
        });
        let started = Instant::now();
        let reply: NextRequestPayload = proxy
            .method_call(KWIN_INTERFACE, "NextRequest", ())
            .unwrap();
        assert_eq!(reply.0, producer.join().unwrap());
        assert_eq!(reply.1, REQUEST_MOVE);
        assert_eq!(reply.2, "window-1");
        assert!(
            started.elapsed() < NEXT_REQUEST_WAIT,
            "queued moves must wake the long poll"
        );
    }

    #[test]
    fn script_moves_identity_restores_state_and_connects_geometry_once() {
        let output = Command::new("node")
            .args(["-e", r#"
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
function signal() {
    const handlers = new Set();
    return {
        connect: callback => handlers.add(callback),
        disconnect: callback => handlers.delete(callback),
        emit: value => Array.from(handlers).forEach(callback => callback(value)),
        count: () => handlers.size,
    };
}
function window(id) {
    const calls = [];
    let geometry = {x: 0, y: 0, width: 640, height: 480};
    return {
        internalId: id, managed: true, normalWindow: true, moveable: true, resizeable: true,
        frameGeometryChanged: signal(), calls,
        setMaximize: (vertical, horizontal) => calls.push([vertical, horizontal]),
        get frameGeometry() { return geometry; },
        set frameGeometry(value) { calls.push('frame'); geometry = value; },
    };
}
const first = window('window-1');
const second = window('window-2');
const updates = [];
const workspace = {
    activeWindow: first, screens: [], stackingOrder: [first, second],
    windowActivated: signal(), windowRemoved: signal(),
};
const context = vm.createContext({
    workspace, console,
    QTimer: function() { this.timeout = signal(); this.start = () => {}; this.stop = () => {}; },
    callDBus: (...args) => { if (args[3] === 'UpdateFocusedWindow') updates.push(JSON.parse(args[4])); },
});
vm.runInContext(fs.readFileSync(process.argv[1], 'utf8'), context);
for (let i = 0; i < 100; i++) {
    workspace.activeWindow = i % 2 ? first : second;
    workspace.windowActivated.emit(workspace.activeWindow);
}
assert.equal(first.frameGeometryChanged.count(), 1);
assert.equal(second.frameGeometryChanged.count(), 1);
updates.length = 0;
first.frameGeometryChanged.emit();
assert.equal(updates.length, 1);
assert.equal(updates[0][1], 'window-1');
workspace.activeWindow = second;
first.tile = {unmanage: value => {
    assert.equal(value, first);
    first.calls.push('untile');
    first.tile = null;
}};
vm.runInContext("moveWindow('window-1', -10, 20, 800, 600, error => { if (error) throw error; })", context);
assert.equal(JSON.stringify(first.frameGeometry), '{"x":-10,"y":20,"width":800,"height":600}');
assert.deepEqual(first.calls, ['untile', [false, false], 'frame']);
assert.equal(second.calls.length, 0);
first.calls.length = 0;
first.tile = {};
vm.runInContext("moveWindow('window-1', 10, 20, 800, 600, error => { if (error) throw error; })", context);
assert.equal(first.tile, null);
assert.deepEqual(first.calls, [[true, true], [false, false], 'frame']);
first.fullScreen = true;
first.calls.length = 0;
assert.throws(() => vm.runInContext("moveWindow('window-1', 0, 0, 800, 600)", context), /leave fullscreen first/);
assert.equal(first.calls.length, 0);
assert.throws(() => vm.runInContext("moveWindow('missing', 0, 0, 800, 600)", context),
    error => error.dbusError === 'org.window_zones.KWin.Error.WindowGone' && error.message === 'missing');
workspace.windowRemoved.emit(first);
assert.equal(first.frameGeometryChanged.count(), 0);
for (const hotkey of ['ctrl+alt+a', 'ALT+a', 'alt+ctrl+f25', 'alt++a', 'alt+enter']) {
    assert.throws(() => vm.runInContext(`shortcutForHotkey(${JSON.stringify(hotkey)})`, context),
        error => error.dbusError === 'org.window_zones.KWin.Error.Unsupported');
}
"#])
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/kwin-script/contents/code/main.js"))
            .output()
            .expect("node must be installed for KWin script contract tests");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
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
    fn script_releases_native_keys_readds_and_refreshes_reserved_work_areas() {
        let output = Command::new("node")
            .args(["-e", r#"
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
function signal() {
    const handlers = [];
    return {connect: callback => handlers.push(callback), emit: () => handlers.forEach(callback => callback())};
}
const left = 'alt+ctrl+left', right = 'alt+ctrl+right', extra = 'alt+ctrl+f1';
const actions = new Map(), nativeKeys = new Map(), writes = [], messages = [];
const timers = [];
let lostClearReply = null;
let workArea = {x: 0, y: 0, width: 1200, height: 860};
const workspace = {
    screens: [{name: 'output-1'}], currentDesktop: {}, activeWindow: null,
    clientArea: () => workArea, currentDesktopChanged: signal(), screensChanged: signal(),
};
const context = vm.createContext({
    workspace, KWin: {MaximizeArea: 2}, console,
    QTimer: function() { this.timeout = signal(); this.start = () => {}; this.stop = () => {}; timers.push(this); },
    registerShortcut: (title, text, sequence, callback) => {
        assert.equal(actions.has(title), false, 'reuse the existing QAction on re-add');
        actions.set(title, callback);
        return true;
    },
    callDBus: (service, path, iface, method, ...args) => {
        const callback = args.pop();
        if (method === 'RegisterCompanion' || method === 'NextRequest')
            return;
        if (method === 'action') {
            const owner = Array.from(nativeKeys).find(([hotkey, key]) => key === args[0]);
            callback(owner ? ['kwin', `Window Zones Hotkey ${owner[0]}`, '', ''] : []);
        } else if (method === 'SetShortcut') {
            assert.ok(actions.has(`Window Zones Hotkey ${args[0]}`));
            writes.push(args);
            if (args[1] === 0)
                nativeKeys.delete(args[0]);
            else
                nativeKeys.set(args[0], args[1]);
            if (args[0] === lostClearReply && args[1] === 0)
                return;
            callback();
        } else {
            messages.push([method, ...args]);
            callback();
        }
    },
});
vm.runInContext(fs.readFileSync(process.argv[1], 'utf8'), context);
function register(keys) {
    context.keys = keys;
    vm.runInContext('registerHotkeys(keys, error => { globalThis.failure = error; globalThis.finished = true; })', context);
    assert.equal(context.finished, true);
    context.finished = false;
    return context.failure;
}
assert.equal(register([left, right]), undefined);
assert.deepEqual(Array.from(nativeKeys.keys()), [left, right]);
lostClearReply = left;
context.keys = [right, extra];
vm.runInContext('registerHotkeys(keys, error => { globalThis.failure = error; globalThis.finished = true; })', context);
assert.equal(context.finished, false);
timers.at(-1).timeout.emit();
assert.equal(context.failure.dbusError, 'org.window_zones.KWin.Error.Unavailable');
assert.equal(nativeKeys.has(left), false, 'the native clear succeeded before its reply was lost');
lostClearReply = null;
assert.equal(register([left, right]), undefined);
assert.deepEqual(Array.from(nativeKeys.keys()).sort(), [left, right].sort(),
    'retrying the old set must restore a key cleared by a partially failed update');
assert.equal(register([right, extra]), undefined);
assert.deepEqual(Array.from(nativeKeys.keys()), [right, extra]);
assert.ok(writes.some(([hotkey, key]) => hotkey === left && key === 0));
const before = writes.length;
assert.throws(() => register([right, 'alt+ctrl+f25']), /unsupported KWin shortcut key/);
assert.equal(writes.length, before, 'preflight rejection must not mutate grabs');
assert.deepEqual(Array.from(nativeKeys.keys()), [right, extra]);
const incumbentKey = vm.runInContext("shortcutForHotkey('alt+ctrl+f2').key", context);
nativeKeys.set('incumbent', incumbentKey);
assert.equal(register([right, 'alt+ctrl+f2']).dbusError, 'org.window_zones.KWin.Error.Unsupported');
assert.equal(writes.length, before, 'native conflict must reject before mutation');
nativeKeys.delete('incumbent');
assert.equal(register([]), undefined);
assert.equal(nativeKeys.size, 0, 'empty set must release native keys, not just callbacks');
assert.equal(register([left]), undefined);
assert.equal(nativeKeys.size, 1);
assert.equal(actions.size, 3);
actions.get(`Window Zones Hotkey ${left}`)();
assert.ok(messages.some(message => message[0] === 'HotkeyPressed' && message[1] === left));
messages.length = 0;
workArea = {x: 0, y: 50, width: 1200, height: 810};
vm.runInContext("processRequest(10, 'refresh-displays', '', 0, 0, 0, 0, '[]', () => {})", context);
assert.deepEqual(JSON.parse(messages[0][1]), [['output-1', 0, 50, 1200, 810]]);
assert.equal(messages[1][0], 'CompleteRequest');
assert.equal(messages[1][2], true, 'complete only after the fresh state was published');
messages.length = 0;
workArea = {x: 0, y: 0, width: 1200, height: 900};
workspace.currentDesktopChanged.emit();
assert.deepEqual(JSON.parse(messages[0][1]), [['output-1', 0, 0, 1200, 900]]);
"#])
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/kwin-script/contents/code/main.js"))
            .output()
            .expect("node must be installed for KWin script contract tests");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn request_payloads_are_claimed_and_complete_atomically() {
        let mut state = CompanionState::default();
        state
            .mark_ready(KWIN_PROTOCOL_MAJOR, "kwin-script".to_string(), true)
            .unwrap();
        let request_id = state.enqueue_move("window-1".to_string(), -123, 456, 777, 888);

        assert_eq!(
            state.next_request("kwin-script").unwrap(),
            (
                request_id,
                REQUEST_MOVE.to_string(),
                "window-1".to_string(),
                -123,
                456,
                777,
                888,
                "[]".to_string()
            )
        );
        assert_eq!(
            state.request_result(request_id),
            (false, String::new(), String::new())
        );
        state
            .complete_request(request_id, true, String::new(), String::new())
            .unwrap();
        assert_eq!(
            state.request_result(request_id),
            (true, String::new(), String::new())
        );
    }

    #[test]
    fn request_ids_start_at_one_wrap_and_survive_reconnection() {
        let mut state = CompanionState::default();
        state
            .mark_ready(KWIN_PROTOCOL_MAJOR, "kwin-script".to_string(), true)
            .unwrap();
        for expected in [1, u32::MAX, 1] {
            if expected == u32::MAX {
                state.last_request_id = u32::MAX - 1;
            }
            assert_eq!(state.enqueue_refresh_displays(), expected);
            assert_eq!(state.next_request("kwin-script").unwrap().0, expected);
            state
                .complete_request(expected, true, String::new(), String::new())
                .unwrap();
            assert!(state.request_result(expected).0);
        }

        state
            .mark_ready(KWIN_PROTOCOL_MAJOR, "replacement-script".to_string(), true)
            .unwrap();
        assert_eq!(state.enqueue_refresh_displays(), 2);
        assert_eq!(state.next_request("replacement-script").unwrap().0, 2);
        assert_eq!(state.next_request("replacement-script").unwrap().0, 0);
    }

    #[test]
    fn rejected_hotkey_registration_keeps_previous_set() {
        let mut state = CompanionState::default();
        state
            .mark_ready(KWIN_PROTOCOL_MAJOR, "kwin-script".to_string(), true)
            .unwrap();
        let first = state.enqueue_hotkeys(vec!["alt+ctrl+left".to_string()]);
        let _ = state.next_request("kwin-script").unwrap();
        state
            .complete_request(first, true, String::new(), String::new())
            .unwrap();
        let _ = state.request_result(first);
        assert_eq!(state.registered_hotkeys, vec!["alt+ctrl+left"]);

        let second = state.enqueue_hotkeys(vec!["alt+ctrl+right".to_string()]);
        let _ = state.next_request("kwin-script").unwrap();
        state
            .complete_request(
                second,
                false,
                "org.window_zones.KWin.Error.Unsupported".to_string(),
                "KWin rejected shortcut".to_string(),
            )
            .unwrap();
        assert_eq!(state.request_result(second).2, "KWin rejected shortcut");
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
            .complete_request(request_id, true, String::new(), String::new())
            .unwrap();

        state
            .authorize_controller("window-zones-2".to_string())
            .unwrap();
        let request_id = state.enqueue_hotkeys(vec!["alt+ctrl+left".to_string()]);
        let _ = state.next_request("kwin-script").unwrap();
        state
            .complete_request(request_id, true, String::new(), String::new())
            .unwrap();
        assert_eq!(state.registered_hotkeys, vec!["alt+ctrl+left"]);
    }

    #[test]
    fn disconnect_release_does_not_release_the_next_controllers_claim() {
        let mut state = CompanionState::default();
        state
            .mark_ready(KWIN_PROTOCOL_MAJOR, "kwin-script".to_string(), true)
            .unwrap();
        state.authorize_controller("old".to_string()).unwrap();
        let first = state.enqueue_hotkeys(vec!["alt+ctrl+left".to_string()]);
        state.next_request("kwin-script").unwrap();
        state
            .complete_request(first, true, String::new(), String::new())
            .unwrap();
        state.owner_lost("old");
        state.authorize_controller("new".to_string()).unwrap();
        let replacement = state.enqueue_hotkeys(vec!["alt+ctrl+right".to_string()]);

        let release = state.next_request("kwin-script").unwrap();
        assert_eq!(release.7, "[]");
        state
            .complete_request(release.0, true, String::new(), String::new())
            .unwrap();
        assert_eq!(state.controller_sender.as_deref(), Some("new"));
        assert_eq!(state.next_request("kwin-script").unwrap().0, replacement);
        state
            .complete_request(replacement, true, String::new(), String::new())
            .unwrap();
        assert_eq!(state.registered_hotkeys, vec!["alt+ctrl+right"]);
        assert!(state.authorize_controller("third".to_string()).is_err());
    }

    #[test]
    fn companion_reconnection_releases_keys_left_after_state_loss() {
        let mut state = CompanionState::default();
        state
            .mark_ready(KWIN_PROTOCOL_MAJOR, "kwin-script".to_string(), true)
            .unwrap();
        state.authorize_controller("old".to_string()).unwrap();
        let first = state.enqueue_hotkeys(vec!["alt+ctrl+left".to_string()]);
        state.next_request("kwin-script").unwrap();
        state
            .complete_request(first, true, String::new(), String::new())
            .unwrap();
        state.last_seen =
            Some(Instant::now() - COMPANION_HEARTBEAT_TIMEOUT - Duration::from_secs(1));
        assert!(state.ensure_ready().is_err());
        state.owner_lost("old");

        state
            .mark_ready(KWIN_PROTOCOL_MAJOR, "kwin-script".to_string(), false)
            .unwrap();
        let (_, kind, _, _, _, _, _, hotkeys) = state.next_request("kwin-script").unwrap();
        assert_eq!(kind, REQUEST_REGISTER_HOTKEYS);
        assert_eq!(hotkeys, "[]");
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
        state
            .complete_request(first, true, String::new(), String::new())
            .unwrap();
        let _ = state.request_result(first);

        state
            .mark_ready(KWIN_PROTOCOL_MAJOR, "kwin-script".to_string(), true)
            .unwrap();
        assert_eq!(state.controller_sender.as_deref(), Some("window-zones-1"));
        let (request_id, kind, _, _, _, _, _, hotkeys) = state.next_request("kwin-script").unwrap();
        assert_ne!(request_id, 0);
        assert_eq!(kind, REQUEST_REGISTER_HOTKEYS);
        assert_eq!(hotkeys, r#"["alt+ctrl+left"]"#);
        state
            .complete_request(request_id, true, String::new(), String::new())
            .unwrap();
        let _ = state.request_result(request_id);

        state
            .mark_ready(KWIN_PROTOCOL_MAJOR, "kwin-script".to_string(), false)
            .unwrap();
        assert_eq!(
            state.next_request("kwin-script").unwrap(),
            (
                0,
                String::new(),
                String::new(),
                0,
                0,
                0,
                0,
                "[]".to_string()
            )
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
        let (_, kind, _, _, _, _, _, payload) = state.next_request("kwin-script").unwrap();
        assert_eq!(kind, REQUEST_REGISTER_HOTKEYS);
        assert_eq!(
            payload, "[]",
            "disconnect must reach the script's native release path"
        );
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
