use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Barrier, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use dbus::blocking::Connection;
use serde_json::{Value, json};

use super::super::protocol::{
    CALL_TIMEOUT, DISPLAYS_CAPABILITY, KWIN_INTERFACE, KWIN_OBJECT_PATH, KWIN_PROTOCOL_MAJOR,
    KWIN_SERVICE_NAME, KwinIntegrationError, MOVE_RESIZE_CAPABILITY, NextRequestPayload,
    classify_dbus_error,
};
use super::super::test_bus::{FakeBus, FakeServiceState, Process, RealBus};
use super::{KwinHotkeySystem, KwinWindowSystem, require_capability};
use crate::{
    DisplayGeometry, FocusedWindow, HotkeyEvent, HotkeySystem, HotkeySystemError, Rect, WindowId,
    WindowMove, WindowSystem, WindowSystemError,
};

#[test]
fn queued_moves_complete_for_independent_clients() {
    let bus = RealBus::new();
    let mut script = Process(
        Command::new("node")
            .args(["-e", include_str!("fixtures/queued_moves.js")])
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/kwin-script/contents/code/main.js"
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("node must be installed for KWin script contract tests"),
    );
    let mut input = script.0.stdin.take().unwrap();
    let output = BufReader::new(script.0.stdout.take().unwrap());
    let address = bus.address.clone();
    let (ready_tx, ready_rx) = mpsc::channel();
    let bridge = thread::spawn(move || {
        let connection = Connection::new_address(&address).unwrap();
        let proxy = connection.with_proxy(KWIN_SERVICE_NAME, KWIN_OBJECT_PATH, CALL_TIMEOUT);
        let mut ready_tx = Some(ready_tx);
        let mut prefetched = VecDeque::new();
        let mut request_ids = Vec::new();
        let mut report = None;
        for line in output.lines() {
            let call: Value = serde_json::from_str(&line.unwrap()).unwrap();
            if call.get("frames").is_some() {
                report = Some(call);
                break;
            }
            assert_eq!(call["service"], KWIN_SERVICE_NAME);
            assert_eq!(call["objectPath"], KWIN_OBJECT_PATH);
            assert_eq!(call["iface"], KWIN_INTERFACE);
            let method = call["method"].as_str().unwrap();
            let args = call["args"].as_array().unwrap();
            let reply = match method {
                "RegisterCompanion" => {
                    proxy
                        .method_call::<(), _, _, _>(
                            KWIN_INTERFACE,
                            method,
                            (args[0].as_str().unwrap(), args[1].as_bool().unwrap()),
                        )
                        .unwrap();
                    json!([])
                }
                "UpdateFocusedWindow" | "UpdateDisplays" => {
                    proxy
                        .method_call::<(), _, _, _>(
                            KWIN_INTERFACE,
                            method,
                            (args[0].as_str().unwrap(),),
                        )
                        .unwrap();
                    if method == "UpdateFocusedWindow"
                        && let Some(ready) = ready_tx.take()
                    {
                        ready.send(()).unwrap();
                    }
                    json!([])
                }
                "NextRequest" => {
                    // Claim all three real enqueues before letting the unchanged script
                    // process any. Only transport delivery is gated: production JS still
                    // applies and observes each frame serially for its full bounded wait.
                    if request_ids.is_empty() {
                        let deadline = Instant::now() + Duration::from_secs(3);
                        while prefetched.len() < 3 {
                            let request: NextRequestPayload =
                                proxy.method_call(KWIN_INTERFACE, method, ()).unwrap();
                            assert!(Instant::now() < deadline, "clients never filled the queue");
                            if request.0 == 0 {
                                continue;
                            }
                            request_ids.push(request.0.to_string());
                            prefetched.push_back(request);
                        }
                    }
                    let Some(request) = prefetched.pop_front() else {
                        // The final completion starts an idle poll before its acknowledgement.
                        // Leave that poll pending while the fixture records the ack and exits.
                        continue;
                    };
                    json!([
                        request.0, request.1, request.2, request.3, request.4, request.5,
                        request.6, request.7
                    ])
                }
                "CompleteRequest" => {
                    proxy
                        .method_call::<(), _, _, _>(
                            KWIN_INTERFACE,
                            method,
                            (
                                args[0].as_str().unwrap(),
                                args[1].as_bool().unwrap(),
                                args[2].as_str().unwrap(),
                                args[3].as_str().unwrap(),
                            ),
                        )
                        .unwrap();
                    json!([])
                }
                _ => panic!("unexpected KWin script method: {method}"),
            };
            writeln!(input, "{}", json!({"id": call["id"], "args": reply})).unwrap();
            input.flush().unwrap();
        }
        assert!(
            script.0.wait().unwrap().success(),
            "production script fixture failed"
        );
        let report = report.expect("script reports committed frames and completions");
        let mut frames: Vec<(String, i32, i32, u32, u32)> =
            serde_json::from_value(report["frames"].clone()).unwrap();
        frames.sort_by(|left, right| left.0.cmp(&right.0));
        let completions: Vec<(String, bool, String, String)> =
            serde_json::from_value(report["completions"].clone()).unwrap();
        assert_eq!(
            completions,
            request_ids
                .into_iter()
                .map(|id| (id, true, String::new(), String::new()))
                .collect::<Vec<_>>()
        );
        // Retain the script owner until every client has collected its completion.
        (frames, connection)
    });
    ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    let start = Arc::new(Barrier::new(4));
    let jobs: Vec<_> = [
        ("window-1", Rect::new(-600, 40, 600, 860)),
        ("window-2", Rect::new(0, 40, 700, 860)),
        ("window-3", Rect::new(700, 40, 800, 860)),
    ]
    .into_iter()
    .map(|(id, frame)| {
        let start = Arc::clone(&start);
        let mut client = KwinWindowSystem::with_bus_address(bus.address.clone());
        thread::spawn(move || {
            // Establish independent real connections before releasing the queue.
            client.capabilities().unwrap();
            start.wait();
            (
                id,
                client.move_window(&WindowMove::new(WindowId::new(id), frame)),
            )
        })
    })
    .collect();
    start.wait();
    let results: Vec<_> = jobs.into_iter().map(|job| job.join().unwrap()).collect();
    let (frames, _script_connection) = bridge.join().unwrap();
    assert_eq!(
        frames,
        vec![
            ("window-1".to_string(), -600, 40, 600, 860),
            ("window-2".to_string(), 0, 40, 700, 860),
            ("window-3".to_string(), 700, 40, 800, 860),
        ]
    );
    for (id, result) in results {
        assert!(
            result.is_ok(),
            "{id} applied and completed without a spurious unavailable error: {result:?}"
        );
    }
}

#[test]
fn stalled_script_completion_remains_bounded() {
    let bus = RealBus::new();
    let script = Connection::new_address(&bus.address).unwrap();
    let proxy = script.with_proxy(KWIN_SERVICE_NAME, KWIN_OBJECT_PATH, CALL_TIMEOUT);
    proxy
        .method_call::<(), _, _, _>(
            KWIN_INTERFACE,
            "RegisterCompanion",
            (KWIN_PROTOCOL_MAJOR.to_string(), true),
        )
        .unwrap();
    let mut client = KwinWindowSystem::with_bus_address(bus.address.clone());
    let (result_tx, result_rx) = mpsc::channel();
    let job = thread::spawn(move || {
        result_tx
            .send(client.move_window(&WindowMove::new(
                WindowId::new("window-1"),
                Rect::new(10, 20, 800, 600),
            )))
            .unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(3);
    let request: NextRequestPayload = loop {
        let request: NextRequestPayload = proxy
            .method_call(KWIN_INTERFACE, "NextRequest", ())
            .unwrap();
        if request.0 != 0 {
            break request;
        }
        assert!(Instant::now() < deadline, "move never reached the script");
    };
    assert_eq!(
        (
            request.2.as_str(),
            request.3,
            request.4,
            request.5,
            request.6
        ),
        ("window-1", 10, 20, 800, 600)
    );
    // The real companion remains responsive, but the script never completes this request.
    let error = result_rx
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap_err();
    job.join().unwrap();
    assert!(
        matches!(&error, WindowSystemError::Platform(message)
        if message.contains("KWin companion is unavailable")
            && message.contains("timed out waiting for KWin request")),
        "stalled completion must return the existing unavailable error: {error}"
    );
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
fn unavailable_kde_companion_is_actionable() {
    let window_system = KwinWindowSystem::with_bus_address("unix:path=/window-zones-no-such-bus");
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
fn lifecycle_errors_are_classified() {
    let error = dbus::Error::new_custom("org.window_zones.KWin.Error.Incompatible", "old script");
    assert!(matches!(
        classify_dbus_error(error),
        KwinIntegrationError::Incompatible { .. }
    ));
}
