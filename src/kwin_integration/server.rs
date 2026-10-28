use std::sync::Arc;
use std::time::{Duration, Instant};

use dbus::blocking::LocalConnection;
use dbus::channel::Sender;
use dbus::message::MatchRule;
use dbus_tree::Factory;
use parking_lot::Mutex;

use super::companion::CompanionState;
use super::kglobalaccel::set_shortcut;
use super::protocol::{
    DISPLAYS_CAPABILITY, FOCUSED_WINDOW_CAPABILITY, HOTKEYS_CAPABILITY, KWIN_INTERFACE,
    KWIN_OBJECT_PATH, KWIN_SERVICE_NAME, MOVE_RESIZE_CAPABILITY, method_error, parse_displays,
    parse_hotkeys, parse_u32_argument, sender_name,
};

const SERVICE_POLL_INTERVAL: Duration = Duration::from_millis(25);

pub fn run_kwin_companion_service() -> Result<(), String> {
    let connection = LocalConnection::new_session()
        .map_err(|error| format!("connect to the user session bus: {error}"))?;
    // Keep outbound accelerator calls off the D-Bus server connection, and never hold
    // CompanionState while KWin processes them.
    let accelerator_connection = LocalConnection::new_session()
        .map_err(|error| format!("connect to KGlobalAccel on the user session bus: {error}"))?;
    connection
        .request_name(KWIN_SERVICE_NAME, false, true, false)
        .map_err(|error| format!("own {KWIN_SERVICE_NAME}: {error}"))?;

    let state = Arc::new(Mutex::new(CompanionState::default()));
    let owner_state = Arc::clone(&state);
    let owner_rule = MatchRule::new_signal("org.freedesktop.DBus", "NameOwnerChanged");
    connection
        .add_match(
            owner_rule,
            move |(name, _old_owner, new_owner): (String, String, String), _, _| {
                if new_owner.is_empty() {
                    owner_state.lock().owner_lost(&name);
                }
                true
            },
        )
        .map_err(|error| format!("watch D-Bus owner lifecycle: {error}"))?;

    let factory = Factory::new_fn::<()>();
    let register_state = Arc::clone(&state);
    let capabilities_state = Arc::clone(&state);
    let focused_state = Arc::clone(&state);
    let displays_state = Arc::clone(&state);
    let refresh_displays_state = Arc::clone(&state);
    let shortcut_state = Arc::clone(&state);
    let move_state = Arc::clone(&state);
    let register_hotkeys_state = Arc::clone(&state);
    let next_request_state = Arc::clone(&state);
    let complete_request_state = Arc::clone(&state);
    let request_result_state = Arc::clone(&state);
    let update_focused_state = Arc::clone(&state);
    let update_displays_state = Arc::clone(&state);
    let hotkey_state = Arc::clone(&state);
    let next_hotkey_state = Arc::clone(&state);

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
                            let (protocol_raw, reset_state): (String, bool) = method.msg.read2()?;
                            let protocol_major = parse_u32_argument(
                                &protocol_raw,
                                "KWin companion protocol major must be an unsigned integer",
                            )?;
                            let sender = sender_name(method.msg)?;
                            register_state.lock().mark_ready(
                                protocol_major,
                                sender,
                                reset_state,
                            )?;
                            Ok(vec![method.msg.method_return()])
                        }))
                        .add_m(factory.method("GetCapabilities", (), move |method| {
                            let mut state = capabilities_state.lock();
                            state.ensure_ready()?;
                            let reply = method.msg.method_return().append1(vec![
                                FOCUSED_WINDOW_CAPABILITY,
                                DISPLAYS_CAPABILITY,
                                MOVE_RESIZE_CAPABILITY,
                                HOTKEYS_CAPABILITY,
                            ]);
                            Ok(vec![reply])
                        }))
                        .add_m(factory.method("GetFocusedWindow", (), move |method| {
                            let mut state = focused_state.lock();
                            state.ensure_ready()?;
                            let mut reply = method.msg.method_return();
                            let present = state.focused.is_some();
                            let (id, x, y, width, height) =
                                state.focused.clone().unwrap_or_default();
                            reply.append_all((present, id, x, y, width, height));
                            Ok(vec![reply])
                        }))
                        .add_m(factory.method("GetDisplays", (), move |method| {
                            let mut state = displays_state.lock();
                            state.ensure_ready()?;
                            let reply = method.msg.method_return().append1(state.displays.clone());
                            Ok(vec![reply])
                        }))
                        .add_m(factory.method("RefreshDisplays", (), move |method| {
                            let mut state = refresh_displays_state.lock();
                            state.ensure_ready()?;
                            let request_id = state.enqueue_refresh_displays();
                            Ok(vec![method.msg.method_return().append1(request_id)])
                        }))
                        .add_m(factory.method("SetShortcut", (), move |method| {
                            let (hotkey, key): (String, i32) = method.msg.read2()?;
                            let sender = sender_name(method.msg)?;
                            shortcut_state.lock().ensure_script(&sender)?;
                            set_shortcut(&accelerator_connection, hotkey, key)?;
                            Ok(vec![method.msg.method_return()])
                        }))
                        .add_m(factory.method("MoveWindow", (), move |method| {
                            let (window, x, y, width, height): (String, i32, i32, u32, u32) =
                                method.msg.read_all()?;
                            if width == 0 || height == 0 {
                                return Err(method_error(
                                    "org.window_zones.KWin.Error.Invalid",
                                    "window target must have positive dimensions",
                                ));
                            }
                            let mut state = move_state.lock();
                            state.ensure_ready()?;
                            let request_id = state.enqueue_move(window, x, y, width, height);
                            Ok(vec![method.msg.method_return().append1(request_id)])
                        }))
                        .add_m(factory.method("RegisterHotkeys", (), move |method| {
                            let payload: String = method.msg.read1()?;
                            let hotkeys = parse_hotkeys(&payload)?;
                            let sender = sender_name(method.msg)?;
                            let mut state = register_hotkeys_state.lock();
                            state.ensure_ready()?;
                            state.authorize_controller(sender)?;
                            let request_id = state.enqueue_hotkeys(hotkeys);
                            Ok(vec![method.msg.method_return().append1(request_id)])
                        }))
                        .add_m(factory.method("NextRequest", (), move |method| {
                            next_request_state.lock().poll_next_request(method.msg)
                        }))
                        .add_m(factory.method("CompleteRequest", (), move |method| {
                            let (id_raw, ok, error_name, message): (String, bool, String, String) =
                                method.msg.read4()?;
                            let id = parse_u32_argument(
                                &id_raw,
                                "KWin request id must be an unsigned integer",
                            )?;
                            let sender = sender_name(method.msg)?;
                            let mut state = complete_request_state.lock();
                            state.ensure_script(&sender)?;
                            state.complete_request(id, ok, error_name, message)?;
                            Ok(vec![method.msg.method_return()])
                        }))
                        .add_m(factory.method("GetRequestResult", (), move |method| {
                            let request_id_raw: String = method.msg.read1()?;
                            let request_id = parse_u32_argument(
                                &request_id_raw,
                                "KWin request id must be an unsigned integer",
                            )?;
                            let mut state = request_result_state.lock();
                            state.ensure_ready()?;
                            let result = state.request_result(request_id);
                            let mut reply = method.msg.method_return();
                            reply.append_all(result);
                            Ok(vec![reply])
                        }))
                        .add_m(factory.method("UpdateFocusedWindow", (), move |method| {
                            let payload_raw: String = method.msg.read1()?;
                            let payload: (bool, String, i32, i32, u32, u32) =
                                serde_json::from_str(&payload_raw).map_err(|_| {
                                    method_error(
                                        "org.window_zones.KWin.Error.Invalid",
                                        "focused-window update payload is not valid JSON",
                                    )
                                })?;
                            let sender = sender_name(method.msg)?;
                            let mut state = update_focused_state.lock();
                            state.ensure_script(&sender)?;
                            state.touch();
                            state.focused = payload
                                .0
                                .then_some((payload.1, payload.2, payload.3, payload.4, payload.5));
                            Ok(vec![method.msg.method_return()])
                        }))
                        .add_m(factory.method("UpdateDisplays", (), move |method| {
                            let payload: String = method.msg.read1()?;
                            let displays = parse_displays(&payload)?;
                            let sender = sender_name(method.msg)?;
                            let mut state = update_displays_state.lock();
                            state.ensure_script(&sender)?;
                            state.touch();
                            state.displays = displays;
                            Ok(vec![method.msg.method_return()])
                        }))
                        .add_m(factory.method("HotkeyPressed", (), move |method| {
                            let hotkey: String = method.msg.read1()?;
                            let sender = sender_name(method.msg)?;
                            let mut state = hotkey_state.lock();
                            state.ensure_script(&sender)?;
                            state.touch();
                            if state
                                .registered_hotkeys
                                .iter()
                                .any(|registered| registered == &hotkey)
                            {
                                state.events.push_back(hotkey);
                            }
                            Ok(vec![method.msg.method_return()])
                        }))
                        .add_m(factory.method("GetNextHotkey", (), move |method| {
                            let sender = sender_name(method.msg)?;
                            let mut state = next_hotkey_state.lock();
                            state.ensure_ready()?;
                            let event = if state.is_controller(&sender)? {
                                state.events.pop_front()
                            } else {
                                None
                            };
                            let mut reply = method.msg.method_return();
                            reply.append_all((event.is_some(), event.unwrap_or_default()));
                            Ok(vec![reply])
                        })),
                ),
        )
        .add(factory.object_path("/", ()).introspectable());
    tree.start_receive(&connection);

    println!("KWin companion service started for the Window Zones KWin script.");
    loop {
        if let Some(reply) = state.lock().take_next_request_reply(Instant::now()) {
            connection
                .send(reply)
                .map_err(|_| "send deferred KWin NextRequest reply".to_string())?;
        }
        connection
            .process(SERVICE_POLL_INTERVAL)
            .map_err(|error| format!("process KWin companion D-Bus requests: {error}"))?;
    }
}
