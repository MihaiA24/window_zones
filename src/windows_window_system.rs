#[cfg(target_os = "windows")]
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
use crate::{DisplayGeometry, FocusedWindow, WindowId, WindowMove, WindowSystem};
use crate::{Rect, WindowSystemError};

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MONITORINFOEXW, MonitorFromWindow,
};
#[cfg(target_os = "windows")]
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
#[cfg(target_os = "windows")]
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    SetProcessDpiAwarenessContext, SetThreadDpiAwarenessContext,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetForegroundWindow, GetWindowLongW, GetWindowRect, IsHungAppWindow, IsIconic,
    IsWindow, IsZoomed, SW_SHOWNOACTIVATE, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOZORDER,
    SWP_SHOWWINDOW, SetWindowPos, ShowWindowAsync, WS_CAPTION, WS_THICKFRAME,
};
#[cfg(target_os = "windows")]
use windows::core::{s, w};

#[cfg(target_os = "windows")]
const PLACEMENT_TIMEOUT: Duration = Duration::from_millis(500);

#[cfg(target_os = "windows")]
fn wait_for_next_observation(deadline: Instant) -> bool {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return false;
    }
    std::thread::sleep(remaining.min(Duration::from_millis(10)));
    true
}

#[cfg(target_os = "windows")]
fn window_is_arranged(hwnd: HWND) -> Option<bool> {
    type IsArranged = unsafe extern "system" fn(HWND) -> BOOL;
    static IS_ARRANGED: std::sync::LazyLock<Option<IsArranged>> = std::sync::LazyLock::new(|| {
        // IsWindowArranged only exists from Windows 10 1903. A static import
        // would prevent older Windows 10 from loading even dry-run commands.
        let module = unsafe { GetModuleHandleW(w!("user32.dll")) }.ok()?;
        let address = unsafe { GetProcAddress(module, s!("IsWindowArranged")) }?;
        Some(unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, IsArranged>(address)
        })
    });
    IS_ARRANGED.map(|query| unsafe { query(hwnd) }.as_bool())
}

#[cfg(target_os = "windows")]
struct PerMonitorDpi(DPI_AWARENESS_CONTEXT);

#[cfg(target_os = "windows")]
impl PerMonitorDpi {
    fn enter() -> Result<Self, WindowSystemError> {
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        if previous.0.is_null() {
            return Err(WindowSystemError::Platform(
                "failed to enable per-monitor-v2 DPI awareness on the Windows geometry thread"
                    .to_string(),
            ));
        }
        Ok(Self(previous))
    }
}

#[cfg(target_os = "windows")]
impl Drop for PerMonitorDpi {
    fn drop(&mut self) {
        unsafe { SetThreadDpiAwarenessContext(self.0) };
    }
}

#[cfg(target_os = "windows")]
#[derive(Debug)]
pub struct WindowsWindowSystem;

#[cfg(target_os = "windows")]
impl WindowsWindowSystem {
    pub fn new() -> Self {
        // The process default is for native UI; geometry requires its own thread context.
        let _ =
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        Self
    }

    fn window_error(hwnd: HWND, id: &WindowId, message: String) -> WindowSystemError {
        if unsafe { IsWindow(hwnd) }.as_bool() {
            WindowSystemError::Platform(message)
        } else {
            WindowSystemError::WindowGone(id.clone())
        }
    }

    fn require_responsive(hwnd: HWND, id: &WindowId) -> Result<(), WindowSystemError> {
        if unsafe { IsHungAppWindow(hwnd) }.as_bool() {
            return Err(Self::window_error(
                hwnd,
                id,
                "Windows window is not responding".to_string(),
            ));
        }
        Ok(())
    }

    fn frame_rect(hwnd: HWND, id: &WindowId, visible: bool) -> Result<Rect, WindowSystemError> {
        let mut rect = RECT::default();
        let result = unsafe {
            if visible {
                DwmGetWindowAttribute(
                    hwnd,
                    DWMWA_EXTENDED_FRAME_BOUNDS,
                    (&mut rect as *mut RECT).cast(),
                    std::mem::size_of::<RECT>() as u32,
                )
            } else {
                GetWindowRect(hwnd, &mut rect)
            }
        };
        result.map_err(|error| {
            Self::window_error(
                hwnd,
                id,
                format!(
                    "failed to query {} window frame: {error}",
                    if visible { "visible" } else { "outer" }
                ),
            )
        })?;
        let width = u32::try_from(i64::from(rect.right) - i64::from(rect.left))
            .map_err(|_| WindowSystemError::Platform("window width out of range".to_string()))?;
        let height = u32::try_from(i64::from(rect.bottom) - i64::from(rect.top))
            .map_err(|_| WindowSystemError::Platform("window height out of range".to_string()))?;
        Ok(Rect::new(rect.left, rect.top, width, height))
    }

    fn fullscreen_like(hwnd: HWND, frame: Rect) -> bool {
        let style = unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32;
        if style & (WS_CAPTION.0 | WS_THICKFRAME.0) != 0 {
            return false;
        }
        let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
            return false;
        }
        frame.x <= info.rcMonitor.left
            && frame.y <= info.rcMonitor.top
            && i64::from(frame.x) + i64::from(frame.width) >= i64::from(info.rcMonitor.right)
            && i64::from(frame.y) + i64::from(frame.height) >= i64::from(info.rcMonitor.bottom)
    }

    fn set_frame(hwnd: HWND, id: &WindowId, target: Rect) -> Result<(), WindowSystemError> {
        let width = i32::try_from(target.width)
            .map_err(|_| WindowSystemError::Platform("window width out of range".to_string()))?;
        let height = i32::try_from(target.height)
            .map_err(|_| WindowSystemError::Platform("window height out of range".to_string()))?;
        unsafe {
            SetWindowPos(
                hwnd,
                HWND::default(),
                target.x,
                target.y,
                width,
                height,
                SWP_ASYNCWINDOWPOS | SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW,
            )
        }
        .map_err(|error| Self::window_error(hwnd, id, format!("SetWindowPos failed: {error}")))
    }
}

#[cfg(target_os = "windows")]
impl Default for WindowsWindowSystem {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "windows")]
impl WindowSystem for WindowsWindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
        let _dpi = PerMonitorDpi::enter()?;
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0.is_null() {
            return Ok(None);
        }
        let id = WindowId::new((hwnd.0 as usize).to_string());
        let geometry = Self::frame_rect(hwnd, &id, true)?;
        Ok(Some(FocusedWindow::new(id, geometry)))
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        let _dpi = PerMonitorDpi::enter()?;
        let mut displays = Vec::<DisplayGeometry>::new();

        let result = unsafe {
            EnumDisplayMonitors(
                HDC::default(),
                None,
                Some(monitor_enum_callback),
                LPARAM(&mut displays as *mut _ as isize),
            )
        };

        if !result.as_bool() {
            return Err(WindowSystemError::Platform(
                "EnumDisplayMonitors failed".to_string(),
            ));
        }

        Ok(displays)
    }

    fn move_window(&mut self, window_move: &WindowMove) -> Result<(), WindowSystemError> {
        let _dpi = PerMonitorDpi::enter()?;
        let id = &window_move.window;
        let value = id.as_str().parse::<usize>().map_err(|_| {
            WindowSystemError::Platform("invalid Windows window identity".to_string())
        })?;
        let hwnd = HWND(value as *mut std::ffi::c_void);
        if !unsafe { IsWindow(hwnd) }.as_bool() {
            return Err(WindowSystemError::WindowGone(id.clone()));
        }
        Self::require_responsive(hwnd, id)?;

        let before = Self::frame_rect(hwnd, id, true)?;
        let maximized = unsafe { IsZoomed(hwnd) }.as_bool();
        let minimized = unsafe { IsIconic(hwnd) }.as_bool();
        if !maximized && !minimized && Self::fullscreen_like(hwnd, before) {
            return Err(WindowSystemError::Platform(
                "fullscreen-like window cannot be moved; leave fullscreen first".to_string(),
            ));
        }
        if unsafe { GetWindowLongW(hwnd, GWL_STYLE) } as u32 & WS_THICKFRAME.0 == 0 {
            return Err(WindowSystemError::Platform(
                "cannot move a non-resizable Windows window".to_string(),
            ));
        }
        let target = window_move.target;
        let deadline = Instant::now() + PLACEMENT_TIMEOUT;
        let result = (|| {
            // Older Windows cannot report a snap. Request a restore when the frame needs moving.
            let needs_restore =
                maximized || minimized || window_is_arranged(hwnd).unwrap_or(before != target);
            let visible = if needs_restore {
                // SW_RESTORE would activate the captured window even if focus changed.
                if !unsafe { ShowWindowAsync(hwnd, SW_SHOWNOACTIVATE) }.as_bool() {
                    return Err(Self::window_error(
                        hwnd,
                        id,
                        "ShowWindowAsync could not restore the Windows window".to_string(),
                    ));
                }
                while unsafe { IsZoomed(hwnd).as_bool() || IsIconic(hwnd).as_bool() }
                    || window_is_arranged(hwnd).unwrap_or(false)
                {
                    Self::require_responsive(hwnd, id)?;
                    if !wait_for_next_observation(deadline) {
                        return Err(WindowSystemError::Platform(
                            "Windows window did not restore before the placement deadline"
                                .to_string(),
                        ));
                    }
                }
                Self::frame_rect(hwnd, id, true)?
            } else {
                before
            };
            if visible == target || Instant::now() >= deadline {
                return Ok(visible);
            }
            let outer = Self::frame_rect(hwnd, id, false)?;
            let outer_target = compensate_invisible_borders(target, outer, visible)?;
            Self::require_responsive(hwnd, id)?;
            Self::set_frame(hwnd, id, outer_target)?;

            let mut last_frames = None;
            let mut corrected = false;
            loop {
                Self::require_responsive(hwnd, id)?;
                let observed = Self::frame_rect(hwnd, id, true)?;
                if observed == target || Instant::now() >= deadline {
                    return Ok(observed);
                }

                if corrected || observed != visible {
                    // DWM can lag the outer frame. Require two matching pairs
                    // before deciding whether the borders need correction.
                    let outer = if corrected {
                        None
                    } else {
                        Some(Self::frame_rect(hwnd, id, false)?)
                    };
                    if last_frames.replace((outer, observed)) == Some((outer, observed)) {
                        let Some(outer) = outer else {
                            return Ok(observed);
                        };
                        let adjusted = compensate_invisible_borders(target, outer, observed)?;
                        if adjusted == outer_target {
                            return Ok(observed);
                        }
                        Self::set_frame(hwnd, id, adjusted)?;
                        corrected = true;
                        last_frames = None;
                    }
                } else {
                    last_frames = None;
                }
                if !wait_for_next_observation(deadline) {
                    return Ok(observed);
                }
            }
        })();
        match result {
            Ok(observed) => verify_placement(before, target, observed),
            Err(error @ WindowSystemError::WindowGone(_)) => Err(error),
            Err(error) => Err(match Self::frame_rect(hwnd, id, true) {
                Ok(observed) if observed != before => WindowSystemError::PartialMove {
                    observed,
                    message: error.to_string(),
                },
                Err(gone @ WindowSystemError::WindowGone(_)) => gone,
                _ => error,
            }),
        }
    }
}

#[cfg(target_os = "windows")]
extern "system" fn monitor_enum_callback(
    h_monitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> windows::Win32::Foundation::BOOL {
    let displays = unsafe { &mut *(lparam.0 as *mut Vec<DisplayGeometry>) };
    let mut info = MONITORINFOEXW {
        monitorInfo: MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        },
        ..Default::default()
    };

    if unsafe { GetMonitorInfoW(h_monitor, &mut info.monitorInfo) }.as_bool() {
        let len = info
            .szDevice
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(info.szDevice.len());
        if len == 0 {
            return windows::Win32::Foundation::BOOL(0);
        }

        let id = String::from_utf16_lossy(&info.szDevice[..len]);
        let area = info.monitorInfo.rcWork;
        let width = u32::try_from(area.right - area.left).ok();
        let height = u32::try_from(area.bottom - area.top).ok();
        if let (Some(width), Some(height)) = (width, height) {
            displays.push(DisplayGeometry::new(
                id,
                Rect::new(area.left, area.top, width, height),
            ));
        }
    }

    windows::Win32::Foundation::BOOL(1)
}

/// Translate a visible-frame target into a Win32 outer-frame target.
fn compensate_invisible_borders(
    target: Rect,
    outer: Rect,
    visible: Rect,
) -> Result<Rect, WindowSystemError> {
    let x = i64::from(target.x) + i64::from(outer.x) - i64::from(visible.x);
    let y = i64::from(target.y) + i64::from(outer.y) - i64::from(visible.y);
    let width = i64::from(target.width) + i64::from(outer.width) - i64::from(visible.width);
    let height = i64::from(target.height) + i64::from(outer.height) - i64::from(visible.height);
    let error =
        || WindowSystemError::Platform("border-compensated window frame out of range".to_string());
    Ok(Rect::new(
        i32::try_from(x).map_err(|_| error())?,
        i32::try_from(y).map_err(|_| error())?,
        u32::try_from(width).map_err(|_| error())?,
        u32::try_from(height).map_err(|_| error())?,
    ))
}

fn verify_placement(before: Rect, target: Rect, observed: Rect) -> Result<(), WindowSystemError> {
    if observed == target {
        return Ok(());
    }
    let message = format!(
        "Windows window did not apply placement: requested {target:?}, observed {observed:?}"
    );
    Err(if observed != before {
        WindowSystemError::PartialMove { observed, message }
    } else {
        WindowSystemError::Platform(message)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    #[test]
    fn geometry_uses_per_monitor_dpi_and_restores_the_callers_context() {
        use windows::Win32::UI::HiDpi::{
            AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT_UNAWARE,
            GetThreadDpiAwarenessContext,
        };

        let previous = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_UNAWARE) };
        assert!(!previous.0.is_null());
        let _restore = PerMonitorDpi(previous);
        {
            let _dpi = PerMonitorDpi::enter().unwrap();
            assert!(unsafe {
                AreDpiAwarenessContextsEqual(
                    GetThreadDpiAwarenessContext(),
                    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                )
                .as_bool()
            });
        }
        assert!(unsafe {
            AreDpiAwarenessContextsEqual(
                GetThreadDpiAwarenessContext(),
                DPI_AWARENESS_CONTEXT_UNAWARE,
            )
            .as_bool()
        });
    }

    #[test]
    fn constrained_placement_reports_observed_frame_instead_of_success() {
        let before = Rect::new(120, 100, 800, 600);
        let target = Rect::new(-1920, 0, 480, 1080);
        let observed = Rect::new(-1920, 0, 640, 1080);
        assert!(matches!(
            verify_placement(before, target, observed),
            Err(WindowSystemError::PartialMove { observed: frame, .. }) if frame == observed
        ));
        assert_eq!(verify_placement(before, target, target), Ok(()));
    }

    #[test]
    fn unchanged_frame_is_refused_instead_of_reported_as_a_partial_move() {
        let before = Rect::new(120, 100, 800, 600);
        let target = Rect::new(-1920, 0, 480, 1080);
        assert!(matches!(
            verify_placement(before, target, before),
            Err(WindowSystemError::Platform(_))
        ));
    }

    #[test]
    fn compensates_asymmetric_invisible_borders_on_negative_monitor() {
        let outer = Rect::new(93, 100, 816, 607);
        let visible = Rect::new(100, 100, 800, 600);
        assert_eq!(
            compensate_invisible_borders(Rect::new(-1920, -200, 960, 1080), outer, visible)
                .unwrap(),
            Rect::new(-1927, -200, 976, 1087),
        );
    }

    #[test]
    fn borderless_frame_needs_no_compensation() {
        let frame = Rect::new(20, 30, 500, 400);
        let target = Rect::new(100, 200, 900, 700);
        assert_eq!(
            compensate_invisible_borders(target, frame, frame).unwrap(),
            target
        );
    }

    #[test]
    fn rejects_border_compensation_overflow_instead_of_wrapping() {
        let outer = Rect::new(-8, 0, 116, 100);
        let visible = Rect::new(0, 0, 100, 100);
        assert!(
            compensate_invisible_borders(Rect::new(i32::MIN, 0, 100, 100), outer, visible).is_err()
        );
        assert!(
            compensate_invisible_borders(Rect::new(0, 0, u32::MAX, 100), outer, visible).is_err()
        );
        assert!(compensate_invisible_borders(Rect::new(0, 0, 1, 1), visible, outer).is_err());
    }
}
