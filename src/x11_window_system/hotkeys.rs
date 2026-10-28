use crate::{HotkeyEvent, HotkeySystem, HotkeySystemError};
use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::xkb::{self, ConnectionExt as XkbConnectionExt};
use x11rb::protocol::xproto::{ConnectionExt as XProtoConnectionExt, GrabMode, Mapping, Window};
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
        Event::XkbStateNotify(event) => {
            event.changed & xkb::StatePart::GROUP_STATE != xkb::StatePart::default()
        }
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
    let map = xkb::MapPart::KEY_TYPES
        | xkb::MapPart::KEY_SYMS
        | xkb::MapPart::MODIFIER_MAP
        | xkb::MapPart::VIRTUAL_MODS
        | xkb::MapPart::VIRTUAL_MOD_MAP;
    conn.xkb_select_events(
        u16::from(xkb::ID::USE_CORE_KBD),
        xkb::EventType::default(),
        xkb::EventType::NEW_KEYBOARD_NOTIFY | xkb::EventType::MAP_NOTIFY,
        map,
        map,
        &xkb::SelectEventsAux::default().state_notify(xkb::SelectEventsAuxStateNotify {
            affect_state: xkb::StatePart::GROUP_STATE,
            state_details: xkb::StatePart::GROUP_STATE,
        }),
    )
    .map_err(|error| unavailable(format!("watch keyboard mapping: {error}")))?
    .check()
    .map_err(|error| unavailable(format!("watch keyboard mapping: {error}")))?;
    Ok(())
}

fn keyboard_mapping(conn: &RustConnection) -> Result<xkb::GetMapReply, HotkeySystemError> {
    conn.xkb_get_map(
        u16::from(xkb::ID::USE_CORE_KBD),
        xkb::MapPart::KEY_TYPES | xkb::MapPart::KEY_SYMS,
        xkb::MapPart::default(),
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        xkb::VMod::default(),
        0,
        0,
        0,
        0,
        0,
        0,
    )
    .map_err(|error| unavailable(format!("read XKB keyboard mapping: {error}")))?
    .reply()
    .map_err(|error| unavailable(format!("read XKB keyboard mapping: {error}")))
}

fn resolve_grabs(conn: &RustConnection, hotkeys: &[String]) -> Result<Grabs, HotkeySystemError> {
    let mapping = keyboard_mapping(conn)?;
    let state = conn
        .xkb_get_state(u16::from(xkb::ID::USE_CORE_KBD))
        .map_err(|error| unavailable(format!("read XKB keyboard group: {error}")))?
        .reply()
        .map_err(|error| unavailable(format!("read XKB keyboard group: {error}")))?;
    let modifiers = conn
        .get_modifier_mapping()
        .map_err(|error| unavailable(format!("read modifier mapping: {error}")))?
        .reply()
        .map_err(|error| unavailable(format!("read modifier mapping: {error}")))?;
    grabs_for_mapping(
        hotkeys,
        mapping.first_key_sym,
        &mapping.map,
        u8::from(state.group),
        &modifiers.keycodes,
    )
}

fn grabs_for_mapping(
    hotkeys: &[String],
    min_keycode: u8,
    mapping: &xkb::GetMapMap,
    group: u8,
    modifier_keycodes: &[u8],
) -> Result<Grabs, HotkeySystemError> {
    let symbols = mapping
        .syms_rtrn
        .as_deref()
        .ok_or_else(|| unavailable("read XKB key symbols"))?;
    let types = mapping
        .types_rtrn
        .as_deref()
        .ok_or_else(|| unavailable("read XKB key types"))?;
    let locks = modifier_mask(min_keycode, symbols, modifier_keycodes, &[0xffe5, 0xff7f]);
    let mut grabs = Grabs::new();
    for hotkey in hotkeys {
        let (keysym, modifiers) = parse_hotkey(hotkey)?;
        let mut masks = vec![0];
        let mut explicit_shift = 0;
        for modifier in modifiers.split('+').filter(|token| !token.is_empty()) {
            let keysyms: &[u32] = match modifier {
                "alt" => &[0xffe9, 0xffea],
                "ctrl" => &[0xffe3, 0xffe4],
                "shift" => &[0xffe1, 0xffe2],
                "cmd" => &[0xffeb, 0xffec],
                _ => unreachable!("parse_hotkey validates modifier vocabulary"),
            };
            let slots = modifier_mask(min_keycode, symbols, modifier_keycodes, keysyms);
            if slots == 0 {
                return Err(HotkeySystemError::Rejected(format!(
                    "{hotkey} has no mapped X11 {modifier} modifier; choose a mapped modifier"
                )));
            }
            if modifier == "shift" {
                explicit_shift = slots;
            }
            masks = masks
                .into_iter()
                .flat_map(|mask| lock_variants(0, slots).skip(1).map(move |slot| mask | slot))
                .collect();
        }
        let mut found = false;
        for (index, key) in symbols.iter().enumerate() {
            let groups = key.group_info & 0x0f;
            if groups == 0 || key.width == 0 {
                continue;
            }
            if groups > 4 {
                return Err(unavailable("read valid XKB key groups"));
            }
            // XKB applies each key's own wrap/clamp/redirect policy to the active group.
            let group = if group < groups {
                group
            } else {
                match key.group_info & 0xc0 {
                    0x40 => groups - 1,
                    0x80 => {
                        let redirect = (key.group_info >> 4) & 3;
                        if redirect < groups { redirect } else { 0 }
                    }
                    _ => group % groups,
                }
            };
            let key_type = types
                .get(usize::from(key.kt_index[usize::from(group)]))
                .ok_or_else(|| unavailable("read valid XKB key type"))?;
            let start = usize::from(group) * usize::from(key.width);
            let levels = key
                .syms
                .get(start..start + usize::from(key.width))
                .ok_or_else(|| unavailable("read valid XKB key symbols"))?;
            for (level, symbol) in levels.iter().enumerate() {
                if *symbol != keysym || level >= usize::from(key_type.num_levels) {
                    continue;
                }
                for selector in std::iter::once(0).chain(
                    key_type
                        .map
                        .iter()
                        .filter(|entry| entry.active)
                        .map(|entry| u16::from(entry.mods_mask)),
                ) {
                    if selector & locks != 0 {
                        continue;
                    }
                    for modifiers in &masks {
                        // Named keys use their layout's level; an explicit Shift additionally
                        // modifies that key (for example ctrl+shift+1 on a US layout).
                        let state = selector | (modifiers & !explicit_shift);
                        let effective_level = key_type
                            .map
                            .iter()
                            .find(|entry| {
                                entry.active
                                    && state & u16::from(key_type.mods_mask)
                                        == u16::from(entry.mods_mask)
                            })
                            .map_or(0, |entry| usize::from(entry.level));
                        if effective_level != level {
                            continue;
                        }
                        let code = u8::try_from(usize::from(min_keycode) + index)
                            .map_err(|_| unavailable("read valid XKB keycode"))?;
                        found = true;
                        for variant in lock_variants(selector | modifiers, locks) {
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
            }
        }
        if !found {
            return Err(HotkeySystemError::Rejected(format!(
                "{hotkey} is unavailable in the active X11 keyboard group; choose a mapped key"
            )));
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

fn modifier_mask(
    min: u8,
    mapping: &[xkb::KeySymMap],
    modifier_keycodes: &[u8],
    keysyms: &[u32],
) -> u16 {
    let per_modifier = modifier_keycodes.len() / 8;
    if per_modifier == 0 {
        return 0;
    }
    modifier_keycodes
        .chunks_exact(per_modifier)
        .enumerate()
        .filter(|(_, codes)| {
            codes.iter().any(|code| {
                code.checked_sub(min)
                    .and_then(|index| mapping.get(usize::from(index)))
                    .is_some_and(|key| key.syms.iter().any(|symbol| keysyms.contains(symbol)))
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

fn parse_hotkey(hotkey: &str) -> Result<(u32, &str), HotkeySystemError> {
    let unsupported = || {
        HotkeySystemError::Rejected(format!(
            "unsupported X11 hotkey {hotkey:?}; use a canonical binding from config"
        ))
    };
    if crate::config::normalize_hotkey(hotkey).ok().as_deref() != Some(hotkey) {
        return Err(unsupported());
    }
    let (modifiers, key) = hotkey.rsplit_once('+').unwrap_or(("", hotkey));
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
    Ok((keysym, modifiers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use x11rb::protocol::xproto::ModMask;

    fn keyboard_map(width: u8, rows: &[&[u32]]) -> xkb::GetMapMap {
        xkb::GetMapMap {
            types_rtrn: Some(vec![xkb::KeyType {
                mods_mask: ModMask::SHIFT,
                num_levels: 2,
                map: vec![xkb::KTMapEntry {
                    active: true,
                    mods_mask: ModMask::SHIFT,
                    level: 1,
                    ..Default::default()
                }],
                ..Default::default()
            }]),
            syms_rtrn: Some(
                rows.iter()
                    .map(|symbols| xkb::KeySymMap {
                        group_info: symbols.len() as u8 / width,
                        width,
                        syms: symbols.to_vec(),
                        ..Default::default()
                    })
                    .collect(),
            ),
            ..Default::default()
        }
    }

    #[test]
    fn x11_hotkey_keysyms_cover_canonical_vocabulary() {
        for key in b'a'..=b'z' {
            assert_eq!(
                parse_hotkey(&(key as char).to_string()).unwrap().0,
                u32::from(key)
            );
        }
        for key in b'0'..=b'9' {
            assert_eq!(
                parse_hotkey(&(key as char).to_string()).unwrap().0,
                u32::from(key)
            );
        }
        for number in 1..=24 {
            assert_eq!(
                parse_hotkey(&format!("f{number}")).unwrap().0,
                0xffbe + number - 1
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
                parse_hotkey(&format!("cmd+{key}")).unwrap().0,
                symbol,
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
        let mapping = keyboard_map(
            2,
            &[&[0x61, 0x41], &[0xffe5, 0], &[0xff7f, 0], &[0xffe3, 0]],
        );
        let modifier_codes = [0, 9, 11, 0, 0, 10, 0, 0];
        let grabs =
            grabs_for_mapping(&["ctrl+a".to_string()], 8, &mapping, 0, &modifier_codes).unwrap();
        assert_eq!(
            grabs.keys().copied().collect::<Vec<_>>(),
            [(8, 4), (8, 6), (8, 36), (8, 38)]
        );
        assert!(!grabs.contains_key(&(8, 4 | 16))); // Mod2 is not a lock in this map.
        assert_eq!(lock_variants(4, 0).collect::<Vec<_>>(), [4]);
        assert_eq!(lock_variants(4, 4).collect::<Vec<_>>(), [4]);
    }

    #[test]
    fn x11_hotkeys_rebuild_keycodes_and_locks_without_changing_accepted_bindings() {
        let accepted = ["ctrl+y".to_string()];
        let before = grabs_for_mapping(
            &accepted,
            8,
            &keyboard_map(1, &[&[0x79], &[0x7a], &[0xff7f], &[0xffe3]]),
            0,
            &[0, 0, 11, 0, 10, 0, 0, 0],
        )
        .unwrap();
        let after = grabs_for_mapping(
            &accepted,
            8,
            &keyboard_map(1, &[&[0x7a], &[0x79], &[0xff7f], &[0xffe3]]),
            0,
            &[0, 0, 11, 0, 0, 10, 0, 0],
        )
        .unwrap();
        assert_eq!(
            before.keys().copied().collect::<Vec<_>>(),
            [(8, 4), (8, 20)]
        );
        assert_eq!(after.keys().copied().collect::<Vec<_>>(), [(9, 4), (9, 36)]);
        assert!(matches!(
            grabs_for_mapping(
                &accepted,
                8,
                &keyboard_map(1, &[&[0x7a], &[0x78], &[0xff7f], &[0xffe3]]),
                0,
                &[0, 0, 11, 0, 0, 10, 0, 0],
            ),
            Err(HotkeySystemError::Rejected(_))
        ));
    }

    #[test]
    fn x11_hotkeys_refresh_on_mapping_and_effective_group_notifications() {
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
        assert!(keyboard_mapping_changed(&Event::XkbStateNotify(
            xkb::StateNotifyEvent {
                changed: xkb::StatePart::GROUP_STATE,
                ..Default::default()
            }
        )));
        assert!(!keyboard_mapping_changed(&Event::XkbStateNotify(
            xkb::StateNotifyEvent {
                changed: xkb::StatePart::MODIFIER_STATE,
                ..Default::default()
            }
        )));
    }

    #[test]
    fn x11_hotkeys_follow_remapped_alt_and_super_modifier_slots() {
        let mapping = keyboard_map(1, &[&[0x61], &[0xffe9], &[0xffeb], &[0xffe3]]);
        let accepted = ["alt+a".to_string(), "cmd+a".to_string()];
        let before =
            grabs_for_mapping(&accepted, 8, &mapping, 0, &[0, 0, 11, 9, 0, 0, 10, 0]).unwrap();
        let after =
            grabs_for_mapping(&accepted, 8, &mapping, 0, &[0, 0, 11, 0, 10, 9, 0, 0]).unwrap();
        assert_eq!(
            before,
            BTreeMap::from([
                ((8, 8), "alt+a".to_string()),
                ((8, 64), "cmd+a".to_string())
            ])
        );
        assert_eq!(
            after,
            BTreeMap::from([
                ((8, 16), "cmd+a".to_string()),
                ((8, 32), "alt+a".to_string())
            ])
        );
        assert!(matches!(
            grabs_for_mapping(&accepted, 8, &mapping, 0, &[]),
            Err(HotkeySystemError::Rejected(_))
        ));
    }

    #[test]
    fn x11_hotkeys_keep_y_and_z_distinct_in_the_active_keyboard_group() {
        let mapping = keyboard_map(
            2,
            &[
                &[0x79, 0x59, 0x7a, 0x5a],
                &[0x7a, 0x5a, 0x79, 0x59],
                &[0xffe3, 0],
            ],
        );
        let accepted = ["ctrl+y".to_string(), "ctrl+z".to_string()];
        let modifiers = [0, 0, 10, 0, 0, 0, 0, 0];
        let us = grabs_for_mapping(&accepted, 8, &mapping, 0, &modifiers).unwrap();
        let de = grabs_for_mapping(&accepted, 8, &mapping, 1, &modifiers).unwrap();
        assert_eq!(
            us,
            BTreeMap::from([
                ((8, 4), "ctrl+y".to_string()),
                ((9, 4), "ctrl+z".to_string())
            ])
        );
        assert_eq!(
            de,
            BTreeMap::from([
                ((8, 4), "ctrl+z".to_string()),
                ((9, 4), "ctrl+y".to_string())
            ])
        );
        assert_eq!(
            grabs_for_mapping(&accepted, 8, &mapping, 2, &modifiers).unwrap(),
            us
        );
    }

    #[test]
    fn x11_hotkeys_include_required_levels_without_grabbing_unshifted_keys() {
        let mapping = keyboard_map(
            2,
            &[&[0x30, 0x3d], &[0x79, 0x59], &[0xffe3, 0], &[0xffe1, 0]],
        );
        let accepted = ["ctrl+equal".to_string(), "ctrl+shift+y".to_string()];
        let grabs =
            grabs_for_mapping(&accepted, 8, &mapping, 0, &[11, 0, 10, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(
            grabs,
            BTreeMap::from([
                ((8, 5), "ctrl+equal".to_string()),
                ((9, 5), "ctrl+shift+y".to_string())
            ])
        );
        assert!(!grabs.contains_key(&(8, 4)));
        assert!(!grabs.contains_key(&(9, 4)));
        assert!(matches!(
            grabs_for_mapping(
                &["ctrl+equal".to_string(), "ctrl+shift+0".to_string()],
                8,
                &mapping,
                0,
                &[11, 0, 10, 0, 0, 0, 0, 0],
            ),
            Err(HotkeySystemError::Rejected(_))
        ));
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
        enable_detectable_repeat(&observer).unwrap();
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
        let keyboard = keyboard_mapping(&observer).unwrap();
        let symbols = keyboard.map.syms_rtrn.as_deref().unwrap();
        let modifiers = observer.get_modifier_mapping().unwrap().reply().unwrap();
        let locks = modifier_mask(
            keyboard.first_key_sym,
            symbols,
            &modifiers.keycodes,
            &[0xffe5, 0xff7f],
        );
        let ctrl = keyboard.first_key_sym
            + u8::try_from(
                symbols
                    .iter()
                    .position(|key| key.syms.contains(&0xffe3))
                    .unwrap(),
            )
            .unwrap();
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
        assert!(matches!(rejected, Err(HotkeySystemError::Rejected(_))));
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
        use x11rb::protocol::xtest::ConnectionExt as XTestConnectionExt;

        fn set_symbols(
            conn: &RustConnection,
            mapping: &xkb::GetMapReply,
            symbols: Vec<xkb::KeySymMap>,
        ) -> Result<(), ReplyError> {
            let total =
                u16::try_from(symbols.iter().map(|key| key.syms.len()).sum::<usize>()).unwrap();
            conn.xkb_set_map(
                u16::from(xkb::ID::USE_CORE_KBD),
                xkb::SetMapFlags::default(),
                mapping.min_key_code,
                mapping.max_key_code,
                0,
                0,
                mapping.first_key_sym,
                u8::try_from(symbols.len()).unwrap(),
                total,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                xkb::VMod::default(),
                &xkb::SetMapAux::default().syms(symbols),
            )?
            .check()
        }

        fn set_group(conn: &RustConnection, group: xkb::Group) -> Result<(), ReplyError> {
            conn.xkb_latch_lock_state(
                u16::from(xkb::ID::USE_CORE_KBD),
                ModMask::default(),
                ModMask::default(),
                true,
                group,
                ModMask::default(),
                false,
                0,
            )?
            .check()
        }

        struct RestoreMapping<'a> {
            conn: &'a RustConnection,
            min: u8,
            count: u8,
            keyboard: GetKeyboardMappingReply,
            modifiers: GetModifierMappingReply,
            xkb: xkb::GetMapReply,
            group: xkb::Group,
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
                let _ = set_symbols(
                    self.conn,
                    &self.xkb,
                    self.xkb.map.syms_rtrn.clone().unwrap(),
                );
                let _ = set_group(self.conn, self.group);
            }
        }

        let display =
            std::env::var("WZ_X11_TEST_DISPLAY").expect("supply an isolated test DISPLAY");
        assert_ne!(display, ":0", "never change the user's keyboard mapping");
        let (observer, screen) = x11rb::connect(Some(&display)).unwrap();
        enable_detectable_repeat(&observer).unwrap();
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
            xkb: keyboard_mapping(&observer).unwrap(),
            group: observer
                .xkb_get_state(u16::from(xkb::ID::USE_CORE_KBD))
                .unwrap()
                .reply()
                .unwrap()
                .group,
        };
        let width = saved.keyboard.keysyms_per_keycode;
        let code = |hotkey: &str| {
            resolve_grabs(&observer, &[hotkey.to_string()])
                .unwrap()
                .keys()
                .next()
                .unwrap()
                .0
        };
        let f7 = code("ctrl+f7");
        let f8 = code("ctrl+f8");
        let numlock = code("ctrl+numlock");
        assert_ne!(f7, f8);
        let xkb_mapping = keyboard_mapping(&observer).unwrap();
        let symbols = xkb_mapping.map.syms_rtrn.as_deref().unwrap();
        let locks = modifier_mask(
            xkb_mapping.first_key_sym,
            symbols,
            &saved.modifiers.keycodes,
            &[0xffe5, 0xff7f],
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

        // A real US/German Y/Z pair must resolve independently and follow a group lock.
        let y = code("ctrl+y");
        let z = code("ctrl+z");
        let mut grouped = saved.xkb.map.syms_rtrn.clone().unwrap();
        for (code, syms) in [
            (y, vec![0x79, 0x59, 0x7a, 0x5a]),
            (z, vec![0x7a, 0x5a, 0x79, 0x59]),
        ] {
            let key = &mut grouped[usize::from(code - saved.xkb.first_key_sym)];
            key.kt_index = [key.kt_index[0]; 4];
            key.group_info = 2;
            key.width = 2;
            key.syms = syms;
        }
        set_group(&observer, xkb::Group::M1).unwrap();
        set_symbols(&observer, &saved.xkb, grouped).unwrap();
        let mut grouped_listener = X11HotkeySystem::with_display(Some(display.clone()));
        grouped_listener
            .register_hotkeys(&["ctrl+y".to_string(), "ctrl+z".to_string()])
            .unwrap();
        grouped_listener
            .register_hotkeys(&["ctrl+y".to_string()])
            .unwrap();
        set_group(&observer, xkb::Group::M2).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            grouped_listener.next_hotkey().unwrap();
            match grab(y, 4) {
                Ok(()) => {
                    ungrab_all(&competitor, root, [(y, 4)].into_iter()).unwrap();
                    break;
                }
                Err(ReplyError::X11Error(error)) if error.error_kind == ErrorKind::Access => {}
                Err(error) => panic!("probe old keyboard group grab: {error}"),
            }
            assert!(
                Instant::now() < deadline,
                "group switch retained the old Y key"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(
            grab(z, 4),
            Err(ReplyError::X11Error(error)) if error.error_kind == ErrorKind::Access
        ));
        let ctrl = saved.xkb.first_key_sym
            + u8::try_from(
                symbols
                    .iter()
                    .position(|key| key.syms.contains(&0xffe3))
                    .unwrap(),
            )
            .unwrap();
        for (event, key) in [
            (x11rb::protocol::xproto::KEY_PRESS_EVENT, ctrl),
            (x11rb::protocol::xproto::KEY_PRESS_EVENT, z),
            (x11rb::protocol::xproto::KEY_RELEASE_EVENT, z),
            (x11rb::protocol::xproto::KEY_RELEASE_EVENT, ctrl),
        ] {
            observer
                .xtest_fake_input(event, key, 0, root, 0, 0, 0)
                .unwrap()
                .check()
                .unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(event) = grouped_listener.next_hotkey().unwrap() {
                assert_eq!(
                    event,
                    HotkeyEvent::Pressed {
                        hotkey: "ctrl+y".to_string()
                    }
                );
                break;
            }
            assert!(
                Instant::now() < deadline,
                "active-group accelerator did not fire"
            );
            thread::sleep(Duration::from_millis(5));
        }
        drop(grouped_listener);
        set_group(&observer, saved.group).unwrap();
        set_symbols(
            &observer,
            &saved.xkb,
            saved.xkb.map.syms_rtrn.clone().unwrap(),
        )
        .unwrap();

        // Exchange the actual Alt/Super slots, without changing their keysyms.
        let alt = modifier_mask(
            saved.xkb.first_key_sym,
            symbols,
            &saved.modifiers.keycodes,
            &[0xffe9, 0xffea],
        );
        let super_ = modifier_mask(
            saved.xkb.first_key_sym,
            symbols,
            &saved.modifiers.keycodes,
            &[0xffeb, 0xffec],
        );
        assert!(alt.is_power_of_two() && super_.is_power_of_two() && alt != super_);
        let mut modifier_listener = X11HotkeySystem::with_display(Some(display.clone()));
        modifier_listener
            .register_hotkeys(&["alt+f7".to_string(), "cmd+f8".to_string()])
            .unwrap();
        let mut relocated = saved.modifiers.keycodes.clone();
        let per_modifier = usize::from(saved.modifiers.keycodes_per_modifier());
        for offset in 0..per_modifier {
            relocated.swap(
                alt.trailing_zeros() as usize * per_modifier + offset,
                super_.trailing_zeros() as usize * per_modifier + offset,
            );
        }
        assert_eq!(
            observer
                .set_modifier_mapping(&relocated)
                .unwrap()
                .reply()
                .unwrap()
                .status,
            MappingStatus::SUCCESS
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            modifier_listener.next_hotkey().unwrap();
            match grab(f7, alt) {
                Ok(()) => {
                    ungrab_all(&competitor, root, [(f7, alt)].into_iter()).unwrap();
                    break;
                }
                Err(ReplyError::X11Error(error)) if error.error_kind == ErrorKind::Access => {}
                Err(error) => panic!("probe old Alt slot grab: {error}"),
            }
            assert!(
                Instant::now() < deadline,
                "modifier remap retained the old Alt slot"
            );
            thread::sleep(Duration::from_millis(5));
        }
        for (key, modifiers) in [(f7, super_), (f8, alt)] {
            assert!(matches!(
                grab(key, modifiers),
                Err(ReplyError::X11Error(error)) if error.error_kind == ErrorKind::Access
            ));
        }
        grab(f8, super_).unwrap();
        ungrab_all(&competitor, root, [(f8, super_)].into_iter()).unwrap();
        drop(modifier_listener);
        assert_eq!(
            observer
                .set_modifier_mapping(&saved.modifiers.keycodes)
                .unwrap()
                .reply()
                .unwrap()
                .status,
            MappingStatus::SUCCESS
        );
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
        let new_locks = modifier_mask(
            xkb_mapping.first_key_sym,
            symbols,
            &modifier_codes,
            &[0xffe5, 0xff7f],
        );
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
