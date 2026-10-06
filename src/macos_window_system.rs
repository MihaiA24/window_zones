//! macOS adapter for `WindowSystem`.
//!
//! System Events does not expose a stable native window handle. Identities are
//! `<pid>:name:<title>` for uniquely titled windows, otherwise `<pid>:index:<n>`
//! (one-based). Titles can change and unnamed/duplicate windows can reorder;
//! these identities cannot distinguish a closed window from its replacement.
//! AXFullScreen is rejected; AXZoomed and AXMinimized are cleared when exposed.
//! Apps that do not expose AXZoomed cannot have their zoom state restored here.

use crate::{
    DisplayGeometry, FocusedWindow, Rect, WindowId, WindowMove, WindowSystem, WindowSystemError,
};
use serde::Deserialize;
use std::convert::TryFrom;
use std::io::{self, Read};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

const OSASCRIPT_TIMEOUT: Duration = Duration::from_secs(2);
const ACCESSIBILITY_HINT: &str =
    "allow Window Zones (or its terminal) in System Settings > Privacy & Security > Accessibility";

#[derive(Debug, Default)]
pub struct MacOSWindowSystem;

impl MacOSWindowSystem {
    pub fn new() -> Self {
        Self
    }

    fn run_osascript(script: &str) -> Result<String, WindowSystemError> {
        let mut command = Command::new("osascript");
        command.arg("-l").arg("JavaScript").arg("-e").arg(script);
        let output = run_command_with_timeout(&mut command, OSASCRIPT_TIMEOUT)?;

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
                "osascript failed: {details}; {ACCESSIBILITY_HINT}"
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn focused_window_payload() -> Result<FocusedWindowPayload, WindowSystemError> {
        let output = Self::run_osascript(
            r#"
            var se = Application("System Events");

            function focused_payload() {
                var procs = se.processes.whose({ frontmost: true })();
                if (procs.length === 0) {
                    return { focused: false };
                }

                var process = procs[0];
                var windows = process.windows();
                if (windows.length === 0) {
                    return { focused: false };
                }

                var window = windows[0];
                var name = window.name();
                var uniqueName = typeof name === "string" && name.length > 0 &&
                    windows.filter(function(candidate) { return candidate.name() === name; }).length === 1;
                var position = window.position();
                var size = window.size();
                return {
                    focused: true,
                    pid: process.unixId(),
                    window_name: uniqueName ? name : null,
                    window_index: uniqueName ? null : 1,
                    x: position[0],
                    y: position[1],
                    width: size[0],
                    height: size[1],
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
            var mainScreenFrameHeight = screens.objectAtIndex(0).frame.size.height;
            for (var i = 0; i < screens.count; i++) {
                var screen = screens.objectAtIndex(i);
                var bounds = screen.visibleFrame;
                out.push({
                    id: "display-" + i,
                    x: bounds.origin.x,
                    y: mainScreenFrameHeight - (bounds.origin.y + bounds.size.height),
                    width: bounds.size.width,
                    height: bounds.size.height,
                });
            }
            JSON.stringify(out);
            "#,
        )?;

        parse_display_payloads(&output)
    }

    fn move_window_payload(window_move: &WindowMove) -> Result<(), WindowSystemError> {
        let script = move_script(window_move)?;
        let output = Self::run_osascript(&script)?;
        match output.as_str() {
            "gone" => Err(WindowSystemError::WindowGone(window_move.window.clone())),
            "fullscreen" => Err(WindowSystemError::Platform(
                "fullscreen window cannot be moved; leave fullscreen first".to_string(),
            )),
            "ok" => Ok(()),
            _ => Err(WindowSystemError::Platform(format!(
                "unexpected osascript move response: {output}"
            ))),
        }
    }
}

impl WindowSystem for MacOSWindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
        let payload = Self::focused_window_payload()?;
        if !payload.focused {
            return Ok(None);
        }

        Ok(Some(FocusedWindow::new(
            focused_identity(&payload)?,
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

    fn move_window(&mut self, window_move: &WindowMove) -> Result<(), WindowSystemError> {
        Self::move_window_payload(window_move)
    }
}

#[derive(Debug, Deserialize)]
struct FocusedWindowPayload {
    focused: bool,
    pid: Option<u32>,
    window_name: Option<String>,
    window_index: Option<usize>,
    #[serde(default)]
    x: f64,
    #[serde(default)]
    y: f64,
    #[serde(default)]
    width: f64,
    #[serde(default)]
    height: f64,
}

fn focused_identity(payload: &FocusedWindowPayload) -> Result<WindowId, WindowSystemError> {
    let pid = payload.pid.filter(|pid| *pid > 0).ok_or_else(|| {
        WindowSystemError::Platform("focused-window payload has no valid process id".to_string())
    })?;
    if let Some(name) = &payload.window_name
        && !name.is_empty()
    {
        return Ok(WindowId::new(format!("{pid}:name:{name}")));
    }
    let index = payload
        .window_index
        .filter(|index| *index > 0)
        .ok_or_else(|| {
            WindowSystemError::Platform("focused-window payload has no window selector".to_string())
        })?;
    Ok(WindowId::new(format!("{pid}:index:{index}")))
}

fn move_script(window_move: &WindowMove) -> Result<String, WindowSystemError> {
    let invalid_id = || WindowSystemError::Platform("invalid macOS window identity".to_string());
    let (pid, selector) = window_move
        .window
        .as_str()
        .split_once(':')
        .ok_or_else(invalid_id)?;
    let pid = pid
        .parse::<u32>()
        .ok()
        .filter(|pid| *pid > 0)
        .ok_or_else(invalid_id)?;
    let selector = if let Some(name) = selector.strip_prefix("name:") {
        if name.is_empty() {
            return Err(invalid_id());
        }
        let name = serde_json::to_string(name)
            .map_err(|error| {
                WindowSystemError::Platform(format!("failed to encode window name: {error}"))
            })?
            .replace('\u{2028}', "\\u2028")
            .replace('\u{2029}', "\\u2029");
        format!(
            "var matches = windows.filter(function(candidate) {{ return candidate.name() === {name}; }});\n\
             if (matches.length !== 1) return \"gone\";\n\
             window = matches[0];"
        )
    } else if let Some(index) = selector.strip_prefix("index:") {
        let index = index
            .parse::<usize>()
            .ok()
            .filter(|index| *index > 0)
            .ok_or_else(invalid_id)?;
        format!(
            "if (windows.length < {index}) return \"gone\";\nwindow = windows[{}];",
            index - 1
        )
    } else {
        return Err(invalid_id());
    };
    let Rect {
        x,
        y,
        width,
        height,
    } = window_move.target;
    Ok(format!(
        r#"
        var se = Application("System Events");
        function move_payload() {{
            var window = null;
            try {{
                var procs = se.processes.whose({{ unixId: {pid} }})();
                if (procs.length === 0) return "gone";
                var windows = procs[0].windows();
                {selector}
                var fullscreen = window.attributes.byName("AXFullScreen");
                if (fullscreen.exists() && fullscreen.value()) return "fullscreen";
                ["AXZoomed", "AXMinimized"].forEach(function(name) {{
                    var attribute = window.attributes.byName(name);
                    if (attribute.exists() && attribute.value()) {{
                        if (!attribute.settable()) {{
                            throw new Error(name + " cannot be restored; restore the window first");
                        }}
                        attribute.value = false;
                    }}
                }});
                window.position = [{x}, {y}];
                window.size = [{width}, {height}];
                return "ok";
            }} catch (error) {{
                if (se.processes.whose({{ unixId: {pid} }})().length === 0 ||
                    (window !== null && !window.exists())) return "gone";
                throw error;
            }}
        }}
        move_payload();
        "#,
    ))
}

fn run_command_with_timeout(
    command: &mut Command,
    timeout: Duration,
) -> Result<Output, WindowSystemError> {
    let deadline = Instant::now() + timeout;
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            WindowSystemError::Platform(format!(
                "failed to execute osascript: {error}; {ACCESSIBILITY_HINT}"
            ))
        })?;
    let result = (|| {
        let stdout = child.stdout.take().ok_or_else(|| {
            WindowSystemError::Platform("osascript stdout was not piped".to_string())
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            WindowSystemError::Platform("osascript stderr was not piped".to_string())
        })?;
        // Drain both pipes concurrently: a full pipe must not stall the child.
        let stdout_rx = read_pipe(stdout)?;
        let stderr_rx = read_pipe(stderr)?;
        let mut status = None;
        let mut stdout = None;
        let mut stderr = None;
        loop {
            if status.is_none() {
                status = child.try_wait().map_err(|error| {
                    WindowSystemError::Platform(format!(
                        "failed to wait for osascript: {error}; {ACCESSIBILITY_HINT}"
                    ))
                })?;
            }
            receive_pipe(&stdout_rx, &mut stdout)?;
            receive_pipe(&stderr_rx, &mut stderr)?;
            if let Some(status) = status
                && stdout.is_some()
                && stderr.is_some()
            {
                return Ok(Output {
                    status,
                    stdout: stdout.unwrap(),
                    stderr: stderr.unwrap(),
                });
            }
            if Instant::now() >= deadline {
                return Err(WindowSystemError::Platform(format!(
                    "osascript timed out after {} ms; {ACCESSIBILITY_HINT}",
                    timeout.as_millis(),
                )));
            }
            thread::sleep(Duration::from_millis(10));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn read_pipe(
    mut pipe: impl Read + Send + 'static,
) -> Result<Receiver<io::Result<Vec<u8>>>, WindowSystemError> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("window-zones-osascript-output".to_string())
        .spawn(move || {
            let mut bytes = Vec::new();
            let result = pipe.read_to_end(&mut bytes).map(|_| bytes);
            let _ = tx.send(result);
        })
        .map_err(|error| {
            WindowSystemError::Platform(format!(
                "failed to read osascript output: {error}; {ACCESSIBILITY_HINT}"
            ))
        })?;
    Ok(rx)
}

fn receive_pipe(
    rx: &Receiver<io::Result<Vec<u8>>>,
    bytes: &mut Option<Vec<u8>>,
) -> Result<(), WindowSystemError> {
    if bytes.is_none() {
        match rx.try_recv() {
            Ok(result) => {
                *bytes = Some(result.map_err(|error| {
                    WindowSystemError::Platform(format!(
                        "failed to read osascript output: {error}; {ACCESSIBILITY_HINT}"
                    ))
                })?)
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                return Err(WindowSystemError::Platform(format!(
                    "osascript output reader stopped; {ACCESSIBILITY_HINT}"
                )));
            }
        }
    }
    Ok(())
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

    #[test]
    fn focused_identity_uses_process_and_unique_name_or_index() {
        let named = parse_focused_window_payload(
            r#"{"focused":true,"pid":42,"window_name":"document: one","x":0,"y":0,"width":10,"height":20}"#,
        ).unwrap();
        assert_eq!(
            focused_identity(&named).unwrap().as_str(),
            "42:name:document: one"
        );
        let unnamed =
            parse_focused_window_payload(r#"{"focused":true,"pid":42,"window_index":1}"#).unwrap();
        assert_eq!(focused_identity(&unnamed).unwrap().as_str(), "42:index:1");
        assert!(
            focused_identity(&parse_focused_window_payload(r#"{"focused":true}"#).unwrap())
                .is_err()
        );
    }

    #[test]
    fn move_script_targets_original_process_and_escapes_window_name() {
        let title = "a\"; throw new Error('injected'); //\\\n\u{2028}";
        let movement = WindowMove::new(
            WindowId::new(format!("42:name:{title}")),
            Rect::new(-100, 20, 300, 400),
        );
        let script = move_script(&movement).unwrap();
        assert!(script.contains("unixId: 42"));
        assert!(!script.contains("frontmost"));
        assert!(
            script.contains(
                &serde_json::to_string(title)
                    .unwrap()
                    .replace('\u{2028}', "\\u2028")
            )
        );
        assert!(!script.contains(title));
        assert!(script.contains("window.position = [-100, 20]"));
        assert!(script.contains("AXFullScreen"));
        assert!(script.contains("AXZoomed"));
    }

    #[test]
    fn move_script_rejects_invalid_or_injected_identity() {
        for id in [
            "0:index:1",
            "42:index:0",
            "42:index:1;quit()",
            "42:name:",
            "evil:index:1",
        ] {
            assert!(
                move_script(&WindowMove::new(WindowId::new(id), Rect::new(0, 0, 1, 1))).is_err()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn command_deadline_kills_a_hung_child_without_accessing_desktop() {
        let mut command = Command::new("sleep");
        command.arg("5");
        let started = Instant::now();
        let error = run_command_with_timeout(&mut command, Duration::from_millis(40)).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(matches!(error, WindowSystemError::Platform(message)
            if message.contains("timed out") && message.contains("Accessibility")));
    }

    #[cfg(unix)]
    #[test]
    fn command_deadline_drains_output_larger_than_a_pipe_buffer() {
        let mut command = Command::new("printf");
        command.args(["%070000d", "0"]);
        let output = run_command_with_timeout(&mut command, Duration::from_secs(2)).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 70_000);
        assert!(output.stdout.iter().all(|byte| *byte == b'0'));
    }
}
