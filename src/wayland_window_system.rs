//! Linux Wayland adapter for `WindowSystem`.
//!
//! Sway and Hyprland use compositor-bound dispatch and identify windows before
//! moving them. Session routing also recognizes GNOME and KDE, whose separate
//! companion integrations own their window operations.

use crate::{
    DisplayGeometry, FocusedWindow, Rect, WindowId, WindowMove, WindowSystem, WindowSystemError,
};

use serde::Deserialize;
use std::env;
use std::ffi::OsString;
use std::fmt::Write as _;
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

/// Compositors controlled through their command-line IPC clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptedCompositor {
    Sway,
    Hyprland,
}

#[derive(Debug)]
pub struct WaylandWindowSystem {
    compositor: ScriptedCompositor,
}

impl WaylandWindowSystem {
    pub fn new(compositor: ScriptedCompositor) -> Self {
        Self { compositor }
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

impl WindowSystem for WaylandWindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
        match self.compositor {
            ScriptedCompositor::Sway => focused_window_sway(),
            ScriptedCompositor::Hyprland => focused_window_hypr(),
        }
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        match self.compositor {
            ScriptedCompositor::Sway => displays_sway(),
            ScriptedCompositor::Hyprland => displays_hypr(),
        }
    }

    fn move_window(&mut self, window_move: &WindowMove) -> Result<(), WindowSystemError> {
        match self.compositor {
            ScriptedCompositor::Sway => move_window_sway(window_move),
            ScriptedCompositor::Hyprland => move_window_hypr(window_move),
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
    app_id: Option<String>,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    floating: Option<String>,
    #[serde(default)]
    fullscreen_mode: u32,
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
    floating: bool,
    fullscreen: u32,
    #[serde(rename = "fullscreenClient")]
    fullscreen_client: u32,
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

    Ok(sway_focused_window(&tree))
}

fn sway_focused_window(tree: &SwayTree) -> Option<FocusedWindow> {
    let focused = find_focused_sway_node(&tree.nodes)
        .or_else(|| find_focused_sway_node(&tree.floating_nodes))?;
    Some(FocusedWindow::new(
        WindowId::new(focused.id?.to_string()),
        focused.rect.to_rect(),
    ))
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

fn move_window_sway(window_move: &WindowMove) -> Result<(), WindowSystemError> {
    let tree: SwayTree = run_wayland_json_command("swaymsg", &["-t", "get_tree"], "sway get_tree")?;
    let command = sway_move_command(&tree, window_move)?;
    run_sway_command(&command, &window_move.window)
}

fn sway_move_command(
    tree: &SwayTree,
    window_move: &WindowMove,
) -> Result<String, WindowSystemError> {
    let id = window_move
        .window
        .as_str()
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(|| WindowSystemError::WindowGone(window_move.window.clone()))?;
    let (_, floating, fullscreen) = find_sway_window(&tree.nodes, id, false, false)
        .or_else(|| find_sway_window(&tree.floating_nodes, id, true, false))
        .ok_or_else(|| WindowSystemError::WindowGone(window_move.window.clone()))?;
    if fullscreen {
        return Err(WindowSystemError::Platform(
            "Sway window is fullscreen; leave fullscreen first".to_string(),
        ));
    }

    let mut command = String::new();
    if !floating {
        write!(command, "[con_id={id}] floating enable; ").unwrap();
    }
    // move.c adds the workspace offset unless "absolute" is present.
    // resize.c recenters floating frames, so position must be applied last.
    // https://github.com/swaywm/sway/blob/master/sway/commands/{move,resize}.c
    write!(
        command,
        "[con_id={id}] resize set width {} px height {} px; \
         [con_id={id}] move absolute position {} {}",
        window_move.target.width,
        window_move.target.height,
        window_move.target.x,
        window_move.target.y
    )
    .unwrap();
    Ok(command)
}

fn focused_window_hypr() -> Result<Option<FocusedWindow>, WindowSystemError> {
    Ok(hypr_activewindow()?.and_then(hypr_focused_window))
}

fn hypr_focused_window(window: HyprWindow) -> Option<FocusedWindow> {
    let geometry = window.geometry();
    if geometry.width == 0 || geometry.height == 0 {
        return None;
    }
    Some(FocusedWindow::new(WindowId::new(window.address), geometry))
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

fn move_window_hypr(window_move: &WindowMove) -> Result<(), WindowSystemError> {
    let windows: Vec<HyprWindow> =
        run_wayland_json_command("hyprctl", &["-j", "clients"], "hyprctl clients")?;
    let command = hypr_move_command(&windows, window_move)?;
    let reply = run_command_capture("hyprctl", &["--batch", &command], "hyprctl move/resize")?;
    validate_hypr_batch_reply(&reply, command.split(';').count(), &window_move.window)
}

fn hypr_move_command(
    windows: &[HyprWindow],
    window_move: &WindowMove,
) -> Result<String, WindowSystemError> {
    let address = window_move.window.as_str();
    let window = windows
        .iter()
        .find(|window| window.address == address)
        .ok_or_else(|| WindowSystemError::WindowGone(window_move.window.clone()))?;
    if !address
        .strip_prefix("0x")
        .is_some_and(|hex| !hex.is_empty() && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(WindowSystemError::Platform(
            "hyprctl reported an invalid window address".to_string(),
        ));
    }
    // Hyprland's fullscreen mode is a bitmask: 1 = maximized, 2 = fullscreen.
    if (window.fullscreen | window.fullscreen_client) & 2 != 0 {
        return Err(WindowSystemError::Platform(
            "Hyprland window is fullscreen; leave fullscreen first".to_string(),
        ));
    }

    let mut command = String::new();
    // setfloating is a no-op for an already-floating maximized window. A
    // targeted settiled/setfloating restores it without changing focus.
    // IHyprLayout::changeWindowFloatingMode clears the internal maximized state.
    if window.floating && window.fullscreen != 0 {
        write!(command, "dispatch settiled address:{address}; ").unwrap();
    }
    if !window.floating || window.fullscreen != 0 {
        write!(command, "dispatch setfloating address:{address}; ").unwrap();
    }
    // KeybindManager::{resizeWindow,moveWindow} resolve the explicit address
    // and Compositor::parseWindowVectorArgsRelative treats "exact" as global.
    // https://github.com/hyprwm/Hyprland/blob/v0.52.0/src/managers/KeybindManager.cpp
    write!(
        command,
        "dispatch resizewindowpixel exact {} {},address:{address}; \
         dispatch movewindowpixel exact {} {},address:{address}",
        window_move.target.width,
        window_move.target.height,
        window_move.target.x,
        window_move.target.y
    )
    .unwrap();
    Ok(command)
}

fn find_focused_sway_node(nodes: &[SwayTreeNode]) -> Option<&SwayTreeNode> {
    nodes.iter().find_map(|node| {
        if node.focused && is_sway_window_node(node) && node.rect.width > 0 && node.rect.height > 0
        {
            return Some(node);
        }

        find_focused_sway_node(&node.nodes).or_else(|| find_focused_sway_node(&node.floating_nodes))
    })
}

fn is_sway_window_node(node: &SwayTreeNode) -> bool {
    matches!(
        node.node_type.as_deref(),
        Some("con") | Some("floating_con")
    ) && node.id.is_some()
        && (node.pid.is_some() || node.app_id.is_some())
}

fn find_sway_window(
    nodes: &[SwayTreeNode],
    id: i64,
    floating: bool,
    fullscreen: bool,
) -> Option<(&SwayTreeNode, bool, bool)> {
    nodes.iter().find_map(|node| {
        let floating = floating
            || node.node_type.as_deref() == Some("floating_con")
            || matches!(node.floating.as_deref(), Some("auto_on") | Some("user_on"));
        // Workspaces always report fullscreen_mode=1; only containers
        // represent a real fullscreen state (including fullscreen parents).
        let fullscreen = fullscreen
            || (matches!(
                node.node_type.as_deref(),
                Some("con") | Some("floating_con")
            ) && node.fullscreen_mode != 0);
        if node.id == Some(id) && is_sway_window_node(node) {
            return Some((node, floating, fullscreen));
        }
        find_sway_window(&node.nodes, id, floating, fullscreen)
            .or_else(|| find_sway_window(&node.floating_nodes, id, true, fullscreen))
    })
}

fn run_sway_command(command: &str, window: &WindowId) -> Result<(), WindowSystemError> {
    // -r keeps the IPC JSON even when swaymsg exits 2 for a rejected command.
    let output = Command::new("swaymsg")
        .args(["-r", command])
        .output()
        .map_err(|error| {
            WindowSystemError::Platform(format!("failed to execute swaymsg: {error}"))
        })?;
    if !output.stdout.is_empty() {
        let reply = std::str::from_utf8(&output.stdout).map_err(|error| {
            WindowSystemError::Platform(format!("swaymsg returned non-utf8 output: {error}"))
        })?;
        validate_sway_reply(reply, command.split(';').count(), window)?;
    } else if output.status.success() {
        return Err(WindowSystemError::Platform(
            "swaymsg returned no command reply".to_string(),
        ));
    }
    if !output.status.success() {
        return Err(WindowSystemError::Platform(format!(
            "swaymsg move/resize failed with exit status {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

#[derive(Deserialize)]
struct SwayCommandReply {
    success: bool,
    error: Option<String>,
}

fn validate_sway_reply(
    reply: &str,
    expected: usize,
    window: &WindowId,
) -> Result<(), WindowSystemError> {
    let replies: Vec<SwayCommandReply> = serde_json::from_str(reply).map_err(|error| {
        WindowSystemError::Platform(format!("swaymsg command reply JSON parse failed: {error}"))
    })?;
    for reply in &replies {
        if !reply.success {
            let error = reply.error.as_deref().unwrap_or("unknown command error");
            if error == "No matching node." {
                return Err(WindowSystemError::WindowGone(window.clone()));
            }
            return Err(WindowSystemError::Platform(format!(
                "swaymsg move/resize failed: {error}"
            )));
        }
    }
    if replies.len() != expected {
        return Err(WindowSystemError::Platform(format!(
            "swaymsg returned {} command replies; expected {expected}",
            replies.len()
        )));
    }
    Ok(())
}

fn validate_hypr_batch_reply(
    reply: &str,
    expected: usize,
    window: &WindowId,
) -> Result<(), WindowSystemError> {
    // v0.52 HyprCtl::dispatchBatch separates replies with three newlines;
    // hyprctl's exit status does not describe individual dispatch success.
    let mut count = 0;
    for reply in reply.trim().split("\n\n\n") {
        let reply = reply.trim();
        count += 1;
        match reply {
            "ok" => {}
            "Window not found" | "moveWindow: no window" | "resizeWindow: no window" => {
                return Err(WindowSystemError::WindowGone(window.clone()));
            }
            "Window is fullscreen" => {
                return Err(WindowSystemError::Platform(
                    "Hyprland window is fullscreen; leave fullscreen first".to_string(),
                ));
            }
            _ => {
                return Err(WindowSystemError::Platform(format!(
                    "hyprctl move/resize failed: {}",
                    if reply.is_empty() {
                        "empty dispatch reply"
                    } else {
                        reply
                    }
                )));
            }
        }
    }
    if count != expected {
        return Err(WindowSystemError::Platform(format!(
            "hyprctl returned {count} dispatch replies; expected {expected}"
        )));
    }
    Ok(())
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

    #[test]
    fn non_wayland_session_returns_session_error() {
        let error = resolve_wayland_backend_with_env(|name| {
            (name == "XDG_SESSION_TYPE").then(|| OsString::from("x11"))
        })
        .unwrap_err();
        assert!(matches!(
            error,
            WindowSystemError::Platform(message)
                if message.contains("can only be used when XDG_SESSION_TYPE=wayland")
        ));
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

    // Captured swaymsg -t get_tree (Sway 1.9), with unrelated output metadata
    // and unused fields removed; node identity, frame, and state are unchanged.
    // https://gist.github.com/o-alquimista/821755fd1133c90bf8b73b045234cd81#file-swaymsg-t-get_tree-json
    const SWAY_TREE_JSON: &str = r#"{
        "id":1, "type":"root", "nodes":[{
            "id":3, "type":"output", "name":"HDMI-A-1",
            "rect":{"x":0,"y":0,"width":2560,"height":1080},
            "nodes":[{
                "id":4, "type":"workspace", "name":"1", "fullscreen_mode":1,
                "rect":{"x":0,"y":23,"width":2560,"height":1057},
                "nodes":[{
                    "id":8, "type":"con", "name":"Steam", "focused":false,
                    "rect":{"x":0,"y":48,"width":2560,"height":1032},
                    "window":23068699, "pid":8512, "app_id":null,
                    "fullscreen_mode":0, "nodes":[], "floating_nodes":[]
                }],
                "floating_nodes":[{
                    "id":9, "type":"floating_con", "name":"Counter-Strike 2",
                    "focused":false, "border":"csd",
                    "rect":{"x":0,"y":0,"width":2560,"height":1080},
                    "window":44040218, "pid":9659, "app_id":null,
                    "fullscreen_mode":0, "nodes":[], "floating_nodes":[]
                }]
            },{
                "id":10, "type":"workspace", "name":"2", "fullscreen_mode":1,
                "rect":{"x":0,"y":23,"width":2560,"height":1057},
                "nodes":[{
                    "id":11, "type":"con", "name":"foot", "focused":true,
                    "rect":{"x":0,"y":48,"width":2560,"height":1032},
                    "window_rect":{"x":2,"y":0,"width":2556,"height":1030},
                    "geometry":{"x":0,"y":0,"width":696,"height":494},
                    "window":null, "pid":10625, "app_id":"foot",
                    "fullscreen_mode":0, "nodes":[], "floating_nodes":[]
                }],
                "floating_nodes":[]
            }],
            "floating_nodes":[]
        }], "floating_nodes":[]
    }"#;

    #[test]
    fn focused_window_parser_finds_frame_and_identity_below_workspace_nodes() {
        let tree: SwayTree = serde_json::from_str(SWAY_TREE_JSON).unwrap();
        assert_eq!(
            sway_focused_window(&tree),
            Some(FocusedWindow::new(
                WindowId::new("11"),
                Rect::new(0, 48, 2560, 1032)
            ))
        );
        let (_, floating, fullscreen) = find_sway_window(&tree.nodes, 9, false, false).unwrap();
        assert!(floating);
        assert!(!fullscreen);
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

    fn hypr_window_json() -> &'static str {
        // Captured hyprctl -j activewindow, also the object shape used by clients.
        // https://github.com/hyprwm/Hyprland/discussions/14292#discussioncomment-16833698
        r#"{
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
}"#
    }

    #[test]
    fn hypr_activewindow_parser_reads_real_geometry_and_rejects_missing_size() {
        let json = hypr_window_json();
        let window = parse_hypr_activewindow(json).unwrap().unwrap();
        assert_eq!(window.geometry(), Rect::new(1320, 680, 600, 400));
        assert!(window.floating);
        assert_eq!(window.fullscreen, 0);
        assert_eq!(window.fullscreen_client, 0);
        assert_eq!(
            hypr_focused_window(window),
            Some(FocusedWindow::new(
                WindowId::new("0x55e0ed70afc0"),
                Rect::new(1320, 680, 600, 400)
            ))
        );
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

    #[test]
    fn sway_commands_restore_resize_then_move_identified_window_across_outputs() {
        let mut tree: SwayTree = serde_json::from_str(SWAY_TREE_JSON).unwrap();
        // Focus changed since discovery; it must not redirect the action.
        tree.nodes[0].nodes[0].nodes[0].focused = true;
        tree.nodes[0].nodes[1].nodes[0].focused = false;
        let window_move = WindowMove::new(WindowId::new("11"), Rect::new(-1920, -200, 960, 1080));
        assert_eq!(
            sway_move_command(&tree, &window_move).unwrap(),
            "[con_id=11] floating enable; \
             [con_id=11] resize set width 960 px height 1080 px; \
             [con_id=11] move absolute position -1920 -200"
        );
        let floating_move = WindowMove::new(WindowId::new("9"), Rect::new(1920, 23, 800, 600));
        assert_eq!(
            sway_move_command(&tree, &floating_move).unwrap(),
            "[con_id=9] resize set width 800 px height 600 px; \
             [con_id=9] move absolute position 1920 23"
        );
        // Modern Sway also emits the floating reason for a con node.
        tree.nodes[0].nodes[1].nodes[0].floating = Some("user_on".to_string());
        assert_eq!(
            sway_move_command(&tree, &window_move).unwrap(),
            "[con_id=11] resize set width 960 px height 1080 px; \
             [con_id=11] move absolute position -1920 -200"
        );
    }

    #[test]
    fn sway_commands_reject_closed_non_window_and_fullscreen_targets() {
        let mut tree: SwayTree = serde_json::from_str(SWAY_TREE_JSON).unwrap();
        for id in ["999", "10", "focused", "11] move left"] {
            let window_move = WindowMove::new(WindowId::new(id), Rect::new(0, 0, 800, 600));
            assert_eq!(
                sway_move_command(&tree, &window_move),
                Err(WindowSystemError::WindowGone(window_move.window))
            );
        }
        for mode in [1, 2] {
            tree.nodes[0].nodes[1].nodes[0].fullscreen_mode = mode;
            let window_move = WindowMove::new(WindowId::new("11"), Rect::new(0, 0, 800, 600));
            assert!(matches!(
                sway_move_command(&tree, &window_move),
                Err(WindowSystemError::Platform(message)) if message.contains("leave fullscreen first")
            ));
        }
        tree.nodes[0].nodes[1].nodes[0].fullscreen_mode = 0;
        let parent = &mut tree.nodes[0].nodes[1].nodes[0];
        let child: SwayTreeNode = serde_json::from_str(
            r#"{"id":12,"type":"con","pid":123,"rect":{"x":0,"y":0,"width":800,"height":600}}"#,
        )
        .unwrap();
        parent.fullscreen_mode = 1;
        parent.nodes.push(child);
        let child_move = WindowMove::new(WindowId::new("12"), Rect::new(0, 0, 800, 600));
        assert!(matches!(
            sway_move_command(&tree, &child_move),
            Err(WindowSystemError::Platform(message)) if message.contains("leave fullscreen first")
        ));
    }

    #[test]
    fn sway_focused_parser_skips_non_windows_and_empty_frames() {
        let mut tree: SwayTree = serde_json::from_str(SWAY_TREE_JSON).unwrap();
        tree.nodes[0].focused = true;
        tree.nodes[0].nodes[1].focused = true;
        tree.nodes[0].nodes[1].nodes[0].focused = false;
        assert_eq!(sway_focused_window(&tree), None);
        let node = &mut tree.nodes[0].nodes[1].nodes[0];
        node.focused = true;
        node.pid = None;
        node.app_id = None;
        assert_eq!(sway_focused_window(&tree), None);
        let node = &mut tree.nodes[0].nodes[1].nodes[0];
        node.pid = Some(10625);
        node.rect.width = 0;
        assert_eq!(sway_focused_window(&tree), None);
    }

    #[test]
    fn sway_replies_check_every_success_and_surface_compositor_errors() {
        let window = WindowId::new("11");
        assert_eq!(
            validate_sway_reply(r#"[{"success":true},{"success":true}]"#, 2, &window),
            Ok(())
        );
        assert!(matches!(
            validate_sway_reply(
                r#"[{"success":true},{"success":false,"error":"Cannot resize a hidden scratchpad container"}]"#,
                2,
                &window
            ),
            Err(WindowSystemError::Platform(message)) if message.contains("hidden scratchpad")
        ));
        assert_eq!(
            validate_sway_reply(
                r#"[{"success":false,"error":"No matching node."}]"#,
                2,
                &window
            ),
            Err(WindowSystemError::WindowGone(window.clone()))
        );
        for reply in ["[]", "[{}]", r#"[{"success":true}]"#, "not JSON"] {
            assert!(validate_sway_reply(reply, 2, &window).is_err());
        }
    }

    #[test]
    fn hypr_commands_target_client_address_restore_then_resize_and_move() {
        let mut clients: Vec<HyprWindow> =
            serde_json::from_str(&format!("[{}]", hypr_window_json())).unwrap();
        let target = WindowMove::new(
            WindowId::new("0x55e0ed70afc0"),
            Rect::new(-2560, -100, 1280, 720),
        );
        assert_eq!(
            hypr_move_command(&clients, &target).unwrap(),
            "dispatch resizewindowpixel exact 1280 720,address:0x55e0ed70afc0; \
             dispatch movewindowpixel exact -2560 -100,address:0x55e0ed70afc0"
        );
        clients[0].floating = false;
        assert_eq!(
            hypr_move_command(&clients, &target).unwrap(),
            "dispatch setfloating address:0x55e0ed70afc0; \
             dispatch resizewindowpixel exact 1280 720,address:0x55e0ed70afc0; \
             dispatch movewindowpixel exact -2560 -100,address:0x55e0ed70afc0"
        );
        clients[0].fullscreen = 1;
        clients[0].floating = true;
        assert_eq!(
            hypr_move_command(&clients, &target).unwrap(),
            "dispatch settiled address:0x55e0ed70afc0; \
             dispatch setfloating address:0x55e0ed70afc0; \
             dispatch resizewindowpixel exact 1280 720,address:0x55e0ed70afc0; \
             dispatch movewindowpixel exact -2560 -100,address:0x55e0ed70afc0"
        );
        clients[0].fullscreen = 0;
        // The active/focused client need not be first or even in this snapshot.
        let mut other = parse_hypr_activewindow(hypr_window_json())
            .unwrap()
            .unwrap();
        other.address = "0x1234".to_string();
        clients.insert(0, other);
        assert_eq!(
            hypr_move_command(&clients, &target).unwrap(),
            "dispatch resizewindowpixel exact 1280 720,address:0x55e0ed70afc0; \
             dispatch movewindowpixel exact -2560 -100,address:0x55e0ed70afc0"
        );
    }

    #[test]
    fn hypr_commands_reject_missing_addresses_fullscreen_and_invalid_addresses() {
        let mut clients = vec![
            parse_hypr_activewindow(hypr_window_json())
                .unwrap()
                .unwrap(),
        ];
        let target = WindowMove::new(WindowId::new("0x55e0ed70afc0"), Rect::new(0, 0, 800, 600));
        assert_eq!(
            hypr_move_command(&[], &target),
            Err(WindowSystemError::WindowGone(target.window.clone()))
        );
        for (internal, client) in [(2, 0), (0, 2), (3, 0)] {
            clients[0].fullscreen = internal;
            clients[0].fullscreen_client = client;
            assert!(matches!(
                hypr_move_command(&clients, &target),
                Err(WindowSystemError::Platform(message)) if message.contains("leave fullscreen first")
            ));
        }
        for address in ["0x", "0x123; dispatch killactive", "active"] {
            clients[0].address = address.to_string();
            let target = WindowMove::new(WindowId::new(address), target.target);
            assert!(matches!(
                hypr_move_command(&clients, &target),
                Err(WindowSystemError::Platform(message)) if message.contains("invalid window address")
            ));
        }
    }

    #[test]
    fn hypr_batch_replies_require_one_ok_per_dispatch_not_just_exit_success() {
        let window = WindowId::new("0x55e0ed70afc0");
        assert_eq!(
            validate_hypr_batch_reply("ok\n\n\nok\n\n\nok\n", 3, &window),
            Ok(())
        );
        assert!(matches!(
            validate_hypr_batch_reply("ok\n\n\nInvalid size provided\n", 2, &window),
            Err(WindowSystemError::Platform(message)) if message.contains("Invalid size provided")
        ));
        for reply in [
            "Window not found",
            "moveWindow: no window",
            "resizeWindow: no window",
        ] {
            assert_eq!(
                validate_hypr_batch_reply(&format!("ok\n\n\n{reply}\n"), 2, &window),
                Err(WindowSystemError::WindowGone(window.clone()))
            );
        }
        assert!(matches!(
            validate_hypr_batch_reply("Window is fullscreen\n", 1, &window),
            Err(WindowSystemError::Platform(message)) if message.contains("leave fullscreen first")
        ));
        for reply in ["", "ok", "ok\nok", "ok\n\n\nok\n\n\nunexpected", "OK"] {
            assert!(validate_hypr_batch_reply(reply, 2, &window).is_err());
        }
    }
}
