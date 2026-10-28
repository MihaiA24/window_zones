//! macOS adapter using retained Accessibility window references.
//!
//! Window identity is adapter-scoped and independent of titles, window order,
//! and focus. Dead elements are released only on confirmed WindowGone errors,
//! and their IDs are never reused. Fullscreen and non-resizable windows are
//! rejected before changes.
//! AXZoomed/AXMinimized are restored when exposed; ordinary AppKit zoom/tiling
//! is released by setting the window's size and position.

use crate::{
    DisplayGeometry, FocusedWindow, Rect, WindowId, WindowMove, WindowSystem, WindowSystemError,
};
use std::cell::{Cell, RefCell};
use std::convert::TryFrom;
const ACCESSIBILITY_HINT: &str =
    "allow Window Zones (or its terminal) in System Settings > Privacy & Security > Accessibility";

#[derive(Debug, Default)]
pub struct MacOSWindowSystem {
    accessibility: native::Accessibility,
    windows: RefCell<Vec<(WindowId, native::Value)>>,
    next_window_id: Cell<u64>,
}

impl MacOSWindowSystem {
    pub fn new() -> Self {
        Self::default()
    }
}

impl WindowSystem for MacOSWindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
        let Some(window) = self.accessibility.focused_window()? else {
            return Ok(None);
        };
        let frame = self.accessibility.frame(&window, None)?;
        let mut windows = self.windows.borrow_mut();
        let id = if let Some((id, _)) = windows.iter().find(|(_, saved)| saved.same(&window)) {
            id.clone()
        } else {
            // ponytail: sweep on new captures; AX destroy notifications if churn makes this slow.
            windows.retain(|(id, saved)| {
                !matches!(
                    self.accessibility.frame(saved, Some(id)),
                    Err(WindowSystemError::WindowGone(_))
                )
            });
            let next = self.next_window_id.get();
            self.next_window_id.set(next.checked_add(1).ok_or_else(|| {
                WindowSystemError::Platform("macOS window identity exhausted".into())
            })?);
            let id = WindowId::new(format!("macos:{next}"));
            windows.push((id.clone(), window));
            id
        };
        Ok(Some(FocusedWindow::new(id, frame)))
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        native::displays()
    }

    fn move_window(&mut self, window_move: &WindowMove) -> Result<(), WindowSystemError> {
        let windows = self.windows.get_mut();
        let index = windows
            .iter()
            .position(|(id, _)| id == &window_move.window)
            .ok_or_else(|| WindowSystemError::WindowGone(window_move.window.clone()))?;
        let result = self.accessibility.place(&windows[index].1, window_move);
        if matches!(&result, Err(WindowSystemError::WindowGone(_))) {
            windows.remove(index);
        }
        result
    }
}

fn as_i32(field: &str, value: f64) -> Result<i32, WindowSystemError> {
    if !value.is_finite() {
        return Err(WindowSystemError::Platform(format!(
            "{field} is not finite: {value}"
        )));
    }

    let rounded = value.round();

    let min = f64::from(i32::MIN);
    let max = f64::from(i32::MAX);
    if rounded < min || rounded > max {
        return Err(WindowSystemError::Platform(format!(
            "{field} out of i32 range: {value}"
        )));
    }

    Ok(rounded as i32)
}

fn as_u32(field: &str, value: f64) -> Result<u32, WindowSystemError> {
    if !value.is_finite() {
        return Err(WindowSystemError::Platform(format!(
            "{field} is not finite: {value}"
        )));
    }

    let rounded = value.round();

    if rounded < 0.0 {
        return Err(WindowSystemError::Platform(format!(
            "{field} cannot be negative: {value}"
        )));
    }

    u32::try_from(rounded as i128)
        .map_err(|_| WindowSystemError::Platform(format!("{field} out of u32 range: {value}")))
}

/// Owned Core Foundation values; all Accessibility messaging stays on the
/// calling thread. The SDK defines AXUIElement identity through CFEqual.
mod native {
    use super::*;
    use cocoa::appkit::NSScreen;
    use cocoa::base::nil;
    use cocoa::foundation::NSArray;
    use objc::rc::autoreleasepool;
    use std::ffi::{CStr, c_void};
    use std::ptr::{self, NonNull};

    type Ref = *const c_void;
    const INVALID_ELEMENT: i32 = -25202;
    const CANNOT_COMPLETE: i32 = -25204;
    const UNSUPPORTED: i32 = -25205;
    const NO_VALUE: i32 = -25212;

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> u8;
        fn AXUIElementCreateSystemWide() -> Ref;
        fn AXUIElementSetMessagingTimeout(element: Ref, seconds: f32) -> i32;
        fn AXUIElementCopyAttributeValue(element: Ref, name: Ref, value: *mut Ref) -> i32;
        fn AXUIElementIsAttributeSettable(element: Ref, name: Ref, settable: *mut u8) -> i32;
        fn AXUIElementSetAttributeValue(element: Ref, name: Ref, value: Ref) -> i32;
        fn AXUIElementGetPid(element: Ref, pid: *mut i32) -> i32;
        fn AXUIElementGetTypeID() -> usize;
        fn AXValueGetTypeID() -> usize;
        fn AXValueCreate(kind: u32, value: *const c_void) -> Ref;
        fn AXValueGetValue(value: Ref, kind: u32, result: *mut c_void) -> u8;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: Ref);
        fn CFEqual(first: Ref, second: Ref) -> u8;
        fn CFGetTypeID(value: Ref) -> usize;
        fn CFStringCreateWithCString(allocator: Ref, string: *const i8, encoding: u32) -> Ref;
        fn CFBooleanGetTypeID() -> usize;
        fn CFBooleanGetValue(value: Ref) -> u8;
        static kCFBooleanFalse: Ref;
    }

    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }

    #[derive(Debug)]
    pub(super) struct Value(NonNull<c_void>);

    impl Value {
        // Create/Copy APIs return one ownership reference. Null here is allocation
        // failure, not an absent AX attribute (which is handled before wrapping).
        fn owned(value: Ref) -> Self {
            Self(NonNull::new(value.cast_mut()).expect("Core Foundation allocation failed"))
        }

        fn raw(&self) -> Ref {
            self.0.as_ptr()
        }

        pub(super) fn same(&self, other: &Self) -> bool {
            // SAFETY: both values remain retained during this call.
            unsafe { CFEqual(self.raw(), other.raw()) != 0 }
        }
    }

    impl Drop for Value {
        fn drop(&mut self) {
            // SAFETY: this value owns exactly one Create/Copy reference.
            unsafe { CFRelease(self.raw()) };
        }
    }

    #[repr(C)]
    #[derive(Default)]
    struct Pair {
        first: f64,
        second: f64,
    }

    #[derive(Clone, Copy)]
    enum Attribute {
        Application,
        Window,
        Position,
        Size,
        Fullscreen,
        Minimized,
        Zoomed,
    }

    #[derive(Debug)]
    pub(super) struct Accessibility {
        system: Value,
        attributes: [Value; 7],
    }

    impl Default for Accessibility {
        fn default() -> Self {
            let names: [&CStr; 7] = [
                c"AXFocusedApplication",
                c"AXFocusedWindow",
                c"AXPosition",
                c"AXSize",
                c"AXFullScreen",
                c"AXMinimized",
                c"AXZoomed",
            ];
            Self {
                // SAFETY: Create returns a retained system-wide element.
                system: Value::owned(unsafe { AXUIElementCreateSystemWide() }),
                attributes: names.map(|name| {
                    // SAFETY: names are static NUL-terminated ASCII/UTF-8 strings.
                    Value::owned(unsafe {
                        CFStringCreateWithCString(ptr::null(), name.as_ptr(), 0x0800_0100)
                    })
                }),
            }
        }
    }

    impl Accessibility {
        fn name(&self, attribute: Attribute) -> Ref {
            self.attributes[attribute as usize].raw()
        }

        fn check(
            code: i32,
            operation: &str,
            id: Option<&WindowId>,
        ) -> Result<(), WindowSystemError> {
            if code == 0 {
                return Ok(());
            }
            if code == INVALID_ELEMENT
                && let Some(id) = id
            {
                return Err(WindowSystemError::WindowGone(id.clone()));
            }
            Err(WindowSystemError::Platform(format!(
                "Accessibility {operation} failed ({code}); {ACCESSIBILITY_HINT}"
            )))
        }

        fn check_window(
            code: i32,
            operation: &str,
            window: &Value,
            id: Option<&WindowId>,
        ) -> Result<(), WindowSystemError> {
            // AX reports messaging failure rather than invalid-element when the
            // owning process exits. Do not classify a busy, live app as gone.
            if code == CANNOT_COMPLETE
                && let Some(id) = id
            {
                let mut pid = 0;
                // SAFETY: the retained element provides its positive process id;
                // signal 0 only checks existence (Darwin ESRCH is 3).
                let gone = unsafe {
                    AXUIElementGetPid(window.raw(), &mut pid) == 0
                        && pid > 0
                        && kill(pid, 0) != 0
                        && std::io::Error::last_os_error().raw_os_error() == Some(3)
                };
                if gone {
                    return Err(WindowSystemError::WindowGone(id.clone()));
                }
            }
            Self::check(code, operation, id)
        }

        fn attribute(
            &self,
            element: &Value,
            name: Attribute,
            id: Option<&WindowId>,
        ) -> Result<Option<Value>, WindowSystemError> {
            let mut value = ptr::null();
            // SAFETY: element and name are retained; output receives a Copy reference.
            let code = unsafe {
                AXUIElementCopyAttributeValue(element.raw(), self.name(name), &mut value)
            };
            if code == UNSUPPORTED || code == NO_VALUE {
                return Ok(None);
            }
            Self::check_window(code, "read attribute", element, id)?;
            if value.is_null() {
                return Err(WindowSystemError::Platform(
                    "Accessibility returned no value".into(),
                ));
            }
            Ok(Some(Value::owned(value)))
        }

        fn boolean(
            &self,
            window: &Value,
            name: Attribute,
            id: Option<&WindowId>,
        ) -> Result<bool, WindowSystemError> {
            let Some(value) = self.attribute(window, name, id)? else {
                return Ok(false);
            };
            // SAFETY: validate the remote value's type before interpreting it.
            if unsafe { CFGetTypeID(value.raw()) != CFBooleanGetTypeID() } {
                return Err(WindowSystemError::Platform(
                    "invalid Accessibility boolean".into(),
                ));
            }
            Ok(unsafe { CFBooleanGetValue(value.raw()) != 0 })
        }

        fn settable(
            &self,
            window: &Value,
            name: Attribute,
            id: &WindowId,
        ) -> Result<bool, WindowSystemError> {
            let mut settable = 0;
            // SAFETY: window and name are retained; output is a Boolean.
            let code = unsafe {
                AXUIElementIsAttributeSettable(window.raw(), self.name(name), &mut settable)
            };
            Self::check_window(code, "check writable attribute", window, Some(id))?;
            Ok(settable != 0)
        }

        fn set(
            &self,
            window: &Value,
            name: Attribute,
            value: Ref,
            id: &WindowId,
        ) -> Result<(), WindowSystemError> {
            // SAFETY: window, attribute and the correctly typed value live across the call.
            let code =
                unsafe { AXUIElementSetAttributeValue(window.raw(), self.name(name), value) };
            Self::check_window(code, "set attribute", window, Some(id))
        }

        pub(super) fn focused_window(&self) -> Result<Option<Value>, WindowSystemError> {
            // SAFETY: neither function takes ownership; the timeout is positive.
            if unsafe { AXIsProcessTrusted() } == 0 {
                return Err(WindowSystemError::Platform(ACCESSIBILITY_HINT.into()));
            }
            Self::check(
                unsafe { AXUIElementSetMessagingTimeout(self.system.raw(), 2.0) },
                "set timeout",
                None,
            )?;
            // Query AX directly; Cocoa application state depends on a main run loop.
            let Some(application) = self.attribute(&self.system, Attribute::Application, None)?
            else {
                return Ok(None);
            };
            // SAFETY: the Copy reference is retained; check its type before messaging it.
            if unsafe { CFGetTypeID(application.raw()) != AXUIElementGetTypeID() } {
                return Err(WindowSystemError::Platform(
                    "invalid Accessibility application".into(),
                ));
            }
            let window = self.attribute(&application, Attribute::Window, None)?;
            if let Some(window) = &window
                && unsafe { CFGetTypeID(window.raw()) != AXUIElementGetTypeID() }
            {
                return Err(WindowSystemError::Platform(
                    "invalid Accessibility window".into(),
                ));
            }
            Ok(window)
        }

        fn pair(
            &self,
            window: &Value,
            name: Attribute,
            kind: u32,
            id: Option<&WindowId>,
        ) -> Result<Pair, WindowSystemError> {
            let value = self.attribute(window, name, id)?.ok_or_else(|| {
                WindowSystemError::Platform("window has no Accessibility geometry".into())
            })?;
            let mut pair = Pair::default();
            // SAFETY: CGPoint and CGSize each consist of two CGFloat (f64) fields.
            let valid = unsafe {
                CFGetTypeID(value.raw()) == AXValueGetTypeID()
                    && AXValueGetValue(value.raw(), kind, (&mut pair as *mut Pair).cast()) != 0
            };
            if !valid {
                return Err(WindowSystemError::Platform(
                    "invalid Accessibility geometry".into(),
                ));
            }
            Ok(pair)
        }

        pub(super) fn frame(
            &self,
            window: &Value,
            id: Option<&WindowId>,
        ) -> Result<Rect, WindowSystemError> {
            let position = self.pair(window, Attribute::Position, 1, id)?;
            let size = self.pair(window, Attribute::Size, 2, id)?;
            Ok(Rect::new(
                as_i32("x", position.first)?,
                as_i32("y", position.second)?,
                as_u32("width", size.first)?,
                as_u32("height", size.second)?,
            ))
        }

        pub(super) fn place(
            &self,
            window: &Value,
            movement: &WindowMove,
        ) -> Result<(), WindowSystemError> {
            let id = &movement.window;
            // Read geometry first to distinguish a closed native object from an
            // app that merely omits optional state attributes.
            let before = self.frame(window, Some(id))?;
            if self.boolean(window, Attribute::Fullscreen, Some(id))? {
                return Err(WindowSystemError::Platform(
                    "fullscreen window cannot be moved; leave fullscreen first".into(),
                ));
            }
            if !self.settable(window, Attribute::Position, id)?
                || !self.settable(window, Attribute::Size, id)?
            {
                return Err(WindowSystemError::Platform(
                    "window is not movable/resizable".into(),
                ));
            }
            if movement.target.width == 0 || movement.target.height == 0 {
                return Err(WindowSystemError::Platform(
                    "window dimensions must be positive".into(),
                ));
            }
            let result = (|| {
                for name in [Attribute::Zoomed, Attribute::Minimized] {
                    if self.boolean(window, name, Some(id))? {
                        if !self.settable(window, name, id)? {
                            return Err(WindowSystemError::Platform(
                                "window state cannot be restored".into(),
                            ));
                        }
                        self.set(window, name, unsafe { kCFBooleanFalse }, id)?;
                    }
                }
                let position = Pair {
                    first: f64::from(movement.target.x),
                    second: f64::from(movement.target.y),
                };
                let size = Pair {
                    first: f64::from(movement.target.width),
                    second: f64::from(movement.target.height),
                };
                // SAFETY: AXValueCreate copies the matching CGPoint/CGSize layouts.
                let position =
                    Value::owned(unsafe { AXValueCreate(1, (&position as *const Pair).cast()) });
                let size = Value::owned(unsafe { AXValueCreate(2, (&size as *const Pair).cast()) });
                // Resize first to release native zoom/tiling. Growth can clamp at
                // the old origin, so complete that resize only after moving.
                self.set(window, Attribute::Size, size.raw(), id)?;
                self.set(window, Attribute::Position, position.raw(), id)?;
                let mut observed = self.frame(window, Some(id))?;
                if observed.width != movement.target.width
                    || observed.height != movement.target.height
                {
                    self.set(window, Attribute::Size, size.raw(), id)?;
                    observed = self.frame(window, Some(id))?;
                    if observed.x != movement.target.x || observed.y != movement.target.y {
                        self.set(window, Attribute::Position, position.raw(), id)?;
                        observed = self.frame(window, Some(id))?;
                    }
                }
                if observed != movement.target {
                    return Err(WindowSystemError::Platform(format!(
                        "window constrained placement: requested {:?}, observed {observed:?}",
                        movement.target
                    )));
                }
                Ok(())
            })();
            result.map_err(|error| {
                if matches!(error, WindowSystemError::WindowGone(_)) {
                    return error;
                }
                match self.frame(window, Some(id)) {
                    Ok(observed) if observed != before => WindowSystemError::PartialMove {
                        observed,
                        message: error.to_string(),
                    },
                    Err(gone @ WindowSystemError::WindowGone(_)) => gone,
                    _ => error,
                }
            })
        }
    }

    pub(super) fn displays() -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        autoreleasepool(|| {
            // SAFETY: NSScreen provides Cocoa screen rectangles in points;
            // objects stay alive in this autorelease pool.
            unsafe {
                let screens = NSScreen::screens(nil);
                let count = screens.count();
                if count == 0 {
                    return Ok(Vec::new());
                }
                let first = screens.objectAtIndex(0);
                let main_height = first.frame().size.height;
                let mut displays = Vec::with_capacity(count as usize);
                for index in 0..count {
                    let screen = screens.objectAtIndex(index);
                    let frame = screen.visibleFrame();
                    displays.push(DisplayGeometry::new(
                        format!("display-{index}"),
                        Rect::new(
                            as_i32("x", frame.origin.x)?,
                            as_i32("y", main_height - frame.origin.y - frame.size.height)?,
                            as_u32("width", frame.size.width)?,
                            as_u32("height", frame.size.height)?,
                        ),
                    ));
                }
                Ok(displays)
            }
        })
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        #[ignore = "terminates an owned fixture; needs WZ_MACOS_TEST_PID"]
        fn closed_fixture_releases_retained_window() {
            let expected_pid: i32 = std::env::var("WZ_MACOS_TEST_PID")
                .expect("set WZ_MACOS_TEST_PID to a disposable owned fixture")
                .parse()
                .expect("fixture PID");
            assert!(expected_pid > 0);
            let mut system = MacOSWindowSystem::new();
            let captured = system.focused_window().unwrap().unwrap();
            let mut actual_pid = 0;
            // SAFETY: the adapter retains this Accessibility element.
            assert_eq!(
                unsafe { AXUIElementGetPid(system.windows.borrow()[0].1.raw(), &mut actual_pid) },
                0
            );
            assert_eq!(
                actual_pid, expected_pid,
                "refusing to terminate an unowned window"
            );
            // SAFETY: the caller explicitly designated this disposable fixture.
            assert_eq!(unsafe { kill(expected_pid, 15) }, 0);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            // SAFETY: signal zero probes existence without changing the process.
            while unsafe { kill(expected_pid, 0) } == 0 {
                assert!(std::time::Instant::now() < deadline, "fixture did not exit");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(3));
            assert_eq!(
                system.move_window(&WindowMove::new(captured.id.clone(), captured.geometry)),
                Err(WindowSystemError::WindowGone(captured.id.clone()))
            );
            assert!(
                system
                    .windows
                    .borrow()
                    .iter()
                    .all(|(id, _)| id != &captured.id)
            );
        }

        #[test]
        #[ignore = "needs an owned lower-screen window and WZ_MACOS_TEST_PID"]
        fn expansion_finishes_at_target_origin() {
            let expected_pid: i32 = std::env::var("WZ_MACOS_TEST_PID")
                .expect("set WZ_MACOS_TEST_PID to the owned fixture PID")
                .parse()
                .expect("fixture PID");
            assert!(expected_pid > 0);
            let mut system = MacOSWindowSystem::new();
            let captured = system.focused_window().unwrap().unwrap();
            let mut actual_pid = 0;
            // SAFETY: the captured element is retained by this adapter.
            assert_eq!(
                unsafe { AXUIElementGetPid(system.windows.borrow()[0].1.raw(), &mut actual_pid) },
                0
            );
            assert_eq!(
                actual_pid, expected_pid,
                "refusing to move an unowned window"
            );
            let area = system.displays().unwrap()[0].usable_area;
            assert!(
                captured.geometry.y > area.y,
                "start below the target origin"
            );
            assert!(
                captured.geometry.height < area.height,
                "start with a small window"
            );
            let target = Rect::new(area.x, area.y, area.width / 2, area.height);
            system
                .move_window(&WindowMove::new(captured.id.clone(), target))
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(200));
            let observed = system.focused_window().unwrap().unwrap();
            assert_eq!(observed.id, captured.id);
            assert_eq!(observed.geometry, target);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_rounding_preserves_finite_and_range_validation() {
        assert_eq!(as_i32("x", -1.5).unwrap(), -2);
        assert_eq!(as_u32("width", 1.5).unwrap(), 2);
        assert_eq!(as_i32("x", f64::from(i32::MIN)).unwrap(), i32::MIN);
        assert_eq!(as_u32("width", f64::from(u32::MAX)).unwrap(), u32::MAX);
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(as_i32("x", invalid).is_err());
            assert!(as_u32("width", invalid).is_err());
        }
        assert!(as_i32("x", f64::from(i32::MIN) - 1.0).is_err());
        assert!(as_i32("x", f64::from(i32::MAX) + 1.0).is_err());
        assert!(as_u32("width", -1.0).is_err());
        assert!(as_u32("width", f64::from(u32::MAX) + 1.0).is_err());
    }

    #[test]
    fn unknown_window_identity_is_gone_without_accessing_another_window() {
        let mut system = MacOSWindowSystem::new();
        let id = WindowId::new("not-from-this-adapter");
        assert_eq!(
            system.move_window(&WindowMove::new(id.clone(), Rect::new(100, 100, 300, 200))),
            Err(WindowSystemError::WindowGone(id)),
        );
    }
}
