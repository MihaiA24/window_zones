#[cfg(target_os = "linux")]
use crate::{DisplayGeometry, FocusedWindow, Rect, WindowMove, WindowSystem, WindowSystemError};

#[cfg(target_os = "linux")]
use std::convert::TryFrom;

#[cfg(target_os = "linux")]
use x11rb::connection::Connection;
#[cfg(target_os = "linux")]
use x11rb::protocol::randr::{ConnectionExt as RandrConnectionExt, MonitorInfo};
#[cfg(target_os = "linux")]
use x11rb::protocol::xproto::{self, AtomEnum, ConnectionExt as XProtoConnectionExt, Window};
#[cfg(target_os = "linux")]
use x11rb::rust_connection::RustConnection;

#[cfg(target_os = "linux")]
const ACTIVE_WINDOW_ATOM: &str = "_NET_ACTIVE_WINDOW";

#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
pub struct X11WindowSystem;

#[cfg(target_os = "linux")]
impl X11WindowSystem {
    pub fn new() -> Self {
        Self
    }

    fn connect() -> Result<(RustConnection, usize), WindowSystemError> {
        x11rb::connect(None)
            .map_err(|error| WindowSystemError::Platform(format!("x11 connect failed: {error}")))
    }

    fn root_window(
        conn: &RustConnection,
        screen_index: usize,
    ) -> Result<Window, WindowSystemError> {
        let root = conn
            .setup()
            .roots
            .get(screen_index)
            .ok_or_else(|| {
                WindowSystemError::Platform("invalid X11 screen index in setup".to_string())
            })?
            .root;
        Ok(root)
    }

    fn active_window_id(
        conn: &RustConnection,
        root: Window,
    ) -> Result<Option<Window>, WindowSystemError> {
        let atom = conn
            .intern_atom(false, ACTIVE_WINDOW_ATOM.as_bytes())
            .map_err(|error| {
                WindowSystemError::Platform(format!("failed to intern atom: {error}"))
            })?
            .reply()
            .map_err(|error| {
                WindowSystemError::Platform(format!("failed to resolve atom reply: {error}"))
            })?
            .atom;

        let reply = conn
            .get_property(false, root, atom, AtomEnum::WINDOW, 0, 1)
            .map_err(|error| {
                WindowSystemError::Platform(format!("failed to query _NET_ACTIVE_WINDOW: {error}"))
            })?
            .reply()
            .map_err(|error| {
                WindowSystemError::Platform(format!(
                    "failed to fetch _NET_ACTIVE_WINDOW reply: {error}"
                ))
            })?;

        let Some(raw) = reply
            .value32()
            .and_then(|mut values| values.next())
            .map(|window| window as Window)
        else {
            return Ok(None);
        };

        if raw != 0 {
            return Ok(Some(raw));
        }

        let focused = conn
            .get_input_focus()
            .map_err(|error| {
                WindowSystemError::Platform(format!("failed to query focused window: {error}"))
            })?
            .reply()
            .map_err(|error| {
                WindowSystemError::Platform(format!("failed to read focused window reply: {error}"))
            })?;

        if focused.focus == 0 {
            Ok(None)
        } else {
            Ok(Some(focused.focus))
        }
    }

    fn frame_extents(conn: &RustConnection, window: Window) -> Result<[u32; 4], WindowSystemError> {
        let atom = conn
            .intern_atom(false, b"_NET_FRAME_EXTENTS")
            .map_err(|error| {
                WindowSystemError::Platform(format!("failed to intern frame extents atom: {error}"))
            })?
            .reply()
            .map_err(|error| {
                WindowSystemError::Platform(format!("failed to read frame extents atom: {error}"))
            })?
            .atom;
        let reply = conn
            .get_property(false, window, atom, AtomEnum::CARDINAL, 0, 4)
            .map_err(|error| {
                WindowSystemError::Platform(format!("failed to query frame extents: {error}"))
            })?
            .reply()
            .map_err(|error| {
                WindowSystemError::Platform(format!("failed to read frame extents: {error}"))
            })?;

        if reply.type_ == u32::from(AtomEnum::NONE) {
            return Ok([0; 4]);
        }
        reply
            .value32()
            .filter(|_| {
                reply.type_ == u32::from(AtomEnum::CARDINAL)
                    && reply.value_len == 4
                    && reply.bytes_after == 0
            })
            .and_then(|mut values| {
                Some([
                    values.next()?,
                    values.next()?,
                    values.next()?,
                    values.next()?,
                ])
            })
            .ok_or_else(|| {
                WindowSystemError::Platform("invalid _NET_FRAME_EXTENTS property".to_string())
            })
    }

    fn focused_window_geometry(
        conn: &RustConnection,
        window: Window,
        root: Window,
    ) -> Result<Rect, WindowSystemError> {
        let geometry = conn
            .get_geometry(window)
            .map_err(|error| {
                WindowSystemError::Platform(format!("failed to query window geometry: {error}"))
            })?
            .reply()
            .map_err(|error| {
                WindowSystemError::Platform(format!("failed to read window geometry: {error}"))
            })?;

        let origin = conn
            .translate_coordinates(window, root, 0, 0)
            .map_err(|error| {
                WindowSystemError::Platform(format!("failed to translate window origin: {error}"))
            })?
            .reply()
            .map_err(|error| {
                WindowSystemError::Platform(format!("failed to read window origin: {error}"))
            })?;
        let [left, right, top, bottom] = Self::frame_extents(conn, window)?;
        let invalid_frame =
            || WindowSystemError::Platform("window frame geometry is out of range".to_string());

        Ok(Rect::new(
            i32::try_from(i64::from(origin.dst_x) - i64::from(left))
                .map_err(|_| invalid_frame())?,
            i32::try_from(i64::from(origin.dst_y) - i64::from(top)).map_err(|_| invalid_frame())?,
            u32::from(geometry.width)
                .checked_add(left)
                .and_then(|width| width.checked_add(right))
                .ok_or_else(invalid_frame)?,
            u32::from(geometry.height)
                .checked_add(top)
                .and_then(|height| height.checked_add(bottom))
                .ok_or_else(invalid_frame)?,
        ))
    }
}

#[cfg(target_os = "linux")]
impl WindowSystem for X11WindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
        let (conn, screen_index) = Self::connect()?;
        let root = Self::root_window(&conn, screen_index)?;

        let window = match Self::active_window_id(&conn, root)? {
            Some(window) => window,
            None => return Ok(None),
        };

        let geometry = Self::focused_window_geometry(&conn, window, root)?;
        Ok(Some(FocusedWindow::new(geometry)))
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        let (conn, screen_index) = Self::connect()?;
        let root = Self::root_window(&conn, screen_index)?;
        collect_displays(&conn, root)
    }

    fn move_focused_window(&mut self, window_move: WindowMove) -> Result<(), WindowSystemError> {
        let (conn, screen_index) = Self::connect()?;
        let root = Self::root_window(&conn, screen_index)?;

        let window = Self::active_window_id(&conn, root)?
            .ok_or_else(|| WindowSystemError::Platform("no focused window".to_string()))?;

        let [left, right, top, bottom] = Self::frame_extents(&conn, window)?;
        let invalid_size = || {
            WindowSystemError::Platform("target size does not contain the window frame".to_string())
        };
        let x = window_move.target.x;
        let y = window_move.target.y;
        let width = window_move
            .target
            .width
            .checked_sub(left)
            .and_then(|width| width.checked_sub(right))
            .filter(|width| *width > 0)
            .ok_or_else(invalid_size)?;
        let height = window_move
            .target
            .height
            .checked_sub(top)
            .and_then(|height| height.checked_sub(bottom))
            .filter(|height| *height > 0)
            .ok_or_else(invalid_size)?;

        let mut atoms = [0; 3];
        for (atom, name) in atoms.iter_mut().zip([
            "_NET_WM_STATE",
            "_NET_WM_STATE_MAXIMIZED_HORZ",
            "_NET_WM_STATE_MAXIMIZED_VERT",
        ]) {
            *atom = conn
                .intern_atom(false, name.as_bytes())
                .map_err(|error| {
                    WindowSystemError::Platform(format!("failed to intern {name}: {error}"))
                })?
                .reply()
                .map_err(|error| {
                    WindowSystemError::Platform(format!("failed to read {name} atom: {error}"))
                })?
                .atom;
        }
        let [state, maximized_horz, maximized_vert] = atoms;
        let event = xproto::ClientMessageEvent::new(
            32,
            window,
            state,
            [0, maximized_horz, maximized_vert, 2, 0],
        );
        conn.send_event(
            false,
            root,
            xproto::EventMask::SUBSTRUCTURE_REDIRECT | xproto::EventMask::SUBSTRUCTURE_NOTIFY,
            event,
        )
        .map_err(|error| {
            WindowSystemError::Platform(format!("failed to unmaximize focused window: {error}"))
        })?;

        let values = xproto::ConfigureWindowAux::new()
            .x(x)
            .y(y)
            .width(width)
            .height(height);

        conn.configure_window(window, &values).map_err(|error| {
            WindowSystemError::Platform(format!("failed to configure focused window: {error}"))
        })?;
        conn.flush().map_err(|error| {
            WindowSystemError::Platform(format!("failed to flush X11 queue: {error}"))
        })?;

        Ok(())
    }
}

#[cfg(target_os = "linux")]
/// Uses RandR monitor bounds; panels and struts are not subtracted.
fn collect_displays(
    conn: &RustConnection,
    root: Window,
) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
    let monitors_reply = conn
        .randr_get_monitors(root, true)
        .map_err(|error| {
            WindowSystemError::Platform(format!("failed to query X11 monitors: {error}"))
        })?
        .reply()
        .map_err(|error| {
            WindowSystemError::Platform(format!("failed to read X11 monitor reply: {error}"))
        })?;

    if monitors_reply.monitors.is_empty() {
        return Ok(Vec::new());
    }

    let displays: Vec<_> = monitors_reply
        .monitors
        .into_iter()
        .enumerate()
        .map(|(index, monitor)| monitor_to_display(index, monitor))
        .collect();

    Ok(displays)
}

#[cfg(target_os = "linux")]
fn monitor_to_display(index: usize, monitor: MonitorInfo) -> DisplayGeometry {
    let id = monitor_id(monitor.name).unwrap_or_else(|| format!("x11-monitor-{index}"));
    DisplayGeometry::new(
        id,
        Rect::new(
            i32::from(monitor.x),
            i32::from(monitor.y),
            u32::from(monitor.width),
            u32::from(monitor.height),
        ),
    )
}

#[cfg(target_os = "linux")]
fn monitor_id(name_atom: x11rb::protocol::xproto::Atom) -> Option<String> {
    if name_atom == 0 {
        None
    } else {
        Some(format!("x11-monitor-{name_atom}"))
    }
}
