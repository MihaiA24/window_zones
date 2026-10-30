use thiserror::Error;

#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::collections::{HashMap, HashSet};
#[cfg(target_os = "macos")]
use std::sync::PoisonError;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::sync::mpsc::{self, TryRecvError};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::sync::{Arc, Mutex};
#[cfg(target_os = "macos")]
use std::thread;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

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
/// an implementation releases its registration. A restarted runtime must never
/// leave a second listener behind.
pub trait HotkeySystem {
    fn register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), HotkeySystemError>;

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError>;
}

/// Native keyboard capture for Windows/macOS using platform key codes.
/// Windows owns and joins its hook thread on replacement/drop; macOS reuses
/// its event tap while it remains live. Registration waits for native readiness.
#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Debug)]
pub struct NativeHotkeySystem {
    listener: Arc<Mutex<NativeListenerState>>,
    owner: Arc<()>,
    event_tx: mpsc::Sender<HotkeyEvent>,
    events: mpsc::Receiver<HotkeyEvent>,
    #[cfg(target_os = "windows")]
    native_listener: Option<Arc<windows::NativeListener>>,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyEvent {
    KeyPress(u32),
    KeyRelease(u32),
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
#[derive(Debug, Default)]
struct NativeListenerState {
    bindings: HashMap<(u8, u32), String>,
    owner: Option<Arc<()>>,
    event_tx: Option<mpsc::Sender<HotkeyEvent>>,
    error: Option<String>,
    pressed: HashSet<u32>,
    swallowed: HashSet<u32>,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
impl NativeListenerState {
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

    /// Record that the native hook is not running; `new` may then replace it.
    fn stop(&mut self, error: String) {
        self.clear_registration();
        self.error = Some(error);
    }

    /// Return true only when the native event should be consumed.
    fn handle_event(&mut self, event: KeyEvent) -> bool {
        match event {
            KeyEvent::KeyPress(key) => {
                if !self.pressed.insert(key) {
                    return self.swallowed.contains(&key);
                }
                if modifier_mask(&key).is_some() {
                    return false;
                }
                #[cfg(target_os = "windows")]
                if self.pressed.contains(&0xa5) && self.pressed.contains(&0xa2) {
                    // AltGr synthesizes left Ctrl + right Alt on Windows.
                    // Reserve that chord for typing, not Ctrl+Alt bindings.
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
                self.swallowed.insert(key);
                true
            }
            KeyEvent::KeyRelease(key) => {
                self.pressed.remove(&key);
                self.swallowed.remove(&key)
            }
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
impl NativeHotkeySystem {
    pub fn new() -> Self {
        #[cfg(target_os = "windows")]
        {
            let native = windows::new_listener();
            let mut system = Self::with_listener(Arc::clone(&native.state));
            system.native_listener = Some(native);
            system
        }
        #[cfg(target_os = "macos")]
        {
            static LISTENER: Mutex<Option<Arc<Mutex<NativeListenerState>>>> = Mutex::new(None);
            Self::with_listener(Self::shared_listener(&LISTENER, Self::spawn_listener))
        }
    }

    #[cfg(target_os = "macos")]
    /// Reuse the shared listener while its native hook may still run; build a
    /// new one only after the old hook stopped, so a single hook stays live.
    fn shared_listener(
        slot: &Mutex<Option<Arc<Mutex<NativeListenerState>>>>,
        spawn: impl FnOnce(Arc<Mutex<NativeListenerState>>),
    ) -> Arc<Mutex<NativeListenerState>> {
        let mut slot = slot.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(listener) = slot.as_ref() {
            let running = listener
                .lock()
                .unwrap_or_else(|poisoned| {
                    // A wrapper panicked mid-update: keep the hook, drop its registration.
                    listener.clear_poison();
                    let mut state = poisoned.into_inner();
                    state.clear_registration();
                    state
                })
                .error
                .is_none();
            if running {
                return Arc::clone(listener);
            }
        }
        let listener = Arc::new(Mutex::new(NativeListenerState::default()));
        spawn(Arc::clone(&listener));
        *slot = Some(Arc::clone(&listener));
        listener
    }

    fn with_listener(listener: Arc<Mutex<NativeListenerState>>) -> Self {
        let (event_tx, events) = mpsc::channel();
        Self {
            listener,
            owner: Arc::new(()),
            event_tx,
            events,
            #[cfg(target_os = "windows")]
            native_listener: None,
        }
    }

    #[cfg(target_os = "macos")]
    fn spawn_listener(listener: Arc<Mutex<NativeListenerState>>) {
        let error_state = Arc::clone(&listener);
        let spawn_error_state = Arc::clone(&listener);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let spawn = thread::Builder::new()
            .name("window-zones-hotkeys".to_string())
            .spawn(move || {
                let result = std::panic::catch_unwind(move || macos::listen(listener, ready_tx));
                let details = match result {
                    Ok(Err(error)) => error,
                    Ok(Ok(())) => "native listener exited".to_string(),
                    Err(_) => "native listener panicked".to_string(),
                };
                error_state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .stop(format!(
                        "global hotkey listener stopped: {details}; restore Accessibility permission and restart Window Zones"
                    ));
            });
        match spawn {
            Ok(worker) => {
                // A disconnected readiness channel means initialization failed;
                // joining publishes its error before registration can succeed.
                if ready_rx.recv().is_err() {
                    let _ = worker.join();
                }
            }
            Err(error) => spawn_error_state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .stop(format!(
                    "failed to start global hotkey listener: {error}; restart Window Zones"
                )),
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
impl Default for NativeHotkeySystem {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
impl HotkeySystem for NativeHotkeySystem {
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

#[cfg(any(target_os = "windows", target_os = "macos"))]
impl Drop for NativeHotkeySystem {
    fn drop(&mut self) {
        if let Ok(mut state) = self.listener.lock()
            && state
                .owner
                .as_ref()
                .is_some_and(|owner| Arc::ptr_eq(owner, &self.owner))
        {
            state.clear_registration();
        }
        #[cfg(target_os = "windows")]
        if let Some(native) = &self.native_listener {
            let _ = native.stop();
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
const MOD_ALT: u8 = 0b0001;
#[cfg(any(target_os = "windows", target_os = "macos"))]
const MOD_CTRL: u8 = 0b0010;
#[cfg(any(target_os = "windows", target_os = "macos"))]
const MOD_SHIFT: u8 = 0b0100;
#[cfg(any(target_os = "windows", target_os = "macos"))]
const MOD_CMD: u8 = 0b1000;

#[cfg(target_os = "windows")]
fn modifier_mask(key: &u32) -> Option<u8> {
    Some(match key {
        0xa4 | 0xa5 => MOD_ALT,
        0xa2 | 0xa3 => MOD_CTRL,
        0xa0 | 0xa1 => MOD_SHIFT,
        0x5b | 0x5c => MOD_CMD,
        _ => return None,
    })
}

#[cfg(target_os = "macos")]
fn modifier_mask(key: &u32) -> Option<u8> {
    Some(match key {
        58 | 61 => MOD_ALT,
        59 | 62 => MOD_CTRL,
        56 | 60 => MOD_SHIFT,
        55 | 54 => MOD_CMD,
        _ => return None,
    })
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn canonical_binding(raw: &str) -> Result<((u8, u32), String), HotkeySystemError> {
    let canonical = crate::config::normalize_hotkey(raw)
        .map_err(|error| HotkeySystemError::Rejected(format!("invalid hotkey `{raw}`: {error}")))?;
    if canonical != raw {
        return Err(HotkeySystemError::Rejected(format!(
            "hotkey `{raw}` is not canonical; use `{canonical}`"
        )));
    }
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
    Ok(((modifiers, key), canonical))
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn key_token_to_code(token: &str) -> Option<u32> {
    // Win32 virtual keys and Carbon HIToolbox virtual keycodes, respectively.
    let codes: (u32, Option<u32>) = match token {
        "a" => (0x41, Some(0)),
        "b" => (0x42, Some(11)),
        "c" => (0x43, Some(8)),
        "d" => (0x44, Some(2)),
        "e" => (0x45, Some(14)),
        "f" => (0x46, Some(3)),
        "g" => (0x47, Some(5)),
        "h" => (0x48, Some(4)),
        "i" => (0x49, Some(34)),
        "j" => (0x4a, Some(38)),
        "k" => (0x4b, Some(40)),
        "l" => (0x4c, Some(37)),
        "m" => (0x4d, Some(46)),
        "n" => (0x4e, Some(45)),
        "o" => (0x4f, Some(31)),
        "p" => (0x50, Some(35)),
        "q" => (0x51, Some(12)),
        "r" => (0x52, Some(15)),
        "s" => (0x53, Some(1)),
        "t" => (0x54, Some(17)),
        "u" => (0x55, Some(32)),
        "v" => (0x56, Some(9)),
        "w" => (0x57, Some(13)),
        "x" => (0x58, Some(7)),
        "y" => (0x59, Some(16)),
        "z" => (0x5a, Some(6)),
        "0" => (0x30, Some(29)),
        "1" => (0x31, Some(18)),
        "2" => (0x32, Some(19)),
        "3" => (0x33, Some(20)),
        "4" => (0x34, Some(21)),
        "5" => (0x35, Some(23)),
        "6" => (0x36, Some(22)),
        "7" => (0x37, Some(26)),
        "8" => (0x38, Some(28)),
        "9" => (0x39, Some(25)),
        "f1" => (0x70, Some(122)),
        "f2" => (0x71, Some(120)),
        "f3" => (0x72, Some(99)),
        "f4" => (0x73, Some(118)),
        "f5" => (0x74, Some(96)),
        "f6" => (0x75, Some(97)),
        "f7" => (0x76, Some(98)),
        "f8" => (0x77, Some(100)),
        "f9" => (0x78, Some(101)),
        "f10" => (0x79, Some(109)),
        "f11" => (0x7a, Some(103)),
        "f12" => (0x7b, Some(111)),
        "f13" => (0x7c, Some(105)),
        "f14" => (0x7d, Some(107)),
        "f15" => (0x7e, Some(113)),
        "f16" => (0x7f, Some(106)),
        "f17" => (0x80, Some(64)),
        "f18" => (0x81, Some(79)),
        "f19" => (0x82, Some(80)),
        "f20" => (0x83, Some(90)),
        "f21" => (0x84, None),
        "f22" => (0x85, None),
        "f23" => (0x86, None),
        "f24" => (0x87, None),
        "space" => (0x20, Some(49)),
        "tab" => (0x09, Some(48)),
        "backspace" => (0x08, Some(51)),
        "escape" => (0x1b, Some(53)),
        "insert" => (0x2d, Some(114)),
        "home" => (0x24, Some(115)),
        "end" => (0x23, Some(119)),
        "pageup" => (0x21, Some(116)),
        "pagedown" => (0x22, Some(121)),
        "delete" => (0x2e, Some(117)),
        "left" => (0x25, Some(123)),
        "right" => (0x27, Some(124)),
        "up" => (0x26, Some(126)),
        "down" => (0x28, Some(125)),
        "minus" => (0xbd, Some(27)),
        "equal" => (0xbb, Some(24)),
        "comma" => (0xbc, Some(43)),
        "dot" => (0xbe, Some(47)),
        "slash" => (0xbf, Some(44)),
        "quote" => (0xde, Some(39)),
        "semicolon" => (0xba, Some(41)),
        "leftbracket" => (0xdb, Some(33)),
        "rightbracket" => (0xdd, Some(30)),
        "backslash" => (0xdc, Some(42)),
        "backquote" => (0xc0, Some(50)),
        "return" => (0x0d, Some(36)),
        "printscreen" => (0x2c, None),
        "scrolllock" => (0x91, None),
        "pause" => (0x13, None),
        "capslock" => (0x14, Some(57)),
        "numlock" => (0x90, None),
        _ => return None,
    };
    #[cfg(target_os = "windows")]
    {
        Some(codes.0)
    }
    #[cfg(target_os = "macos")]
    {
        codes.1
    }
}

#[cfg(all(test, any(target_os = "windows", target_os = "macos")))]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    const A: u32 = 0x41;
    #[cfg(target_os = "macos")]
    const A: u32 = 0;
    #[cfg(target_os = "windows")]
    const B: u32 = 0x42;
    #[cfg(target_os = "macos")]
    const B: u32 = 11;
    #[cfg(target_os = "windows")]
    const CONTROL_LEFT: u32 = 0xa2;
    #[cfg(target_os = "macos")]
    const CONTROL_LEFT: u32 = 59;
    #[cfg(target_os = "windows")]
    const CONTROL_RIGHT: u32 = 0xa3;
    #[cfg(target_os = "macos")]
    const CONTROL_RIGHT: u32 = 62;

    fn system_without_native_listener() -> NativeHotkeySystem {
        NativeHotkeySystem::with_listener(Arc::new(Mutex::new(NativeListenerState::default())))
    }

    fn listener_state(
        system: &NativeHotkeySystem,
    ) -> std::sync::MutexGuard<'_, NativeListenerState> {
        let Ok(state) = system.listener.lock() else {
            panic!("test listener lock was poisoned");
        };
        state
    }

    fn press(system: &NativeHotkeySystem, key: u32) -> bool {
        listener_state(system).handle_event(KeyEvent::KeyPress(key))
    }

    fn release(system: &NativeHotkeySystem, key: u32) -> bool {
        listener_state(system).handle_event(KeyEvent::KeyRelease(key))
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
    fn maps_every_letter_digit_and_function_key() {
        for (token, _windows, _macos) in [
            ("a", 65, Some(0)),
            ("b", 66, Some(11)),
            ("c", 67, Some(8)),
            ("d", 68, Some(2)),
            ("e", 69, Some(14)),
            ("f", 70, Some(3)),
            ("g", 71, Some(5)),
            ("h", 72, Some(4)),
            ("i", 73, Some(34)),
            ("j", 74, Some(38)),
            ("k", 75, Some(40)),
            ("l", 76, Some(37)),
            ("m", 77, Some(46)),
            ("n", 78, Some(45)),
            ("o", 79, Some(31)),
            ("p", 80, Some(35)),
            ("q", 81, Some(12)),
            ("r", 82, Some(15)),
            ("s", 83, Some(1)),
            ("t", 84, Some(17)),
            ("u", 85, Some(32)),
            ("v", 86, Some(9)),
            ("w", 87, Some(13)),
            ("x", 88, Some(7)),
            ("y", 89, Some(16)),
            ("z", 90, Some(6)),
            ("0", 48, Some(29)),
            ("1", 49, Some(18)),
            ("2", 50, Some(19)),
            ("3", 51, Some(20)),
            ("4", 52, Some(21)),
            ("5", 53, Some(23)),
            ("6", 54, Some(22)),
            ("7", 55, Some(26)),
            ("8", 56, Some(28)),
            ("9", 57, Some(25)),
            ("f1", 112, Some(122)),
            ("f2", 113, Some(120)),
            ("f3", 114, Some(99)),
            ("f4", 115, Some(118)),
            ("f5", 116, Some(96)),
            ("f6", 117, Some(97)),
            ("f7", 118, Some(98)),
            ("f8", 119, Some(100)),
            ("f9", 120, Some(101)),
            ("f10", 121, Some(109)),
            ("f11", 122, Some(103)),
            ("f12", 123, Some(111)),
            ("f13", 124, Some(105)),
            ("f14", 125, Some(107)),
            ("f15", 126, Some(113)),
            ("f16", 127, Some(106)),
            ("f17", 128, Some(64)),
            ("f18", 129, Some(79)),
            ("f19", 130, Some(80)),
            ("f20", 131, Some(90)),
            ("f21", 132, None),
            ("f22", 133, None),
            ("f23", 134, None),
            ("f24", 135, None),
        ] {
            #[cfg(target_os = "windows")]
            let expected = Some(_windows);
            #[cfg(target_os = "macos")]
            let expected = _macos;
            assert_eq!(key_token_to_code(token), expected, "{token}");
            if let Some(code) = expected {
                assert_eq!(canonical_binding(token).unwrap().0, (0, code), "{token}");
            } else {
                assert!(
                    matches!(
                        canonical_binding(token),
                        Err(HotkeySystemError::Rejected(_))
                    ),
                    "{token}"
                );
            }
        }
        for token in ["", "A", "aa", "00", "f0", "f01", "f25"] {
            assert_eq!(key_token_to_code(token), None, "{token}");
        }
    }

    #[test]
    fn maps_named_keys_to_native_codes() {
        for (token, _windows, _macos) in [
            ("space", 32, Some(49)),
            ("tab", 9, Some(48)),
            ("backspace", 8, Some(51)),
            ("escape", 27, Some(53)),
            ("insert", 45, Some(114)),
            ("home", 36, Some(115)),
            ("end", 35, Some(119)),
            ("pageup", 33, Some(116)),
            ("pagedown", 34, Some(121)),
            ("delete", 46, Some(117)),
            ("left", 37, Some(123)),
            ("right", 39, Some(124)),
            ("up", 38, Some(126)),
            ("down", 40, Some(125)),
            ("minus", 189, Some(27)),
            ("equal", 187, Some(24)),
            ("comma", 188, Some(43)),
            ("dot", 190, Some(47)),
            ("slash", 191, Some(44)),
            ("quote", 222, Some(39)),
            ("semicolon", 186, Some(41)),
            ("leftbracket", 219, Some(33)),
            ("rightbracket", 221, Some(30)),
            ("backslash", 220, Some(42)),
            ("backquote", 192, Some(50)),
            ("return", 13, Some(36)),
            ("printscreen", 44, None),
            ("scrolllock", 145, None),
            ("pause", 19, None),
            ("capslock", 20, Some(57)),
            ("numlock", 144, None),
        ] {
            #[cfg(target_os = "windows")]
            let expected = Some(_windows);
            #[cfg(target_os = "macos")]
            let expected = _macos;
            assert_eq!(key_token_to_code(token), expected, "{token}");
            if let Some(code) = expected {
                assert_eq!(canonical_binding(token).unwrap().0, (0, code), "{token}");
            } else {
                assert!(
                    matches!(
                        canonical_binding(token),
                        Err(HotkeySystemError::Rejected(_))
                    ),
                    "{token}"
                );
            }
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
        press(&system, CONTROL_LEFT);
        assert!(!press(&system, B));
        assert!(press(&system, A));
        assert_eq!(
            system.next_hotkey().unwrap(),
            Some(HotkeyEvent::Pressed {
                hotkey: "ctrl+a".to_string()
            })
        );
        assert_eq!(system.next_hotkey().unwrap(), None);
    }

    #[test]
    fn replacing_bindings_keeps_already_queued_presses() {
        let mut system = system_without_native_listener();
        system.register_hotkeys(&["a".to_string()]).unwrap();
        assert!(press(&system, A));
        release(&system, A);
        system.register_hotkeys(&["b".to_string()]).unwrap();
        assert!(press(&system, B));
        for key in ["a", "b"] {
            assert_eq!(
                system.next_hotkey().unwrap(),
                Some(HotkeyEvent::Pressed {
                    hotkey: key.to_string()
                })
            );
        }
        assert_eq!(system.next_hotkey().unwrap(), None);
    }

    #[test]
    fn exclusive_combo_consumes_repeat_and_release_even_after_modifiers_and_registration_change() {
        let mut system = system_without_native_listener();
        system.register_hotkeys(&["ctrl+a".to_string()]).unwrap();
        assert!(!press(&system, CONTROL_LEFT));
        assert!(press(&system, A));
        assert!(press(&system, A));
        assert!(!press(&system, B));
        assert!(!release(&system, CONTROL_LEFT));
        assert_eq!(
            system.next_hotkey().unwrap(),
            Some(HotkeyEvent::Pressed {
                hotkey: "ctrl+a".to_string()
            })
        );
        assert_eq!(system.next_hotkey().unwrap(), None);
        system.register_hotkeys(&[]).unwrap();
        assert!(release(&system, A));
        assert!(!release(&system, B));
        assert!(!press(&system, A));
    }

    #[test]
    fn both_modifier_sides_are_tracked_independently() {
        let mut system = system_without_native_listener();
        system.register_hotkeys(&["ctrl+a".to_string()]).unwrap();
        press(&system, CONTROL_LEFT);
        press(&system, CONTROL_RIGHT);
        release(&system, CONTROL_LEFT);
        assert!(press(&system, A));
    }

    #[test]
    fn shared_registration_and_drop_do_not_leave_duplicate_delivery() {
        let mut first = system_without_native_listener();
        let mut second = NativeHotkeySystem::with_listener(Arc::clone(&first.listener));
        first.register_hotkeys(&["a".to_string()]).unwrap();
        press(&first, A);
        second.register_hotkeys(&["b".to_string()]).unwrap();
        assert_eq!(first.next_hotkey().unwrap(), None);
        drop(first);
        assert!(press(&second, B));
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
        assert!(state.handle_event(KeyEvent::KeyRelease(B)));
        assert!(!state.handle_event(KeyEvent::KeyPress(B)));
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

    #[cfg(target_os = "macos")]
    #[test]
    fn restart_reuses_a_live_listener_and_rebuilds_a_stopped_one() {
        let slot = Mutex::new(None);
        let mut spawned = 0;
        let mut shared = || {
            NativeHotkeySystem::with_listener(NativeHotkeySystem::shared_listener(&slot, |_| {
                spawned += 1
            }))
        };
        let first = shared();
        let mut second = shared();
        second.register_hotkeys(&["a".to_string()]).unwrap();
        assert!(press(&second, A));
        listener_state(&second).stop("EventTapError".to_string());
        drop(first);
        let mut restarted = shared();
        let _reused = shared();
        assert_eq!(spawned, 2);

        restarted.register_hotkeys(&["a".to_string()]).unwrap();
        assert!(!release(&restarted, A));
        assert!(press(&restarted, A));
        assert_eq!(
            restarted.next_hotkey().unwrap(),
            Some(HotkeyEvent::Pressed {
                hotkey: "a".to_string()
            })
        );
        assert!(matches!(
            second.register_hotkeys(&["a".to_string()]),
            Err(HotkeySystemError::Unavailable(_))
        ));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_function_keys_above_f12_use_native_virtual_key_codes() {
        assert_eq!(canonical_binding("ctrl+f13").unwrap().0, (MOD_CTRL, 124));
        assert_eq!(canonical_binding("f24").unwrap().0, (0, 135));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_extended_keys_use_native_key_codes() {
        assert_eq!(canonical_binding("f20").unwrap().0, (0, 90));
        assert_eq!(canonical_binding("delete").unwrap().0, (0, 117));
        assert!(matches!(
            canonical_binding("f24"),
            Err(HotkeySystemError::Rejected(_))
        ));
        assert!(matches!(
            canonical_binding("printscreen"),
            Err(HotkeySystemError::Rejected(_))
        ));
    }

    #[test]
    fn preserves_supported_key_boundaries_and_aliases() {
        for (raw, expected) in [
            ("A", "a"),
            ("Z", "z"),
            ("0", "0"),
            ("9", "9"),
            ("F1", "f1"),
            ("F12", "f12"),
            ("Esc", "escape"),
            ("Page_Up", "pageup"),
            ("Left_Arrow", "left"),
            ("RightBracket", "rightbracket"),
            ("PrintScreen", "printscreen"),
        ] {
            assert_eq!(
                crate::config::normalize_hotkey(raw).unwrap(),
                expected,
                "{raw}"
            );
        }
        for raw in [
            "F0",
            "F256",
            "F0013",
            "é",
            "ß",
            "９",
            "ab",
            "print_screen",
            "caps_lock",
        ] {
            assert!(crate::config::normalize_hotkey(raw).is_err(), "{raw}");
        }
    }
}
