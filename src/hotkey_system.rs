use thiserror::Error;

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
use std::collections::{HashMap, HashSet};
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
use std::sync::mpsc::{self, TryRecvError};
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
use std::sync::{Arc, LazyLock, Mutex};
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
use std::thread;

#[cfg(any(target_os = "windows", target_os = "macos"))]
use rdev::grab;
#[cfg(target_os = "linux")]
use rdev::listen;
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
use rdev::{Event, EventType, Key};

/// Event generated when a configured global hotkey is activated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyEvent {
    Pressed { hotkey: String },
}

/// Errors produced by platform hotkey adapters.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HotkeySystemError {
    /// The requested set was refused as a whole (an unsupported key, an
    /// accelerator another program owns); the previously registered set stays
    /// active. Retrying the same set cannot succeed.
    #[error("hotkey set rejected: {0}")]
    Rejected(String),
    /// Hotkeys cannot be registered or delivered right now (companion absent or
    /// busy, listener failure); nothing is known to be registered and a later
    /// retry may succeed.
    #[error("hotkeys unavailable: {0}")]
    Unavailable(String),
}

/// Platform adapter contract for global hotkey registration and dispatch.
///
/// `register_hotkeys` replaces the complete registered set atomically. Dropping
/// an implementation releases every grab and listener it owns, so a restarted
/// runtime never leaves a second listener behind.
pub trait HotkeySystem {
    fn register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), HotkeySystemError>;

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError>;
}

/// Instances share one process-owned rdev listener: rdev's global callback must
/// never be replaced by a second `listen`/`grab`. The singleton passes all input
/// through after its active registration is dropped (apart from releases whose
/// presses it already consumed); the next instance can reuse it on restart.
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
#[derive(Debug)]
pub struct RdevHotkeySystem {
    listener: Arc<Mutex<RdevListener>>,
    owner: Arc<()>,
    event_tx: mpsc::Sender<HotkeyEvent>,
    events: mpsc::Receiver<HotkeyEvent>,
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
#[derive(Debug, Default)]
struct RdevListener {
    bindings: HashMap<(u8, Key), String>,
    owner: Option<Arc<()>>,
    event_tx: Option<mpsc::Sender<HotkeyEvent>>,
    error: Option<String>,
    pressed: HashSet<Key>,
    swallowed: HashSet<Key>,
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
impl RdevListener {
    fn status(&self) -> Result<(), HotkeySystemError> {
        match &self.error {
            Some(error) => Err(HotkeySystemError::Unavailable(error.clone())),
            None => Ok(()),
        }
    }

    fn clear_registration(&mut self) {
        self.bindings.clear();
        self.event_tx = None;
        self.owner = None;
    }

    /// Return true only when the native event should be consumed.
    fn handle_event(&mut self, event: EventType, exclusive: bool) -> bool {
        match event {
            EventType::KeyPress(key) => {
                if !self.pressed.insert(key) {
                    return self.swallowed.contains(&key);
                }
                if modifier_mask(&key).is_some() {
                    return false;
                }
                let modifiers = self
                    .pressed
                    .iter()
                    .filter_map(modifier_mask)
                    .fold(0, |a, b| a | b);
                let Some(hotkey) = self.bindings.get(&(modifiers, key)) else {
                    return false;
                };
                let Some(sender) = &self.event_tx else {
                    return false;
                };
                if sender
                    .send(HotkeyEvent::Pressed {
                        hotkey: hotkey.clone(),
                    })
                    .is_err()
                {
                    self.clear_registration();
                    return false;
                }
                if exclusive {
                    self.swallowed.insert(key);
                }
                exclusive
            }
            EventType::KeyRelease(key) => {
                self.pressed.remove(&key);
                self.swallowed.remove(&key)
            }
            _ => false,
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
impl RdevHotkeySystem {
    pub fn new() -> Self {
        static LISTENER: LazyLock<Arc<Mutex<RdevListener>>> = LazyLock::new(|| {
            let state = Arc::new(Mutex::new(RdevListener::default()));
            RdevHotkeySystem::spawn_listener(Arc::clone(&state));
            state
        });
        Self::with_listener(Arc::clone(&LISTENER))
    }

    fn with_listener(listener: Arc<Mutex<RdevListener>>) -> Self {
        let (event_tx, events) = mpsc::channel();
        Self {
            listener,
            owner: Arc::new(()),
            event_tx,
            events,
        }
    }

    fn spawn_listener(listener: Arc<Mutex<RdevListener>>) {
        let error_state = Arc::clone(&listener);
        let spawn_error_state = Arc::clone(&listener);
        let spawn = thread::Builder::new().name("window-zones-hotkeys".to_string()).spawn(move || {
            let result = std::panic::catch_unwind(move || {
                #[cfg(any(target_os = "windows", target_os = "macos"))]
                let result = grab(move |event: Event| {
                    let consume = listener.lock().map(|mut state| state.handle_event(event.event_type, true))
                        .unwrap_or(false);
                    if consume { None } else { Some(event) }
                }).map_err(|error| format!("{error:?}"));

                #[cfg(target_os = "linux")]
                let result = listen(move |event: Event| {
                    if let Ok(mut state) = listener.lock() {
                        state.handle_event(event.event_type, false);
                    }
                }).map_err(|error| format!("{error:?}"));
                result
            });
            let details = match result {
                Ok(Err(error)) => error,
                Ok(Ok(())) => "native listener exited".to_string(),
                Err(_) => "native listener panicked".to_string(),
            };
            if let Ok(mut state) = error_state.lock() {
                state.clear_registration();
                state.error = Some(format!(
                    "global hotkey listener stopped: {details}; restore input/Accessibility permission and restart Window Zones"
                ));
            }
        });
        if let Err(error) = spawn
            && let Ok(mut state) = spawn_error_state.lock()
        {
            state.clear_registration();
            state.error = Some(format!(
                "failed to start global hotkey listener: {error}; restart Window Zones"
            ));
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
impl Default for RdevHotkeySystem {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
impl HotkeySystem for RdevHotkeySystem {
    fn register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), HotkeySystemError> {
        let mut next = HashMap::with_capacity(hotkeys.len());
        for hotkey in hotkeys {
            let (binding, canonical) = canonical_binding(hotkey)?;
            next.insert(binding, canonical);
        }
        let mut state = self.listener.lock().map_err(|_| {
            HotkeySystemError::Unavailable(
                "failed to update hotkey listener state; restart Window Zones".to_string(),
            )
        })?;
        state.status()?;
        while self.events.try_recv().is_ok() {}
        state.bindings = next;
        state.owner = Some(Arc::clone(&self.owner));
        state.event_tx = Some(self.event_tx.clone());
        Ok(())
    }

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
        let state = self.listener.lock().map_err(|_| {
            HotkeySystemError::Unavailable(
                "failed to read hotkey listener state; restart Window Zones".to_string(),
            )
        })?;
        state.status()?;
        if !state
            .owner
            .as_ref()
            .is_some_and(|owner| Arc::ptr_eq(owner, &self.owner))
        {
            return Ok(None);
        }
        match self.events.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(HotkeySystemError::Unavailable(
                "global hotkey listener is unavailable; restart Window Zones".to_string(),
            )),
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
impl Drop for RdevHotkeySystem {
    fn drop(&mut self) {
        if let Ok(mut state) = self.listener.lock()
            && state
                .owner
                .as_ref()
                .is_some_and(|owner| Arc::ptr_eq(owner, &self.owner))
        {
            state.clear_registration();
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
const MOD_ALT: u8 = 0b0001;
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
const MOD_CTRL: u8 = 0b0010;
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
const MOD_SHIFT: u8 = 0b0100;
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
const MOD_CMD: u8 = 0b1000;

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
fn modifier_mask(key: &Key) -> Option<u8> {
    Some(match key {
        Key::Alt | Key::AltGr => MOD_ALT,
        Key::ControlLeft | Key::ControlRight => MOD_CTRL,
        Key::ShiftLeft | Key::ShiftRight => MOD_SHIFT,
        Key::MetaLeft | Key::MetaRight => MOD_CMD,
        _ => return None,
    })
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
fn canonical_binding(raw: &str) -> Result<((u8, Key), String), HotkeySystemError> {
    let canonical = crate::config::normalize_hotkey(raw)
        .map_err(|error| HotkeySystemError::Rejected(format!("invalid hotkey `{raw}`: {error}")))?;
    if canonical != raw {
        return Err(HotkeySystemError::Rejected(format!(
            "hotkey `{raw}` is not canonical; use `{canonical}`"
        )));
    }
    Ok((binding_code(&canonical)?, canonical))
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
fn binding_code(canonical: &str) -> Result<(u8, Key), HotkeySystemError> {
    let mut tokens = canonical.split('+');
    let key_token = tokens.next_back().unwrap_or_default();
    let modifiers = tokens.fold(0, |mask, token| {
        mask | match token {
            "alt" => MOD_ALT,
            "ctrl" => MOD_CTRL,
            "shift" => MOD_SHIFT,
            "cmd" => MOD_CMD,
            _ => 0,
        }
    });
    let key = key_token_to_code(key_token).ok_or_else(|| {
        HotkeySystemError::Rejected(format!(
            "unsupported hotkey key `{key_token}` in `{canonical}`"
        ))
    })?;
    Ok((modifiers, key))
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
fn key_token_to_code(token: &str) -> Option<Key> {
    #[cfg(target_os = "windows")]
    if let Some(number) = token
        .strip_prefix('f')
        .and_then(|number| number.parse::<u32>().ok())
        && (13..=24).contains(&number)
    {
        return Some(Key::Unknown(111 + number));
    }
    #[cfg(target_os = "macos")]
    match token {
        "f13" => return Some(Key::Unknown(105)),
        "f14" => return Some(Key::Unknown(107)),
        "f15" => return Some(Key::Unknown(113)),
        "f16" => return Some(Key::Unknown(106)),
        "f17" => return Some(Key::Unknown(64)),
        "f18" => return Some(Key::Unknown(79)),
        "f19" => return Some(Key::Unknown(80)),
        "f20" => return Some(Key::Unknown(90)),
        "delete" => return Some(Key::Unknown(117)),
        "insert" => return Some(Key::Unknown(114)),
        "home" => return Some(Key::Unknown(115)),
        "end" => return Some(Key::Unknown(119)),
        "pageup" => return Some(Key::Unknown(116)),
        "pagedown" => return Some(Key::Unknown(121)),
        // No distinct macOS hardware keys exist for these PC-only key names.
        "printscreen" | "scrolllock" | "pause" | "numlock" => return None,
        _ => {}
    }
    if let Some(c) = token.chars().next() {
        if token.len() == 1 {
            return match c {
                'a'..='z' => Some(match c {
                    'a' => Key::KeyA,
                    'b' => Key::KeyB,
                    'c' => Key::KeyC,
                    'd' => Key::KeyD,
                    'e' => Key::KeyE,
                    'f' => Key::KeyF,
                    'g' => Key::KeyG,
                    'h' => Key::KeyH,
                    'i' => Key::KeyI,
                    'j' => Key::KeyJ,
                    'k' => Key::KeyK,
                    'l' => Key::KeyL,
                    'm' => Key::KeyM,
                    'n' => Key::KeyN,
                    'o' => Key::KeyO,
                    'p' => Key::KeyP,
                    'q' => Key::KeyQ,
                    'r' => Key::KeyR,
                    's' => Key::KeyS,
                    't' => Key::KeyT,
                    'u' => Key::KeyU,
                    'v' => Key::KeyV,
                    'w' => Key::KeyW,
                    'x' => Key::KeyX,
                    'y' => Key::KeyY,
                    'z' => Key::KeyZ,
                    _ => return None,
                }),
                '0'..='9' => Some(match c {
                    '0' => Key::Num0,
                    '1' => Key::Num1,
                    '2' => Key::Num2,
                    '3' => Key::Num3,
                    '4' => Key::Num4,
                    '5' => Key::Num5,
                    '6' => Key::Num6,
                    '7' => Key::Num7,
                    '8' => Key::Num8,
                    '9' => Key::Num9,
                    _ => return None,
                }),
                _ => None,
            };
        }

        if let Some(stripped) = token.strip_prefix('f') {
            return match stripped {
                "1" => Some(Key::F1),
                "2" => Some(Key::F2),
                "3" => Some(Key::F3),
                "4" => Some(Key::F4),
                "5" => Some(Key::F5),
                "6" => Some(Key::F6),
                "7" => Some(Key::F7),
                "8" => Some(Key::F8),
                "9" => Some(Key::F9),
                "10" => Some(Key::F10),
                "11" => Some(Key::F11),
                "12" => Some(Key::F12),
                _ => None,
            };
        }
    }

    Some(match token {
        "space" => Key::Space,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "escape" => Key::Escape,
        "insert" => Key::Insert,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "delete" => Key::Delete,
        "left" => Key::LeftArrow,
        "right" => Key::RightArrow,
        "up" => Key::UpArrow,
        "down" => Key::DownArrow,
        "minus" => Key::Minus,
        "equal" => Key::Equal,
        "comma" => Key::Comma,
        "dot" => Key::Dot,
        "slash" => Key::Slash,
        "quote" => Key::Quote,
        "semicolon" => Key::SemiColon,
        "leftbracket" => Key::LeftBracket,
        "rightbracket" => Key::RightBracket,
        "backslash" => Key::BackSlash,
        "backquote" => Key::BackQuote,
        "return" => Key::Return,
        "printscreen" => Key::PrintScreen,
        "scrolllock" => Key::ScrollLock,
        "pause" => Key::Pause,
        "capslock" => Key::CapsLock,
        "numlock" => Key::NumLock,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system_without_native_listener() -> RdevHotkeySystem {
        RdevHotkeySystem::with_listener(Arc::new(Mutex::new(RdevListener::default())))
    }

    fn listener_state(system: &RdevHotkeySystem) -> std::sync::MutexGuard<'_, RdevListener> {
        let Ok(state) = system.listener.lock() else {
            panic!("test listener lock was poisoned");
        };
        state
    }

    fn press(system: &RdevHotkeySystem, key: Key, exclusive: bool) -> bool {
        listener_state(system).handle_event(EventType::KeyPress(key), exclusive)
    }

    fn release(system: &RdevHotkeySystem, key: Key, exclusive: bool) -> bool {
        listener_state(system).handle_event(EventType::KeyRelease(key), exclusive)
    }

    #[test]
    fn accepts_only_canonical_hotkey_tokens() {
        for hotkey in [
            "alt+ctrl+left",
            "shift+return",
            "a",
            "alt+ctrl+shift+cmd+f12",
        ] {
            assert_eq!(canonical_binding(hotkey).unwrap().1, hotkey);
        }
        for hotkey in [
            "ctrl+alt+left",
            " shift + Return ",
            "Alt+",
            "  ",
            "ctrl+ctrl+a",
        ] {
            assert!(
                matches!(
                    canonical_binding(hotkey),
                    Err(HotkeySystemError::Rejected(_))
                ),
                "{hotkey}"
            );
        }
    }

    #[test]
    fn rejects_unsupported_keys_and_alias_tokens() {
        for hotkey in [
            "ctrl+unknown",
            "ctrl+esc",
            "ctrl+enter",
            "ctrl+page_up",
            "option+a",
            "super+a",
            "ctrl+left_bracket",
            "ctrl+print",
            "ctrl+f01",
        ] {
            assert!(
                matches!(
                    canonical_binding(hotkey),
                    Err(HotkeySystemError::Rejected(_))
                ),
                "{hotkey}"
            );
        }
    }

    #[test]
    fn refusal_keeps_the_complete_previous_set_active() {
        let mut system = system_without_native_listener();
        system.register_hotkeys(&["ctrl+a".to_string()]).unwrap();
        assert!(matches!(
            system.register_hotkeys(&["ctrl+b".to_string(), "ctrl+bad".to_string()]),
            Err(HotkeySystemError::Rejected(_))
        ));
        press(&system, Key::ControlLeft, true);
        assert!(!press(&system, Key::KeyB, true));
        assert!(press(&system, Key::KeyA, true));
        assert_eq!(
            system.next_hotkey().unwrap(),
            Some(HotkeyEvent::Pressed {
                hotkey: "ctrl+a".to_string()
            })
        );
        assert_eq!(system.next_hotkey().unwrap(), None);
    }

    #[test]
    fn exclusive_combo_consumes_repeat_and_release_even_after_modifiers_and_registration_change() {
        let mut system = system_without_native_listener();
        system.register_hotkeys(&["ctrl+a".to_string()]).unwrap();
        assert!(!press(&system, Key::ControlLeft, true));
        assert!(press(&system, Key::KeyA, true));
        assert!(press(&system, Key::KeyA, true));
        assert!(!press(&system, Key::KeyB, true));
        assert!(!release(&system, Key::ControlLeft, true));
        assert_eq!(
            system.next_hotkey().unwrap(),
            Some(HotkeyEvent::Pressed {
                hotkey: "ctrl+a".to_string()
            })
        );
        assert_eq!(system.next_hotkey().unwrap(), None);
        system.register_hotkeys(&[]).unwrap();
        assert!(release(&system, Key::KeyA, true));
        assert!(!release(&system, Key::KeyB, true));
        assert!(!press(&system, Key::KeyA, true));
    }

    #[test]
    fn both_modifier_sides_are_tracked_independently() {
        let mut system = system_without_native_listener();
        system.register_hotkeys(&["ctrl+a".to_string()]).unwrap();
        press(&system, Key::ControlLeft, true);
        press(&system, Key::ControlRight, true);
        release(&system, Key::ControlLeft, true);
        assert!(press(&system, Key::KeyA, true));
    }

    #[test]
    fn linux_style_listener_delivers_without_consuming() {
        let mut system = system_without_native_listener();
        system.register_hotkeys(&["a".to_string()]).unwrap();
        assert!(!press(&system, Key::KeyA, false));
        assert!(!release(&system, Key::KeyA, false));
        assert_eq!(
            system.next_hotkey().unwrap(),
            Some(HotkeyEvent::Pressed {
                hotkey: "a".to_string()
            })
        );
    }

    #[test]
    fn shared_registration_and_drop_do_not_leave_duplicate_delivery() {
        let mut first = system_without_native_listener();
        let mut second = RdevHotkeySystem::with_listener(Arc::clone(&first.listener));
        first.register_hotkeys(&["a".to_string()]).unwrap();
        press(&first, Key::KeyA, true);
        second.register_hotkeys(&["b".to_string()]).unwrap();
        assert_eq!(first.next_hotkey().unwrap(), None);
        drop(first);
        assert!(press(&second, Key::KeyB, true));
        assert_eq!(
            second.next_hotkey().unwrap(),
            Some(HotkeyEvent::Pressed {
                hotkey: "b".to_string()
            })
        );
        let listener = Arc::clone(&second.listener);
        drop(second);
        let Ok(mut state) = listener.lock() else {
            panic!("test listener lock was poisoned");
        };
        assert!(state.bindings.is_empty());
        assert!(state.event_tx.is_none());
        assert!(state.handle_event(EventType::KeyRelease(Key::KeyB), true));
        assert!(!state.handle_event(EventType::KeyPress(Key::KeyB), true));
    }

    #[test]
    fn listener_failures_are_unavailable_for_registration_and_reads() {
        let mut system = system_without_native_listener();
        listener_state(&system).error = Some("EventTapError".to_string());
        assert_eq!(
            system.register_hotkeys(&["ctrl+a".to_string()]),
            Err(HotkeySystemError::Unavailable("EventTapError".to_string()))
        );
        assert_eq!(
            system.next_hotkey(),
            Err(HotkeySystemError::Unavailable("EventTapError".to_string()))
        );
    }

    #[test]
    fn empty_replacement_releases_the_previous_set_and_discards_queued_events() {
        let mut system = system_without_native_listener();
        system.register_hotkeys(&["a".to_string()]).unwrap();
        press(&system, Key::KeyA, true);
        system.register_hotkeys(&[]).unwrap();
        assert_eq!(system.next_hotkey().unwrap(), None);
        assert!(release(&system, Key::KeyA, true));
        assert!(!press(&system, Key::KeyA, true));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_function_keys_above_f12_use_native_virtual_key_codes() {
        assert_eq!(
            canonical_binding("ctrl+f13").unwrap().0,
            (MOD_CTRL, Key::Unknown(124))
        );
        assert_eq!(canonical_binding("f24").unwrap().0, (0, Key::Unknown(135)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_keys_missing_from_rdev_enum_use_native_key_codes() {
        assert_eq!(canonical_binding("f20").unwrap().0, (0, Key::Unknown(90)));
        assert_eq!(
            canonical_binding("delete").unwrap().0,
            (0, Key::Unknown(117))
        );
        assert!(matches!(
            canonical_binding("f24"),
            Err(HotkeySystemError::Rejected(_))
        ));
        assert!(matches!(
            canonical_binding("printscreen"),
            Err(HotkeySystemError::Rejected(_))
        ));
    }
}
