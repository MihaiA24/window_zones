//! One owned Win32 low-level keyboard hook, with an explicit ready/stop handshake.

use super::{KeyEvent, NativeListenerState, modifier_mask};
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

const MODIFIERS: [u32; 8] = [0xa0, 0xa1, 0xa2, 0xa3, 0xa4, 0xa5, 0x5b, 0x5c];

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
    pub(super) state: Arc<Mutex<NativeListenerState>>,
    worker: Mutex<Option<Worker>>,
}

impl NativeListener {
    fn start() -> Self {
        let state = Arc::new(Mutex::new(NativeListenerState::default()));
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
        let mut state = NativeListenerState::default();
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
    listener: Arc<Mutex<NativeListenerState>>,
    failed: bool,
}

thread_local! {
    static CONTEXT: RefCell<Option<CallbackContext>> = const { RefCell::new(None) };
}

fn run(
    listener: Arc<Mutex<NativeListenerState>>,
    ready: mpsc::SyncSender<Ready>,
) -> Result<(), String> {
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

fn handle_event(
    state: &mut NativeListenerState,
    event: KeyEvent,
    down: impl Fn(u32) -> bool,
) -> bool {
    let current = match event {
        KeyEvent::KeyPress(key) | KeyEvent::KeyRelease(key) => key,
    };
    let mut missed_modifier = false;
    for key in MODIFIERS {
        // LowLevelKeyboardProc runs BEFORE GetAsyncKeyState changes for this
        // event. Reconcile other modifiers, then apply the current hook event.
        if key == current {
            continue;
        }
        if down(key) {
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

fn key_event(message: u32, native: &KBDLLHOOKSTRUCT) -> Option<KeyEvent> {
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
        WM_KEYDOWN | WM_SYSKEYDOWN => Some(KeyEvent::KeyPress(key)),
        WM_KEYUP | WM_SYSKEYUP => Some(KeyEvent::KeyRelease(key)),
        _ => None,
    }
}

fn key_from_code(code: u32) -> Option<u32> {
    matches!(
        code,
        0x08 | 0x09 | 0x0d | 0x13 | 0x14 | 0x1b
            | 0x20..=0x28
            | 0x2c..=0x2e
            | 0x30..=0x39
            | 0x41..=0x5c
            | 0x70..=0x87
            | 0x90 | 0x91
            | 0xa0..=0xa5
            | 0xba..=0xc0
            | 0xdb..=0xde
    )
    .then_some(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkey_system::{HotkeyEvent, HotkeySystem, NativeHotkeySystem};

    fn system() -> NativeHotkeySystem {
        NativeHotkeySystem::with_listener(Arc::new(Mutex::new(NativeListenerState::default())))
    }

    fn event(system: &NativeHotkeySystem, event: KeyEvent, modifiers: &[u32]) -> bool {
        handle_event(&mut system.listener.lock().unwrap(), event, |code| {
            modifiers.contains(&code)
        })
    }

    #[test]
    fn native_key_filter_preserves_the_supported_set() {
        let admitted = [
            8, 9, 13, 19, 20, 27, 32, 33, 34, 35, 36, 37, 38, 39, 40, 44, 45, 46, 48, 49, 50, 51,
            52, 53, 54, 55, 56, 57, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80,
            81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 112, 113, 114, 115, 116, 117, 118, 119,
            120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132, 133, 134, 135, 144,
            145, 160, 161, 162, 163, 164, 165, 186, 187, 188, 189, 190, 191, 192, 219, 220, 221,
            222,
        ];
        for code in 0..=0xff {
            assert_eq!(
                key_from_code(code),
                admitted.contains(&code).then_some(code),
                "{code}"
            );
        }
        assert_eq!(key_from_code(u32::MAX), None);
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
            Some(KeyEvent::KeyPress(0xa5))
        );
        assert!(!event(&system, KeyEvent::KeyPress(0xa2), &[]));
        assert!(!event(&system, KeyEvent::KeyPress(0xa5), &[0xa2]));
        assert!(!event(&system, KeyEvent::KeyPress(0x45), &[0xa2, 0xa5]));
        assert!(!event(&system, KeyEvent::KeyRelease(0x45), &[0xa2, 0xa5]));
        assert!(!event(&system, KeyEvent::KeyRelease(0xa5), &[0xa2, 0xa5]));
        assert!(!event(&system, KeyEvent::KeyRelease(0xa2), &[0xa2]));
        assert_eq!(system.next_hotkey().unwrap(), None);
        event(&system, KeyEvent::KeyPress(0xa2), &[]);
        event(&system, KeyEvent::KeyPress(0xa4), &[0xa2]);
        assert!(event(&system, KeyEvent::KeyPress(0x45), &[0xa2, 0xa4]));
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
        event(&system, KeyEvent::KeyPress(0xa2), &[]);
        event(&system, KeyEvent::KeyPress(0xa4), &[0xa2]);
        assert!(event(&system, KeyEvent::KeyPress(0x25), &[0xa2, 0xa4]));
        assert!(system.next_hotkey().unwrap().is_some());
        // All releases happened on the secure desktop: only the next plain
        // key is observed, with the authoritative native modifiers now up.
        assert!(!event(&system, KeyEvent::KeyPress(0x25), &[]));
        assert!(!event(&system, KeyEvent::KeyRelease(0x25), &[]));
        assert_eq!(system.next_hotkey().unwrap(), None);
    }

    #[test]
    fn current_modifier_event_wins_over_pre_event_async_state() {
        let mut system = system();
        system.register_hotkeys(&["ctrl+a".to_string()]).unwrap();
        event(&system, KeyEvent::KeyPress(0xa2), &[]);
        assert!(system.listener.lock().unwrap().pressed.contains(&0xa2));
        assert!(event(&system, KeyEvent::KeyPress(0x41), &[0xa2]));
        assert!(event(&system, KeyEvent::KeyPress(0x41), &[0xa2]));
        event(&system, KeyEvent::KeyRelease(0xa2), &[0xa2]);
        assert!(!system.listener.lock().unwrap().pressed.contains(&0xa2));
        assert!(event(&system, KeyEvent::KeyRelease(0x41), &[]));
        assert!(!event(&system, KeyEvent::KeyPress(0x41), &[]));
        assert!(system.next_hotkey().unwrap().is_some());
        assert_eq!(system.next_hotkey().unwrap(), None);
    }

    #[test]
    fn native_restart_replaces_silently_removed_hook_only_after_join() {
        let mut first = NativeHotkeySystem::new();
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
        let mut second = NativeHotkeySystem::new();
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
