//! Linux Wayland adapter for `WindowSystem`.
//!
//! This adapter selects a compositor-specific implementation when Wayland support is
//! available (currently GNOME, KDE Plasma, sway, or Hyprland) and returns explicit
//! diagnostics when it is not.

use crate::{DisplayGeometry, FocusedWindow, Rect, WindowMove, WindowSystem, WindowSystemError};

use serde::Deserialize;
use std::env;
use std::ffi::OsString;
#[cfg(target_os = "linux")]
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaylandBackend {
    Gnome,
    Kde,
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
        resolve_wayland_backend()
    }

    fn session_error_for(is_wayland: bool) -> WindowSystemError {
        if is_wayland {
            WindowSystemError::Platform(
                "Wayland compositor is unknown or conflicting. Supported signals identify GNOME (GNOME desktop variables), KDE Plasma (KDE_FULL_SESSION/KDE_SESSION_VERSION or KDE desktop variables), Sway (SWAYSOCK and swaymsg), or Hyprland (HYPRLAND_INSTANCE_SIGNATURE/XDG_CURRENT_DESKTOP=hyprland and hyprctl).".to_string(),
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
            WaylandBackend::Gnome => Err(gnome_backend_error()),
            WaylandBackend::Kde => Err(kde_backend_error()),
            WaylandBackend::Sway => focused_window_sway(),
            WaylandBackend::Hyprland => focused_window_hypr(),
        }
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        match WaylandWindowSystem::backend()? {
            WaylandBackend::Gnome => Err(gnome_backend_error()),
            WaylandBackend::Kde => Err(kde_backend_error()),
            WaylandBackend::Sway => displays_sway(),
            WaylandBackend::Hyprland => displays_hypr(),
        }
    }

    fn move_focused_window(&mut self, window_move: WindowMove) -> Result<(), WindowSystemError> {
        match WaylandWindowSystem::backend()? {
            WaylandBackend::Gnome => Err(gnome_backend_error()),
            WaylandBackend::Kde => Err(kde_backend_error()),
            WaylandBackend::Sway => move_focused_window_sway(window_move),
            WaylandBackend::Hyprland => move_focused_window_hypr(window_move),
        }
    }
}

fn gnome_backend_error() -> WindowSystemError {
    WindowSystemError::Platform(
        "GNOME Wayland requires the org.window_zones.Gnome companion integration".to_string(),
    )
}

fn kde_backend_error() -> WindowSystemError {
    WindowSystemError::Platform(
        "KDE Plasma Wayland requires the org.window_zones.KWin companion integration".to_string(),
    )
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
    active: bool,
    rect: SwayRect,
}

#[derive(Debug, Deserialize)]
struct SwayWorkspace {
    output: String,
    visible: bool,
    rect: SwayRect,
}

#[derive(Debug, Deserialize)]
struct HyprWindow {
    address: String,
    at: [i32; 2],
    size: [u32; 2],
    #[serde(rename = "monitor")]
    _monitor: i64,
}

#[derive(Debug, Deserialize)]
struct HyprMonitor {
    name: String,
    #[serde(rename = "id")]
    _id: i64,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale: f64,
    transform: u32,
    reserved: [u32; 4],
    disabled: bool,
}

pub fn resolve_wayland_backend() -> Result<WaylandBackend, WindowSystemError> {
    resolve_wayland_backend_with_env(|name| env::var_os(name))
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
    let is_wayland = is_wayland_session_with_env(&get_env);
    if !is_wayland {
        return Err(WaylandWindowSystem::session_error_for(is_wayland));
    }

    let gnome = is_gnome_session_with_env(&get_env);
    let kde = is_kde_session_with_env(&get_env);
    let sway = is_sway_session_with_env(&get_env);
    let hyprland = is_hyprland_session_with_env(&get_env);
    let compositor_count = [gnome, kde, sway, hyprland]
        .into_iter()
        .filter(|detected| *detected)
        .count();

    if compositor_count > 1 {
        return Err(WindowSystemError::Platform(
            "conflicting Wayland compositor signals; keep only one of GNOME, KDE Plasma, Sway, or Hyprland session identities".to_string(),
        ));
    }

    if gnome {
        return Ok(WaylandBackend::Gnome);
    }

    if kde {
        return Ok(WaylandBackend::Kde);
    }

    if sway {
        if command_exists("swaymsg") {
            return Ok(WaylandBackend::Sway);
        }
        return Err(WindowSystemError::Platform(
            "Detected a Sway session but 'swaymsg' is not available in PATH as an executable command. Install swaymsg and ensure it is on PATH.".to_string(),
        ));
    }

    if hyprland {
        if command_exists("hyprctl") {
            return Ok(WaylandBackend::Hyprland);
        }

        return Err(WindowSystemError::Platform(
            "Detected a Hyprland session but 'hyprctl' is not available in PATH as an executable command. Install hyprctl and ensure it is on PATH.".to_string(),
        ));
    }

    Err(WaylandWindowSystem::session_error_for(is_wayland))
}

fn is_gnome_session_with_env(get_env: impl for<'a> Fn(&'a str) -> Option<OsString>) -> bool {
    get_env("GNOME_DESKTOP_SESSION_ID").is_some() || desktop_signal_matches(&get_env, "gnome")
}

fn is_kde_session_with_env(get_env: impl for<'a> Fn(&'a str) -> Option<OsString>) -> bool {
    get_env("KDE_FULL_SESSION").is_some()
        || get_env("KDE_SESSION_VERSION").is_some()
        || desktop_signal_matches(&get_env, "kde")
        || desktop_signal_matches(&get_env, "plasma")
}

fn is_sway_session_with_env(get_env: impl for<'a> Fn(&'a str) -> Option<OsString>) -> bool {
    get_env("SWAYSOCK").is_some() || desktop_signal_matches(&get_env, "sway")
}

fn is_hyprland_session_with_env(get_env: impl for<'a> Fn(&'a str) -> Option<OsString>) -> bool {
    get_env("HYPRLAND_INSTANCE_SIGNATURE").is_some() || desktop_signal_matches(&get_env, "hyprland")
}

fn desktop_signal_matches(
    get_env: &impl for<'a> Fn(&'a str) -> Option<OsString>,
    expected: &str,
) -> bool {
    [
        "XDG_CURRENT_DESKTOP",
        "XDG_SESSION_DESKTOP",
        "DESKTOP_SESSION",
    ]
    .into_iter()
    .filter_map(get_env)
    .filter_map(|value| value.into_string().ok())
    .flat_map(|value| {
        value
            .split([':', ',', ';'])
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>()
    })
    .any(|token| token == expected || token.starts_with(&format!("{expected}-")))
}

fn focused_window_sway() -> Result<Option<FocusedWindow>, WindowSystemError> {
    let tree: SwayTree = run_wayland_json_command("swaymsg", &["-t", "get_tree"], "sway get_tree")?;

    let Some(focused) = find_focused_sway_node(&tree.nodes)
        .or_else(|| find_focused_sway_node(&tree.floating_nodes))
    else {
        return Ok(None);
    };

    if focused.rect.width == 0 || focused.rect.height == 0 {
        return Ok(None);
    }

    Ok(Some(FocusedWindow::new(focused.rect.to_rect())))
}

fn displays_sway() -> Result<Vec<DisplayGeometry>, WindowSystemError> {
    let outputs: Vec<SwayOutput> =
        run_wayland_json_command("swaymsg", &["-t", "get_outputs"], "sway get_outputs")?;
    let workspaces: Vec<SwayWorkspace> =
        run_wayland_json_command("swaymsg", &["-t", "get_workspaces"], "sway get_workspaces")?;

    if outputs.is_empty() {
        return Err(WindowSystemError::Platform(
            "sway reported no outputs in get_outputs".to_string(),
        ));
    }

    Ok(sway_displays(outputs, &workspaces))
}

fn sway_displays(outputs: Vec<SwayOutput>, workspaces: &[SwayWorkspace]) -> Vec<DisplayGeometry> {
    outputs
        .into_iter()
        .filter(|output| output.active)
        .map(|output| {
            let usable_area = workspaces
                .iter()
                .find(|workspace| workspace.visible && workspace.output == output.name)
                .map_or(&output.rect, |workspace| &workspace.rect)
                .to_rect();
            DisplayGeometry::new(output.name, usable_area)
        })
        .collect()
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
    let Some(window) = hypr_activewindow()? else {
        return Ok(None);
    };

    let geometry = window.geometry();
    if geometry.width == 0 || geometry.height == 0 {
        return Ok(None);
    }

    Ok(Some(FocusedWindow::new(geometry)))
}

fn hypr_activewindow() -> Result<Option<HyprWindow>, WindowSystemError> {
    let output = run_command_capture("hyprctl", &["activewindow", "-j"], "hyprctl activewindow")?;
    parse_hypr_activewindow(&output)
}

fn parse_hypr_activewindow(output: &str) -> Result<Option<HyprWindow>, WindowSystemError> {
    if output
        .bytes()
        .filter(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        .eq(b"{}".iter().copied())
    {
        return Ok(None);
    }
    serde_json::from_str(output).map_err(|error| {
        WindowSystemError::Platform(format!("hyprctl activewindow JSON parse failed: {error}"))
    })
}

fn displays_hypr() -> Result<Vec<DisplayGeometry>, WindowSystemError> {
    let monitors: Vec<HyprMonitor> =
        run_wayland_json_command("hyprctl", &["-j", "monitors"], "hyprctl monitors")?;

    if monitors.is_empty() {
        return Err(WindowSystemError::Platform(
            "hyprctl reported no monitors in -j monitors".to_string(),
        ));
    }

    monitors
        .into_iter()
        .filter(|monitor| !monitor.disabled)
        .map(|monitor| {
            let usable_area = monitor.usable_area()?;
            Ok(DisplayGeometry::new(monitor.name, usable_area))
        })
        .collect()
}

fn move_focused_window_hypr(window_move: WindowMove) -> Result<(), WindowSystemError> {
    let Some(window) = hypr_activewindow()? else {
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

impl HyprWindow {
    fn geometry(&self) -> Rect {
        Rect::new(self.at[0], self.at[1], self.size[0], self.size[1])
    }
}

impl HyprMonitor {
    fn usable_area(&self) -> Result<Rect, WindowSystemError> {
        if !self.scale.is_finite() || self.scale <= 0.0 {
            return Err(WindowSystemError::Platform(format!(
                "hyprctl monitor {} has invalid scale {}",
                self.name, self.scale
            )));
        }
        let mut width = (f64::from(self.width) / self.scale).round() as u32;
        let mut height = (f64::from(self.height) / self.scale).round() as u32;
        if matches!(self.transform, 1 | 3 | 5 | 7) {
            std::mem::swap(&mut width, &mut height);
        }
        // HyprCtl.cpp emits top-left x/y followed by bottom-right x/y.
        let [left, top, right, bottom] = self.reserved;
        Ok(Rect::new(
            self.x.saturating_add_unsigned(left),
            self.y.saturating_add_unsigned(top),
            width.saturating_sub(left).saturating_sub(right),
            height.saturating_sub(top).saturating_sub(bottom),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::ffi::OsString;

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
    fn kde_backend_is_selected_from_plasma_signals_even_with_xwayland_display() {
        let values = [
            ("XDG_SESSION_TYPE", "wayland"),
            ("XDG_CURRENT_DESKTOP", "KDE"),
            ("KDE_SESSION_VERSION", "6"),
            ("DISPLAY", ":0"),
        ];
        let backend = resolve_wayland_backend_with_env(|name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        })
        .unwrap();

        assert_eq!(backend, WaylandBackend::Kde);
    }

    #[test]
    fn kde_and_gnome_signals_are_rejected_as_conflicting() {
        let values = [
            ("XDG_SESSION_TYPE", "wayland"),
            ("XDG_CURRENT_DESKTOP", "KDE:GNOME"),
            ("KDE_SESSION_VERSION", "6"),
        ];
        let error = resolve_wayland_backend_with_env(|name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        })
        .unwrap_err();

        assert!(matches!(
            error,
            WindowSystemError::Platform(message)
                if message.contains("conflicting Wayland compositor signals")
        ));
    }

    #[test]
    fn gnome_backend_is_selected_from_desktop_signal() {
        let values = [
            ("XDG_SESSION_TYPE", "wayland"),
            ("XDG_CURRENT_DESKTOP", "GNOME"),
        ];
        let backend = resolve_wayland_backend_with_env(|name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        })
        .unwrap();

        assert_eq!(backend, WaylandBackend::Gnome);
    }

    #[test]
    fn conflicting_compositor_signals_are_rejected() {
        let values = [
            ("XDG_SESSION_TYPE", "wayland"),
            ("SWAYSOCK", "/tmp/sway"),
            ("HYPRLAND_INSTANCE_SIGNATURE", "instance"),
        ];
        let error = resolve_wayland_backend_with_env(|name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        })
        .unwrap_err();

        assert!(matches!(
            error,
            WindowSystemError::Platform(message)
                if message.contains("conflicting Wayland compositor signals")
        ));
    }

    #[test]
    fn unknown_wayland_compositor_is_rejected() {
        let values = [("XDG_SESSION_TYPE", "wayland")];
        let error = resolve_wayland_backend_with_env(|name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        })
        .unwrap_err();

        assert!(matches!(
            error,
            WindowSystemError::Platform(message)
                if message.contains("unknown or conflicting")
        ));
    }

    #[test]
    fn focused_window_parser_finds_window_geometry_below_output_and_workspace_nodes() {
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
        assert_eq!(focused.rect.to_rect(), Rect::new(10, 10, 800, 600));
    }

    #[test]
    fn sway_displays_use_visible_workspaces_and_skip_inactive_outputs() {
        let outputs: Vec<SwayOutput> = serde_json::from_str(
            r#"
        [
            {"name":"eDP-1","active":true,"rect":{"x":0,"y":0,"width":1920,"height":1080}},
            {"name":"DP-1","active":true,"rect":{"x":1920,"y":0,"width":1280,"height":1024}},
            {"name":"HDMI-A-1","active":false,"rect":{"x":0,"y":0,"width":0,"height":0}}
        ]
        "#,
        )
        .unwrap();
        let workspaces: Vec<SwayWorkspace> = serde_json::from_str(
            r#"
        [
            {"output":"eDP-1","visible":false,"rect":{"x":0,"y":0,"width":1920,"height":1080}},
            {"output":"eDP-1","visible":true,"rect":{"x":0,"y":23,"width":1920,"height":1057}}
        ]
        "#,
        )
        .unwrap();

        assert_eq!(
            sway_displays(outputs, &workspaces),
            vec![
                DisplayGeometry::new("eDP-1", Rect::new(0, 23, 1920, 1057)),
                DisplayGeometry::new("DP-1", Rect::new(1920, 0, 1280, 1024)),
            ]
        );
    }

    #[test]
    fn hypr_activewindow_parser_reads_real_geometry_and_rejects_missing_size() {
        // https://github.com/hyprwm/Hyprland/discussions/14292#discussioncomment-16833698
        let json = r#"{
    "address": "0x55e0ed70afc0",
    "mapped": true,
    "hidden": false,
    "visible": true,
    "acceptsInput": true,
    "at": [1320, 680],
    "size": [600, 400],
    "workspace": {
        "id": 4,
        "name": "em₄"
    },
    "floating": true,
    "monitor": 0,
    "class": "X1F",
    "title": "vladimir@theor:~",
    "initialClass": "X1F",
    "initialTitle": "Alacritty",
    "pid": 111214,
    "xwayland": false,
    "pinned": false,
    "fullscreen": 0,
    "fullscreenClient": 0,
    "overFullscreen": true,
    "grouped": [],
    "tags": [],
    "swallowing": "0x0",
    "focusHistoryID": 0,
    "inhibitingIdle": false,
    "xdgTag": "",
    "xdgDescription": "",
    "contentType": "none",
    "stableId": "18000017"
}"#;
        let window = parse_hypr_activewindow(json).unwrap().unwrap();
        assert_eq!(window.geometry(), Rect::new(1320, 680, 600, 400));
        let mut missing_size: serde_json::Value = serde_json::from_str(json).unwrap();
        missing_size.as_object_mut().unwrap().remove("size");
        assert!(parse_hypr_activewindow(&missing_size.to_string()).is_err());
        assert!(parse_hypr_activewindow("{}").unwrap().is_none());
        assert!(parse_hypr_activewindow(" { \n\t} ").unwrap().is_none());
        assert!(parse_hypr_activewindow(r#"{"address": "0x1"}"#).is_err());
    }

    #[test]
    fn hypr_monitor_parser_reads_real_fields_and_subtracts_reserved_edges() {
        // https://github.com/hyprwm/Hyprland/pull/12019
        let json = r#"[{
    "id": 0,
    "name": "WAYLAND-1",
    "description": "",
    "make": "",
    "model": "",
    "serial": "",
    "width": 1756,
    "height": 1542,
    "physicalWidth": 0,
    "physicalHeight": 0,
    "refreshRate": 60.00000,
    "x": 0,
    "y": 0,
    "activeWorkspace": {
        "id": 1,
        "name": "1"
    },
    "specialWorkspace": {
        "id": 0,
        "name": ""
    },
    "reserved": [0, 0, 0, 0],
    "scale": 2.00,
    "transform": 0,
    "focused": true,
    "dpmsStatus": true,
    "vrr": false,
    "solitary": "0",
    "solitaryBlockedBy": ["WINDOWED","CANDIDATE"],
    "activelyTearing": false,
    "tearingBlockedBy": ["NOT_TORN","USER","SUPPORT","CANDIDATE"],
    "directScanoutTo": "0",
    "directScanoutBlockedBy": ["USER","CANDIDATE"],
    "disabled": false,
    "currentFormat": "XRGB8888",
    "mirrorOf": "none",
    "availableModes": [],
    "colorManagementPreset": "srgb",
    "sdrBrightness": 1.00,
    "sdrSaturation": 1.00,
    "sdrMinLuminance": 0.20,
    "sdrMaxLuminance": 80
}]"#;
        let mut monitors: Vec<HyprMonitor> = serde_json::from_str(json).unwrap();
        let monitor = &mut monitors[0];
        assert_eq!(monitor.usable_area().unwrap(), Rect::new(0, 0, 878, 771));
        monitor.reserved = [5, 23, 7, 11];
        assert_eq!(monitor.usable_area().unwrap(), Rect::new(5, 23, 866, 737));
        let rotated: HyprMonitor = serde_json::from_str(
            r#"{
            "id": 1, "name": "DP-1", "x": -720, "y": 30,
            "width": 2560, "height": 1440, "scale": 2.0, "transform": 3,
            "reserved": [5, 23, 7, 11], "disabled": false
        }"#,
        )
        .unwrap();
        assert_eq!(
            rotated.usable_area().unwrap(),
            Rect::new(-715, 53, 708, 1246)
        );
        monitor.scale = 0.0;
        assert!(monitor.usable_area().is_err());
        let mut missing_width: serde_json::Value = serde_json::from_str(json).unwrap();
        missing_width[0].as_object_mut().unwrap().remove("width");
        assert!(serde_json::from_value::<Vec<HyprMonitor>>(missing_width).is_err());
    }
}
