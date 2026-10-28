//! One owned Win32 low-level keyboard hook, with an explicit ready/stop handshake.

use super::{EventType, Key, RdevListener, modifier_mask};
use ::windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use ::windows::Win32::System::LibraryLoader::GetModuleHandleW;
use ::windows::Win32::System::Threading::GetCurrentThreadId;
use ::windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use ::windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, HHOOK, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG, PM_NOREMOVE,
    PeekMessageW, PostQuitMessage, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use ::windows::core::PCWSTR;
use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread::{self, JoinHandle};

const MODIFIERS: [(u32, Key); 8] = [
    (0xa0, Key::ShiftLeft),
    (0xa1, Key::ShiftRight),
    (0xa2, Key::ControlLeft),
    (0xa3, Key::ControlRight),
    (0xa4, Key::Alt),
    (0xa5, Key::AltGr),
    (0x5b, Key::MetaLeft),
    (0x5c, Key::MetaRight),
];

struct Ready {
    id: u32,
    #[cfg(test)]
    hook: usize,
}

#[derive(Debug)]
struct Worker {
    id: u32,
    thread: JoinHandle<()>,
    #[cfg(test)]
    hook: usize,
}

#[derive(Debug)]
pub(super) struct NativeListener {
    pub(super) state: Arc<Mutex<RdevListener>>,
    worker: Mutex<Option<Worker>>,
}

impl NativeListener {
    fn start() -> Self {
        let state = Arc::new(Mutex::new(RdevListener::default()));
        let listener = Arc::clone(&state);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let spawn = thread::Builder::new()
            .name("window-zones-hotkeys".to_string())
            .spawn(move || {
                let result =
                    catch_unwind(AssertUnwindSafe(|| run(Arc::clone(&listener), ready_tx)));
                let details = match result {
                    Ok(Ok(())) => "native listener exited".to_string(),
                    Ok(Err(error)) => error,
                    Err(_) => "native listener panicked".to_string(),
                };
                // run's native hook has already been released, including on panic.
                listener
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .stop(format!(
                        "global hotkey listener stopped: {details}; restart Window Zones"
                    ));
            });
        let worker = match spawn {
            Ok(thread) => match ready_rx.recv() {
                Ok(ready) => Some(Worker {
                    id: ready.id,
                    thread,
                    #[cfg(test)]
                    hook: ready.hook,
                }),
                Err(_) => {
                    // Initialization failed before readiness. Publish its actual
                    // error before register_hotkeys can inspect listener status.
                    let _ = thread.join();
                    None
                }
            },
            Err(error) => {
                state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .stop(format!(
                        "failed to start global hotkey listener: {error}; restart Window Zones"
                    ));
                None
            }
        };
        Self {
            state,
            worker: Mutex::new(worker),
        }
    }

    pub(super) fn stop(&self) -> Result<(), String> {
        let mut worker = self.worker.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(running) = worker.as_ref() else {
            return Ok(());
        };
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .stop("native hotkey listener released; restart Window Zones".to_string());
        if !running.thread.is_finished() {
            // The thread creates its queue before acknowledging readiness, so
            // shutdown cannot race the creation of the native message queue.
            unsafe { PostThreadMessageW(running.id, WM_QUIT, WPARAM(0), LPARAM(0)) }
                .map_err(|error| format!("could not stop native hotkey listener: {error}"))?;
        }
        if let Some(running) = worker.take() {
            let _ = running.thread.join();
        }
        Ok(())
    }
}

impl Drop for NativeListener {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub(super) fn new_listener() -> Arc<NativeListener> {
    // Retain the owner even if shutdown fails: a later constructor must not
    // forget an unjoined thread and install another hook alongside it.
    static LISTENER: Mutex<Option<Arc<NativeListener>>> = Mutex::new(None);
    let mut slot = LISTENER.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(previous) = slot.as_ref()
        && let Err(error) = previous.stop()
    {
        // Never install a second hook unless the previous thread was joined.
        let mut state = RdevListener::default();
        state.stop(error);
        return Arc::new(NativeListener {
            state: Arc::new(Mutex::new(state)),
            worker: Mutex::new(None),
        });
    }
    // Windows can silently remove a timed-out hook without ending its loop.
    // Always replace it, rather than guessing health from elapsed time/events.
    let listener = Arc::new(NativeListener::start());
    *slot = Some(Arc::clone(&listener));
    listener
}

struct NativeHook(HHOOK);

impl Drop for NativeHook {
    fn drop(&mut self) {
        // An already silently removed hook may report an invalid handle. The
        // owner also joins this thread before installing any replacement.
        let _ = unsafe { UnhookWindowsHookEx(self.0) };
    }
}

struct CallbackContext {
    listener: Arc<Mutex<RdevListener>>,
    failed: bool,
}

thread_local! {
    static CONTEXT: RefCell<Option<CallbackContext>> = const { RefCell::new(None) };
}

fn run(listener: Arc<Mutex<RdevListener>>, ready: mpsc::SyncSender<Ready>) -> Result<(), String> {
    let mut message = MSG::default();
    let _ = unsafe { PeekMessageW(&mut message, HWND::default(), 0, 0, PM_NOREMOVE) };
    {
        let mut state = listener.lock().unwrap_or_else(PoisonError::into_inner);
        // Held keys at installation must not activate on their first repeat.
        for code in 1..=0xfe {
            if let Some(key) = key_from_code(code)
                && native_down(code)
            {
                state.pressed.insert(key);
            }
        }
    }
    CONTEXT.with(|context| {
        *context.borrow_mut() = Some(CallbackContext {
            listener,
            failed: false,
        });
    });
    let result = (|| {
        let module = unsafe { GetModuleHandleW(PCWSTR::null()) }
            .map_err(|error| format!("could not locate keyboard-hook module: {error}"))?;
        let hook = NativeHook(
            unsafe {
                SetWindowsHookExW(
                    WH_KEYBOARD_LL,
                    Some(keyboard_callback),
                    HINSTANCE(module.0),
                    0,
                )
            }
            .map_err(|error| format!("could not install Windows keyboard hook: {error}"))?,
        );
        ready
            .send(Ready {
                id: unsafe { GetCurrentThreadId() },
                #[cfg(test)]
                hook: hook.0.0 as usize,
            })
            .map_err(|_| "keyboard-hook readiness receiver closed".to_string())?;
        loop {
            let status = unsafe { GetMessageW(&mut message, HWND::default(), 0, 0) }.0;
            if status == -1 {
                return Err(format!(
                    "keyboard-hook message loop failed: {}",
                    ::windows::core::Error::from_win32()
                ));
            }
            if status == 0 {
                break;
            }
        }
        drop(hook);
        if CONTEXT.with(|context| {
            context
                .borrow()
                .as_ref()
                .is_some_and(|context| context.failed)
        }) {
            return Err("Windows keyboard-hook callback panicked".to_string());
        }
        Ok(())
    })();
    CONTEXT.with(|context| {
        context.borrow_mut().take();
    });
    result
}

unsafe extern "system" fn keyboard_callback(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == 0 && lparam.0 != 0 {
        let consume = catch_unwind(AssertUnwindSafe(|| {
            CONTEXT.with(|context| {
                let mut context = context.borrow_mut();
                let Some(context) = context.as_mut() else {
                    return false;
                };
                if context.failed {
                    return false;
                }
                let native = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
                let Some(event) = key_event(wparam.0 as u32, native) else {
                    return false;
                };
                // Parsing and native work stay outside the registration lock;
                // preserve key releases rather than losing them on contention.
                let Ok(mut state) = context.listener.lock() else {
                    return false;
                };
                handle_event(&mut state, event, native_down)
            })
        }));
        match consume {
            Ok(true) => return LRESULT(1),
            Ok(false) => {}
            Err(_) => {
                CONTEXT.with(|context| {
                    if let Some(context) = context.borrow_mut().as_mut() {
                        context.failed = true;
                    }
                });
                unsafe { PostQuitMessage(0) };
            }
        }
    }
    unsafe { CallNextHookEx(HHOOK::default(), code, wparam, lparam) }
}

fn native_down(code: u32) -> bool {
    (unsafe { GetAsyncKeyState(code as i32) }) < 0
}

fn handle_event(state: &mut RdevListener, event: EventType, down: impl Fn(u32) -> bool) -> bool {
    let current = match event {
        EventType::KeyPress(key) | EventType::KeyRelease(key) => key,
        _ => return false,
    };
    let mut missed_modifier = false;
    for (code, key) in MODIFIERS {
        // LowLevelKeyboardProc runs BEFORE GetAsyncKeyState changes for this
        // event. Reconcile other modifiers, then apply the current hook event.
        if key == current {
            continue;
        }
        if down(code) {
            state.pressed.insert(key);
        } else if state.pressed.remove(&key) {
            missed_modifier = true;
        }
    }
    if missed_modifier {
        // Across a secure desktop transition the consumed key's release can
        // also be lost. Do not keep swallowing later plain typing as repeats.
        state.pressed.retain(|key| modifier_mask(key).is_some());
        state.swallowed.clear();
    }
    state.handle_event(event)
}

fn key_event(message: u32, native: &KBDLLHOOKSTRUCT) -> Option<EventType> {
    let code = match native.vkCode {
        0x10 => {
            if native.scanCode == 0x36 {
                0xa1
            } else {
                0xa0
            }
        }
        0x11 => {
            if native.flags.contains(LLKHF_EXTENDED) {
                0xa3
            } else {
                0xa2
            }
        }
        0x12 => {
            if native.flags.contains(LLKHF_EXTENDED) {
                0xa5
            } else {
                0xa4
            }
        }
        code => code,
    };
    let key = key_from_code(code)?;
    match message {
        WM_KEYDOWN | WM_SYSKEYDOWN => Some(EventType::KeyPress(key)),
        WM_KEYUP | WM_SYSKEYUP => Some(EventType::KeyRelease(key)),
        _ => None,
    }
}

fn key_from_code(code: u32) -> Option<Key> {
    Some(match code {
        0x08 => Key::Backspace,
        0x09 => Key::Tab,
        0x0d => Key::Return,
        0x13 => Key::Pause,
        0x14 => Key::CapsLock,
        0x1b => Key::Escape,
        0x20 => Key::Space,
        0x21 => Key::PageUp,
        0x22 => Key::PageDown,
        0x23 => Key::End,
        0x24 => Key::Home,
        0x25 => Key::LeftArrow,
        0x26 => Key::UpArrow,
        0x27 => Key::RightArrow,
        0x28 => Key::DownArrow,
        0x2c => Key::PrintScreen,
        0x2d => Key::Insert,
        0x2e => Key::Delete,
        0x30 => Key::Num0,
        0x31 => Key::Num1,
        0x32 => Key::Num2,
        0x33 => Key::Num3,
        0x34 => Key::Num4,
        0x35 => Key::Num5,
        0x36 => Key::Num6,
        0x37 => Key::Num7,
        0x38 => Key::Num8,
        0x39 => Key::Num9,
        0x41 => Key::KeyA,
        0x42 => Key::KeyB,
        0x43 => Key::KeyC,
        0x44 => Key::KeyD,
        0x45 => Key::KeyE,
        0x46 => Key::KeyF,
        0x47 => Key::KeyG,
        0x48 => Key::KeyH,
        0x49 => Key::KeyI,
        0x4a => Key::KeyJ,
        0x4b => Key::KeyK,
        0x4c => Key::KeyL,
        0x4d => Key::KeyM,
        0x4e => Key::KeyN,
        0x4f => Key::KeyO,
        0x50 => Key::KeyP,
        0x51 => Key::KeyQ,
        0x52 => Key::KeyR,
        0x53 => Key::KeyS,
        0x54 => Key::KeyT,
        0x55 => Key::KeyU,
        0x56 => Key::KeyV,
        0x57 => Key::KeyW,
        0x58 => Key::KeyX,
        0x59 => Key::KeyY,
        0x5a => Key::KeyZ,
        0x5b => Key::MetaLeft,
        0x5c => Key::MetaRight,
        0x70 => Key::F1,
        0x71 => Key::F2,
        0x72 => Key::F3,
        0x73 => Key::F4,
        0x74 => Key::F5,
        0x75 => Key::F6,
        0x76 => Key::F7,
        0x77 => Key::F8,
        0x78 => Key::F9,
        0x79 => Key::F10,
        0x7a => Key::F11,
        0x7b => Key::F12,
        0x7c..=0x87 => Key::Unknown(code),
        0x90 => Key::NumLock,
        0x91 => Key::ScrollLock,
        0xa0 => Key::ShiftLeft,
        0xa1 => Key::ShiftRight,
        0xa2 => Key::ControlLeft,
        0xa3 => Key::ControlRight,
        0xa4 => Key::Alt,
        0xa5 => Key::AltGr,
        0xba => Key::SemiColon,
        0xbb => Key::Equal,
        0xbc => Key::Comma,
        0xbd => Key::Minus,
        0xbe => Key::Dot,
        0xbf => Key::Slash,
        0xc0 => Key::BackQuote,
        0xdb => Key::LeftBracket,
        0xdc => Key::BackSlash,
        0xdd => Key::RightBracket,
        0xde => Key::Quote,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkey_system::{HotkeyEvent, HotkeySystem, RdevHotkeySystem};

    fn system() -> RdevHotkeySystem {
        RdevHotkeySystem::with_listener(Arc::new(Mutex::new(RdevListener::default())))
    }

    fn event(system: &RdevHotkeySystem, event: EventType, modifiers: &[u32]) -> bool {
        handle_event(&mut system.listener.lock().unwrap(), event, |code| {
            modifiers.contains(&code)
        })
    }

    #[test]
    fn altgr_synthetic_control_does_not_capture_international_typing() {
        let mut system = system();
        system
            .register_hotkeys(&["alt+ctrl+e".to_string()])
            .unwrap();
        let right_alt = KBDLLHOOKSTRUCT {
            vkCode: 0x12,
            flags: LLKHF_EXTENDED,
            ..Default::default()
        };
        assert_eq!(
            key_event(WM_SYSKEYDOWN, &right_alt),
            Some(EventType::KeyPress(Key::AltGr))
        );
        assert!(!event(&system, EventType::KeyPress(Key::ControlLeft), &[]));
        assert!(!event(&system, EventType::KeyPress(Key::AltGr), &[0xa2]));
        assert!(!event(
            &system,
            EventType::KeyPress(Key::KeyE),
            &[0xa2, 0xa5]
        ));
        assert!(!event(
            &system,
            EventType::KeyRelease(Key::KeyE),
            &[0xa2, 0xa5]
        ));
        assert!(!event(
            &system,
            EventType::KeyRelease(Key::AltGr),
            &[0xa2, 0xa5]
        ));
        assert!(!event(
            &system,
            EventType::KeyRelease(Key::ControlLeft),
            &[0xa2]
        ));
        assert_eq!(system.next_hotkey().unwrap(), None);
        event(&system, EventType::KeyPress(Key::ControlLeft), &[]);
        event(&system, EventType::KeyPress(Key::Alt), &[0xa2]);
        assert!(event(
            &system,
            EventType::KeyPress(Key::KeyE),
            &[0xa2, 0xa4]
        ));
        assert_eq!(
            system.next_hotkey().unwrap(),
            Some(HotkeyEvent::Pressed {
                hotkey: "alt+ctrl+e".to_string(),
            })
        );
    }

    #[test]
    fn missed_modifier_and_consumed_key_releases_cannot_hijack_plain_typing() {
        let mut system = system();
        system
            .register_hotkeys(&["alt+ctrl+left".to_string()])
            .unwrap();
        event(&system, EventType::KeyPress(Key::ControlLeft), &[]);
        event(&system, EventType::KeyPress(Key::Alt), &[0xa2]);
        assert!(event(
            &system,
            EventType::KeyPress(Key::LeftArrow),
            &[0xa2, 0xa4]
        ));
        assert!(system.next_hotkey().unwrap().is_some());
        // All releases happened on the secure desktop: only the next plain
        // key is observed, with the authoritative native modifiers now up.
        assert!(!event(&system, EventType::KeyPress(Key::LeftArrow), &[]));
        assert!(!event(&system, EventType::KeyRelease(Key::LeftArrow), &[]));
        assert_eq!(system.next_hotkey().unwrap(), None);
    }

    #[test]
    fn current_modifier_event_wins_over_pre_event_async_state() {
        let mut system = system();
        system.register_hotkeys(&["ctrl+a".to_string()]).unwrap();
        event(&system, EventType::KeyPress(Key::ControlLeft), &[]);
        assert!(
            system
                .listener
                .lock()
                .unwrap()
                .pressed
                .contains(&Key::ControlLeft)
        );
        assert!(event(&system, EventType::KeyPress(Key::KeyA), &[0xa2]));
        assert!(event(&system, EventType::KeyPress(Key::KeyA), &[0xa2]));
        event(&system, EventType::KeyRelease(Key::ControlLeft), &[0xa2]);
        assert!(
            !system
                .listener
                .lock()
                .unwrap()
                .pressed
                .contains(&Key::ControlLeft)
        );
        assert!(event(&system, EventType::KeyRelease(Key::KeyA), &[]));
        assert!(!event(&system, EventType::KeyPress(Key::KeyA), &[]));
        assert!(system.next_hotkey().unwrap().is_some());
        assert_eq!(system.next_hotkey().unwrap(), None);
    }

    #[test]
    fn native_restart_replaces_silently_removed_hook_only_after_join() {
        let mut first = RdevHotkeySystem::new();
        first.register_hotkeys(&[]).unwrap();
        let previous = Arc::clone(first.native_listener.as_ref().unwrap());
        {
            let worker = previous.worker.lock().unwrap();
            let worker = worker.as_ref().unwrap();
            // Reproduce Windows' removal with its real unhook API: the message
            // loop remains live and the listener has not received any error.
            unsafe { UnhookWindowsHookEx(HHOOK(worker.hook as *mut _)) }.unwrap();
            assert!(!worker.thread.is_finished());
        }
        assert_eq!(previous.state.lock().unwrap().status(), Ok(()));
        let mut second = RdevHotkeySystem::new();
        second.register_hotkeys(&[]).unwrap();
        assert!(previous.worker.lock().unwrap().is_none());
        assert!(first.next_hotkey().is_err());
        let current = Arc::clone(second.native_listener.as_ref().unwrap());
        assert!(current.worker.lock().unwrap().is_some());
        drop(first);
        assert!(current.worker.lock().unwrap().is_some());
        drop(second);
        assert!(current.worker.lock().unwrap().is_none());
    }
}
