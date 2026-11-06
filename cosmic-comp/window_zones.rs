// SPDX-License-Identifier: GPL-3.0-only
// Privileged, explicitly enabled local-session placement API. Shell objects never
// leave the compositor's calloop thread; D-Bus handlers only exchange messages.
use std::{
    cell::RefCell,
    ffi::OsStr,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use calloop::{
    LoopHandle,
    channel::{self, Event},
    timer::{TimeoutAction, Timer},
};
use cosmic::iced::futures::channel::oneshot;
use smithay::{
    desktop::space::SpaceElement,
    utils::{IsAlive, Rectangle, Serial},
};

use crate::{
    shell::{
        ModalBehavior, Shell,
        element::{CosmicMapped, CosmicSurface},
        focus::target::KeyboardFocusTarget,
    },
    state::State,
    utils::prelude::*,
    wayland::protocols::toplevel_info::{
        toplevel_enter_output, toplevel_enter_workspace, toplevel_leave_output,
        toplevel_leave_workspace,
    },
};

const NAME: &str = "org.window_zones.Cosmic";
const PATH: &str = "/org/window_zones/Cosmic";
const DEADLINE: Duration = Duration::from_millis(1400);
type Frame = Rectangle<i32, Global>;
type FocusReply = (bool, u64, i32, i32, u32, u32);
type DisplayReply = Vec<(String, i32, i32, u32, u32)>;
type MoveReply = (u32, i32, i32, u32, u32, &'static str);

struct WindowId(u64);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn id(window: &CosmicSurface) -> u64 {
    window
        .0
        .user_data()
        .get_or_insert_threadsafe(|| WindowId(NEXT_ID.fetch_add(1, Ordering::Relaxed)))
        .0
}

fn reply(code: u32, frame: Option<Frame>, message: &'static str) -> MoveReply {
    let (x, y, w, h) = frame
        .map(|f| {
            (
                f.loc.x,
                f.loc.y,
                f.size.w.max(0) as u32,
                f.size.h.max(0) as u32,
            )
        })
        .unwrap_or_default();
    (code, x, y, w, h, message)
}

// SpaceElement::geometry reads the committed client geometry, including COSMIC
// server decorations, whereas CosmicSurface::global_geometry caches configure
// targets. Only the space location is compositor-owned positioning evidence.
fn observed(shell: &Shell, mapped: &CosmicMapped) -> Option<Frame> {
    if !mapped.alive() || !mapped.active_window().alive() || mapped.is_minimized() {
        return None;
    }
    let mut frame = shell.element_geometry(mapped)?;
    frame.size = mapped.geometry().size.as_global();
    (frame.size.w > 0 && frame.size.h > 0).then_some(frame)
}

fn observed_id(shell: &Shell, mapped: &CosmicMapped, window_id: u64) -> Option<Frame> {
    let (window, offset) = mapped.windows().find(|(window, _)| {
        window
            .0
            .user_data()
            .get::<WindowId>()
            .is_some_and(|id| id.0 == window_id)
    })?;
    if !window.alive() || mapped.is_minimized() {
        return None;
    }
    let mut frame = shell.element_geometry(mapped)?;
    frame.size = window.geometry().size.as_global();
    frame.size.h += offset.y;
    (frame.size.w > 0 && frame.size.h > 0).then_some(frame)
}

fn find(shell: &Shell, window_id: u64) -> Option<CosmicMapped> {
    shell
        .mapped()
        .find(|mapped| {
            mapped.windows().any(|(window, _)| {
                window
                    .0
                    .user_data()
                    .get::<WindowId>()
                    .is_some_and(|id| id.0 == window_id)
            })
        })
        .cloned()
}

fn fullscreen(shell: &Shell, window_id: u64) -> Option<CosmicSurface> {
    shell
        .workspaces
        .spaces()
        .flat_map(|w| w.get_fullscreen_surfaces())
        .map(|f| &f.surface)
        .find(|w| {
            w.0.user_data()
                .get::<WindowId>()
                .is_some_and(|id| id.0 == window_id)
        })
        .cloned()
}

fn focused(shell: &Shell) -> FocusReply {
    if shell.session_lock.is_some() {
        return (false, 0, 0, 0, 0, 0);
    }
    let focus = shell
        .seats
        .last_active()
        .get_keyboard()
        .and_then(|k| k.current_focus());
    let (window, frame) = match focus {
        Some(KeyboardFocusTarget::Element(mapped)) => {
            (mapped.active_window(), observed(shell, &mapped))
        }
        Some(KeyboardFocusTarget::Fullscreen(window)) => {
            let frame = window.global_geometry().map(|mut f| {
                f.size = window.geometry().size.as_global();
                f
            });
            (window, frame)
        }
        Some(KeyboardFocusTarget::Popup(popup)) => {
            let Some(surface) =
                crate::wayland::handlers::xdg_shell::popup::get_popup_toplevel(&popup)
            else {
                return (false, 0, 0, 0, 0, 0);
            };
            let Some(mapped) = shell.element_for_surface(&surface) else {
                return (false, 0, 0, 0, 0, 0);
            };
            (mapped.active_window(), observed(shell, mapped))
        }
        _ => return (false, 0, 0, 0, 0, 0),
    };
    match frame.filter(|f| f.size.w > 0 && f.size.h > 0) {
        Some(f) => (
            true,
            id(&window),
            f.loc.x,
            f.loc.y,
            f.size.w as u32,
            f.size.h as u32,
        ),
        None => (false, 0, 0, 0, 0, 0),
    }
}

fn displays(shell: &Shell) -> DisplayReply {
    shell
        .outputs()
        .filter_map(|output| {
            let f = smithay::desktop::layer_map_for_output(output)
                .non_exclusive_zone()
                .as_local()
                .to_global(output);
            (f.size.w > 0 && f.size.h > 0).then(|| {
                (
                    output.name(),
                    f.loc.x,
                    f.loc.y,
                    f.size.w as u32,
                    f.size.h as u32,
                )
            })
        })
        .collect()
}

enum Request {
    Focus(oneshot::Sender<FocusReply>),
    Displays(oneshot::Sender<DisplayReply>),
    Move {
        id: u64,
        frame: Frame,
        response: oneshot::Sender<MoveReply>,
    },
}

struct Pending {
    window_id: u64,
    before: Frame,
    target: Frame,
    serial: Option<Serial>,
    changed: bool,
    deadline: Instant,
    response: oneshot::Sender<MoveReply>,
}

struct Bridge {
    session: String,
    sender: channel::Sender<Request>,
    moving: Arc<AtomicBool>,
}

struct MoveGuard(Arc<AtomicBool>);
impl Drop for MoveGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Bridge {
    async fn ask<T>(
        &self,
        request: impl FnOnce(oneshot::Sender<T>) -> Request,
    ) -> zbus::fdo::Result<T> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(request(tx))
            .map_err(|_| zbus::fdo::Error::Failed("Compositor bridge stopped".into()))?;
        rx.await
            .map_err(|_| zbus::fdo::Error::Failed("Compositor bridge stopped".into()))
    }
}

#[zbus::interface(name = "org.window_zones.Cosmic1")]
impl Bridge {
    fn get_session(&self) -> &str {
        &self.session
    }
    fn get_capabilities(&self) -> &'static [&'static str] {
        &["focused-window", "displays", "move-resize"]
    }
    async fn get_focused_window(&self) -> zbus::fdo::Result<FocusReply> {
        self.ask(Request::Focus).await
    }
    async fn get_displays(&self) -> zbus::fdo::Result<DisplayReply> {
        self.ask(Request::Displays).await
    }
    async fn move_window(
        &self,
        id: u64,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    ) -> zbus::fdo::Result<MoveReply> {
        if id == 0
            || width == 0
            || height == 0
            || width > i32::MAX as u32
            || height > i32::MAX as u32
            || x.checked_add(width as i32).is_none()
            || y.checked_add(height as i32).is_none()
        {
            return Ok(reply(2, None, "Invalid frame"));
        }
        if self
            .moving
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(reply(2, None, "Another placement is in flight"));
        }
        let _guard = MoveGuard(self.moving.clone());
        let frame = Rectangle::new((x, y).into(), (width as i32, height as i32).into());
        self.ask(|response| Request::Move {
            id,
            frame,
            response,
        })
        .await
    }
}

// Returns a configure serial, never an asserted result. The caller observes the
// real space and waits for this serial to be committed before reporting success.
fn place(
    shell: &mut Shell,
    mapped: &CosmicMapped,
    target: Frame,
    before: Frame,
    changed: &mut bool,
) -> Result<Option<Serial>, &'static str> {
    if shell.session_lock.is_some() {
        return Err("Session is locked");
    }
    if !mapped.is_window()
        || mapped.is_minimized()
        || mapped.is_fullscreen(true)
        || mapped.is_fullscreen(false)
    {
        return Err("Unsupported, minimized, stacked or fullscreen window");
    }
    if mapped.has_pending_resize()
        || mapped.is_resizing(true) == Some(true)
        || shell.seats.iter().any(|s| {
            s.get_pointer().is_some_and(|p| p.is_grabbed())
                || s.get_touch().is_some_and(|t| t.is_grabbed())
        })
    {
        return Err("Window or input is busy");
    }
    if let (Some(min), Some(max)) = (mapped.min_size(), mapped.max_size()) {
        if min.w > 0 && min.h > 0 && min == max {
            return Err("Window is not resizable");
        }
    }
    // Maximal physical-output overlap; the origin breaks ties consistently.
    let output = shell
        .outputs()
        .filter_map(|o| {
            o.geometry().intersection(target).map(|overlap| {
                let area = i64::from(overlap.size.w) * i64::from(overlap.size.h);
                (o.clone(), (area, o.geometry().contains(target.loc)))
            })
        })
        .max_by_key(|(_, score)| *score)
        .map(|(o, _)| o)
        .ok_or("Target does not intersect an output")?;
    let source = shell
        .space_for(mapped)
        .map(|w| (w.handle, w.output.clone()));
    let sticky = shell
        .workspaces
        .sets
        .iter()
        .find(|(_, set)| set.sticky_layer.mapped().any(|w| w == mapped))
        .map(|(o, _)| o.clone());
    if source.is_none() && sticky.is_none() {
        return Err("Window is not managed");
    }
    // Resolve the destination before any state changes. Output-bound workspaces
    // keep their existing workspace on same-output moves; transfers use the
    // destination's active workspace, without switching either workspace.
    let destination = if source.as_ref().is_some_and(|(_, from)| *from == output) {
        source.as_ref().map(|(handle, _)| *handle)
    } else if source.is_some() {
        Some(
            shell
                .active_space(&output)
                .ok_or("No destination workspace")?
                .handle,
        )
    } else {
        None
    };

    if mapped.is_maximized(true) || mapped.maximized_state.lock().unwrap().is_some() {
        shell.unmaximize_request(mapped, ModalBehavior::Ignore);
        *changed |= observed(shell, mapped).is_some_and(|f| f != before);
    }
    if let Some((from, from_output)) = source {
        let to = destination.unwrap();
        if from != to {
            let from_workspace = shell.workspaces.space_for_handle_mut(&from).unwrap();
            from_workspace
                .unmap_element(mapped)
                .ok_or("Unable to detach window")?;
            for (window, _) in mapped.windows() {
                toplevel_leave_workspace(&window, &from);
                toplevel_leave_output(&window, &from_output);
                toplevel_enter_output(&window, &output);
                toplevel_enter_workspace(&window, &to);
            }
            let to_workspace = shell.workspaces.space_for_handle_mut(&to).unwrap();
            to_workspace
                .floating_layer
                .map(mapped.clone(), Some(target.loc.to_local(&output)));
        } else {
            let workspace = shell.workspaces.space_for_handle_mut(&from).unwrap();
            if workspace.tiling_layer.mapped().any(|(w, _)| w == mapped) {
                workspace
                    .tiling_layer
                    .unmap(mapped, None)
                    .map_err(|_| "Unable to untile window")?;
                workspace
                    .floating_layer
                    .map(mapped.clone(), Some(target.loc.to_local(&output)));
            }
        }
    } else if let Some(from_output) = sticky {
        if from_output != output {
            shell
                .workspaces
                .sets
                .get_mut(&from_output)
                .unwrap()
                .sticky_layer
                .unmap(mapped, None)
                .ok_or("Unable to detach sticky window")?;
            for (window, _) in mapped.windows() {
                toplevel_leave_output(&window, &from_output);
                toplevel_enter_output(&window, &output);
            }
            shell
                .workspaces
                .sets
                .get_mut(&output)
                .unwrap()
                .sticky_layer
                .map(mapped.clone(), Some(target.loc.to_local(&output)));
        }
    }
    let mut applied = target;
    // Respect advertised per-axis constraints, but preserve the requested frame
    // separately: a constrained placement must never be reported as success.
    if let Some(min) = mapped.min_size() {
        applied.size.w = applied.size.w.max(min.w);
        applied.size.h = applied.size.h.max(min.h);
    }
    if let Some(max) = mapped.max_size() {
        if max.w > 0 {
            applied.size.w = applied.size.w.min(max.w);
        }
        if max.h > 0 {
            applied.size.h = applied.size.h.min(max.h);
        }
    }
    mapped.set_maximized(false);
    mapped.set_tiled(false);
    *mapped.floating_tiled.lock().unwrap() = None;
    let serial = if let Some(workspace) = shell.space_for_mut(mapped) {
        workspace.floating_layer.place_window(mapped, applied)
    } else {
        shell
            .workspaces
            .sets
            .get_mut(&output)
            .unwrap()
            .sticky_layer
            .place_window(mapped, applied)
    };
    shell.update_reactive_popups(mapped);
    Ok(serial)
}

fn process_move(
    state: &mut State,
    window_id: u64,
    target: Frame,
    response: oneshot::Sender<MoveReply>,
    pending: &mut Option<Pending>,
) {
    let mut shell = state.common.shell.write();
    let mapped = find(&shell, window_id);
    let before = mapped
        .as_ref()
        .and_then(|w| observed_id(&shell, w, window_id));
    if pending.is_some() {
        let _ = response.send(reply(2, before, "Another placement is in flight"));
        return;
    }
    if let Some(window) = fullscreen(&shell, window_id) {
        let actual = window.global_geometry().map(|mut f| {
            f.size = window.geometry().size.as_global();
            f
        });
        let _ = response.send(reply(2, actual, "Fullscreen window"));
        return;
    }
    let Some(mapped) = mapped else {
        let _ = response.send(reply(1, None, "Window is gone"));
        return;
    };
    if !mapped.alive()
        || !mapped.windows().any(|(w, _)| {
            w.alive()
                && w.0
                    .user_data()
                    .get::<WindowId>()
                    .is_some_and(|id| id.0 == window_id)
        })
    {
        let _ = response.send(reply(1, None, "Window is gone"));
        return;
    }
    let Some(before) = before else {
        let _ = response.send(reply(2, None, "Window has no visible frame"));
        return;
    };
    let mut changed = false;
    let result = place(&mut shell, &mapped, target, before, &mut changed);
    let actual = observed_id(&shell, &mapped, window_id);
    changed |= actual.is_some_and(|f| f != before);
    match result {
        Err(message) => {
            let _ = response.send(reply(if changed { 3 } else { 2 }, actual, message));
        }
        Ok(serial) => {
            // Publish only observed geometry to the ordinary COSMIC info protocol.
            if let Some(actual) = actual {
                mapped.active_window().observe_geometry(actual);
            }
            *pending = Some(Pending {
                window_id,
                before,
                target,
                serial,
                changed,
                deadline: Instant::now() + DEADLINE,
                response,
            });
        }
    }
    for output in shell.outputs() {
        state.backend.schedule_render(output);
    }
}

fn poll_move(state: &mut State, pending: &mut Option<Pending>) {
    let Some(p) = pending.as_mut() else {
        return;
    };
    let shell = state.common.shell.read();
    let Some(mapped) = find(&shell, p.window_id) else {
        let actual = fullscreen(&shell, p.window_id).and_then(|window| {
            window.global_geometry().map(|mut f| {
                f.size = window.geometry().size.as_global();
                f
            })
        });
        if let Some(actual) = actual {
            p.changed |= actual != p.before;
        }
        let p = pending.take().unwrap();
        let code = if actual.is_none() {
            1
        } else if p.changed {
            3
        } else {
            2
        };
        let _ = p.response.send(reply(
            code,
            actual,
            "Window closed, unmapped or became fullscreen",
        ));
        return;
    };
    let Some(actual) = observed_id(&shell, &mapped, p.window_id) else {
        let p = pending.take().unwrap();
        let _ = p.response.send(reply(1, None, "Window was unmapped"));
        return;
    };
    p.changed |= actual != p.before;
    if !mapped.is_window() {
        let p = pending.take().unwrap();
        let _ = p.response.send(reply(
            if p.changed { 3 } else { 2 },
            Some(actual),
            "Window became stacked during placement",
        ));
        return;
    }
    mapped.active_window().observe_geometry(actual);
    let committed = p.serial.map_or_else(
        || mapped.latest_size_committed(),
        |serial| mapped.active_window().serial_past(&serial),
    );
    let success = committed && actual == p.target;
    if success || Instant::now() >= p.deadline {
        let p = pending.take().unwrap();
        let (code, message) = if success {
            (0, "Committed")
        } else if p.changed {
            (
                3,
                "Client constrained or did not commit the requested frame",
            )
        } else {
            (
                2,
                "Client did not commit the requested frame; no frame change",
            )
        };
        let _ = p.response.send(reply(code, Some(actual), message));
    }
}

pub(super) fn init(
    handle: &LoopHandle<'static, State>,
    executor: &calloop::futures::Scheduler<()>,
    socket: &OsStr,
) {
    if std::env::var_os("WINDOW_ZONES_COSMIC_BRIDGE").as_deref() != Some(OsStr::new("1")) {
        return;
    }
    let socket = PathBuf::from(socket);
    let socket = if socket.is_absolute() {
        socket
    } else {
        let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") else {
            tracing::error!("Window Zones bridge requires XDG_RUNTIME_DIR");
            return;
        };
        PathBuf::from(runtime).join(socket)
    };
    let Ok(socket) = std::fs::canonicalize(socket) else {
        tracing::error!("Cannot resolve this compositor's Window Zones socket");
        return;
    };
    let pending: Rc<RefCell<Option<Pending>>> = Rc::new(RefCell::new(None));
    let (sender, receiver) = channel::channel();
    let completion_handle = handle.clone();
    if let Err(err) = handle.insert_source(receiver, move |event, _, state| {
        if let Event::Msg(request) = event {
            match request {
                Request::Focus(response) => { let _ = response.send(focused(&state.common.shell.read())); }
                Request::Displays(response) => { let _ = response.send(displays(&state.common.shell.read())); }
                Request::Move { id, frame, response } => {
                    if pending.borrow().is_none() {
                        let timer_pending = pending.clone();
                        if let Err(err) = completion_handle.insert_source(
                            Timer::from_duration(Duration::from_millis(10)),
                            move |_, _, state| {
                                let mut pending = timer_pending.borrow_mut();
                                poll_move(state, &mut pending);
                                if pending.is_some() {
                                    TimeoutAction::ToDuration(Duration::from_millis(10))
                                } else {
                                    TimeoutAction::Drop
                                }
                            },
                        ) {
                            tracing::error!(?err.error, "Cannot initialize Window Zones completion timer");
                            let _ = response.send(reply(2, None, "Completion timer is unavailable"));
                            return;
                        }
                    }
                    process_move(state, id, frame, response, &mut pending.borrow_mut());
                }
            }
        }
    }) {
        tracing::error!(?err.error, "Cannot initialize Window Zones request channel");
        return;
    }
    let session = socket.to_string_lossy().into_owned();
    let _ = executor.schedule(async move {
        let result = async {
            // A separate connection and DoNotQueue without ReplaceExisting:
            // never steal another compositor's bridge, or queue behind it.
            let connection = zbus::Connection::session().await?;
            connection
                .object_server()
                .at(
                    PATH,
                    Bridge {
                        session,
                        sender,
                        moving: Arc::new(AtomicBool::new(false)),
                    },
                )
                .await?;
            let acquired = connection
                .request_name_with_flags(NAME, zbus::fdo::RequestNameFlags::DoNotQueue.into())
                .await?;
            if !matches!(acquired, zbus::fdo::RequestNameReply::PrimaryOwner) {
                return Err(zbus::Error::Failure(
                    "Window Zones bridge name is already owned".into(),
                ));
            }
            tracing::info!("Window Zones compositor bridge ready");
            std::future::pending::<()>().await;
            Ok::<(), zbus::Error>(())
        }
        .await;
        if let Err(err) = result {
            tracing::error!(?err, "Cannot start Window Zones bridge");
        }
    });
}
