//! Native window control provided by the opt-in patched COSMIC compositor.
//! Key capture stays with COSMIC custom-command shortcuts, like Sway/Hyprland.

use std::env;
use std::fmt;
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use dbus::arg::{AppendAll, ReadAll};
use dbus::blocking::Connection;
use parking_lot::Mutex;

use crate::{
    DisplayGeometry, FocusedWindow, Rect, WindowId, WindowMove, WindowSystem, WindowSystemError,
};

pub const COSMIC_SERVICE_NAME: &str = "org.window_zones.Cosmic";
pub const COSMIC_OBJECT_PATH: &str = "/org/window_zones/Cosmic";
pub const COSMIC_INTERFACE: &str = "org.window_zones.Cosmic1";
const CALL_TIMEOUT: Duration = Duration::from_secs(3);
type DisplayPayload = (String, i32, i32, u32, u32);
type MoveReply = (u32, i32, i32, u32, u32, String);

struct BridgeConnection {
    connection: Connection,
    owner: String,
    socket_path: String,
    socket_identity: (u64, u64),
}

#[derive(Default)]
pub struct CosmicWindowSystem {
    connection: Mutex<Option<BridgeConnection>>,
}

impl fmt::Debug for CosmicWindowSystem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CosmicWindowSystem")
            .field("connected", &self.connection.lock().is_some())
            .finish()
    }
}

impl CosmicWindowSystem {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn capabilities(&self) -> Result<Vec<String>, WindowSystemError> {
        self.call::<(Vec<String>,), _>("GetCapabilities", ())
            .map(|(capabilities,)| capabilities)
    }

    fn call<R: ReadAll, A: AppendAll>(
        &self,
        method: &str,
        args: A,
    ) -> Result<R, WindowSystemError> {
        let mut slot = self.connection.lock();
        if slot.is_none() {
            let connection = Connection::new_session().map_err(bridge_unavailable)?;
            let (owner,): (String,) = connection
                .with_proxy(
                    "org.freedesktop.DBus",
                    "/org/freedesktop/DBus",
                    CALL_TIMEOUT,
                )
                .method_call(
                    "org.freedesktop.DBus",
                    "GetNameOwner",
                    (COSMIC_SERVICE_NAME,),
                )
                .map_err(bridge_unavailable)?;
            let (socket_path,): (String,) = connection
                .with_proxy(owner.as_str(), COSMIC_OBJECT_PATH, CALL_TIMEOUT)
                .method_call(COSMIC_INTERFACE, "GetSession", ())
                .map_err(bridge_unavailable)?;
            let socket_identity = validate_session(&socket_path)?;
            *slot = Some(BridgeConnection {
                connection,
                owner,
                socket_path,
                socket_identity,
            });
        } else {
            let bridge = slot.as_ref().unwrap();
            if validate_session(&bridge.socket_path)? != bridge.socket_identity {
                return Err(bridge_unavailable(
                    "compositor socket was replaced; close and relaunch Window Zones",
                ));
            }
        }
        let bridge = slot.as_ref().unwrap();
        // Never retarget a live adapter's captured window IDs to a replacement
        // compositor. Explicit runtime restart creates a new adapter.
        bridge
            .connection
            .with_proxy(bridge.owner.as_str(), COSMIC_OBJECT_PATH, CALL_TIMEOUT)
            .method_call(COSMIC_INTERFACE, method, args)
            .map_err(|error| {
                WindowSystemError::Platform(format!(
                    "COSMIC bridge {method} failed: {error}; ensure the compositor implements {COSMIC_INTERFACE}, then restart Window Zones if the bridge restarted"
                ))
            })
    }
}

fn bridge_unavailable(error: impl fmt::Display) -> WindowSystemError {
    WindowSystemError::Platform(format!(
        "COSMIC bridge is unavailable: {error}; stock COSMIC has no arbitrary window placement interface. Build with scripts/build-cosmic.sh and start that compositor with WINDOW_ZONES_COSMIC_BRIDGE=1"
    ))
}

fn validate_session(bridge_socket: &str) -> Result<(u64, u64), WindowSystemError> {
    let display = env::var_os("WAYLAND_DISPLAY")
        .filter(|display| !display.is_empty())
        .ok_or_else(|| bridge_unavailable("WAYLAND_DISPLAY is not set"))?;
    let mut selected = PathBuf::from(display);
    if !selected.is_absolute() {
        let runtime = env::var_os("XDG_RUNTIME_DIR")
            .filter(|runtime| Path::new(runtime).is_absolute())
            .ok_or_else(|| bridge_unavailable("an absolute XDG_RUNTIME_DIR is required"))?;
        selected = PathBuf::from(runtime).join(selected);
    }
    let bridge = Path::new(bridge_socket);
    if !bridge.is_absolute() {
        return Err(bridge_unavailable(
            "bridge returned a relative Wayland socket",
        ));
    }
    let selected_metadata = fs::metadata(&selected).map_err(bridge_unavailable)?;
    let bridge_metadata = fs::metadata(bridge).map_err(bridge_unavailable)?;
    if !selected_metadata.file_type().is_socket()
        || !bridge_metadata.file_type().is_socket()
        || selected_metadata.dev() != bridge_metadata.dev()
        || selected_metadata.ino() != bridge_metadata.ino()
    {
        return Err(bridge_unavailable(format!(
            "bridge belongs to {}, not the selected {} compositor",
            bridge.display(),
            selected.display()
        )));
    }
    Ok((selected_metadata.dev(), selected_metadata.ino()))
}

impl WindowSystem for CosmicWindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
        let (present, id, x, y, width, height): (bool, u64, i32, i32, u32, u32) =
            self.call("GetFocusedWindow", ())?;
        if !present {
            return Ok(None);
        }
        if id == 0 || width == 0 || height == 0 {
            return Err(bridge_unavailable(
                "invalid focused-window frame or identity",
            ));
        }
        Ok(Some(FocusedWindow::new(
            WindowId::new(id.to_string()),
            Rect::new(x, y, width, height),
        )))
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        let (displays,): (Vec<DisplayPayload>,) = self.call("GetDisplays", ())?;
        displays
            .into_iter()
            .map(|(id, x, y, width, height)| {
                if id.is_empty() || width == 0 || height == 0 {
                    return Err(bridge_unavailable("invalid display usable area"));
                }
                Ok(DisplayGeometry::new(id, Rect::new(x, y, width, height)))
            })
            .collect()
    }

    fn move_window(&mut self, window_move: &WindowMove) -> Result<(), WindowSystemError> {
        let id = window_move
            .window
            .as_str()
            .parse::<u64>()
            .map_err(|error| {
                WindowSystemError::Platform(format!("invalid COSMIC window identity: {error}"))
            })?;
        let target = window_move.target;
        if id == 0 || target.width == 0 || target.height == 0 {
            return Err(WindowSystemError::Platform(
                "COSMIC placement requires a window identity and positive frame dimensions".into(),
            ));
        }
        let (result, x, y, width, height, message): MoveReply = self.call(
            "MoveWindow",
            (id, target.x, target.y, target.width, target.height),
        )?;
        let observed = Rect::new(x, y, width, height);
        match result {
            0 if observed == target => Ok(()),
            0 | 3 if width > 0 && height > 0 => Err(WindowSystemError::PartialMove {
                observed,
                message: format!("COSMIC placement did not reach its target: {message}"),
            }),
            1 => Err(WindowSystemError::WindowGone(window_move.window.clone())),
            2 => Err(WindowSystemError::Platform(format!(
                "COSMIC refused placement: {message}"
            ))),
            _ => Err(bridge_unavailable(format!(
                "invalid MoveWindow result {result} or observed frame: {message}"
            ))),
        }
    }
}
