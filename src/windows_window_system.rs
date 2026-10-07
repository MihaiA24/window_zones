#[cfg(target_os = "windows")]
use crate::{DisplayGeometry, FocusedWindow, WindowId, WindowMove, WindowSystem};
use crate::{Rect, WindowSystemError};

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{E_ACCESSDENIED, HWND, LPARAM, RECT};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Dwm::{DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFO,
    MONITORINFOEXW, MonitorFromWindow,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetForegroundWindow, GetWindowLongW, GetWindowRect, IsIconic, IsWindow,
    IsWindowArranged, IsZoomed, SW_RESTORE, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW,
    SetWindowPos, ShowWindow, WS_CAPTION, WS_THICKFRAME,
};

#[cfg(target_os = "windows")]
#[derive(Debug)]
pub struct WindowsWindowSystem {
    dpi_error: Option<String>,
}

#[cfg(target_os = "windows")]
impl WindowsWindowSystem {
    pub fn new() -> Self {
        let dpi_error =
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
                .err()
                // Windows returns access denied when a manifest or earlier call set awareness.
                .filter(|error| error.code() != E_ACCESSDENIED)
                .map(|error| format!("failed to enable per-monitor-v2 DPI awareness: {error}"));
        Self { dpi_error }
    }

    fn dpi_status(&self) -> Result<(), WindowSystemError> {
        match &self.dpi_error {
            Some(error) => Err(WindowSystemError::Platform(error.clone())),
            None => Ok(()),
        }
    }

    fn window_error(hwnd: HWND, id: &WindowId, message: String) -> WindowSystemError {
        if unsafe { IsWindow(hwnd) }.as_bool() {
            WindowSystemError::Platform(message)
        } else {
            WindowSystemError::WindowGone(id.clone())
        }
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
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW,
            )
        }
        .map_err(|error| Self::window_error(hwnd, id, format!("SetWindowPos failed: {error}")))
    }

    fn collect_displays() -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        let mut displays = Vec::<DisplayGeometry>::new();

        let mut callback_state = MonitorEnumState::new();
        let result = unsafe {
            EnumDisplayMonitors(
                HDC::default(),
                None,
                Some(monitor_enum_callback),
                LPARAM(&mut callback_state as *mut _ as isize),
            )
        };

        if !result.as_bool() {
            return Err(WindowSystemError::Platform(
                "EnumDisplayMonitors failed".to_string(),
            ));
        }

        displays.append(&mut callback_state.displays);

        if displays.is_empty() {
            return Ok(Vec::new());
        }

        Ok(displays)
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
        self.dpi_status()?;
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0.is_null() {
            return Ok(None);
        }
        let id = WindowId::new((hwnd.0 as usize).to_string());
        let geometry = Self::frame_rect(hwnd, &id, true)?;
        Ok(Some(FocusedWindow::new(id, geometry)))
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        self.dpi_status()?;
        Self::collect_displays()
    }

    fn move_window(&mut self, window_move: &WindowMove) -> Result<(), WindowSystemError> {
        self.dpi_status()?;
        let id = &window_move.window;
        let value = id.as_str().parse::<usize>().map_err(|_| {
            WindowSystemError::Platform("invalid Windows window identity".to_string())
        })?;
        let hwnd = HWND(value as *mut std::ffi::c_void);
        if !unsafe { IsWindow(hwnd) }.as_bool() {
            return Err(WindowSystemError::WindowGone(id.clone()));
        }

        let maximized = unsafe { IsZoomed(hwnd) }.as_bool();
        let minimized = unsafe { IsIconic(hwnd) }.as_bool();
        if !maximized
            && !minimized
            && Self::fullscreen_like(hwnd, Self::frame_rect(hwnd, id, true)?)
        {
            return Err(WindowSystemError::Platform(
                "fullscreen-like window cannot be moved; leave fullscreen first".to_string(),
            ));
        }
        if maximized || minimized || unsafe { IsWindowArranged(hwnd) }.as_bool() {
            let _ = unsafe { ShowWindow(hwnd, SW_RESTORE) };
        }

        let outer = Self::frame_rect(hwnd, id, false)?;
        let visible = Self::frame_rect(hwnd, id, true)?;
        let target = compensate_invisible_borders(window_move.target, outer, visible)?;
        Self::set_frame(hwnd, id, target)?;

        // Crossing DPI boundaries can change invisible borders during SetWindowPos.
        let visible = Self::frame_rect(hwnd, id, true)?;
        if visible != window_move.target {
            let outer = Self::frame_rect(hwnd, id, false)?;
            let target = compensate_invisible_borders(window_move.target, outer, visible)?;
            Self::set_frame(hwnd, id, target)?;
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
struct MonitorEnumState {
    displays: Vec<DisplayGeometry>,
}

#[cfg(target_os = "windows")]
impl MonitorEnumState {
    fn new() -> Self {
        Self {
            displays: Vec::new(),
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
    let state = unsafe { &mut *(lparam.0 as *mut MonitorEnumState) };
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
            state.displays.push(DisplayGeometry::new(
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

#[cfg(test)]
mod tests {
    use super::*;

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
