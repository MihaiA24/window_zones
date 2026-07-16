#[cfg(target_os = "linux")]
use crate::{DisplayGeometry, FocusedWindow, Rect, WindowMove, WindowSystem, WindowSystemError};

#[cfg(target_os = "linux")]
use std::convert::TryFrom;

#[cfg(target_os = "linux")]
use x11rb::protocol::randr::{self, ConnectionExt as RandrConnectionExt, MonitorInfo};
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

        if let Some(raw) = reply.value32().next().map(|window| window as Window) {
            if raw != 0 {
                return Ok(Some(raw));
            }
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

    fn focused_window_geometry(
        conn: &RustConnection,
        window: Window,
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

        let width = u32::try_from(geometry.width).map_err(|_| {
            WindowSystemError::Platform("window geometry width out of range".to_string())
        })?;
        let height = u32::try_from(geometry.height).map_err(|_| {
            WindowSystemError::Platform("window geometry height out of range".to_string())
        })?;

        Ok(Rect::new(
            geometry.x as i32,
            geometry.y as i32,
            width,
            height,
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

        let geometry = Self::focused_window_geometry(&conn, window)?;
        let displays = collect_displays(&conn, root)?;
        let center_x = geometry
            .x
            .saturating_add(i32::try_from(geometry.width / 2).map_err(|_| {
                WindowSystemError::Platform("focused window center X out of range".to_string())
            })?);
        let center_y = geometry
            .y
            .saturating_add(i32::try_from(geometry.height / 2).map_err(|_| {
                WindowSystemError::Platform("focused window center Y out of range".to_string())
            })?);
        let display_id = match find_display_for_point(&displays, center_x, center_y) {
            Some(id) => id.to_string(),
            None => format!("x11-unmatched:{center_x}:{center_y}"),
        };
        Ok(Some(FocusedWindow::new(display_id, geometry)))
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

        let x = window_move.target.x;
        let y = window_move.target.y;
        let width = u16::try_from(window_move.target.width).map_err(|_| {
            WindowSystemError::Platform("target window width out of range".to_string())
        })?;
        let height = u16::try_from(window_move.target.height).map_err(|_| {
            WindowSystemError::Platform("target window height out of range".to_string())
        })?;

        let values = xproto::ConfigureWindowAux::new()
            .x(x)
            .y(y)
            .width(width)
            .height(height)
            .border_width(0);

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

    if monitors_reply.number_of_monitors == 0 {
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
    let id = monitor_id(monitor.name.clone()).unwrap_or_else(|| format!("x11-monitor-{index}"));
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
fn monitor_id(mut raw_name: Vec<u8>) -> Option<String> {
    if raw_name.is_empty() {
        return None;
    }

    while raw_name.last().is_some_and(|byte| *byte == 0) {
        raw_name.pop();
    }

    let trimmed = String::from_utf8(raw_name)
        .ok()
        .filter(|name| !name.is_empty())?;

    Some(trimmed)
}

#[cfg(target_os = "linux")]
fn find_display_for_point(displays: &[DisplayGeometry], x: i32, y: i32) -> Option<&str> {
    displays
        .iter()
        .find(|display| point_in_rect(x, y, display.usable_area))
        .map(|display| display.id.as_str())
}

#[cfg(target_os = "linux")]
fn point_in_rect(x: i32, y: i32, rect: Rect) -> bool {
    x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom()
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::*;

    #[test]
    fn finds_display_for_point() {
        let displays = vec![
            DisplayGeometry::new("left".to_string(), Rect::new(0, 0, 800, 600)),
            DisplayGeometry::new("right".to_string(), Rect::new(800, 0, 800, 600)),
        ];

        assert_eq!(find_display_for_point(&displays, 10, 10), Some("left"));
        assert_eq!(find_display_for_point(&displays, 1200, 10), Some("right"));
        assert_eq!(find_display_for_point(&displays, 2000, 10), None);
    }
}
