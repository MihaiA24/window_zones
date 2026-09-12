use thiserror::Error;

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
use std::collections::HashSet;
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
use std::sync::mpsc::{self, TryRecvError};
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
use std::sync::{Arc, Mutex};
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
use std::thread;

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
use rdev::{Event, EventType, Key, listen};

/// Event generated when a configured global hotkey is activated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyEvent {
    Pressed { hotkey: String },
}

/// Errors produced by platform hotkey adapters.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HotkeySystemError {
    #[error("platform hotkey error: {0}")]
    Platform(String),
}

/// Platform adapter contract for global hotkey registration and dispatch.
pub trait HotkeySystem {
    fn register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), HotkeySystemError>;

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError>;
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
#[derive(Debug)]
pub struct RdevHotkeySystem {
    registered_hotkeys: Arc<Mutex<HashSet<String>>>,
    events: mpsc::Receiver<HotkeyEvent>,
    listener_error: Arc<Mutex<Option<String>>>,
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
impl RdevHotkeySystem {
    pub fn new() -> Self {
        let registered_hotkeys = Arc::new(Mutex::new(HashSet::new()));
        let listener_error = Arc::new(Mutex::new(None));
        let (event_tx, event_rx) = mpsc::channel();

        Self::spawn_listener(
            Arc::clone(&registered_hotkeys),
            event_tx,
            Arc::clone(&listener_error),
        );

        Self {
            registered_hotkeys,
            events: event_rx,
            listener_error,
        }
    }

    fn spawn_listener(
        registered_hotkeys: Arc<Mutex<HashSet<String>>>,
        event_tx: mpsc::Sender<HotkeyEvent>,
        listener_error: Arc<Mutex<Option<String>>>,
    ) {
        thread::spawn(move || {
            let mut pressed_modifiers: u8 = 0;
            let mut pressed_keys = HashSet::new();
            let callback = move |event: Event| match event.event_type {
                EventType::KeyPress(key) => {
                    if let Some(mask) = modifier_mask(&key) {
                        pressed_modifiers |= mask;
                        return;
                    }
                    if !pressed_keys.insert(key) {
                        return;
                    }

                    let Some(bindings) = registered_hotkeys.lock().ok() else {
                        return;
                    };
                    let Some(key_token) = key_to_token(&key) else {
                        return;
                    };
                    let Some(combo) = build_binding_string(pressed_modifiers, key_token) else {
                        return;
                    };

                    if bindings.contains(&combo) {
                        let _ = event_tx.send(HotkeyEvent::Pressed { hotkey: combo });
                    }
                }
                EventType::KeyRelease(key) => {
                    if let Some(mask) = modifier_mask(&key) {
                        pressed_modifiers &= !mask;
                    }
                    pressed_keys.remove(&key);
                }
                _ => {}
            };

            // Preserve the native failure, rather than treating a dead listener as idle.
            if let Err(error) = listen(callback)
                && let Ok(mut state) = listener_error.lock()
            {
                *state = Some(format!("{error:?}"));
            }
        });
    }

    fn listener_status(&self) -> Result<(), HotkeySystemError> {
        let error = self.listener_error.lock().map_err(|_| {
            HotkeySystemError::Platform("failed to read hotkey listener state".to_string())
        })?;
        match error.as_ref() {
            Some(error) => Err(HotkeySystemError::Platform(error.clone())),
            None => Ok(()),
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
        self.listener_status()?;
        let mut next = HashSet::new();

        for raw_hotkey in hotkeys {
            next.insert(normalized_hotkey(raw_hotkey)?);
        }

        let mut bindings = self.registered_hotkeys.lock().map_err(|_| {
            HotkeySystemError::Platform("failed to update hotkey state".to_string())
        })?;
        *bindings = next;

        self.listener_status()
    }

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
        self.listener_status()?;
        match self.events.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(HotkeySystemError::Platform(
                "global hotkey listener is unavailable".to_string(),
            )),
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
fn build_binding_string(modifiers: u8, key: &str) -> Option<String> {
    let mut output = String::new();

    if modifiers & MOD_ALT != 0 {
        output.push_str("alt+");
    }
    if modifiers & MOD_CTRL != 0 {
        output.push_str("ctrl+");
    }
    if modifiers & MOD_SHIFT != 0 {
        output.push_str("shift+");
    }
    if modifiers & MOD_CMD != 0 {
        output.push_str("cmd+");
    }

    if key.is_empty() {
        None
    } else {
        output.push_str(key);
        Some(output)
    }
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
fn normalized_hotkey(raw: &str) -> Result<String, HotkeySystemError> {
    let mut modifiers = 0;
    let mut key_token: Option<String> = None;

    for token in raw.split('+') {
        let canonical = canonical_token(token)?;
        match canonical.as_str() {
            "alt" => modifiers |= MOD_ALT,
            "ctrl" => modifiers |= MOD_CTRL,
            "shift" => modifiers |= MOD_SHIFT,
            "cmd" => modifiers |= MOD_CMD,
            _ => {
                if key_token.is_some() {
                    return Err(HotkeySystemError::Platform(format!(
                        "invalid hotkey {raw}: expected exactly one non-modifier key"
                    )));
                }
                if key_token_to_code(&canonical).is_none() {
                    return Err(HotkeySystemError::Platform(format!(
                        "unsupported hotkey key `{canonical}` in `{raw}`"
                    )));
                }
                key_token = Some(canonical);
            }
        }
    }

    let key_token = key_token.ok_or_else(|| {
        HotkeySystemError::Platform(format!("invalid hotkey {raw}: missing hotkey key"))
    })?;

    build_binding_string(modifiers, &key_token).ok_or_else(|| {
        HotkeySystemError::Platform(format!("invalid hotkey {raw}: key token empty"))
    })
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
fn canonical_token(raw: &str) -> Result<String, HotkeySystemError> {
    let token = raw.trim().to_ascii_lowercase();
    if token.is_empty() {
        return Err(HotkeySystemError::Platform(
            "invalid hotkey: empty token".to_string(),
        ));
    }

    Ok(token)
}

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
fn key_token_to_code(token: &str) -> Option<Key> {
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

#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
fn key_to_token(key: &Key) -> Option<&'static str> {
    Some(match key {
        Key::KeyA => "a",
        Key::KeyB => "b",
        Key::KeyC => "c",
        Key::KeyD => "d",
        Key::KeyE => "e",
        Key::KeyF => "f",
        Key::KeyG => "g",
        Key::KeyH => "h",
        Key::KeyI => "i",
        Key::KeyJ => "j",
        Key::KeyK => "k",
        Key::KeyL => "l",
        Key::KeyM => "m",
        Key::KeyN => "n",
        Key::KeyO => "o",
        Key::KeyP => "p",
        Key::KeyQ => "q",
        Key::KeyR => "r",
        Key::KeyS => "s",
        Key::KeyT => "t",
        Key::KeyU => "u",
        Key::KeyV => "v",
        Key::KeyW => "w",
        Key::KeyX => "x",
        Key::KeyY => "y",
        Key::KeyZ => "z",
        Key::Num0 => "0",
        Key::Num1 => "1",
        Key::Num2 => "2",
        Key::Num3 => "3",
        Key::Num4 => "4",
        Key::Num5 => "5",
        Key::Num6 => "6",
        Key::Num7 => "7",
        Key::Num8 => "8",
        Key::Num9 => "9",
        Key::F1 => "f1",
        Key::F2 => "f2",
        Key::F3 => "f3",
        Key::F4 => "f4",
        Key::F5 => "f5",
        Key::F6 => "f6",
        Key::F7 => "f7",
        Key::F8 => "f8",
        Key::F9 => "f9",
        Key::F10 => "f10",
        Key::F11 => "f11",
        Key::F12 => "f12",
        Key::Backspace => "backspace",
        Key::Tab => "tab",
        Key::Return => "return",
        Key::Escape => "escape",
        Key::Space => "space",
        Key::Insert => "insert",
        Key::Delete => "delete",
        Key::Home => "home",
        Key::End => "end",
        Key::PageUp => "pageup",
        Key::PageDown => "pagedown",
        Key::LeftArrow => "left",
        Key::RightArrow => "right",
        Key::UpArrow => "up",
        Key::DownArrow => "down",
        Key::Minus => "minus",
        Key::Equal => "equal",
        Key::Comma => "comma",
        Key::Dot => "dot",
        Key::Slash => "slash",
        Key::Quote => "quote",
        Key::SemiColon => "semicolon",
        Key::LeftBracket => "leftbracket",
        Key::RightBracket => "rightbracket",
        Key::BackSlash => "backslash",
        Key::BackQuote => "backquote",
        Key::Pause => "pause",
        Key::CapsLock => "capslock",
        Key::PrintScreen => "printscreen",
        Key::ScrollLock => "scrolllock",
        Key::NumLock => "numlock",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_canonical_hotkey_tokens() {
        assert_eq!(normalized_hotkey("ctrl+alt+left").unwrap(), "alt+ctrl+left");
        assert_eq!(
            normalized_hotkey(" shift + Return ").unwrap(),
            "shift+return"
        );
        assert!(normalized_hotkey("Alt+").is_err());
        assert!(normalized_hotkey("  ").is_err());
    }

    #[test]
    fn rejects_unsupported_keys() {
        assert!(normalized_hotkey("ctrl+unknown").is_err());
    }

    #[test]
    fn rejects_alias_tokens() {
        for hotkey in [
            "ctrl+esc",
            "ctrl+enter",
            "ctrl+page_up",
            "option+a",
            "super+a",
            "ctrl+left_bracket",
            "ctrl+print",
            "ctrl+f01",
        ] {
            assert!(normalized_hotkey(hotkey).is_err(), "{hotkey}");
        }
    }

    #[test]
    fn listener_failures_reject_registration_and_event_reads() {
        let (event_tx, events) = mpsc::channel();
        let mut system = RdevHotkeySystem {
            registered_hotkeys: Arc::new(Mutex::new(HashSet::new())),
            events,
            listener_error: Arc::new(Mutex::new(Some("EventTapError".to_string()))),
        };
        let expected = Err(HotkeySystemError::Platform("EventTapError".to_string()));
        assert_eq!(system.register_hotkeys(&["ctrl+a".to_string()]), expected);
        assert_eq!(
            system.next_hotkey(),
            Err(HotkeySystemError::Platform("EventTapError".to_string()))
        );

        system.listener_error = Arc::new(Mutex::new(None));
        drop(event_tx);
        assert!(system.next_hotkey().is_err());
    }
}
