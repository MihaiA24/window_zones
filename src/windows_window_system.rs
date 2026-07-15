#[cfg(target_os = "windows")]
use crate::{DisplayGeometry, FocusedWindow, Rect, WindowMove, WindowSystem, WindowSystemError};

#[cfg(target_os = "windows")]
use std::convert::TryFrom;

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, RECT};
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST, MONITORINFOEXW,
    MonitorFromWindow,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowRect, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowPos,
};

#[cfg(target_os = "windows")]
const FALLBACK_DISPLAY_ID: &str = "display-0";

#[cfg(target_os = "windows")]
#[derive(Debug, Default)]
pub struct WindowsWindowSystem;

#[cfg(target_os = "windows")]
impl WindowsWindowSystem {
    pub fn new() -> Self {
        Self
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
            return Ok(vec![DisplayGeometry::new(
                FALLBACK_DISPLAY_ID.to_string(),
                Rect::new(0, 0, 0, 0),
            )]);
        }

        Ok(displays)
    }

    fn display_id_for_monitor(handle: HMONITOR) -> Result<String, WindowSystemError> {
        let mut info = MONITORINFOEXW {
            cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
            ..Default::default()
        };

        unsafe {
            GetMonitorInfoW(handle, &mut info).ok()?;
        }

        let len = info
            .szDevice
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(info.szDevice.len());
        if len == 0 {
            return Err(WindowSystemError::Platform(
                "monitor device name missing".to_string(),
            ));
        }

        Ok(String::from_utf16_lossy(&info.szDevice[..len]))
    }

    fn display_for_window(handle: HWND) -> Result<String, WindowSystemError> {
        let monitor = unsafe { MonitorFromWindow(handle, MONITOR_DEFAULTTONEAREST) };
        if monitor.is_invalid() {
            return Err(WindowSystemError::Platform(
                "failed to identify monitor for focused window".to_string(),
            ));
        }

        Self::display_id_for_monitor(monitor).or_else(|_| Ok(FALLBACK_DISPLAY_ID.to_string()))
    }
}

#[cfg(target_os = "windows")]
impl WindowSystem for WindowsWindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0 == 0 {
            return Ok(None);
        }

        let mut rect = RECT::default();
        unsafe {
            GetWindowRect(hwnd, &mut rect).map_err(|error| {
                WindowSystemError::Platform(format!("GetWindowRect failed: {error}"))
            })?;
        }

        let width = u32::try_from(rect.right - rect.left)
            .map_err(|_| WindowSystemError::Platform("window width out of range".to_string()))?;
        let height = u32::try_from(rect.bottom - rect.top)
            .map_err(|_| WindowSystemError::Platform("window height out of range".to_string()))?;

        let geometry = Rect::new(rect.left, rect.top, width, height);
        let display_id =
            Self::display_for_window(hwnd).unwrap_or_else(|_| FALLBACK_DISPLAY_ID.to_string());

        Ok(Some(FocusedWindow::new(display_id, geometry)))
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        Self::collect_displays()
    }

    fn move_focused_window(&mut self, window_move: WindowMove) -> Result<(), WindowSystemError> {
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.0 == 0 {
            return Err(WindowSystemError::Platform("no focused window".to_string()));
        }

        let width = i32::try_from(window_move.target.width)
            .map_err(|_| WindowSystemError::Platform("window width out of range".to_string()))?;
        let height = i32::try_from(window_move.target.height)
            .map_err(|_| WindowSystemError::Platform("window height out of range".to_string()))?;

        let moved = unsafe {
            SetWindowPos(
                hwnd,
                HWND::default(),
                window_move.target.x,
                window_move.target.y,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW,
            )
        };

        if !moved.as_bool() {
            return Err(WindowSystemError::Platform(
                "SetWindowPos failed".to_string(),
            ));
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
        cbSize: std::mem::size_of::<MONITORINFOEXW>() as u32,
        ..Default::default()
    };

    if unsafe { GetMonitorInfoW(h_monitor, &mut info) }.as_bool() {
        let len = info
            .szDevice
            .iter()
            .position(|character| *character == 0)
            .unwrap_or(info.szDevice.len());

        let id = if len == 0 {
            FALLBACK_DISPLAY_ID.to_string()
        } else {
            String::from_utf16_lossy(&info.szDevice[..len])
        };

        let area = info.rcWork;
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
