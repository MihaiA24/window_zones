//! Linux Wayland adapter for `WindowSystem`.
//!
//! This adapter selects a compositor-specific implementation when Wayland support is
//! available (currently sway or Hyprland) and returns explicit diagnostics when it is
//! not.

use crate::{DisplayGeometry, FocusedWindow, Rect, WindowMove, WindowSystem, WindowSystemError};

use serde::Deserialize;
use std::collections::HashSet;
use std::env;
use std::ffi::OsString;
#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaylandBackend {
    Sway,
    Hyprland,
}

#[derive(Debug)]
pub struct WaylandWindowSystem;

impl WaylandWindowSystem {
    pub fn new() -> Self {
        Self
    }

    fn backend() -> Result<WaylandBackend, WindowSystemError> {
        resolve_wayland_backend_with_env(|name| env::var_os(name))
    }

    fn session_error() -> WindowSystemError {
        if is_wayland_session() {
            WindowSystemError::Platform(
                "Wayland adapter supports only detected Wayland compositor backends: sway (SWAYSOCK + swaymsg) or hyprland (HYPRLAND_INSTANCE_SIGNATURE/XDG_CURRENT_DESKTOP=hyprland + hyprctl)".to_string(),
            )
        } else {
            WindowSystemError::Platform(
                "Wayland adapter can only be used when XDG_SESSION_TYPE=wayland or WAYLAND_DISPLAY is set".to_string(),
            )
        }
    }
}

impl Default for WaylandWindowSystem {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowSystem for WaylandWindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
        match WaylandWindowSystem::backend()? {
            WaylandBackend::Sway => focused_window_sway(),
            WaylandBackend::Hyprland => focused_window_hypr(),
        }
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        match WaylandWindowSystem::backend()? {
            WaylandBackend::Sway => displays_sway(),
            WaylandBackend::Hyprland => displays_hypr(),
        }
    }

    fn move_focused_window(&mut self, window_move: WindowMove) -> Result<(), WindowSystemError> {
        match WaylandWindowSystem::backend()? {
            WaylandBackend::Sway => move_focused_window_sway(window_move),
            WaylandBackend::Hyprland => move_focused_window_hypr(window_move),
        }
    }
}

#[derive(Debug, Deserialize)]
struct SwayTree {
    #[serde(default)]
    nodes: Vec<SwayTreeNode>,
    #[serde(default, rename = "floating_nodes")]
    floating_nodes: Vec<SwayTreeNode>,
}

#[derive(Debug, Deserialize)]
struct SwayTreeNode {
    #[serde(rename = "type", default)]
    node_type: Option<String>,
    #[serde(default)]
    focused: bool,
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    app_id: Option<String>,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    rect: SwayRect,
    #[serde(default)]
    nodes: Vec<SwayTreeNode>,
    #[serde(default, rename = "floating_nodes")]
    floating_nodes: Vec<SwayTreeNode>,
}

#[derive(Debug, Default, Deserialize)]
struct SwayRect {
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

#[derive(Debug, Deserialize)]
struct SwayOutput {
    name: String,
    #[serde(default)]
    rect: SwayRect,
}

#[derive(Debug, Deserialize)]
struct HyprWindow {
    address: String,
    #[serde(default)]
    x: i32,
    #[serde(default)]
    y: i32,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
    #[serde(default)]
    monitor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HyprMonitor {
    name: String,
    #[serde(default)]
    x: i32,
    #[serde(default)]
    y: i32,
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
}

fn is_wayland_session() -> bool {
    is_wayland_session_with_env(|name| env::var_os(name))
}

fn is_wayland_session_with_env(get_env: impl for<'a> Fn(&'a str) -> Option<OsString>) -> bool {
    get_env("XDG_SESSION_TYPE")
        .and_then(|value| value.to_str().map(|value| value.to_ascii_lowercase()))
        .is_some_and(|value| value == "wayland")
        || get_env("WAYLAND_DISPLAY").is_some()
}

fn resolve_wayland_backend_with_env(
    get_env: impl for<'a> Fn(&'a str) -> Option<OsString>,
) -> Result<WaylandBackend, WindowSystemError> {
    if !is_wayland_session_with_env(&get_env) {
        return Err(WaylandWindowSystem::session_error());
    }

    if get_env("SWAYSOCK").is_some() {
        if command_exists("swaymsg") {
            return Ok(WaylandBackend::Sway);
        }

        return Err(WindowSystemError::Platform(
            "Detected a Sway session but 'swaymsg' is not available in PATH as an executable command. Install swaymsg and ensure it is on PATH.".to_string(),
        ));
    }

    if is_hyprland_session_with_env(&get_env) {
        if command_exists("hyprctl") {
            return Ok(WaylandBackend::Hyprland);
        }

        return Err(WindowSystemError::Platform(
            "Detected a Hyprland session but 'hyprctl' is not available in PATH as an executable command. Install hyprctl and ensure it is on PATH.".to_string(),
        ));
    }

    Err(WaylandWindowSystem::session_error())
}

fn is_hyprland_session_with_env(get_env: impl for<'a> Fn(&'a str) -> Option<OsString>) -> bool {
    get_env("HYPRLAND_INSTANCE_SIGNATURE").is_some()
        || get_env("XDG_CURRENT_DESKTOP")
            .and_then(|value| value.to_str().map(|value| value.to_ascii_lowercase()))
            .is_some_and(|value| value == "hyprland")
}

fn focused_window_sway() -> Result<Option<FocusedWindow>, WindowSystemError> {
    let tree: SwayTree = run_wayland_json_command("swaymsg", &["-t", "get_tree"], "sway get_tree")?;
    let displays = displays_sway()?;

    let Some(focused) = find_focused_sway_node(&tree.nodes)
        .or_else(|| find_focused_sway_node(&tree.floating_nodes))
    else {
        return Ok(None);
    };

    if focused.rect.width == 0 || focused.rect.height == 0 {
        return Ok(None);
    }

    let usable_display_ids: HashSet<_> =
        displays.iter().map(|display| display.id.as_str()).collect();
    let display_id = match focused.output.as_deref() {
        Some(id) if usable_display_ids.contains(id) => id.to_string(),
        Some(id) => format!("wayland-output-unmatched:{id}"),
        None => "wayland-output-unknown".to_string(),
    };
    Ok(Some(FocusedWindow::new(display_id, focused.rect.to_rect())))
}

fn displays_sway() -> Result<Vec<DisplayGeometry>, WindowSystemError> {
    let outputs: Vec<SwayOutput> =
        run_wayland_json_command("swaymsg", &["-t", "get_outputs"], "sway get_outputs")?;

    if outputs.is_empty() {
        return Err(WindowSystemError::Platform(
            "sway reported no outputs in get_outputs".to_string(),
        ));
    }

    Ok(outputs
        .into_iter()
        .map(|output| {
            DisplayGeometry::new(
                output.name,
                Rect::new(
                    output.rect.x,
                    output.rect.y,
                    output.rect.width,
                    output.rect.height,
                ),
            )
        })
        .collect())
}

fn move_focused_window_sway(window_move: WindowMove) -> Result<(), WindowSystemError> {
    let tree =
        run_wayland_json_command::<SwayTree>("swaymsg", &["-t", "get_tree"], "sway get_tree")?;
    let focused = find_focused_sway_node(&tree.nodes)
        .or_else(|| find_focused_sway_node(&tree.floating_nodes))
        .and_then(|window| window.id);

    let Some(window_id) = focused else {
        return Err(WindowSystemError::Platform("no focused window".to_string()));
    };

    run_sway_command(&format!(
        "[con_id={window_id}] move position {} {}",
        window_move.target.x, window_move.target.y
    ))?;

    run_sway_command(&format!(
        "[con_id={window_id}] resize set width {} px height {} px",
        window_move.target.width, window_move.target.height
    ))
}

fn focused_window_hypr() -> Result<Option<FocusedWindow>, WindowSystemError> {
    let Some(window) = run_wayland_json_command::<Option<HyprWindow>>(
        "hyprctl",
        &["activewindow", "-j"],
        "hyprctl activewindow",
    )?
    else {
        return Ok(None);
    };

    let geometry = Rect::new(window.x, window.y, window.width, window.height);
    if geometry.width == 0 || geometry.height == 0 {
        return Ok(None);
    }

    let displays = displays_hypr()?;
    let usable_display_ids: HashSet<_> =
        displays.iter().map(|display| display.id.as_str()).collect();
    let display_id = match window.monitor {
        Some(id) if usable_display_ids.contains(id.as_str()) => id,
        Some(id) => format!("wayland-unmatched:{id}"),
        None => "wayland-output-unknown".to_string(),
    };

    Ok(Some(FocusedWindow::new(display_id, geometry)))
}

fn displays_hypr() -> Result<Vec<DisplayGeometry>, WindowSystemError> {
    let monitors: Vec<HyprMonitor> =
        run_wayland_json_command("hyprctl", &["-j", "monitors"], "hyprctl monitors")?;

    if monitors.is_empty() {
        return Err(WindowSystemError::Platform(
            "hyprctl reported no monitors in -j monitors".to_string(),
        ));
    }

    Ok(monitors
        .into_iter()
        .map(|monitor| {
            DisplayGeometry::new(
                monitor.name,
                Rect::new(monitor.x, monitor.y, monitor.width, monitor.height),
            )
        })
        .collect())
}

fn move_focused_window_hypr(window_move: WindowMove) -> Result<(), WindowSystemError> {
    let Some(window) = run_wayland_json_command::<Option<HyprWindow>>(
        "hyprctl",
        &["activewindow", "-j"],
        "hyprctl activewindow",
    )?
    else {
        return Err(WindowSystemError::Platform("no focused window".to_string()));
    };

    run_hyprctl_window_dispatch(
        &[
            "dispatch",
            "movewindowpixel",
            &format!(
                "exact {} {},address:{}",
                window_move.target.x, window_move.target.y, window.address
            ),
        ],
        "hyprctl movewindowpixel",
    )?;

    run_hyprctl_window_dispatch(
        &[
            "dispatch",
            "resizewindowpixel",
            &format!(
                "exact {} {},address:{}",
                window_move.target.width, window_move.target.height, window.address
            ),
        ],
        "hyprctl resizewindowpixel",
    )
}

fn find_focused_sway_node(nodes: &[SwayTreeNode]) -> Option<&SwayTreeNode> {
    nodes.iter().find_map(|node| {
        if is_sway_window_node(node) {
            return Some(node);
        }

        find_focused_sway_node(&node.nodes).or_else(|| find_focused_sway_node(&node.floating_nodes))
    })
}

fn is_sway_window_node(node: &SwayTreeNode) -> bool {
    if !node.focused {
        return false;
    }

    if node.rect.width == 0 || node.rect.height == 0 {
        return false;
    }

    if matches!(
        node.node_type.as_deref(),
        Some("output") | Some("workspace")
    ) {
        return false;
    }

    node.pid.is_some() || node.app_id.is_some() || node.id.is_some()
}

fn run_sway_command(command: &str) -> Result<(), WindowSystemError> {
    run_command("swaymsg", &["-q", command], "swaymsg")
}

fn run_hyprctl_window_dispatch(command: &[&str], context: &str) -> Result<(), WindowSystemError> {
    run_command("hyprctl", command, context)
}

fn run_wayland_json_command<T>(
    command: &str,
    args: &[&str],
    context: &str,
) -> Result<T, WindowSystemError>
where
    T: for<'de> Deserialize<'de>,
{
    let output = run_command_capture(command, args, context)?;

    serde_json::from_str(&output).map_err(|error| {
        WindowSystemError::Platform(format!("{context} JSON parse failed: {error}"))
    })
}

fn run_command(command: &str, args: &[&str], context: &str) -> Result<(), WindowSystemError> {
    let status = Command::new(command).args(args).status().map_err(|error| {
        WindowSystemError::Platform(format!("failed to execute {command}: {error}"))
    })?;

    if !status.success() {
        return Err(WindowSystemError::Platform(format!(
            "{context} failed: exit status {status}"
        )));
    }

    Ok(())
}

fn run_command_capture(
    command: &str,
    args: &[&str],
    context: &str,
) -> Result<String, WindowSystemError> {
    let output = Command::new(command).args(args).output().map_err(|error| {
        WindowSystemError::Platform(format!("failed to execute {command}: {error}"))
    })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let mut message = format!("{context} failed with exit status {}", output.status);
        if !stderr.trim().is_empty() {
            message.push_str(&format!(": {stderr}"));
        }

        return Err(WindowSystemError::Platform(message));
    }

    String::from_utf8(output.stdout).map_err(|error| {
        WindowSystemError::Platform(format!("{command} returned non-utf8 output: {error}"))
    })
}

fn command_exists(command: &str) -> bool {
    if command.is_empty() {
        return false;
    }

    let Some(path) = env::var_os("PATH") else {
        return false;
    };

    env::split_paths(&path)
        .map(|entry| entry.join(command))
        .any(|candidate| {
            let Ok(metadata) = candidate.metadata() else {
                return false;
            };

            metadata.is_file() && (metadata.permissions().mode() & 0o111 != 0)
        })
}

impl SwayRect {
    fn to_rect(&self) -> Rect {
        Rect::new(self.x, self.y, self.width, self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;

    fn set_env(key: &str, value: Option<&str>) {
        unsafe {
            if let Some(value) = value {
                env::set_var(key, value);
            } else {
                env::remove_var(key);
            }
        }
    }

    fn restore_env(vars: &[(String, Option<OsString>)]) {
        for (key, value) in vars {
            unsafe {
                match value {
                    Some(value) => env::set_var(key, value),
                    None => env::remove_var(key),
                }
            }
        }
    }

    #[test]
    fn non_wayland_session_returns_session_error() {
        let backups = [
            (
                "XDG_SESSION_TYPE".to_string(),
                env::var_os("XDG_SESSION_TYPE"),
            ),
            (
                "WAYLAND_DISPLAY".to_string(),
                env::var_os("WAYLAND_DISPLAY"),
            ),
            ("SWAYSOCK".to_string(), env::var_os("SWAYSOCK")),
            (
                "HYPRLAND_INSTANCE_SIGNATURE".to_string(),
                env::var_os("HYPRLAND_INSTANCE_SIGNATURE"),
            ),
            ("PATH".to_string(), env::var_os("PATH")),
        ];

        set_env("XDG_SESSION_TYPE", Some("x11"));
        set_env("WAYLAND_DISPLAY", None);

        let error = WaylandWindowSystem::new().displays().unwrap_err();
        assert!(matches!(
            error,
            WindowSystemError::Platform(message)
                if message.contains("can only be used when XDG_SESSION_TYPE=wayland")
        ));

        restore_env(&backups);
    }

    #[test]
    fn sway_backend_is_selected_before_hyprland() {
        let backups = [
            (
                "XDG_SESSION_TYPE".to_string(),
                env::var_os("XDG_SESSION_TYPE"),
            ),
            (
                "WAYLAND_DISPLAY".to_string(),
                env::var_os("WAYLAND_DISPLAY"),
            ),
            ("SWAYSOCK".to_string(), env::var_os("SWAYSOCK")),
            (
                "HYPRLAND_INSTANCE_SIGNATURE".to_string(),
                env::var_os("HYPRLAND_INSTANCE_SIGNATURE"),
            ),
            ("PATH".to_string(), env::var_os("PATH")),
        ];

        let temp_dir =
            std::env::temp_dir().join(format!("window_zones_wayland_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let swaymsg = temp_dir.join("swaymsg");
        std::fs::write(&swaymsg, "#!/bin/sh\n").unwrap();
        let mut perms = std::fs::metadata(&swaymsg).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&swaymsg, perms).unwrap();

        set_env("XDG_SESSION_TYPE", Some("wayland"));
        set_env("SWAYSOCK", Some("/tmp/sway"));
        set_env("HYPRLAND_INSTANCE_SIGNATURE", Some("1"));
        set_env("WAYLAND_DISPLAY", Some("wayland-0"));
        set_env("PATH", Some(temp_dir.to_str().unwrap()));

        let backend = resolve_wayland_backend_with_env(|name| env::var_os(name)).unwrap();
        assert_eq!(backend, WaylandBackend::Sway);

        restore_env(&backups);
    }

    #[test]
    fn focused_window_parser_prefers_window_nodes_and_keeps_native_display_id() {
        let tree_json = r#"
        {
            "nodes": [
                {
                    "id": 1,
                    "name": "",
                    "type": "output",
                    "rect": {"x":0,"y":0,"width":1920,"height":1080},
                    "nodes": [
                        {
                            "id": 2,
                            "name": "workspace",
                            "type": "workspace",
                            "focused": false,
                            "rect": {"x":0,"y":0,"width":1920,"height":1080},
                            "nodes": [
                                {
                                    "id": 10,
                                    "name": "Alacritty",
                                    "type": "con",
                                    "app_id": "alacritty",
                                    "focused": true,
                                    "output": "DP-1",
                                    "rect": {"x": 10,"y": 10,"width": 800,"height": 600},
                                    "nodes": [],
                                    "floating_nodes": []
                                }
                            ],
                            "floating_nodes": []
                        }
                    ],
                    "floating_nodes": []
                }
            ],
            "floating_nodes": []
        }
        "#;

        let tree: SwayTree = serde_json::from_str(tree_json).unwrap();

        let focused = find_focused_sway_node(&tree.nodes)
            .or_else(|| find_focused_sway_node(&tree.floating_nodes))
            .expect("focused node");

        assert_eq!(focused.id, Some(10));
        assert_eq!(focused.output, Some("DP-1".to_string()));
    }

    #[test]
    fn parse_display_lists_returns_wayland_ids() {
        let output_json = r#"
        [
            { "name": "DP-1", "rect": {"x":0,"y":0,"width":1920,"height":1080}, "focused": true },
            { "name": "HDMI-A-1", "rect": {"x":1920,"y":0,"width":1280,"height":1024}, "focused": false }
        ]
        "#;

        let outputs: Vec<SwayOutput> = serde_json::from_str(output_json).unwrap();
        let displays: Vec<DisplayGeometry> = outputs
            .into_iter()
            .map(|output| {
                DisplayGeometry::new(
                    output.name,
                    Rect::new(
                        output.rect.x,
                        output.rect.y,
                        output.rect.width,
                        output.rect.height,
                    ),
                )
            })
            .collect();

        let ids: HashSet<_> = displays.iter().map(|display| display.id.as_str()).collect();
        assert!(ids.contains("DP-1"));
        assert!(ids.contains("HDMI-A-1"));
    }

    #[test]
    fn hypr_move_command_has_no_space_before_address_selector() {
        let window = HyprWindow {
            address: "0xfeedbeef".to_string(),
            x: 1,
            y: 2,
            width: 10,
            height: 20,
            monitor: Some("DP-1".to_string()),
        };

        let move_args = format!("exact {} {},address:{}", -123, 456, window.address);
        let resize_args = format!(
            "exact {} {},address:{}",
            window.width, window.height, window.address
        );

        assert_eq!(move_args, "exact -123 456,address:0xfeedbeef");
        assert_eq!(resize_args, "exact 10 20,address:0xfeedbeef");
    }
}
