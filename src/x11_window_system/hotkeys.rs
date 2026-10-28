use crate::{HotkeyEvent, HotkeySystem, HotkeySystemError};
use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::xkb::{self, ConnectionExt as XkbConnectionExt};
use x11rb::protocol::xproto::{
    ConnectionExt as XProtoConnectionExt, GrabMode, Mapping, ModMask, Window,
};
use x11rb::protocol::{ErrorKind, Event};
use x11rb::rust_connection::RustConnection;

/// Exclusive root-window accelerator grabs on a connection owned by one thread.
#[derive(Debug)]
pub struct X11HotkeySystem {
    commands: Sender<Command>,
    events: Receiver<HotkeyEvent>,
    failures: Receiver<HotkeySystemError>,
    failure: Option<HotkeySystemError>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Debug)]
enum Command {
    Register(Vec<String>, Sender<Result<(), HotkeySystemError>>),
    Stop,
}

type Grabs = BTreeMap<(u8, u16), String>;

impl X11HotkeySystem {
    pub fn new() -> Self {
        Self::with_display(None)
    }

    fn with_display(display: Option<String>) -> Self {
        let (commands, command_rx) = mpsc::channel();
        let (event_tx, events) = mpsc::channel();
        let (failure_tx, failures) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("window-zones-x11-hotkeys".to_string())
            .spawn(move || {
                if let Err(error) = run_listener(display.as_deref(), command_rx, event_tx) {
                    let _ = failure_tx.send(error);
                }
            });
        let (thread, failure) = match spawned {
            Ok(thread) => (Some(thread), None),
            Err(error) => (None, Some(unavailable(format!("start listener: {error}")))),
        };
        Self {
            commands,
            events,
            failures,
            failure,
            thread,
        }
    }

    fn listener_status(&mut self) -> Result<(), HotkeySystemError> {
        if self.failure.is_none() {
            match self.failures.try_recv() {
                Ok(error) => self.failure = Some(error),
                Err(TryRecvError::Disconnected) => {
                    self.failure = Some(unavailable("listener stopped"));
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        self.failure.clone().map_or(Ok(()), Err)
    }

    fn stopped_error(&mut self) -> HotkeySystemError {
        self.listener_status()
            .err()
            .unwrap_or_else(|| unavailable("listener stopped"))
    }
}

impl Default for X11HotkeySystem {
    fn default() -> Self {
        Self::new()
    }
}

impl HotkeySystem for X11HotkeySystem {
    fn register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), HotkeySystemError> {
        // Validate before contacting X11: invalid vocabulary is always a rejection.
        for hotkey in hotkeys {
            parse_hotkey(hotkey)?;
        }
        self.listener_status()?;
        let (reply_tx, reply_rx) = mpsc::channel();
        if self
            .commands
            .send(Command::Register(hotkeys.to_vec(), reply_tx))
            .is_err()
        {
            return Err(self.stopped_error());
        }
        let result = reply_rx
            .recv()
            .unwrap_or_else(|_| Err(self.stopped_error()));
        if let Err(error @ HotkeySystemError::Unavailable(_)) = &result {
            self.failure = Some(error.clone());
        }
        result
    }

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
        self.listener_status()?;
        match self.events.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(self.stopped_error()),
        }
    }
}

impl Drop for X11HotkeySystem {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn unavailable(operation: impl std::fmt::Display) -> HotkeySystemError {
    HotkeySystemError::Unavailable(format!(
        "X11 hotkeys could not {operation}; check DISPLAY and X server access, then restart"
    ))
}

fn run_listener(
    display: Option<&str>,
    commands: Receiver<Command>,
    events: Sender<HotkeyEvent>,
) -> Result<(), HotkeySystemError> {
    let (conn, screen) =
        x11rb::connect(display).map_err(|error| unavailable(format!("open display: {error}")))?;
    let root = conn.setup().roots[screen].root;
    enable_detectable_repeat(&conn)?;
    let mut grabs = Grabs::new();
    let mut accepted = Vec::new();
    let mut pressed = [false; 256];
    loop {
        match commands.recv_timeout(Duration::from_millis(5)) {
            Ok(Command::Register(hotkeys, response)) => {
                let result = resolve_grabs(&conn, &hotkeys)
                    .and_then(|new| replace_grabs(&conn, root, &mut grabs, new));
                if result.is_ok() {
                    accepted = hotkeys;
                }
                let failed = match &result {
                    Err(error @ HotkeySystemError::Unavailable(_)) => Some(error.clone()),
                    _ => None,
                };
                let _ = response.send(result);
                if let Some(error) = failed {
                    return Err(error); // Closing the connection releases even partially changed grabs.
                }
            }
            Ok(Command::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                ungrab_all(&conn, root, grabs.keys().copied())?;
                return Ok(());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        while let Some(event) = conn
            .poll_for_event()
            .map_err(|error| unavailable(format!("read key events: {error}")))?
        {
            match event {
                Event::KeyPress(event) => {
                    if std::mem::replace(&mut pressed[usize::from(event.detail)], true) {
                        continue;
                    }
                    let modifiers = u16::from(event.state) & 0xff;
                    if let Some(hotkey) = grabs.get(&(event.detail, modifiers)) {
                        let _ = events.send(HotkeyEvent::Pressed {
                            hotkey: hotkey.clone(),
                        });
                    }
                }
                Event::KeyRelease(event) => {
                    pressed[usize::from(event.detail)] = false;
                }
                event if keyboard_mapping_changed(&event) => {
                    // A rejected remap cannot retain grabs for the old physical keys.
                    // Closing this connection releases them and reports a listener failure.
                    resolve_grabs(&conn, &accepted)
                        .and_then(|new| replace_grabs(&conn, root, &mut grabs, new))
                        .map_err(|error| {
                            unavailable(format!(
                                "rebuild grabs after keyboard mapping change: {error}"
                            ))
                        })?;
                }
                Event::Error(error) => {
                    return Err(unavailable(format!("process X11 request: {error:?}")));
                }
                _ => {}
            }
        }
    }
}

fn keyboard_mapping_changed(event: &Event) -> bool {
    match event {
        Event::MappingNotify(event) => {
            event.request == Mapping::KEYBOARD || event.request == Mapping::MODIFIER
        }
        Event::XkbMapNotify(_) | Event::XkbNewKeyboardNotify(_) => true,
        _ => false,
    }
}

fn enable_detectable_repeat(conn: &RustConnection) -> Result<(), HotkeySystemError> {
    let extension = conn
        .xkb_use_extension(1, 0)
        .map_err(|error| unavailable(format!("enable XKB: {error}")))?
        .reply()
        .map_err(|error| unavailable(format!("enable XKB: {error}")))?;
    if !extension.supported {
        return Err(unavailable(
            "enable XKB detectable auto-repeat (XKB 1.0 is required)",
        ));
    }
    let repeat = xkb::PerClientFlag::DETECTABLE_AUTO_REPEAT;
    let reply = conn
        .xkb_per_client_flags(
            u16::from(xkb::ID::USE_CORE_KBD),
            repeat,
            repeat,
            xkb::BoolCtrl::default(),
            xkb::BoolCtrl::default(),
            xkb::BoolCtrl::default(),
        )
        .map_err(|error| unavailable(format!("enable detectable auto-repeat: {error}")))?
        .reply()
        .map_err(|error| unavailable(format!("enable detectable auto-repeat: {error}")))?;
    if reply.value & repeat != repeat {
        return Err(unavailable("enable XKB detectable auto-repeat"));
    }
    let map = xkb::MapPart::KEY_SYMS
        | xkb::MapPart::MODIFIER_MAP
        | xkb::MapPart::VIRTUAL_MODS
        | xkb::MapPart::VIRTUAL_MOD_MAP;
    conn.xkb_select_events(
        u16::from(xkb::ID::USE_CORE_KBD),
        xkb::EventType::default(),
        xkb::EventType::NEW_KEYBOARD_NOTIFY | xkb::EventType::MAP_NOTIFY,
        map,
        map,
        &xkb::SelectEventsAux::default(),
    )
    .map_err(|error| unavailable(format!("watch keyboard mapping: {error}")))?
    .check()
    .map_err(|error| unavailable(format!("watch keyboard mapping: {error}")))?;
    Ok(())
}

fn resolve_grabs(conn: &RustConnection, hotkeys: &[String]) -> Result<Grabs, HotkeySystemError> {
    let setup = conn.setup();
    let count = setup
        .max_keycode
        .checked_sub(setup.min_keycode)
        .and_then(|count| count.checked_add(1))
        .ok_or_else(|| unavailable("read keyboard keycode range"))?;
    let mapping = conn
        .get_keyboard_mapping(setup.min_keycode, count)
        .map_err(|error| unavailable(format!("read keyboard mapping: {error}")))?
        .reply()
        .map_err(|error| unavailable(format!("read keyboard mapping: {error}")))?;
    let modifiers = conn
        .get_modifier_mapping()
        .map_err(|error| unavailable(format!("read modifier mapping: {error}")))?
        .reply()
        .map_err(|error| unavailable(format!("read modifier mapping: {error}")))?;
    grabs_for_mapping(
        hotkeys,
        setup.min_keycode,
        mapping.keysyms_per_keycode,
        &mapping.keysyms,
        &modifiers.keycodes,
    )
}

fn grabs_for_mapping(
    hotkeys: &[String],
    min_keycode: u8,
    width: u8,
    keysyms: &[u32],
    modifier_keycodes: &[u8],
) -> Result<Grabs, HotkeySystemError> {
    let locks = lock_mask(min_keycode, width, keysyms, modifier_keycodes);
    let mut grabs = Grabs::new();
    for hotkey in hotkeys {
        let (keysym, modifiers) = parse_hotkey(hotkey)?;
        let codes = keycodes_for_keysym(min_keycode, width, keysyms, keysym);
        if codes.is_empty() {
            return Err(HotkeySystemError::Rejected(format!(
                "{hotkey} is unavailable in the X11 keyboard mapping; choose a mapped key"
            )));
        }
        for code in codes {
            for variant in lock_variants(modifiers, locks) {
                if let Some(previous) = grabs.insert((code, variant), hotkey.clone())
                    && previous != *hotkey
                {
                    return Err(HotkeySystemError::Rejected(format!(
                        "{hotkey} and {previous} resolve to the same X11 key; choose distinct bindings"
                    )));
                }
            }
        }
    }
    Ok(grabs)
}

fn replace_grabs(
    conn: &RustConnection,
    root: Window,
    old: &mut Grabs,
    new: Grabs,
) -> Result<(), HotkeySystemError> {
    let mut added = Vec::new();
    for (&(keycode, modifiers), hotkey) in &new {
        if old.contains_key(&(keycode, modifiers)) {
            continue;
        }
        let result = conn
            .grab_key(
                false,
                root,
                modifiers.into(),
                keycode,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )
            .map_err(ReplyError::from)
            .and_then(|cookie| cookie.check());
        if let Err(error) = result {
            ungrab_all(conn, root, added.into_iter())?;
            return if matches!(&error, ReplyError::X11Error(error) if error.error_kind == ErrorKind::Access)
            {
                Err(HotkeySystemError::Rejected(format!(
                    "{hotkey} is already grabbed by another program"
                )))
            } else {
                Err(unavailable(format!("grab {hotkey}: {error}")))
            };
        }
        added.push((keycode, modifiers));
    }
    ungrab_all(
        conn,
        root,
        old.keys().filter(|key| !new.contains_key(key)).copied(),
    )?;
    *old = new;
    Ok(())
}

fn ungrab_all(
    conn: &RustConnection,
    root: Window,
    grabs: impl Iterator<Item = (u8, u16)>,
) -> Result<(), HotkeySystemError> {
    for (keycode, modifiers) in grabs {
        conn.ungrab_key(keycode, root, modifiers.into())
            .map_err(|error| unavailable(format!("release key grab: {error}")))?
            .check()
            .map_err(|error| unavailable(format!("release key grab: {error}")))?;
    }
    conn.flush()
        .map_err(|error| unavailable(format!("flush key grabs: {error}")))
}

fn lock_mask(min: u8, width: u8, keysyms: &[u32], modifier_keycodes: &[u8]) -> u16 {
    let per_modifier = modifier_keycodes.len() / 8;
    if width == 0 || per_modifier == 0 {
        return 0;
    }
    modifier_keycodes
        .chunks_exact(per_modifier)
        .enumerate()
        .filter(|(_, codes)| {
            codes.iter().any(|code| {
                let Some(index) = code.checked_sub(min) else {
                    return false;
                };
                let start = usize::from(index) * usize::from(width);
                keysyms
                    .get(start..start + usize::from(width))
                    .is_some_and(|symbols| symbols.contains(&0xffe5) || symbols.contains(&0xff7f))
            })
        })
        .fold(0, |mask, (index, _)| mask | (1 << index))
}

fn lock_variants(modifiers: u16, locks: u16) -> impl Iterator<Item = u16> {
    let locks = locks & !modifiers;
    let mut next = Some(0u16);
    std::iter::from_fn(move || {
        let variant = next?;
        next = (variant != locks).then(|| variant.wrapping_sub(locks) & locks);
        Some(modifiers | variant)
    })
}

fn keycodes_for_keysym(min: u8, width: u8, mapping: &[u32], keysym: u32) -> Vec<u8> {
    if width == 0 {
        return Vec::new();
    }
    mapping
        .chunks_exact(usize::from(width))
        .enumerate()
        .filter(|(_, symbols)| symbols.contains(&keysym))
        .filter_map(|(index, _)| u8::try_from(usize::from(min) + index).ok())
        .collect()
}

fn parse_hotkey(hotkey: &str) -> Result<(u32, u16), HotkeySystemError> {
    let unsupported = || {
        HotkeySystemError::Rejected(format!(
            "unsupported X11 hotkey {hotkey:?}; use a canonical binding from config"
        ))
    };
    if crate::config::normalize_hotkey(hotkey).ok().as_deref() != Some(hotkey) {
        return Err(unsupported());
    }
    let mut tokens = hotkey.rsplit('+');
    let key = tokens.next().ok_or_else(unsupported)?;
    let mut modifiers = ModMask::default();
    for token in tokens {
        modifiers |= match token {
            "alt" => ModMask::M1,
            "ctrl" => ModMask::CONTROL,
            "shift" => ModMask::SHIFT,
            "cmd" => ModMask::M4,
            _ => return Err(unsupported()),
        };
    }
    let keysym = match key {
        "escape" => 0xff1b,
        "return" => 0xff0d,
        "space" => 0x20,
        "tab" => 0xff09,
        "backspace" => 0xff08,
        "delete" => 0xffff,
        "insert" => 0xff63,
        "home" => 0xff50,
        "end" => 0xff57,
        "pageup" => 0xff55,
        "pagedown" => 0xff56,
        "left" => 0xff51,
        "right" => 0xff53,
        "up" => 0xff52,
        "down" => 0xff54,
        "minus" => 0x2d,
        "equal" => 0x3d,
        "comma" => 0x2c,
        "dot" => 0x2e,
        "slash" => 0x2f,
        "quote" => 0x27,
        "semicolon" => 0x3b,
        "leftbracket" => 0x5b,
        "rightbracket" => 0x5d,
        "backslash" => 0x5c,
        "backquote" => 0x60,
        "printscreen" => 0xff61,
        "scrolllock" => 0xff14,
        "capslock" => 0xffe5,
        "numlock" => 0xff7f,
        "pause" => 0xff13,
        _ if key.len() == 1 && key.as_bytes()[0].is_ascii_alphanumeric() => {
            u32::from(key.as_bytes()[0])
        }
        _ => {
            let number = key
                .strip_prefix('f')
                .and_then(|number| number.parse::<u32>().ok())
                .filter(|number| (1..=24).contains(number))
                .ok_or_else(unsupported)?;
            0xffbe + number - 1
        }
    };
    Ok((keysym, u16::from(modifiers)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn x11_hotkey_keysyms_and_modifiers_cover_canonical_vocabulary() {
        assert_eq!(
            parse_hotkey("alt+ctrl+shift+cmd+left").unwrap(),
            (0xff51, 1 | 4 | 8 | 64)
        );
        for key in b'a'..=b'z' {
            assert_eq!(
                parse_hotkey(&(key as char).to_string()).unwrap(),
                (u32::from(key), 0)
            );
        }
        for key in b'0'..=b'9' {
            assert_eq!(
                parse_hotkey(&(key as char).to_string()).unwrap(),
                (u32::from(key), 0)
            );
        }
        for number in 1..=24 {
            assert_eq!(
                parse_hotkey(&format!("f{number}")).unwrap(),
                (0xffbe + number - 1, 0)
            );
        }
        for (key, symbol) in [
            ("escape", 0xff1b),
            ("return", 0xff0d),
            ("space", 0x20),
            ("tab", 0xff09),
            ("backspace", 0xff08),
            ("delete", 0xffff),
            ("insert", 0xff63),
            ("home", 0xff50),
            ("end", 0xff57),
            ("pageup", 0xff55),
            ("pagedown", 0xff56),
            ("left", 0xff51),
            ("right", 0xff53),
            ("up", 0xff52),
            ("down", 0xff54),
            ("minus", 0x2d),
            ("equal", 0x3d),
            ("comma", 0x2c),
            ("dot", 0x2e),
            ("slash", 0x2f),
            ("quote", 0x27),
            ("semicolon", 0x3b),
            ("leftbracket", 0x5b),
            ("rightbracket", 0x5d),
            ("backslash", 0x5c),
            ("backquote", 0x60),
            ("printscreen", 0xff61),
            ("scrolllock", 0xff14),
            ("capslock", 0xffe5),
            ("numlock", 0xff7f),
            ("pause", 0xff13),
        ] {
            assert_eq!(
                parse_hotkey(&format!("cmd+{key}")).unwrap(),
                (symbol, 64),
                "{key}"
            );
        }
    }

    #[test]
    fn x11_hotkeys_reject_aliases_noncanonical_order_and_unknown_keys() {
        for hotkey in [
            "CTRL+a",
            "ctrl+alt+a",
            "alt+alt+a",
            "super+a",
            "ctrl+enter",
            "ctrl+.",
            "f25",
            "f01",
            "ctrl+unknown",
            " ctrl+a",
            "",
        ] {
            assert!(
                matches!(parse_hotkey(hotkey), Err(HotkeySystemError::Rejected(_))),
                "{hotkey}"
            );
        }
    }

    #[test]
    fn x11_hotkeys_derive_lock_variants_from_actual_modifier_slots() {
        let symbols = [0x61, 0x41, 0xffe5, 0, 0xff7f, 0];
        let modifier_codes = [0, 9, 0, 0, 0, 10, 0, 0];
        let locks = lock_mask(8, 2, &symbols, &modifier_codes);
        assert_eq!(locks, 2 | 32);
        assert_eq!(
            lock_variants(4 | 8, locks).collect::<Vec<_>>(),
            [12, 14, 44, 46]
        );
        let grabs =
            grabs_for_mapping(&["ctrl+a".to_string()], 8, 2, &symbols, &modifier_codes).unwrap();
        for modifiers in lock_variants(4, locks) {
            assert_eq!(
                grabs.get(&(8, modifiers)).map(String::as_str),
                Some("ctrl+a")
            );
        }
        assert!(!grabs.contains_key(&(8, 4 | 16))); // Mod2 is not a lock in this map.
        assert_eq!(lock_variants(4, 0).collect::<Vec<_>>(), [4]);
        assert_eq!(lock_variants(4, 4).collect::<Vec<_>>(), [4]);
        assert_eq!(lock_mask(8, 0, &symbols, &modifier_codes), 0);
        assert_eq!(lock_mask(8, 2, &symbols, &[]), 0);
    }

    #[test]
    fn x11_hotkeys_rebuild_keycodes_and_locks_without_changing_accepted_bindings() {
        let accepted = ["ctrl+y".to_string()];
        let before = grabs_for_mapping(
            &accepted,
            8,
            1,
            &[0x79, 0x7a, 0xff7f],
            &[0, 0, 0, 0, 10, 0, 0, 0],
        )
        .unwrap();
        let after = grabs_for_mapping(
            &accepted,
            8,
            1,
            &[0x7a, 0x79, 0xff7f],
            &[0, 0, 0, 0, 0, 10, 0, 0],
        )
        .unwrap();
        assert_eq!(
            before.keys().copied().collect::<Vec<_>>(),
            [(8, 4), (8, 20)]
        );
        assert_eq!(after.keys().copied().collect::<Vec<_>>(), [(9, 4), (9, 36)]);
        assert!(matches!(
            grabs_for_mapping(&accepted, 8, 1, &[0x7a, 0x78, 0xff7f], &[]),
            Err(HotkeySystemError::Rejected(_))
        ));
    }

    #[test]
    fn x11_hotkeys_refresh_on_keyboard_and_modifier_notifications_only() {
        use x11rb::protocol::xproto::MappingNotifyEvent;

        for (request, expected) in [
            (Mapping::KEYBOARD, true),
            (Mapping::MODIFIER, true),
            (Mapping::POINTER, false),
        ] {
            assert_eq!(
                keyboard_mapping_changed(&Event::MappingNotify(MappingNotifyEvent {
                    request,
                    ..Default::default()
                })),
                expected
            );
        }
        assert!(keyboard_mapping_changed(&Event::XkbMapNotify(
            Default::default()
        )));
        assert!(keyboard_mapping_changed(&Event::XkbNewKeyboardNotify(
            Default::default()
        )));
    }

    #[test]
    fn x11_hotkeys_resolve_every_matching_keycode_in_keyboard_mapping() {
        let symbols = [0x61, 0x41, 0x62, 0x42, 0xff51, 0, 0x61, 0x41];
        assert_eq!(keycodes_for_keysym(8, 2, &symbols, 0x61), [8, 11]);
        assert_eq!(keycodes_for_keysym(8, 2, &symbols, 0xff51), [10]);
        assert!(keycodes_for_keysym(8, 2, &symbols, 0xffbe).is_empty());
        assert!(keycodes_for_keysym(8, 0, &symbols, 0x61).is_empty());
    }

    #[test]
    fn x11_hotkeys_surface_display_failure_without_panicking_in_constructor() {
        let mut listener = X11HotkeySystem::with_display(Some("not-an-x11-display".to_string()));
        assert!(matches!(
            listener.register_hotkeys(&["ctrl+a".to_string()]),
            Err(HotkeySystemError::Unavailable(_))
        ));
        assert!(matches!(
            listener.next_hotkey(),
            Err(HotkeySystemError::Unavailable(_))
        ));
        assert!(matches!(
            listener.register_hotkeys(&["CTRL+a".to_string()]),
            Err(HotkeySystemError::Rejected(_))
        ));
    }

    #[test]
    #[ignore = "requires WZ_X11_TEST_DISPLAY pointing at an isolated X server"]
    fn x11_real_server_grabs_are_exclusive_atomic_and_released() {
        use std::time::Instant;
        use x11rb::protocol::xproto::{CreateWindowAux, EventMask, InputFocus, WindowClass};
        use x11rb::protocol::xtest::ConnectionExt as XTestConnectionExt;

        let display =
            std::env::var("WZ_X11_TEST_DISPLAY").expect("supply an isolated test DISPLAY");
        assert_ne!(display, ":0", "never test on the user's desktop");
        let (observer, screen) = x11rb::connect(Some(&display)).unwrap();
        let root = observer.setup().roots[screen].root;
        let (competitor, _) = x11rb::connect(Some(&display)).unwrap();
        let window = observer.generate_id().unwrap();
        observer
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                window,
                root,
                50,
                50,
                200,
                100,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new()
                    .override_redirect(1)
                    .event_mask(EventMask::KEY_PRESS | EventMask::KEY_RELEASE),
            )
            .unwrap()
            .check()
            .unwrap();
        observer.map_window(window).unwrap().check().unwrap();
        thread::sleep(Duration::from_millis(100));
        observer
            .set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME)
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(
            observer.get_input_focus().unwrap().reply().unwrap().focus,
            window
        );

        let grab = |connection: &RustConnection, code: u8, modifiers: u16| {
            connection
                .grab_key(
                    false,
                    root,
                    modifiers.into(),
                    code,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )
                .unwrap()
                .check()
        };
        let keycode = |hotkey: &str| {
            resolve_grabs(&observer, &[hotkey.to_string()])
                .unwrap()
                .keys()
                .next()
                .unwrap()
                .0
        };
        let f7 = keycode("ctrl+f7");
        let f8 = keycode("ctrl+f8");
        let f9 = keycode("ctrl+f9");
        let keyboard = observer
            .get_keyboard_mapping(
                observer.setup().min_keycode,
                observer.setup().max_keycode - observer.setup().min_keycode + 1,
            )
            .unwrap()
            .reply()
            .unwrap();
        let modifiers = observer.get_modifier_mapping().unwrap().reply().unwrap();
        let locks = lock_mask(
            observer.setup().min_keycode,
            keyboard.keysyms_per_keycode,
            &keyboard.keysyms,
            &modifiers.keycodes,
        );
        let ctrl = keycodes_for_keysym(
            observer.setup().min_keycode,
            keyboard.keysyms_per_keycode,
            &keyboard.keysyms,
            0xffe3,
        )[0];
        let mut listener = X11HotkeySystem::with_display(Some(display));
        listener.register_hotkeys(&["ctrl+f8".to_string()]).unwrap();
        listener.register_hotkeys(&["ctrl+f8".to_string()]).unwrap(); // Shared old/new grabs must survive.
        for modifiers in lock_variants(4, locks) {
            let error = grab(&competitor, f8, modifiers).unwrap_err();
            assert!(
                matches!(error, ReplyError::X11Error(error) if error.error_kind == ErrorKind::Access)
            );
            grab(&competitor, f9, modifiers).unwrap();
        }
        let rejected = listener.register_hotkeys(&["ctrl+f7".to_string(), "ctrl+f9".to_string()]);
        assert!(
            matches!(rejected, Err(HotkeySystemError::Rejected(message)) if message == "ctrl+f9 is already grabbed by another program")
        );
        for modifiers in lock_variants(4, locks) {
            grab(&competitor, f7, modifiers).unwrap(); // The partially acquired set rolled back.
        }

        let inject = |event_type, keycode| {
            observer
                .xtest_fake_input(event_type, keycode, 0, root, 0, 0, 0)
                .unwrap()
                .check()
                .unwrap();
        };
        while observer.poll_for_event().unwrap().is_some() {}
        inject(x11rb::protocol::xproto::KEY_PRESS_EVENT, ctrl);
        assert_eq!(
            u16::from(observer.query_pointer(root).unwrap().reply().unwrap().mask) & 4,
            4,
            "XTEST input must reach this X server; use Xvfb or rootful XWayland without the EI portal"
        );
        inject(x11rb::protocol::xproto::KEY_PRESS_EVENT, f8);
        inject(x11rb::protocol::xproto::KEY_PRESS_EVENT, f8); // No release: detectable repeat is suppressed.
        thread::sleep(Duration::from_millis(900)); // Exercise server-generated auto-repeat too.
        inject(x11rb::protocol::xproto::KEY_RELEASE_EVENT, f8);
        inject(x11rb::protocol::xproto::KEY_RELEASE_EVENT, ctrl);
        let deadline = Instant::now() + Duration::from_secs(2);
        let event = loop {
            if let Some(event) = listener.next_hotkey().unwrap() {
                break event;
            }
            assert!(
                Instant::now() < deadline,
                "grabbed XTEST key did not reach listener"
            );
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(
            event,
            HotkeyEvent::Pressed {
                hotkey: "ctrl+f8".to_string()
            }
        );
        thread::sleep(Duration::from_millis(50));
        assert_eq!(listener.next_hotkey().unwrap(), None);
        while let Some(event) = observer.poll_for_event().unwrap() {
            assert!(
                !matches!(event, Event::KeyPress(event) if event.detail == f8),
                "the grabbed accelerator leaked into the focused application"
            );
        }

        for code in [f7, f9] {
            ungrab_all(
                &competitor,
                root,
                lock_variants(4, locks).map(|modifiers| (code, modifiers)),
            )
            .unwrap();
        }
        listener.register_hotkeys(&["ctrl+f9".to_string()]).unwrap();
        for modifiers in lock_variants(4, locks) {
            grab(&competitor, f8, modifiers).unwrap(); // Replacing a set released old grabs.
        }
        listener.register_hotkeys(&[]).unwrap();
        for modifiers in lock_variants(4, locks) {
            grab(&competitor, f9, modifiers).unwrap();
        }
        ungrab_all(
            &competitor,
            root,
            lock_variants(4, locks).map(|modifiers| (f9, modifiers)),
        )
        .unwrap();
        listener.register_hotkeys(&["ctrl+f9".to_string()]).unwrap();
        drop(listener);
        for modifiers in lock_variants(4, locks) {
            grab(&competitor, f9, modifiers).unwrap(); // Drop joined the thread and released every variant.
        }
        observer.destroy_window(window).unwrap().check().unwrap();
    }

    #[test]
    #[ignore = "requires WZ_X11_TEST_DISPLAY pointing at an isolated X server; changes its keymap"]
    fn x11_real_server_remaps_accepted_grabs_and_fails_closed_on_conflict() {
        use std::time::Instant;
        use x11rb::protocol::xproto::{
            GetKeyboardMappingReply, GetModifierMappingReply, MappingStatus,
        };

        struct RestoreMapping<'a> {
            conn: &'a RustConnection,
            min: u8,
            count: u8,
            keyboard: GetKeyboardMappingReply,
            modifiers: GetModifierMappingReply,
        }
        impl Drop for RestoreMapping<'_> {
            fn drop(&mut self) {
                if let Ok(cookie) = self.conn.change_keyboard_mapping(
                    self.count,
                    self.min,
                    self.keyboard.keysyms_per_keycode,
                    &self.keyboard.keysyms,
                ) {
                    let _ = cookie.check();
                }
                if let Ok(cookie) = self.conn.set_modifier_mapping(&self.modifiers.keycodes) {
                    let _ = cookie.reply();
                }
            }
        }

        let display =
            std::env::var("WZ_X11_TEST_DISPLAY").expect("supply an isolated test DISPLAY");
        assert_ne!(display, ":0", "never change the user's keyboard mapping");
        let (observer, screen) = x11rb::connect(Some(&display)).unwrap();
        let (competitor, _) = x11rb::connect(Some(&display)).unwrap();
        let root = observer.setup().roots[screen].root;
        let min = observer.setup().min_keycode;
        let count = observer.setup().max_keycode - min + 1;
        let saved = RestoreMapping {
            conn: &observer,
            min,
            count,
            keyboard: observer
                .get_keyboard_mapping(min, count)
                .unwrap()
                .reply()
                .unwrap(),
            modifiers: observer.get_modifier_mapping().unwrap().reply().unwrap(),
        };
        let width = saved.keyboard.keysyms_per_keycode;
        let code = |keysym| keycodes_for_keysym(min, width, &saved.keyboard.keysyms, keysym)[0];
        let f7 = code(0xffc4);
        let f8 = code(0xffc5);
        let numlock = code(0xff7f);
        assert_ne!(f7, f8);
        let locks = lock_mask(
            min,
            width,
            &saved.keyboard.keysyms,
            &saved.modifiers.keycodes,
        );
        let grab = |keycode, modifiers: u16| {
            competitor
                .grab_key(
                    false,
                    root,
                    modifiers.into(),
                    keycode,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                )
                .unwrap()
                .check()
        };
        let mut listener = X11HotkeySystem::with_display(Some(display));
        listener.register_hotkeys(&["ctrl+f7".to_string()]).unwrap();

        let mut remapped = saved.keyboard.keysyms.clone();
        let a = usize::from(f7 - min) * usize::from(width);
        let b = usize::from(f8 - min) * usize::from(width);
        for offset in 0..usize::from(width) {
            remapped.swap(a + offset, b + offset);
        }
        observer
            .change_keyboard_mapping(count, min, width, &remapped)
            .unwrap()
            .check()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            listener.next_hotkey().unwrap();
            match grab(f7, 4) {
                Ok(()) => {
                    ungrab_all(&competitor, root, [(f7, 4)].into_iter()).unwrap();
                    break;
                }
                Err(ReplyError::X11Error(error)) if error.error_kind == ErrorKind::Access => {}
                Err(error) => panic!("probe old key grab: {error}"),
            }
            assert!(
                Instant::now() < deadline,
                "remapping retained the old physical key"
            );
            thread::sleep(Duration::from_millis(5));
        }
        for modifiers in lock_variants(4, locks) {
            assert!(matches!(
                grab(f8, modifiers),
                Err(ReplyError::X11Error(error)) if error.error_kind == ErrorKind::Access
            ));
        }

        // Relocate NumLock to a modifier slot that was not previously a lock.
        let slot = [5, 7, 6, 4, 3]
            .into_iter()
            .find(|slot| locks & (1 << slot) == 0)
            .unwrap();
        let per_modifier = usize::from(saved.modifiers.keycodes_per_modifier());
        let mut groups: Vec<Vec<u8>> = saved
            .modifiers
            .keycodes
            .chunks_exact(per_modifier)
            .map(|codes| {
                codes
                    .iter()
                    .copied()
                    .filter(|code| *code != 0 && *code != numlock)
                    .collect()
            })
            .collect();
        groups[slot].push(numlock);
        let new_width = groups.iter().map(Vec::len).max().unwrap();
        let mut modifier_codes = Vec::new();
        for mut group in groups {
            group.resize(new_width, 0);
            modifier_codes.extend(group);
        }
        assert_eq!(
            observer
                .set_modifier_mapping(&modifier_codes)
                .unwrap()
                .reply()
                .unwrap()
                .status,
            MappingStatus::SUCCESS
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            listener.next_hotkey().unwrap();
            match grab(f8, 4 | (1 << slot)) {
                Err(ReplyError::X11Error(error)) if error.error_kind == ErrorKind::Access => break,
                Ok(()) => {
                    ungrab_all(&competitor, root, [(f8, 4 | (1 << slot))].into_iter()).unwrap();
                }
                Err(error) => panic!("probe relocated NumLock grab: {error}"),
            }
            assert!(
                Instant::now() < deadline,
                "modifier remapping did not rebuild lock variants"
            );
            thread::sleep(Duration::from_millis(5));
        }
        let new_locks = lock_mask(min, width, &remapped, &modifier_codes);
        for modifiers in lock_variants(4, locks & !new_locks).filter(|state| *state != 4) {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                listener.next_hotkey().unwrap();
                match grab(f8, modifiers) {
                    Ok(()) => {
                        ungrab_all(&competitor, root, [(f8, modifiers)].into_iter()).unwrap();
                        break;
                    }
                    Err(ReplyError::X11Error(error)) if error.error_kind == ErrorKind::Access => {}
                    Err(error) => panic!("probe obsolete lock variant: {error}"),
                }
                assert!(
                    Instant::now() < deadline,
                    "remapping retained an obsolete lock variant"
                );
                thread::sleep(Duration::from_millis(5));
            }
        }

        // A conflicting remap must release the old key, not preserve a wrong grab.
        for modifiers in lock_variants(4, new_locks) {
            grab(f7, modifiers).unwrap();
        }
        observer
            .change_keyboard_mapping(count, min, width, &saved.keyboard.keysyms)
            .unwrap()
            .check()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match listener.next_hotkey() {
                Err(HotkeySystemError::Unavailable(message)) => {
                    assert!(message.contains("keyboard mapping change"));
                    break;
                }
                Ok(None) => {}
                other => panic!("unexpected remap result: {other:?}"),
            }
            assert!(
                Instant::now() < deadline,
                "conflicting remap did not fail closed"
            );
            thread::sleep(Duration::from_millis(5));
        }
        for modifiers in lock_variants(4, new_locks) {
            grab(f8, modifiers).unwrap();
        }
        drop(listener);
        drop(competitor);
        drop(saved);
    }
}
