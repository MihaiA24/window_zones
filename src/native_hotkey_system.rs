use std::collections::HashSet;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;

use rdev::{Event, EventType, Key, listen};
use window_zones::{HotkeyEvent, HotkeySystem, HotkeySystemError};

#[derive(Debug)]
pub struct NativeHotkeySystem {
    registered_hotkeys: Arc<Mutex<HashSet<String>>>,
    events: Receiver<HotkeyEvent>,
    start_error: Arc<Mutex<Option<HotkeySystemError>>>,
}

impl NativeHotkeySystem {
    pub fn new() -> Self {
        let (event_sender, events) = mpsc::channel();
        let registered_hotkeys = Arc::new(Mutex::new(HashSet::new()));
        let start_error = Arc::new(Mutex::new(None));

        let registered_hotkeys_for_listener = Arc::clone(&registered_hotkeys);
        let start_error_for_listener = Arc::clone(&start_error);

        thread::spawn(move || {
            let mut pressed = HashSet::new();
            let mut pending_hotkeys = HashSet::new();

            let result = listen(move |event| {
                if let Some(hotkey) = hotkey_from_event(&event, &mut pressed, &mut pending_hotkeys)
                {
                    let hotkeys = lock_guard(&registered_hotkeys_for_listener);
                    if hotkeys.contains(&hotkey) {
                        let _ = event_sender.send(HotkeyEvent::Pressed { hotkey });
                    }
                }
            });

            if let Err(error) = result {
                let mut state = lock_guard(&start_error_for_listener);
                *state = Some(HotkeySystemError::Platform(format!(
                    "failed to start native hotkey listener: {error:?}"
                )));
            }
        });

        Self {
            registered_hotkeys,
            events,
            start_error,
        }
    }

    fn last_error(&self) -> Option<HotkeySystemError> {
        lock_guard(&self.start_error).clone()
    }
}

impl HotkeySystem for NativeHotkeySystem {
    fn register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), HotkeySystemError> {
        if let Some(error) = self.last_error() {
            return Err(error);
        }

        let mut active = lock_guard(&self.registered_hotkeys);
        active.clear();
        for hotkey in hotkeys {
            active.insert(hotkey.to_string());
        }
        Ok(())
    }

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
        if let Some(error) = self.last_error() {
            return Err(error);
        }

        match self.events.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(HotkeySystemError::Platform(
                "native hotkey listener unexpectedly disconnected".to_string(),
            )),
        }
    }
}

fn hotkey_from_event(
    event: &Event,
    pressed: &mut HashSet<Key>,
    pending_hotkeys: &mut HashSet<String>,
) -> Option<String> {
    hotkey_from_event_type(event.event_type, pressed, pending_hotkeys)
}

fn hotkey_from_event_type(
    event_type: EventType,
    pressed: &mut HashSet<Key>,
    pending_hotkeys: &mut HashSet<String>,
) -> Option<String> {
    match event_type {
        EventType::KeyPress(key) => {
            pressed.insert(key);
            let hotkey = hotkey_for_pressed(pressed)?;

            if pending_hotkeys.contains(&hotkey) {
                return None;
            }

            pending_hotkeys.insert(hotkey.clone());
            Some(hotkey)
        }
        EventType::KeyRelease(key) => {
            pressed.remove(&key);
            pending_hotkeys.clear();
            None
        }
        _ => None,
    }
}

fn hotkey_for_pressed(pressed: &HashSet<Key>) -> Option<String> {
    let mut modifiers = Vec::new();
    let mut non_modifiers = Vec::new();

    for key in pressed {
        match key_token(key) {
            Some(KeyToken::Modifier(token)) => modifiers.push(token),
            Some(KeyToken::NonModifier(token)) => non_modifiers.push(token),
            None => {}
        }
    }

    if non_modifiers.len() != 1 {
        return None;
    }

    modifiers.sort_by_key(|token| modifier_sort_key(token));
    modifiers.dedup();

    let mut combo = String::new();
    for modifier in modifiers {
        combo.push_str(modifier);
        combo.push('+');
    }
    combo.push_str(&non_modifiers[0]);

    Some(combo)
}

enum KeyToken<'a> {
    Modifier(&'a str),
    NonModifier(String),
}

fn key_token(key: &Key) -> Option<KeyToken<'_>> {
    if let Some(token) = modifier_token(key) {
        return Some(KeyToken::Modifier(token));
    }

    if let Some(token) = named_key_token(key) {
        return Some(KeyToken::NonModifier(token));
    }

    None
}

fn named_key_token(key: &Key) -> Option<String> {
    let key_name = format_key_name(key);
    normalize_non_modifier_key(&key_name)
}

fn format_key_name(key: &Key) -> String {
    format!("{key:?}").trim().to_ascii_lowercase()
}

fn normalize_non_modifier_key(name: &str) -> Option<String> {
    let key = name.replace(['_', '-'], "");

    if key == "leftarrow" {
        return Some("left".to_string());
    }
    if key == "rightarrow" {
        return Some("right".to_string());
    }
    if key == "uparrow" {
        return Some("up".to_string());
    }
    if key == "downarrow" {
        return Some("down".to_string());
    }

    if key.len() == 1 && key.chars().all(|value| value.is_ascii_alphanumeric()) {
        return Some(key);
    }

    if let Some(token) = key
        .strip_prefix("key")
        .filter(|tail| !tail.is_empty())
        .filter(|tail| tail.chars().all(|value| value.is_ascii_alphanumeric()))
    {
        return Some(token.to_string());
    }

    match key.as_str() {
        "tab" | "space" | "escape" | "return" | "enter" | "backspace" | "delete" => Some(key),
        "pageup" | "pagedown" | "home" | "end" | "insert" | "numlock" | "capslock" => Some(key),
        _ if key.starts_with("numpad") && key.len() > 6 => Some(key.to_string()),
        value if value.starts_with('f') && value.len() > 1 => {
            let tail = &value[1..];
            if !tail.is_empty() && tail.chars().all(|value| value.is_ascii_digit()) {
                return Some(value.to_string());
            }
            None
        }
        _ => None,
    }
}

fn modifier_token(key: &Key) -> Option<&'static str> {
    let key_name = format_key_name(key);

    if key_name.contains("shift") {
        return Some("shift");
    }

    if key_name.contains("control") || key_name == "ctrl" || key_name == "ctl" {
        return Some("ctrl");
    }

    if key_name.contains("meta")
        || key_name == "super"
        || key_name == "cmd"
        || key_name.contains("win")
    {
        return Some("cmd");
    }

    if key_name.contains("alt") {
        return Some("alt");
    }

    None
}

fn modifier_sort_key(token: &str) -> u8 {
    match token {
        "alt" => 0,
        "ctrl" => 1,
        "shift" => 2,
        "cmd" => 3,
        _ => 4,
    }
}

fn lock_guard<T>(state: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match state.lock() {
        Ok(guard) => guard,
        Err(error) => error.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_from_event_type_ignores_modifier_only_presses() {
        let mut pressed = HashSet::new();
        let mut pending = HashSet::new();

        assert_eq!(
            hotkey_from_event_type(
                EventType::KeyPress(Key::ControlLeft),
                &mut pressed,
                &mut pending
            ),
            None
        );
        assert_eq!(
            hotkey_from_event_type(EventType::KeyPress(Key::Alt), &mut pressed, &mut pending),
            None
        );
        assert_eq!(
            hotkey_from_event_type(EventType::KeyRelease(Key::Alt), &mut pressed, &mut pending),
            None
        );
        assert_eq!(pressed.len(), 1);
        assert!(pressed.contains(&Key::ControlLeft));
        assert!(pending.is_empty());
    }

    #[test]
    fn hotkey_from_event_type_maps_stable_modifier_order() {
        let mut pressed = HashSet::new();
        let mut pending = HashSet::new();

        assert_eq!(
            hotkey_from_event_type(
                EventType::KeyPress(Key::ShiftRight),
                &mut pressed,
                &mut pending
            ),
            None
        );
        assert_eq!(
            hotkey_from_event_type(
                EventType::KeyPress(Key::ControlLeft),
                &mut pressed,
                &mut pending
            ),
            None
        );
        assert_eq!(
            hotkey_from_event_type(EventType::KeyPress(Key::Alt), &mut pressed, &mut pending),
            None
        );

        assert_eq!(
            hotkey_from_event_type(
                EventType::KeyPress(Key::LeftArrow),
                &mut pressed,
                &mut pending
            ),
            Some("alt+ctrl+shift+left".to_string())
        );
    }

    #[test]
    fn hotkey_from_event_type_deduplicates_until_release() {
        let mut pressed = HashSet::new();
        let mut pending = HashSet::new();

        assert_eq!(
            hotkey_from_event_type(
                EventType::KeyPress(Key::ControlLeft),
                &mut pressed,
                &mut pending
            ),
            None
        );
        assert_eq!(
            hotkey_from_event_type(EventType::KeyPress(Key::KeyA), &mut pressed, &mut pending),
            Some("ctrl+a".to_string())
        );
        assert_eq!(
            hotkey_from_event_type(EventType::KeyPress(Key::KeyA), &mut pressed, &mut pending),
            None
        );

        assert_eq!(
            hotkey_from_event_type(EventType::KeyRelease(Key::KeyA), &mut pressed, &mut pending),
            None
        );
        assert_eq!(
            hotkey_from_event_type(EventType::KeyPress(Key::KeyS), &mut pressed, &mut pending),
            Some("ctrl+s".to_string())
        );
    }

    #[test]
    fn hotkey_from_event_type_requires_single_non_modifier() {
        let mut pressed = HashSet::new();
        let mut pending = HashSet::new();

        assert_eq!(
            hotkey_from_event_type(EventType::KeyPress(Key::Alt), &mut pressed, &mut pending),
            None
        );
        assert_eq!(
            hotkey_from_event_type(EventType::KeyPress(Key::KeyS), &mut pressed, &mut pending),
            Some("alt+s".to_string())
        );
        assert_eq!(
            hotkey_from_event_type(EventType::KeyPress(Key::KeyD), &mut pressed, &mut pending),
            None
        );
    }

    #[test]
    fn normalize_non_modifier_key_recognizes_known_inputs() {
        assert_eq!(
            normalize_non_modifier_key("leftarrow"),
            Some("left".to_string())
        );
        assert_eq!(
            normalize_non_modifier_key("right-arrow"),
            Some("right".to_string())
        );
        assert_eq!(normalize_non_modifier_key("keya"), Some("a".to_string()));
        assert_eq!(normalize_non_modifier_key("f1"), Some("f1".to_string()));
        assert_eq!(normalize_non_modifier_key("key"), None);
    }

    #[test]
    fn key_token_classifies_key_roles() {
        assert!(matches!(
            key_token(&Key::Alt),
            Some(KeyToken::Modifier("alt"))
        ));
        assert!(matches!(
            key_token(&Key::MetaLeft),
            Some(KeyToken::Modifier("cmd"))
        ));
        assert!(matches!(
            key_token(&Key::KeyC),
            Some(KeyToken::NonModifier(token)) if token == "c"
        ));
        assert!(matches!(
            key_token(&Key::Return),
            Some(KeyToken::NonModifier(token)) if token == "return"
        ));
    }
}
