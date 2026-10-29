//! Keyboard-only event tap owned by the listener thread.

use super::{KeyEvent, NativeListenerState};
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::null_mut;
use std::sync::{Arc, Mutex, PoisonError};

type NativeRef = *mut c_void;
const KEY_DOWN: u32 = 10;
const KEY_UP: u32 = 11;
const FLAGS_CHANGED: u32 = 12;
const TAP_DISABLED_TIMEOUT: u32 = 0xffff_fffe;
const TAP_DISABLED_USER: u32 = 0xffff_ffff;
const KEYCODE_FIELD: u32 = 9;
const AUTOREPEAT_FIELD: u32 = 8;
const CAPS_LOCK_FLAG: u64 = 0x10000;

// CGEventTypes.h and IOKit/hidsystem/IOLLEvent.h: aggregate and per-device flags.
const MODIFIERS: [(u32, u32, u64, u64, u64); 4] = [
    (59, 62, 0x40000, 0x1, 0x2000),
    (56, 60, 0x20000, 0x2, 0x4),
    (55, 54, 0x100000, 0x8, 0x10),
    (58, 61, 0x80000, 0x20, 0x40),
];

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGEventTapCreate(
        location: u32,
        placement: u32,
        options: u32,
        mask: u64,
        callback: unsafe extern "C" fn(NativeRef, u32, NativeRef, NativeRef) -> NativeRef,
        user_info: NativeRef,
    ) -> NativeRef;
    fn CGEventTapEnable(tap: NativeRef, enable: bool);
    fn CGEventTapIsEnabled(tap: NativeRef) -> bool;
    fn CGEventGetIntegerValueField(event: NativeRef, field: u32) -> i64;
    fn CGEventGetFlags(event: NativeRef) -> u64;
    fn CGEventSourceKeyState(source: i32, keycode: u16) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFMachPortCreateRunLoopSource(
        allocator: NativeRef,
        tap: NativeRef,
        order: isize,
    ) -> NativeRef;
    fn CFMachPortInvalidate(tap: NativeRef);
    fn CFRunLoopGetCurrent() -> NativeRef;
    fn CFRunLoopAddSource(run_loop: NativeRef, source: NativeRef, mode: NativeRef);
    fn CFRunLoopRemoveSource(run_loop: NativeRef, source: NativeRef, mode: NativeRef);
    fn CFRunLoopSourceInvalidate(source: NativeRef);
    fn CFRunLoopRun();
    fn CFRunLoopStop(run_loop: NativeRef);
    fn CFRelease(object: *const c_void);
    static kCFRunLoopCommonModes: NativeRef;
}

struct NativeTap {
    port: NativeRef,
    source: NativeRef,
    run_loop: NativeRef,
}

impl NativeTap {
    fn new(context: &mut CallbackContext) -> Result<Self, String> {
        // The callback cannot run until this thread enters its run loop. The
        // local context stays alive until the source and port are invalidated.
        let port = unsafe {
            CGEventTapCreate(
                0, // kCGHIDEventTap
                0, // kCGHeadInsertEventTap
                0, // kCGEventTapOptionDefault: configured presses may be consumed
                (1 << KEY_DOWN) | (1 << KEY_UP) | (1 << FLAGS_CHANGED),
                tap_callback,
                (context as *mut CallbackContext).cast(),
            )
        };
        if port.is_null() {
            return Err("could not create macOS keyboard event tap (EventTapError)".to_string());
        }
        let mut tap = Self {
            port,
            source: null_mut(),
            run_loop: context.run_loop,
        };
        context.tap = port;
        tap.source = unsafe { CFMachPortCreateRunLoopSource(null_mut(), port, 0) };
        if tap.source.is_null() {
            return Err(
                "could not create macOS keyboard run-loop source (LoopSourceError)".to_string(),
            );
        }
        context.reconcile_pressed();
        unsafe {
            CFRunLoopAddSource(tap.run_loop, tap.source, kCFRunLoopCommonModes);
            CGEventTapEnable(port, true);
        }
        if !unsafe { CGEventTapIsEnabled(port) } {
            return Err("macOS keyboard event tap could not be enabled".to_string());
        }
        Ok(tap)
    }
}

impl Drop for NativeTap {
    fn drop(&mut self) {
        // This runs on the callback's thread after CFRunLoopRun has returned,
        // including unwinding: no old callback can survive listener replacement.
        unsafe {
            CGEventTapEnable(self.port, false);
            if !self.source.is_null() {
                CFRunLoopRemoveSource(self.run_loop, self.source, kCFRunLoopCommonModes);
                CFRunLoopSourceInvalidate(self.source);
            }
            CFMachPortInvalidate(self.port);
            if !self.source.is_null() {
                CFRelease(self.source);
            }
            CFRelease(self.port);
        }
    }
}

struct CallbackContext {
    listener: Arc<Mutex<NativeListenerState>>,
    tap: NativeRef,
    run_loop: NativeRef,
    failure: Option<&'static str>,
}

impl CallbackContext {
    fn new(listener: Arc<Mutex<NativeListenerState>>) -> Self {
        Self {
            listener,
            tap: null_mut(),
            run_loop: unsafe { CFRunLoopGetCurrent() },
            failure: None,
        }
    }

    fn reconcile_pressed(&self) {
        let mut state = self.listener.lock().unwrap_or_else(PoisonError::into_inner);
        let state = &mut *state;
        state.pressed.clear();
        // Rebuild from the combined session's physical/synthetic key state.
        // Missed releases must not stick; held keys must not activate on repeat.
        for code in 0..128 {
            if let Some(key) = key_from_code(code)
                && unsafe { CGEventSourceKeyState(0, code) }
            {
                state.pressed.insert(key);
            }
        }
        // Still-held consumed presses retain their matching consumed release.
        state.swallowed.retain(|key| state.pressed.contains(key));
    }

    fn fail(&mut self, message: &'static str) {
        self.failure = Some(message);
        unsafe {
            if !self.tap.is_null() {
                CGEventTapEnable(self.tap, false);
            }
            CFRunLoopStop(self.run_loop);
        }
    }

    fn handle_event(&mut self, event_type: u32, event: NativeRef) -> bool {
        if self.failure.is_some() {
            return false;
        }
        if matches!(event_type, TAP_DISABLED_TIMEOUT | TAP_DISABLED_USER) {
            // Apple delivers these notifications even though they are not in
            // the event mask, and their event pointer need not be a key event.
            self.reconcile_pressed();
            unsafe { CGEventTapEnable(self.tap, true) };
            if !unsafe { CGEventTapIsEnabled(self.tap) } {
                self.fail("macOS keyboard event tap could not be re-enabled");
            }
            return false;
        }
        if event.is_null() || !matches!(event_type, KEY_DOWN | KEY_UP | FLAGS_CHANGED) {
            return false;
        }
        let code = unsafe { CGEventGetIntegerValueField(event, KEYCODE_FIELD) };
        let Some(key) = u16::try_from(code).ok().and_then(key_from_code) else {
            return false;
        };
        let flags = unsafe { CGEventGetFlags(event) };
        let mut state = self.listener.lock().unwrap_or_else(PoisonError::into_inner);
        if event_type == FLAGS_CHANGED {
            if let Some(&(left, right, aggregate, left_flag, right_flag)) = MODIFIERS
                .iter()
                .find(|&&(left, right, ..)| key == left || key == right)
            {
                if flags & aggregate == 0 || flags & (left_flag | right_flag) != 0 {
                    set_pressed(
                        &mut state,
                        left,
                        flags & aggregate != 0 && flags & left_flag != 0,
                    );
                    set_pressed(
                        &mut state,
                        right,
                        flags & aggregate != 0 && flags & right_flag != 0,
                    );
                    return false;
                }
                // Some synthetic sources supply aggregate flags only. The
                // changed key identifies the side, even while both are held.
                let down = !state.pressed.contains(&key);
                set_pressed(&mut state, key, down);
                return false;
            }
            if key != 57 {
                return false;
            }
            let event = if flags & CAPS_LOCK_FLAG != 0 {
                KeyEvent::KeyPress(key)
            } else {
                KeyEvent::KeyRelease(key)
            };
            return state.handle_event(event);
        }
        // Event flags are authoritative if a modifier changed while the tap
        // was unavailable, or was already down when this listener started.
        for (left, right, aggregate, left_flag, right_flag) in MODIFIERS {
            if flags & aggregate == 0 || flags & (left_flag | right_flag) != 0 {
                set_pressed(
                    &mut state,
                    left,
                    flags & aggregate != 0 && flags & left_flag != 0,
                );
                set_pressed(
                    &mut state,
                    right,
                    flags & aggregate != 0 && flags & right_flag != 0,
                );
            } else if !state.pressed.contains(&left) && !state.pressed.contains(&right) {
                state.pressed.insert(left);
            }
        }
        if event_type == KEY_DOWN
            && unsafe { CGEventGetIntegerValueField(event, AUTOREPEAT_FIELD) } != 0
        {
            // A repeat without an observed initial press never creates an action.
            return state.swallowed.contains(&key);
        }
        state.handle_event(if event_type == KEY_DOWN {
            KeyEvent::KeyPress(key)
        } else {
            KeyEvent::KeyRelease(key)
        })
    }
}

fn set_pressed(state: &mut NativeListenerState, key: u32, down: bool) {
    if down {
        state.pressed.insert(key);
    } else {
        state.pressed.remove(&key);
    }
}

unsafe extern "C" fn tap_callback(
    _proxy: NativeRef,
    event_type: u32,
    event: NativeRef,
    user_info: NativeRef,
) -> NativeRef {
    let Some(context) = (unsafe { user_info.cast::<CallbackContext>().as_mut() }) else {
        return event;
    };
    match catch_unwind(AssertUnwindSafe(|| context.handle_event(event_type, event))) {
        Ok(true) => null_mut(),
        Ok(false) => event,
        Err(_) => {
            // Unwinding must never cross this native callback boundary. Stop
            // first; spawn_listener publishes failure only after native cleanup.
            context.fail("macOS keyboard event-tap callback panicked");
            event
        }
    }
}

pub(super) fn listen(
    listener: Arc<Mutex<NativeListenerState>>,
    ready: std::sync::mpsc::SyncSender<()>,
) -> Result<(), String> {
    let mut context = CallbackContext::new(listener);
    let tap = NativeTap::new(&mut context)?;
    if ready.send(()).is_err() {
        return Ok(());
    }
    unsafe { CFRunLoopRun() };
    drop(tap);
    match context.failure {
        Some(error) => Err(error.to_string()),
        None => Ok(()),
    }
}

// Carbon HIToolbox/Events.h virtual keycodes admitted by the keyboard listener.
fn key_from_code(code: u16) -> Option<u32> {
    matches!(
        code,
        0..=9 | 11..=51 | 53..=62 | 64 | 79 | 80 | 90
            | 96..=101 | 103 | 105..=107 | 109 | 111
            | 113..=126
    )
    .then_some(u32::from(code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hotkey_system::{HotkeyEvent, HotkeySystem, NativeHotkeySystem};

    unsafe extern "C" {
        fn CGEventCreateKeyboardEvent(source: NativeRef, keycode: u16, down: bool) -> NativeRef;
        fn CGEventSetFlags(event: NativeRef, flags: u64);
        fn CGEventSetIntegerValueField(event: NativeRef, field: u32, value: i64);
    }

    // Real CGEvents cross the exact translation/FFI boundary used by the tap.
    fn key(
        context: &mut CallbackContext,
        event_type: u32,
        code: u16,
        flags: u64,
        repeat: bool,
    ) -> bool {
        let event = unsafe { CGEventCreateKeyboardEvent(null_mut(), code, event_type != KEY_UP) };
        assert!(!event.is_null());
        unsafe {
            CGEventSetFlags(event, flags);
            CGEventSetIntegerValueField(event, AUTOREPEAT_FIELD, i64::from(repeat));
        }
        let result = unsafe {
            tap_callback(
                null_mut(),
                event_type,
                event,
                (context as *mut CallbackContext).cast(),
            )
        };
        unsafe { CFRelease(event) };
        result.is_null()
    }

    fn system() -> (NativeHotkeySystem, Box<CallbackContext>) {
        let listener = Arc::new(Mutex::new(NativeListenerState::default()));
        let context = Box::new(CallbackContext::new(Arc::clone(&listener)));
        (NativeHotkeySystem::with_listener(listener), context)
    }

    #[test]
    fn native_key_filter_preserves_the_supported_set() {
        let admitted = [
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
            25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46,
            47, 48, 49, 50, 51, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 64, 79, 80, 90, 96, 97, 98,
            99, 100, 101, 103, 105, 106, 107, 109, 111, 113, 114, 115, 116, 117, 118, 119, 120,
            121, 122, 123, 124, 125, 126,
        ];
        for code in 0..=u16::MAX {
            assert_eq!(
                key_from_code(code),
                admitted.contains(&code).then_some(u32::from(code)),
                "{code}"
            );
        }
    }

    #[test]
    fn native_control_sides_consume_one_press_repeat_and_matching_release() {
        for (control, flags) in [(59, 0x40001), (62, 0x42000), (62, 0x40000)] {
            let (mut system, mut context) = system();
            system.register_hotkeys(&["ctrl+f8".to_string()]).unwrap();
            assert!(!key(&mut context, FLAGS_CHANGED, control, flags, false));
            assert!(key(&mut context, KEY_DOWN, 100, flags, false));
            assert!(key(&mut context, KEY_DOWN, 100, flags, true));
            assert_eq!(
                system.next_hotkey().unwrap(),
                Some(HotkeyEvent::Pressed {
                    hotkey: "ctrl+f8".to_string(),
                })
            );
            assert_eq!(system.next_hotkey().unwrap(), None);
            assert!(!key(&mut context, FLAGS_CHANGED, control, 0, false));
            system.register_hotkeys(&[]).unwrap();
            assert!(key(&mut context, KEY_UP, 100, 0, false));
            assert!(!key(&mut context, KEY_DOWN, 100, 0, false));
            assert!(!key(&mut context, KEY_DOWN, 255, flags, false));
        }
    }

    #[test]
    fn native_modifiers_track_both_sides_without_flag_ordering_or_repeat_activation() {
        let (mut system, mut context) = system();
        system.register_hotkeys(&["ctrl+f8".to_string()]).unwrap();
        assert!(!key(&mut context, FLAGS_CHANGED, 59, 0x40001, false));
        assert!(!key(&mut context, FLAGS_CHANGED, 62, 0x42001, false));
        assert!(!key(&mut context, FLAGS_CHANGED, 59, 0x42000, false));
        assert!(key(&mut context, KEY_DOWN, 100, 0x42000, false));
        assert!(key(&mut context, KEY_UP, 100, 0x42000, false));
        assert!(!key(&mut context, KEY_DOWN, 100, 0x42000, true));
        assert!(system.next_hotkey().unwrap().is_some());
        assert_eq!(system.next_hotkey().unwrap(), None);
    }

    #[test]
    fn native_extended_function_and_navigation_keys_reach_registered_bindings() {
        let (mut system, mut context) = system();
        for (code, hotkey) in [
            (105, "f13"),
            (107, "f14"),
            (113, "f15"),
            (106, "f16"),
            (64, "f17"),
            (79, "f18"),
            (80, "f19"),
            (90, "f20"),
            (114, "insert"),
            (115, "home"),
            (119, "end"),
            (116, "pageup"),
            (121, "pagedown"),
            (117, "delete"),
            (123, "left"),
            (124, "right"),
            (125, "down"),
            (126, "up"),
        ] {
            system.register_hotkeys(&[hotkey.to_string()]).unwrap();
            assert!(key(&mut context, KEY_DOWN, code, 0, false), "{hotkey}");
            assert!(key(&mut context, KEY_UP, code, 0, false), "{hotkey}");
            assert_eq!(
                system.next_hotkey().unwrap(),
                Some(HotkeyEvent::Pressed {
                    hotkey: hotkey.to_string(),
                })
            );
        }
    }

    #[test]
    #[ignore = "requires macOS Accessibility permission and all keyboard keys released"]
    fn native_disabled_tap_reenables_same_port_and_survives_registration_restart() {
        let (mut system, mut context) = system();
        for code in [59, 62, 100] {
            assert!(
                !unsafe { CGEventSourceKeyState(0, code) },
                "release Control and F8 first"
            );
        }
        let tap = NativeTap::new(&mut context).unwrap();
        system.register_hotkeys(&["ctrl+f8".to_string()]).unwrap();
        for notification in [TAP_DISABLED_TIMEOUT, TAP_DISABLED_USER] {
            assert!(!key(&mut context, FLAGS_CHANGED, 62, 0x42000, false));
            assert!(key(&mut context, KEY_DOWN, 100, 0x42000, false));
            assert!(system.next_hotkey().unwrap().is_some());
            unsafe {
                CGEventTapEnable(tap.port, false);
                tap_callback(
                    null_mut(),
                    notification,
                    null_mut(),
                    (&mut *context as *mut CallbackContext).cast(),
                );
            }
            assert!(unsafe { CGEventTapIsEnabled(tap.port) });
            assert!(context.failure.is_none());
            let state = context.listener.lock().unwrap();
            assert!(!state.pressed.contains(&62));
            assert!(!state.pressed.contains(&100));
            assert!(state.swallowed.is_empty());
            drop(state);
            // A release missed during the disabled interval cannot leave a
            // stale consumed release or block the next activation.
            assert!(!key(&mut context, KEY_UP, 100, 0, false));
            drop(system);
            system = NativeHotkeySystem::with_listener(Arc::clone(&context.listener));
            system.register_hotkeys(&["ctrl+f8".to_string()]).unwrap();
            assert!(!key(&mut context, FLAGS_CHANGED, 62, 0x42000, false));
            assert!(key(&mut context, KEY_DOWN, 100, 0x42000, false));
            assert!(key(&mut context, KEY_UP, 100, 0x42000, false));
            assert!(!key(&mut context, FLAGS_CHANGED, 62, 0, false));
            assert!(system.next_hotkey().unwrap().is_some());
            assert_eq!(system.next_hotkey().unwrap(), None);
        }
        drop(tap);
    }
}
