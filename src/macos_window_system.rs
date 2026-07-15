//! macOS adapter for `WindowSystem`.

use crate::{DisplayGeometry, FocusedWindow, Rect, WindowMove, WindowSystem, WindowSystemError};
use serde::Deserialize;
use std::convert::TryFrom;
use std::process::Command;

#[derive(Debug, Default)]
pub struct MacOSWindowSystem;

impl MacOSWindowSystem {
    pub fn new() -> Self {
        Self
    }

    fn run_osascript(script: &str) -> Result<String, WindowSystemError> {
        let output = Command::new("osascript")
            .arg("-l")
            .arg("JavaScript")
            .arg("-e")
            .arg(script)
            .output()
            .map_err(|error| {
                WindowSystemError::Platform(format!("failed to execute osascript: {error}"))
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let details = if !stderr.is_empty() {
                stderr
            } else if !stdout.is_empty() {
                stdout
            } else {
                "unknown osascript error".to_string()
            };

            return Err(WindowSystemError::Platform(format!(
                "osascript failed: {details}"
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn focused_window_payload() -> Result<FocusedWindowPayload, WindowSystemError> {
        let output = Self::run_osascript(
            r#"
            ObjC.import("Cocoa");
            var se = Application("System Events");

            function visible_display_for_point(x, y) {
                var screens = $.NSScreen.screens;
                for (var i = 0; i < screens.count; i++) {
                    var screen = screens.objectAtIndex(i);
                    var bounds = screen.frame;
                    var x0 = bounds.origin.x;
                    var y0 = bounds.origin.y;
                    var x1 = x0 + bounds.size.width;
                    var y1 = y0 + bounds.size.height;
                    if (x >= x0 && x <= x1 && y >= y0 && y <= y1) {
                        return "display-" + i;
                    }
                }
                return "display-0";
            }

            function focused_payload() {
                var procs = se.processes.whose({ frontmost: true });
                if (procs.length === 0) {
                    return { focused: false };
                }

                var windows = procs[0].windows();
                if (windows.length === 0) {
                    return { focused: false };
                }

                var window = windows[0];
                var position = window.position();
                var size = window.size();
                var cx = position[0] + size[0] / 2;
                var cy = position[1] + size[1] / 2;

                return {
                    focused: true,
                    x: position[0],
                    y: position[1],
                    width: size[0],
                    height: size[1],
                    display_id: visible_display_for_point(cx, cy),
                };
            }

            JSON.stringify(focused_payload());
            "#,
        )?;
        parse_focused_window_payload(&output)
    }

    fn displays_payload() -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        let output = Self::run_osascript(
            r#"
            ObjC.import("Cocoa");
            var out = [];
            var screens = $.NSScreen.screens;
            for (var i = 0; i < screens.count; i++) {
                var screen = screens.objectAtIndex(i);
                var bounds = screen.visibleFrame;
                out.push({
                    id: "display-" + i,
                    x: bounds.origin.x,
                    y: bounds.origin.y,
                    width: bounds.size.width,
                    height: bounds.size.height,
                });
            }
            JSON.stringify(out);
            "#,
        )?;

        parse_display_payloads(&output)
    }

    fn move_window_payload(window_move: WindowMove) -> Result<(), WindowSystemError> {
        let x = i64::from(window_move.target.x);
        let y = i64::from(window_move.target.y);
        let width = i64::from(window_move.target.width);
        let height = i64::from(window_move.target.height);

        let script = format!(
            r#"
            var se = Application("System Events");
            var procs = se.processes.whose({{ frontmost: true }});
            if (procs.length === 0) {{
                throw new Error("no focused process");
            }}

            var windows = procs[0].windows();
            if (windows.length === 0) {{
                throw new Error("no focused window");
            }}

            var window = windows[0];
            window.position = [{x}, {y}];
            window.size = [{width}, {height}];
            "ok";
            "#,
            x = x,
            y = y,
            width = width,
            height = height
        );

        Self::run_osascript(&script).map(|_| ())
    }
}

impl WindowSystem for MacOSWindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
        let payload = Self::focused_window_payload()?;
        if !payload.focused {
            return Ok(None);
        }

        if payload.display_id.is_empty() {
            return Err(WindowSystemError::Platform(
                "focused window payload is missing display identifier".to_string(),
            ));
        }

        Ok(Some(FocusedWindow::new(
            payload.display_id,
            Rect::new(
                as_i32("x", payload.x)?,
                as_i32("y", payload.y)?,
                as_u32("width", payload.width)?,
                as_u32("height", payload.height)?,
            ),
        )))
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        Self::displays_payload()
    }

    fn move_focused_window(&mut self, window_move: WindowMove) -> Result<(), WindowSystemError> {
        Self::move_window_payload(window_move)
    }
}

#[derive(Debug, Deserialize)]
struct FocusedWindowPayload {
    focused: bool,
    #[serde(default)]
    x: f64,
    #[serde(default)]
    y: f64,
    #[serde(default)]
    width: f64,
    #[serde(default)]
    height: f64,
    #[serde(default)]
    display_id: String,
}

#[derive(Debug, Deserialize)]
struct DisplayPayload {
    id: String,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn parse_focused_window_payload(raw: &str) -> Result<FocusedWindowPayload, WindowSystemError> {
    serde_json::from_str::<FocusedWindowPayload>(raw).map_err(|error| {
        WindowSystemError::Platform(format!("failed to parse focused-window payload: {error}"))
    })
}

fn parse_display_payloads(raw: &str) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
    let payload = serde_json::from_str::<Vec<DisplayPayload>>(raw).map_err(|error| {
        WindowSystemError::Platform(format!("failed to parse display payload: {error}"))
    })?;

    payload
        .into_iter()
        .map(|entry| {
            Ok(DisplayGeometry::new(
                entry.id,
                Rect::new(
                    as_i32("x", entry.x)?,
                    as_i32("y", entry.y)?,
                    as_u32("width", entry.width)?,
                    as_u32("height", entry.height)?,
                ),
            ))
        })
        .collect()
}

fn as_i32(field: &str, value: f64) -> Result<i32, WindowSystemError> {
    if !value.is_finite() {
        return Err(WindowSystemError::Platform(format!(
            "{field} is not finite: {value}"
        )));
    }

    let rounded = value.round();
    if (value - rounded).abs() > 0.5 {
        return Err(WindowSystemError::Platform(format!(
            "{field} is not an integer: {value}"
        )));
    }

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
    if (value - rounded).abs() > 0.5 {
        return Err(WindowSystemError::Platform(format!(
            "{field} is not an integer: {value}"
        )));
    }

    if rounded < 0.0 {
        return Err(WindowSystemError::Platform(format!(
            "{field} cannot be negative: {value}"
        )));
    }

    u32::try_from(rounded as i128)
        .map_err(|_| WindowSystemError::Platform(format!("{field} out of u32 range: {value}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_focused_window_payload_from_focusless_response() {
        let payload = parse_focused_window_payload(r#"{"focused":false}"#).unwrap();
        assert!(!payload.focused);
    }

    #[test]
    fn parse_focused_window_payload_rejects_invalid_payload() {
        assert!(parse_focused_window_payload("not json").is_err());
    }

    #[test]
    fn parse_display_payload_from_json() {
        let payload = parse_display_payloads(
            r#"[{"id":"display-0","x":0,"y":0,"width":1920,"height":1080}]"#,
        )
        .unwrap();
        assert_eq!(
            payload,
            vec![DisplayGeometry::new(
                "display-0",
                Rect::new(0, 0, 1920, 1080)
            )]
        );
    }
}
