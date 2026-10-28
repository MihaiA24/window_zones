use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dbus::blocking::LocalConnection;
use dbus::channel::Sender;
use dbus_tree::Factory;
use parking_lot::Mutex;

use crate::Rect;

use super::companion::CompanionState;
use super::kglobalaccel::{
    KGLOBALACCEL_COMPONENT, KGLOBALACCEL_INTERFACE, KGLOBALACCEL_PATH, KGLOBALACCEL_SERVICE,
};
use super::protocol::{
    DISPLAYS_CAPABILITY, DisplayPayload, FOCUSED_WINDOW_CAPABILITY, HOTKEYS_CAPABILITY,
    KWIN_INTERFACE, KWIN_OBJECT_PATH, KWIN_SERVICE_NAME, MOVE_RESIZE_CAPABILITY, parse_hotkeys,
    parse_u32_argument, sender_name,
};

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
pub(super) struct FakeServiceState {
    capabilities: Vec<String>,
    pub(super) capabilities_error: Option<(&'static str, String)>,
    pub(super) request_error: Option<(&'static str, String)>,
    pub(super) move_error: Option<(&'static str, String)>,
    pub(super) move_observed: Option<Rect>,
    pub(super) expose_stale_accelerator: bool,
    pub(super) focused: Option<(String, i32, i32, u32, u32)>,
    pub(super) displays: Vec<DisplayPayload>,
    pub(super) next_displays: Option<Vec<DisplayPayload>>,
    pub(super) moves: Vec<(String, i32, i32, u32, u32)>,
    pub(super) registered_hotkeys: Vec<String>,
    pending: HashMap<u32, FakeRequest>,
    events: VecDeque<String>,
    next_request_id: u32,
    pub(super) script_state: CompanionState,
}

impl FakeServiceState {
    pub(super) fn complete() -> Self {
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

pub(super) struct FakeBus {
    pub(super) address: String,
    pub(super) state: Arc<Mutex<FakeServiceState>>,
    stop: Arc<AtomicBool>,
    service_thread: Option<JoinHandle<()>>,
    daemon: Child,
}

impl FakeBus {
    pub(super) fn new(state: FakeServiceState) -> Self {
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
                                    let (protocol, reset): (String, bool) = method.msg.read2()?;
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
                                    if let Some((name, message)) = state.capabilities_error.clone()
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
                                        method.msg.method_return().append1(state.displays.clone()),
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
                                    let Some(request) = state.pending.get_mut(&request_id) else {
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
                                        && let Some((id, x, y, width, height)) = &mut state.focused
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
                                            } => state.moves.push((window, x, y, width, height)),
                                            FakeRequest::RefreshDisplays { .. } => {
                                                if let Some(displays) = state.next_displays.take() {
                                                    state.displays = displays;
                                                }
                                            }
                                            FakeRequest::RegisterHotkeys { hotkeys, .. } => {
                                                state.registered_hotkeys = hotkeys
                                            }
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
                                    method
                                        .msg
                                        .method_return()
                                        .append1(vec![vec![KGLOBALACCEL_COMPONENT.to_string()]]),
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

    pub(super) fn update_state(&self, update: impl FnOnce(&mut FakeServiceState)) {
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
