use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use dbus::blocking::Connection;

use super::super::protocol::{
    CALL_TIMEOUT, KWIN_INTERFACE, KWIN_OBJECT_PATH, KWIN_PROTOCOL_MAJOR, KWIN_SERVICE_NAME,
    NextRequestPayload, REQUEST_MOVE, REQUEST_REGISTER_HOTKEYS,
};
use super::super::server::run_kwin_companion_service;
use super::super::test_bus::{FakeBus, FakeServiceState};
use super::{COMPANION_HEARTBEAT_TIMEOUT, CompanionState, NEXT_REQUEST_WAIT};

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
                "kwin_integration::companion::tests::real_service_hotkey_polling_never_claims_controller",
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
                return state
                    .script_state
                    .enqueue_move("window-1".to_string(), -1, 2, 800, 600);
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
    state.last_seen = Some(Instant::now() - COMPANION_HEARTBEAT_TIMEOUT - Duration::from_secs(1));
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
