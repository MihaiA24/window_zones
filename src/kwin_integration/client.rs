use std::fmt;
use std::thread;
use std::time::{Duration, Instant};

use dbus::arg::{AppendAll, ReadAll};
use dbus::blocking::Connection;
use parking_lot::Mutex;

use crate::{
    DisplayGeometry, FocusedWindow, HotkeyEvent, HotkeySystem, HotkeySystemError, Rect, WindowId,
    WindowMove, WindowSystem, WindowSystemError,
};

use super::protocol::{
    CALL_TIMEOUT, DISPLAYS_CAPABILITY, DisplayPayload, FOCUSED_WINDOW_CAPABILITY,
    HOTKEYS_CAPABILITY, KWIN_INTERFACE, KWIN_OBJECT_PATH, KWIN_SERVICE_NAME, KwinIntegrationError,
    MOVE_RESIZE_CAPABILITY, classify_dbus_error,
};

#[derive(Default)]
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
        Self::default()
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

#[derive(Default)]
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
        Self::default()
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

#[cfg(test)]
#[path = "tests/adapters.rs"]
mod tests;
