use std::time::Duration;

use dbus_tree::MethodErr;
use thiserror::Error;

use crate::{HotkeySystemError, Rect, WindowId, WindowSystemError};

pub const KWIN_SERVICE_NAME: &str = "org.window_zones.KWin";
pub const KWIN_OBJECT_PATH: &str = "/org/window_zones/KWin";
pub const KWIN_INTERFACE: &str = "org.window_zones.KWin3";
pub const KWIN_PROTOCOL_MAJOR: u32 = 3;

pub(super) const CALL_TIMEOUT: Duration = Duration::from_secs(1);
pub(super) const FOCUSED_WINDOW_CAPABILITY: &str = "focused-window";
pub(super) const DISPLAYS_CAPABILITY: &str = "displays";
pub(super) const MOVE_RESIZE_CAPABILITY: &str = "move-resize";
pub(super) const HOTKEYS_CAPABILITY: &str = "hotkeys";
pub(super) const REQUEST_MOVE: &str = "move";
pub(super) const REQUEST_REFRESH_DISPLAYS: &str = "refresh-displays";
pub(super) const REQUEST_REGISTER_HOTKEYS: &str = "register-hotkeys";

pub(super) type DisplayPayload = (String, i32, i32, u32, u32);
pub(super) type NextRequestPayload = (u32, String, String, i32, i32, u32, u32, String);

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
    pub(super) fn should_reconnect(&self) -> bool {
        matches!(self, Self::Unavailable { .. } | Self::Incompatible { .. })
    }

    pub(super) fn into_window_error(self) -> WindowSystemError {
        match self {
            Self::WindowGone { window } => WindowSystemError::WindowGone(window),
            _ => WindowSystemError::Platform(self.to_string()),
        }
    }

    /// A refused set leaves the companion's previous set registered; every other
    /// failure means nothing is known to be registered.
    pub(super) fn into_hotkey_error(self) -> HotkeySystemError {
        match self {
            Self::Unsupported { .. } | Self::Invalid { .. } => {
                HotkeySystemError::Rejected(self.to_string())
            }
            _ => HotkeySystemError::Unavailable(self.to_string()),
        }
    }
}

pub(super) fn classify_dbus_error(error: dbus::Error) -> KwinIntegrationError {
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

pub(super) fn method_error(name: &'static str, message: &'static str) -> MethodErr {
    MethodErr::from((name, message))
}

pub(super) fn parse_hotkeys(raw: &str) -> Result<Vec<String>, MethodErr> {
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

pub(super) fn parse_displays(raw: &str) -> Result<Vec<DisplayPayload>, MethodErr> {
    serde_json::from_str(raw).map_err(|_| {
        method_error(
            "org.window_zones.KWin.Error.Invalid",
            "display update payload is not valid JSON",
        )
    })
}
pub(super) fn parse_u32_argument(raw: &str, message: &'static str) -> Result<u32, MethodErr> {
    raw.parse()
        .map_err(|_| method_error("org.window_zones.KWin.Error.Invalid", message))
}

pub(super) fn sender_name(message: &dbus::Message) -> Result<String, MethodErr> {
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
