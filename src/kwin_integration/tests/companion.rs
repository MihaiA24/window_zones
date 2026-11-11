use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use dbus::blocking::Connection;

use crate::HotkeySystemError;

use super::super::protocol::{
    CALL_TIMEOUT, KWIN_INTERFACE, KWIN_OBJECT_PATH, KWIN_PROTOCOL_MAJOR, KWIN_SERVICE_NAME,
    NextRequestPayload, REQUEST_MOVE, REQUEST_REGISTER_HOTKEYS, classify_dbus_error,
};
use super::super::server::run_kwin_companion_service;
use super::super::test_bus::{FakeBus, FakeServiceState, RealBus};
use super::{COMPANION_HEARTBEAT_TIMEOUT, CompanionState, NEXT_REQUEST_WAIT};

#[test]
fn enqueue_budgets_count_pending_work_but_not_uncollected_results() {
    let bus = RealBus::new();
    let script = Connection::new_address(&bus.address).unwrap();
    let script_proxy = script.with_proxy(KWIN_SERVICE_NAME, KWIN_OBJECT_PATH, CALL_TIMEOUT);
    script_proxy
        .method_call::<(), _, _, _>(
            KWIN_INTERFACE,
            "RegisterCompanion",
            (KWIN_PROTOCOL_MAJOR.to_string(), true),
        )
        .unwrap();
    let client = Connection::new_address(&bus.address).unwrap();
    let proxy = client.with_proxy(KWIN_SERVICE_NAME, KWIN_OBJECT_PATH, CALL_TIMEOUT);
    let first: (u32, u64) = proxy
        .method_call(
            KWIN_INTERFACE,
            "MoveWindow",
            ("window-1", -10_i32, 20_i32, 800_u32, 600_u32),
        )
        .unwrap();
    let refresh: (u32, u64) = proxy
        .method_call(KWIN_INTERFACE, "RefreshDisplays", ())
        .unwrap();
    let hotkeys: (u32, u64) = proxy
        .method_call(KWIN_INTERFACE, "RegisterHotkeys", ("[]",))
        .unwrap();
    assert_eq!((first.1, refresh.1, hotkeys.1), (1000, 2000, 3000));
    assert_ne!(first.0, refresh.0);
    assert_ne!(refresh.0, hotkeys.0);
    let claimed: NextRequestPayload = script_proxy
        .method_call(KWIN_INTERFACE, "NextRequest", ())
        .unwrap();
    assert_eq!(claimed.0, first.0);
    // A claimed request still occupies its queue slot.
    let while_claimed: (u32, u64) = proxy
        .method_call(KWIN_INTERFACE, "RefreshDisplays", ())
        .unwrap();
    assert_eq!(while_claimed.1, 4000);
    script_proxy
        .method_call::<(), _, _, _>(
            KWIN_INTERFACE,
            "CompleteRequest",
            (first.0.to_string(), true, String::new(), String::new()),
        )
        .unwrap();
    // Do not consume GetRequestResult: completion alone frees the queue slot.
    let after_completion: (u32, u64) = proxy
        .method_call(KWIN_INTERFACE, "RefreshDisplays", ())
        .unwrap();
    assert_eq!(after_completion.1, 4000);
    let result: (bool, String, String) = proxy
        .method_call(KWIN_INTERFACE, "GetRequestResult", (first.0.to_string(),))
        .unwrap();
    assert_eq!(result, (true, String::new(), String::new()));
}

#[test]
fn real_service_hotkey_polling_never_claims_controller() {
    if std::env::var_os("WINDOW_ZONES_KWIN_SERVICE_TEST_CHILD").is_some() {
        run_kwin_companion_service().unwrap();
        return;
    }

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
    let old = Connection::new_address(&bus.address).unwrap();
    let new = Connection::new_address(&bus.address).unwrap();
    let register = |connection: &Connection, hotkeys: &[&str]| {
        let (id, _timeout_ms): (u32, u64) = connection
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
    proxy
        .method_call::<(), _, _, _>(
            KWIN_INTERFACE,
            "RegisterCompanion",
            (KWIN_PROTOCOL_MAJOR.to_string(), true),
        )
        .unwrap();
    let assert_registration_lost = || {
        let error = classify_dbus_error(poll(&old).unwrap_err()).into_hotkey_error();
        assert!(
            matches!(&error, HotkeySystemError::Unavailable(message)
                if message.contains("retry registration")),
            "pending or failed restoration must invalidate the App's registered cache: {error}"
        );
        assert_eq!(
            poll(&new).unwrap_err().name(),
            Some("org.window_zones.KWin.Error.Busy"),
            "an unrelated poller must not consume recovery or acquire the controller"
        );
    };
    assert_registration_lost();
    let restore: NextRequestPayload = proxy
        .method_call(KWIN_INTERFACE, "NextRequest", ())
        .unwrap();
    assert_eq!(restore.1, REQUEST_REGISTER_HOTKEYS);
    assert_eq!(restore.7, r#"["alt+ctrl+left"]"#);
    proxy
        .method_call::<(), _, _, _>(
            KWIN_INTERFACE,
            "CompleteRequest",
            (
                restore.0.to_string(),
                false,
                "org.window_zones.KWin.Error.Unsupported".to_string(),
                "another action owns the shortcut after script reset".to_string(),
            ),
        )
        .unwrap();
    for _ in 0..2 {
        assert_registration_lost();
    }
    complete_registration(
        register(&old, &["alt+ctrl+left"]).unwrap(),
        &["alt+ctrl+left"],
    );
    proxy
        .method_call::<(), _, _, _>(
            KWIN_INTERFACE,
            "HotkeyPressed",
            ("alt+ctrl+left".to_string(),),
        )
        .unwrap();
    assert_eq!(poll(&old).unwrap(), (true, "alt+ctrl+left".to_string()));
    assert_eq!(poll(&old).unwrap(), (false, String::new()));
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
    let _ = state
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
    let _ = state
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

    let _ = state
        .mark_ready(KWIN_PROTOCOL_MAJOR, "replacement-script".to_string(), true)
        .unwrap();
    assert_eq!(state.enqueue_refresh_displays(), 2);
    assert_eq!(state.next_request("replacement-script").unwrap().0, 2);
    assert_eq!(state.next_request("replacement-script").unwrap().0, 0);
}

#[test]
fn rejected_hotkey_registration_keeps_previous_set() {
    let mut state = CompanionState::default();
    let _ = state
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
    let _ = state
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
    let _ = state
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
    let _ = state
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

    let _ = state
        .mark_ready(KWIN_PROTOCOL_MAJOR, "kwin-script".to_string(), false)
        .unwrap();
    let (_, kind, _, _, _, _, _, hotkeys) = state.next_request("kwin-script").unwrap();
    assert_eq!(kind, REQUEST_REGISTER_HOTKEYS);
    assert_eq!(hotkeys, "[]");
}

#[test]
fn script_reset_makes_inflight_registration_retryable() {
    let mut state = CompanionState::default();
    let _ = state
        .mark_ready(KWIN_PROTOCOL_MAJOR, "kwin-script".to_string(), true)
        .unwrap();
    state
        .authorize_controller("controller".to_string())
        .unwrap();
    let hotkeys = vec!["alt+ctrl+left".to_string()];
    let interrupted = state.enqueue_hotkeys(hotkeys.clone());
    assert_eq!(state.next_request("kwin-script").unwrap().0, interrupted);

    let _ = state
        .mark_ready(KWIN_PROTOCOL_MAJOR, "kwin-script".to_string(), true)
        .unwrap();
    let (complete, name, message) = state.request_result(interrupted);
    assert!(complete);
    assert!(matches!(
        classify_dbus_error(dbus::Error::new_custom(&name, &message)).into_hotkey_error(),
        HotkeySystemError::Unavailable(_)
    ));
    assert!(state.registered_hotkeys.is_empty());

    let retry = state.enqueue_hotkeys(hotkeys.clone());
    assert_ne!(retry, interrupted);
    assert_eq!(state.next_request("kwin-script").unwrap().0, retry);
    assert!(
        state
            .complete_request(interrupted, true, String::new(), String::new())
            .is_err(),
        "a stale completion must not commit the retry"
    );
    state
        .complete_request(retry, true, String::new(), String::new())
        .unwrap();
    assert_eq!(
        state.request_result(retry),
        (true, String::new(), String::new())
    );
    assert_eq!(state.registered_hotkeys, hotkeys);
}

#[test]
fn script_reset_invalidates_its_deferred_poll_before_restored_requests() {
    let mut state = CompanionState::default();
    let _ = state
        .mark_ready(KWIN_PROTOCOL_MAJOR, ":1.20".to_string(), true)
        .unwrap();
    state.registered_hotkeys = vec!["alt+ctrl+left".to_string()];
    let mut call = dbus::Message::new_method_call(
        KWIN_SERVICE_NAME,
        KWIN_OBJECT_PATH,
        KWIN_INTERFACE,
        "NextRequest",
    )
    .unwrap();
    call.set_serial(7);
    call.set_sender(Some(":1.20".into()));
    assert!(state.poll_next_request(&call).unwrap().is_empty());

    let mut cancelled = state
        .mark_ready(KWIN_PROTOCOL_MAJOR, ":1.20".to_string(), true)
        .unwrap()
        .expect("reset must answer the obsolete poll without claiming restored keys");
    assert_eq!(cancelled.get_reply_serial(), Some(7));
    assert_eq!(
        cancelled.as_result().unwrap_err().name(),
        Some("org.window_zones.KWin.Error.Unavailable")
    );
    assert!(state.pending_next_request.is_none());
    assert!(state.take_next_request_reply(Instant::now()).is_none());
    let restored = state.next_request(":1.20").unwrap();
    assert_eq!(restored.1, REQUEST_REGISTER_HOTKEYS);
    assert_eq!(restored.7, r#"["alt+ctrl+left"]"#);
}

#[test]
fn script_reset_requeues_registered_hotkeys_once() {
    let mut state = CompanionState::default();
    let _ = state
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

    let _ = state
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

    let _ = state
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
    let _ = state
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
