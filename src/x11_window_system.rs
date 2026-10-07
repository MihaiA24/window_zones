#[cfg(target_os = "linux")]
use crate::{
    DisplayGeometry, FocusedWindow, Rect, WindowId, WindowMove, WindowSystem, WindowSystemError,
};

#[cfg(target_os = "linux")]
use std::convert::TryFrom;

#[cfg(target_os = "linux")]
mod hotkeys;
#[cfg(target_os = "linux")]
pub use hotkeys::X11HotkeySystem;

#[cfg(target_os = "linux")]
use x11rb::connection::Connection;
#[cfg(target_os = "linux")]
use x11rb::errors::ReplyError;
#[cfg(target_os = "linux")]
use x11rb::protocol::ErrorKind;
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
            .map_err(|error| window_reply_error(window, "read frame extents", error))?;

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
        Ok(Some(FocusedWindow::new(
            WindowId::new(window.to_string()),
            geometry,
        )))
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        let (conn, screen_index) = Self::connect()?;
        let root = Self::root_window(&conn, screen_index)?;
        collect_displays(&conn, root)
    }

    fn move_window(&mut self, window_move: &WindowMove) -> Result<(), WindowSystemError> {
        let (conn, screen_index) = Self::connect()?;
        let root = Self::root_window(&conn, screen_index)?;

        let window = window_move.window.as_str().parse::<Window>().map_err(|_| {
            WindowSystemError::Platform(
                "invalid X11 window id; select an existing window".to_string(),
            )
        })?;
        let clients = property_values(&conn, root, "_NET_CLIENT_LIST", AtomEnum::WINDOW, u32::MAX)?;
        if window == 0 || !clients.contains(&window) {
            return Err(WindowSystemError::WindowGone(window_move.window.clone()));
        }
        conn.get_window_attributes(window)
            .map_err(|error| {
                WindowSystemError::Platform(format!("failed to query window: {error}"))
            })?
            .reply()
            .map_err(|error| window_reply_error(window, "query window", error))?;

        let mut atoms = [0; 5];
        for (atom, name) in atoms.iter_mut().zip([
            "_NET_WM_STATE",
            "_NET_WM_STATE_MAXIMIZED_HORZ",
            "_NET_WM_STATE_MAXIMIZED_VERT",
            "_NET_WM_STATE_SHADED",
            "_NET_WM_STATE_FULLSCREEN",
        ]) {
            *atom = intern_atom(&conn, name)?;
        }
        let [state, maximized_horz, maximized_vert, shaded, fullscreen] = atoms;
        let states = property_values(&conn, window, "_NET_WM_STATE", AtomEnum::ATOM, u32::MAX)?;
        if states.contains(&fullscreen) {
            return Err(WindowSystemError::Platform(
                "cannot move a fullscreen X11 window; leave fullscreen first".to_string(),
            ));
        }
        let mut size = frame_target_size(window_move.target, Self::frame_extents(&conn, window)?)?;
        let restore_states = [maximized_horz, maximized_vert, shaded];
        if states.iter().any(|atom| restore_states.contains(atom)) {
            for removed in [[maximized_horz, maximized_vert], [shaded, 0]] {
                let event = xproto::ClientMessageEvent::new(
                    32,
                    window,
                    state,
                    [0, removed[0], removed[1], 2, 0],
                );
                conn.send_event(
                    false,
                    root,
                    xproto::EventMask::SUBSTRUCTURE_REDIRECT
                        | xproto::EventMask::SUBSTRUCTURE_NOTIFY,
                    event,
                )
                .map_err(|error| {
                    WindowSystemError::Platform(format!("failed to restore window: {error}"))
                })?
                .check()
                .map_err(|error| window_reply_error(window, "restore window", error))?;
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
            loop {
                let states =
                    property_values(&conn, window, "_NET_WM_STATE", AtomEnum::ATOM, u32::MAX)?;
                if !states.iter().any(|atom| restore_states.contains(atom)) {
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    return Err(WindowSystemError::Platform(
                        "X11 window manager did not restore the window; unmaximize or unshade it first".to_string(),
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            size = frame_target_size(window_move.target, Self::frame_extents(&conn, window)?)?;
        }

        let (width, height) = size;
        let values = xproto::ConfigureWindowAux::new()
            .x(window_move.target.x)
            .y(window_move.target.y)
            .width(width)
            .height(height);
        conn.configure_window(window, &values)
            .map_err(|error| {
                WindowSystemError::Platform(format!("failed to configure window: {error}"))
            })?
            .check()
            .map_err(|error| window_reply_error(window, "configure window", error))?;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn frame_target_size(
    target: Rect,
    [left, right, top, bottom]: [u32; 4],
) -> Result<(u32, u32), WindowSystemError> {
    let invalid_size =
        || WindowSystemError::Platform("target size does not contain the window frame".to_string());
    let width = target
        .width
        .checked_sub(left)
        .and_then(|width| width.checked_sub(right))
        .filter(|width| *width > 0)
        .ok_or_else(invalid_size)?;
    let height = target
        .height
        .checked_sub(top)
        .and_then(|height| height.checked_sub(bottom))
        .filter(|height| *height > 0)
        .ok_or_else(invalid_size)?;
    Ok((width, height))
}

#[cfg(target_os = "linux")]
/// RandR monitor usable areas, with EWMH dock/panel reservations removed.
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

    let geometry = conn
        .get_geometry(root)
        .map_err(|error| {
            WindowSystemError::Platform(format!("failed to query root geometry: {error}"))
        })?
        .reply()
        .map_err(|error| {
            WindowSystemError::Platform(format!("failed to read root geometry: {error}"))
        })?;
    let root_width = u32::from(geometry.width);
    let root_height = u32::from(geometry.height);
    let struts = collect_struts(conn, root, root_width, root_height)?;
    let displays: Vec<_> = monitors_reply
        .monitors
        .into_iter()
        .enumerate()
        .map(|(index, monitor)| {
            let mut display = monitor_to_display(index, monitor);
            display.usable_area =
                usable_area(display.usable_area, root_width, root_height, &struts);
            display
        })
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

#[cfg(target_os = "linux")]
fn intern_atom(conn: &RustConnection, name: &str) -> Result<u32, WindowSystemError> {
    conn.intern_atom(false, name.as_bytes())
        .map_err(|error| WindowSystemError::Platform(format!("failed to intern {name}: {error}")))?
        .reply()
        .map(|reply| reply.atom)
        .map_err(|error| {
            WindowSystemError::Platform(format!("failed to read {name} atom: {error}"))
        })
}

#[cfg(target_os = "linux")]
fn window_reply_error(window: Window, operation: &str, error: ReplyError) -> WindowSystemError {
    if matches!(&error, ReplyError::X11Error(error) if error.error_kind == ErrorKind::Window) {
        WindowSystemError::WindowGone(WindowId::new(window.to_string()))
    } else {
        WindowSystemError::Platform(format!("failed to {operation}: {error}"))
    }
}

#[cfg(target_os = "linux")]
fn property_values(
    conn: &RustConnection,
    window: Window,
    name: &str,
    type_: AtomEnum,
    length: u32,
) -> Result<Vec<u32>, WindowSystemError> {
    let atom = intern_atom(conn, name)?;
    let reply = conn
        .get_property(false, window, atom, type_, 0, length)
        .map_err(|error| WindowSystemError::Platform(format!("failed to query {name}: {error}")))?
        .reply()
        .map_err(|error| window_reply_error(window, &format!("read {name}"), error))?;
    Ok(reply
        .value32()
        .map(|values| values.collect())
        .unwrap_or_default())
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
struct Strut {
    edges: [u32; 4],
    // Inclusive start/end coordinates: left Y, right Y, top X, bottom X.
    ranges: [u32; 8],
}

#[cfg(target_os = "linux")]
impl Strut {
    fn from_values(values: &[u32], root_width: u32, root_height: u32) -> Option<Self> {
        let edges = values.get(..4)?.try_into().ok()?;
        let ranges = match values.len() {
            12 => values[4..].try_into().ok()?,
            4 => [
                0,
                root_height.saturating_sub(1),
                0,
                root_height.saturating_sub(1),
                0,
                root_width.saturating_sub(1),
                0,
                root_width.saturating_sub(1),
            ],
            _ => return None,
        };
        Some(Self { edges, ranges })
    }
}

#[cfg(target_os = "linux")]
fn collect_struts(
    conn: &RustConnection,
    root: Window,
    root_width: u32,
    root_height: u32,
) -> Result<Vec<Strut>, WindowSystemError> {
    let mut windows = conn
        .query_tree(root)
        .map_err(|error| {
            WindowSystemError::Platform(format!("failed to query root windows: {error}"))
        })?
        .reply()
        .map_err(|error| {
            WindowSystemError::Platform(format!("failed to read root windows: {error}"))
        })?
        .children;
    windows.extend(property_values(
        conn,
        root,
        "_NET_CLIENT_LIST",
        AtomEnum::WINDOW,
        u32::MAX,
    )?);
    windows.sort_unstable();
    windows.dedup();
    let mut struts = Vec::new();
    for window in windows {
        let partial = match property_values(
            conn,
            window,
            "_NET_WM_STRUT_PARTIAL",
            AtomEnum::CARDINAL,
            12,
        ) {
            Ok(values) => values,
            Err(WindowSystemError::WindowGone(_)) => continue,
            Err(error) => return Err(error),
        };
        if partial.len() == 12 {
            if let Some(strut) = Strut::from_values(&partial, root_width, root_height) {
                struts.push(strut);
            }
        } else {
            let legacy = match property_values(conn, window, "_NET_WM_STRUT", AtomEnum::CARDINAL, 4)
            {
                Ok(values) => values,
                Err(WindowSystemError::WindowGone(_)) => continue,
                Err(error) => return Err(error),
            };
            if let Some(strut) = Strut::from_values(&legacy, root_width, root_height) {
                struts.push(strut);
            }
        }
    }
    Ok(struts)
}

#[cfg(target_os = "linux")]
fn usable_area(bounds: Rect, root_width: u32, root_height: u32, struts: &[Strut]) -> Rect {
    let x = i64::from(bounds.x);
    let y = i64::from(bounds.y);
    let right = x + i64::from(bounds.width);
    let bottom = y + i64::from(bounds.height);
    let (mut left_edge, mut top_edge, mut right_edge, mut bottom_edge) = (x, y, right, bottom);
    let overlaps = |start: u32, end: u32, low: i64, high: i64| {
        start <= end && i64::from(start) < high && i64::from(end) >= low
    };
    for strut in struts {
        let [left, reserved_right, top, reserved_bottom] = strut.edges;
        let [ly0, ly1, ry0, ry1, tx0, tx1, bx0, bx1] = strut.ranges;
        if left > 0 && overlaps(ly0, ly1, y, bottom) {
            left_edge = left_edge.max(i64::from(left).clamp(x, right));
        }
        if reserved_right > 0 && overlaps(ry0, ry1, y, bottom) {
            right_edge =
                right_edge.min((i64::from(root_width) - i64::from(reserved_right)).clamp(x, right));
        }
        if top > 0 && overlaps(tx0, tx1, x, right) {
            top_edge = top_edge.max(i64::from(top).clamp(y, bottom));
        }
        if reserved_bottom > 0 && overlaps(bx0, bx1, x, right) {
            bottom_edge = bottom_edge
                .min((i64::from(root_height) - i64::from(reserved_bottom)).clamp(y, bottom));
        }
    }
    Rect::new(
        left_edge as i32,
        top_edge as i32,
        (right_edge - left_edge).max(0) as u32,
        (bottom_edge - top_edge).max(0) as u32,
    )
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn x11_partial_panel_only_reserves_its_monitor() {
        let strut =
            Strut::from_values(&[0, 0, 30, 0, 0, 0, 0, 0, 0, 1919, 0, 0], 3520, 1080).unwrap();
        assert_eq!(
            usable_area(Rect::new(0, 0, 1920, 1080), 3520, 1080, &[strut]),
            Rect::new(0, 30, 1920, 1050)
        );
        assert_eq!(
            usable_area(Rect::new(1920, 0, 1600, 900), 3520, 1080, &[strut]),
            Rect::new(1920, 0, 1600, 900)
        );
    }

    #[test]
    fn x11_bottom_strut_is_relative_to_root_not_shorter_monitor() {
        let strut =
            Strut::from_values(&[0, 0, 0, 210, 0, 0, 0, 0, 0, 0, 1920, 3519], 3520, 1080).unwrap();
        assert_eq!(
            usable_area(Rect::new(1920, 0, 1600, 900), 3520, 1080, &[strut]),
            Rect::new(1920, 0, 1600, 870)
        );
        assert_eq!(
            usable_area(Rect::new(0, 0, 1920, 1080), 3520, 1080, &[strut]),
            Rect::new(0, 0, 1920, 1080)
        );
    }

    #[test]
    fn x11_legacy_struts_and_multiple_panels_use_maximum_not_sum() {
        let struts = [
            Strut::from_values(&[40, 0, 20, 0], 3520, 1080).unwrap(),
            Strut::from_values(&[0, 0, 30, 0], 3520, 1080).unwrap(),
        ];
        assert_eq!(
            usable_area(Rect::new(0, 0, 1920, 1080), 3520, 1080, &struts),
            Rect::new(40, 30, 1880, 1050)
        );
        assert_eq!(
            usable_area(Rect::new(1920, 0, 1600, 900), 3520, 1080, &struts),
            Rect::new(1920, 30, 1600, 870)
        );
    }

    #[test]
    fn x11_vertical_layout_ranges_are_inclusive_and_clip_to_monitor() {
        let strut =
            Strut::from_values(&[25, 0, 0, 0, 900, 1799, 0, 0, 0, 0, 0, 0], 1600, 1800).unwrap();
        assert_eq!(
            usable_area(Rect::new(0, 0, 1600, 900), 1600, 1800, &[strut]),
            Rect::new(0, 0, 1600, 900)
        );
        assert_eq!(
            usable_area(Rect::new(0, 900, 1600, 900), 1600, 1800, &[strut]),
            Rect::new(25, 900, 1575, 900)
        );
        let huge = Strut::from_values(&[2000, 0, 0, 0], 1600, 1800).unwrap();
        assert_eq!(
            usable_area(Rect::new(0, 0, 1600, 900), 1600, 1800, &[huge]),
            Rect::new(1600, 0, 0, 900)
        );
        assert!(Strut::from_values(&[1, 2, 3], 1600, 1800).is_none());
    }
}

#[cfg(all(test, target_os = "linux"))]
#[test]
fn x11_frame_target_keeps_decoration_space_and_rejects_empty_content() {
    assert_eq!(
        frame_target_size(Rect::new(50, 60, 800, 600), [2, 2, 24, 2]).unwrap(),
        (796, 574)
    );
    assert!(frame_target_size(Rect::new(0, 0, 4, 600), [2, 2, 24, 2]).is_err());
    assert!(frame_target_size(Rect::new(0, 0, 800, 25), [2, 2, 24, 2]).is_err());
}
