use serde::Deserialize;
use serde_json::from_str;
use std::collections::HashMap;
use std::env;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc, Mutex,
    mpsc::{self, Receiver, TryRecvError},
};
use std::thread;

use window_zones::{HotkeyEvent, HotkeySystem, HotkeySystemError};

#[derive(Debug)]
pub struct WaylandHotkeySystem {
    backend: Option<WaylandHotkeyBackend>,
    registered_bindings: Arc<Mutex<HashMap<String, BoundBinding>>>,
    events: Receiver<HotkeyEvent>,
    start_error: Arc<Mutex<Option<HotkeySystemError>>>,
    listener_child: Option<Arc<Mutex<Child>>>,
    listener_thread: Option<thread::JoinHandle<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WaylandHotkeyBackend {
    Sway,
    Hyprland,
}

#[derive(Debug, Clone)]
struct BoundBinding {
    combo: String,
    hotkey: String,
}

#[derive(Debug, Deserialize)]
struct SwayBindingEvent {
    #[serde(default)]
    binding: Option<SwayBindingPayload>,
}

#[derive(Debug, Deserialize)]
struct SwayBindingPayload {
    #[serde(default)]
    command: Option<String>,
}

impl WaylandHotkeySystem {
    pub fn new() -> Self {
        let registered_bindings = Arc::new(Mutex::new(HashMap::new()));
        let (event_sender, events) = mpsc::channel();
        let start_error = Arc::new(Mutex::new(None));

        let mut listener_child = None;
        let mut listener_thread = None;
        let backend = match resolve_wayland_hotkey_backend() {
            Ok(detected) => {
                match spawn_listener(
                    detected,
                    event_sender,
                    Arc::clone(&registered_bindings),
                    Arc::clone(&start_error),
                ) {
                    Ok(runtime) => {
                        listener_child = runtime.child;
                        listener_thread = Some(runtime.thread);
                    }
                    Err(error) => {
                        *lock_guard(&start_error) = Some(error);
                    }
                }

                Some(detected)
            }
            Err(error) => {
                *lock_guard(&start_error) = Some(error);
                None
            }
        };

        Self {
            backend,
            registered_bindings,
            events,
            start_error,
            listener_child,
            listener_thread,
        }
    }

    pub fn is_available() -> bool {
        resolve_wayland_hotkey_backend().is_ok()
    }

    fn last_error(&self) -> Option<HotkeySystemError> {
        lock_guard(&self.start_error).clone()
    }

    fn register_combo(
        &self,
        backend: WaylandHotkeyBackend,
        combo: &str,
        command: &str,
    ) -> Result<(), HotkeySystemError> {
        match backend {
            WaylandHotkeyBackend::Sway => {
                // Remove any existing binding on this combo first to avoid duplicates.
                let _ = run_sway_command(&["unbindsym", combo]);
                run_sway_command(&["bindsym", "--no-repeat", combo, "nop", command])
            }
            WaylandHotkeyBackend::Hyprland => {
                let _ = run_hyprctl_unbind_command(combo);
                run_hyprctl_bind_command(combo, command)
            }
        }
    }

    fn unregister_combo(
        &self,
        backend: WaylandHotkeyBackend,
        combo: &str,
    ) -> Result<(), HotkeySystemError> {
        match backend {
            WaylandHotkeyBackend::Sway => run_sway_command(&["unbindsym", combo]),
            WaylandHotkeyBackend::Hyprland => run_hyprctl_unbind_command(combo),
        }
    }

    fn binding_for_hotkey(&self, hotkey: &str) -> Option<String> {
        match self.backend {
            Some(WaylandHotkeyBackend::Sway) => wayland_binding_from_hotkey(hotkey),
            Some(WaylandHotkeyBackend::Hyprland) => hyprland_binding_from_hotkey(hotkey),
            None => None,
        }
    }
}

impl Drop for WaylandHotkeySystem {
    fn drop(&mut self) {
        let Some(backend) = self.backend else {
            return;
        };

        let active_bindings: Vec<BoundBinding> = {
            let bindings = lock_guard(&self.registered_bindings);
            bindings.values().cloned().collect()
        };

        for binding in active_bindings {
            let _ = self.unregister_combo(backend, &binding.combo);
        }

        if let Some(child) = self.listener_child.take() {
            if let Ok(mut child) = child.lock() {
                let _ = child.kill();
            }
        }

        if let Some(thread) = self.listener_thread.take() {
            let _ = thread.join();
        }

        lock_guard(&self.registered_bindings).clear();
    }
}

impl HotkeySystem for WaylandHotkeySystem {
    fn register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), HotkeySystemError> {
        if let Some(error) = self.last_error() {
            return Err(error);
        }

        let Some(backend) = self.backend else {
            return Err(HotkeySystemError::Platform(
                "wayland native hotkey backend is unavailable".to_string(),
            ));
        };

        let mut next_bindings = Vec::with_capacity(hotkeys.len());
        for (index, hotkey) in hotkeys.iter().enumerate() {
            let combo = self.binding_for_hotkey(hotkey).ok_or_else(|| {
                HotkeySystemError::Platform(format!("unsupported hotkey `{hotkey}`"))
            })?;
            let command = format!("window-zones::{index}::{hotkey}");
            next_bindings.push((
                command,
                BoundBinding {
                    combo,
                    hotkey: hotkey.clone(),
                },
            ));
        }

        let previous_bindings: Vec<(String, BoundBinding)> = {
            let bindings = lock_guard(&self.registered_bindings);
            bindings
                .iter()
                .map(|(command, binding)| (command.clone(), binding.clone()))
                .collect()
        };

        for (_, bound) in &previous_bindings {
            self.unregister_combo(backend, &bound.combo)?;
        }

        let mut installed: Vec<(String, BoundBinding)> = Vec::with_capacity(next_bindings.len());
        for (command, bound) in &next_bindings {
            if let Err(error) = self.register_combo(backend, &bound.combo, command) {
                for installed in &installed {
                    let _ = self.unregister_combo(backend, &installed.1.combo);
                }

                for (command, bound) in &previous_bindings {
                    let _ = self.register_combo(backend, &bound.combo, command);
                }

                let mut bindings = lock_guard(&self.registered_bindings);
                bindings.clear();

                return Err(error);
            }

            installed.push((command.clone(), bound.clone()));
        }

        let mut bindings = lock_guard(&self.registered_bindings);
        bindings.clear();
        for (command, bound) in next_bindings {
            bindings.insert(command, bound);
        }

        Ok(())
    }

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
        if let Some(error) = self.last_error() {
            return Err(error);
        }

        match self.events.try_recv() {
            Ok(event) => Ok(Some(event)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(HotkeySystemError::Platform(
                "wayland hotkey listener disconnected".to_string(),
            )),
        }
    }
}

struct WaylandListenerRuntime {
    child: Option<Arc<Mutex<Child>>>,
    thread: thread::JoinHandle<()>,
}

fn spawn_listener(
    backend: WaylandHotkeyBackend,
    event_sender: mpsc::Sender<HotkeyEvent>,
    registered_bindings: Arc<Mutex<HashMap<String, BoundBinding>>>,
    start_error: Arc<Mutex<Option<HotkeySystemError>>>,
) -> Result<WaylandListenerRuntime, HotkeySystemError> {
    match backend {
        WaylandHotkeyBackend::Sway => {
            spawn_sway_listener(event_sender, registered_bindings, start_error)
        }
        WaylandHotkeyBackend::Hyprland => {
            spawn_hyprland_listener(event_sender, registered_bindings, start_error)
        }
    }
}

fn spawn_sway_listener(
    event_sender: mpsc::Sender<HotkeyEvent>,
    registered_bindings: Arc<Mutex<HashMap<String, BoundBinding>>>,
    start_error: Arc<Mutex<Option<HotkeySystemError>>>,
) -> Result<WaylandListenerRuntime, HotkeySystemError> {
    let mut child = Command::new("swaymsg")
        .args(["-t", "subscribe", "[\"binding\"]"]) // keep the raw JSON array literal intact
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| {
            HotkeySystemError::Platform(format!("failed to spawn sway binding listener: {error}"))
        })?;

    let Some(stdout) = child.stdout.take() else {
        return Err(HotkeySystemError::Platform(
            "sway binding listener failed to expose stdout".to_string(),
        ));
    };

    let child = Arc::new(Mutex::new(child));
    let child_for_thread = Arc::clone(&child);
    let start_error_for_thread = Arc::clone(&start_error);
    let registered_bindings_for_thread = Arc::clone(&registered_bindings);

    let handle = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    *lock_guard(&start_error_for_thread) = Some(HotkeySystemError::Platform(
                        "sway binding listener ended unexpectedly".to_string(),
                    ));
                    break;
                }
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    let Ok(event) = from_str::<SwayBindingEvent>(trimmed) else {
                        continue;
                    };
                    let Some(command) = event.binding.and_then(|binding| binding.command) else {
                        continue;
                    };

                    let hotkey = {
                        let bindings = lock_guard(&registered_bindings_for_thread);
                        bindings.get(&command).map(|binding| binding.hotkey.clone())
                    };

                    if let Some(hotkey) = hotkey {
                        if event_sender.send(HotkeyEvent::Pressed { hotkey }).is_err() {
                            if let Ok(mut child) = child_for_thread.lock() {
                                let _ = child.kill();
                            }
                            break;
                        }
                    }
                }
                Err(error) => {
                    *lock_guard(&start_error_for_thread) = Some(HotkeySystemError::Platform(
                        format!("sway binding listener read failed: {error}",),
                    ));
                    if let Ok(mut child) = child_for_thread.lock() {
                        let _ = child.kill();
                    }
                    return;
                }
            }
        }

        if let Ok(mut child) = child_for_thread.lock() {
            let _ = child.wait();
        }
    });

    Ok(WaylandListenerRuntime {
        child: Some(child),
        thread: handle,
    })
}

fn spawn_hyprland_listener(
    event_sender: mpsc::Sender<HotkeyEvent>,
    registered_bindings: Arc<Mutex<HashMap<String, BoundBinding>>>,
    start_error: Arc<Mutex<Option<HotkeySystemError>>>,
) -> Result<WaylandListenerRuntime, HotkeySystemError> {
    let socket_path = hyprland_event_socket_path()?;
    let stream = UnixStream::connect(&socket_path).map_err(|error| {
        HotkeySystemError::Platform(format!(
            "failed to connect to hyprland event socket {}: {error}",
            socket_path.display()
        ))
    })?;

    let start_error_for_thread = Arc::clone(&start_error);
    let registered_bindings_for_thread = Arc::clone(&registered_bindings);

    let handle = thread::spawn(move || {
        let mut reader = BufReader::new(stream);
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    *lock_guard(&start_error_for_thread) = Some(HotkeySystemError::Platform(
                        "hyprland event socket closed unexpectedly".to_string(),
                    ));
                    break;
                }
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    let Some((event_name, event_data)) = trimmed.split_once(">>") else {
                        continue;
                    };

                    if event_name != "custom" {
                        continue;
                    }

                    let hotkey = {
                        let bindings = lock_guard(&registered_bindings_for_thread);
                        bindings.get(event_data).map(|binding| binding.hotkey.clone())
                    };

                    if let Some(hotkey) = hotkey {
                        if event_sender.send(HotkeyEvent::Pressed { hotkey }).is_err() {
                            break;
                        }
                    }
                }
                Err(error) => {
                    *lock_guard(&start_error_for_thread) = Some(HotkeySystemError::Platform(
                        format!("hyprland event socket read failed: {error}"),
                    ));
                    return;
                }
            }
        }
    });

    Ok(WaylandListenerRuntime {
        child: None,
        thread: handle,
    })
}

fn resolve_wayland_hotkey_backend() -> Result<WaylandHotkeyBackend, HotkeySystemError> {

    if !is_wayland_session() {
        return Err(HotkeySystemError::Platform(
            "wayland native hotkey registration is only available in a Wayland session".to_string(),
        ));
    }

    if env::var_os("SWAYSOCK").is_some() {
        if command_exists("swaymsg") {
            return Ok(WaylandHotkeyBackend::Sway);
        }

        return Err(HotkeySystemError::Platform(
            "swaymsg is required for native Wayland hotkeys; install swaymsg and ensure it is on PATH".to_string(),
        ));
    }

    if is_hyprland_session() {
        if command_exists("hyprctl") {
            return Ok(WaylandHotkeyBackend::Hyprland);
        }

        return Err(HotkeySystemError::Platform(
            "hyprctl is required for native Wayland hotkeys on Hyprland; install hyprctl and ensure it is on PATH".to_string(),
        ));
    }

    Err(HotkeySystemError::Platform(
        "no native Wayland hotkey backend detected".to_string(),
    ))
}

fn is_wayland_session() -> bool {
    env::var_os("XDG_SESSION_TYPE")
        .and_then(|value| value.to_str().map(str::to_ascii_lowercase))
        .is_some_and(|value| value == "wayland")
        || env::var_os("WAYLAND_DISPLAY").is_some()
}

fn is_hyprland_session() -> bool {
    env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
        || env::var_os("XDG_CURRENT_DESKTOP")
            .and_then(|value| value.to_str().map(|value| value.to_ascii_lowercase()))
            .is_some_and(|value| value == "hyprland")
}

fn wayland_binding_from_hotkey(hotkey: &str) -> Option<String> {
    wayland_binding_from_hotkey_with(hotkey, "+", map_sway_modifier_token)
}

fn hyprland_binding_from_hotkey(hotkey: &str) -> Option<String> {
    wayland_binding_from_hotkey_with(hotkey, "_", map_hyprland_modifier_token)
}

fn wayland_binding_from_hotkey_with(
    hotkey: &str,
    modifier_join: &str,
    map_modifier_token: fn(&str) -> Option<&'static str>,
) -> Option<String> {
    let mut modifiers = Vec::new();
    let mut key = None;

    for token in hotkey.split('+') {
        let token = token.trim();
        if token.is_empty() {
            return None;
        }

        if is_modifier_token(token) {
            modifiers.push(token);
            continue;
        }

        if key.is_none() {
            key = Some(token);
            continue;
        }

        return None;
    }

    let key = key?;
    let key = map_non_modifier_key(key)?;

    modifiers.sort_by_key(|token| modifier_sort_key(*token));
    let mut binding = Vec::with_capacity(modifiers.len() + 1);
    for modifier in modifiers {
        let Some(prefix) = map_modifier_token(modifier) else {
            return None;
        };
        binding.push(prefix.to_string());
    }
    binding.push(key);

    Some(binding.join(modifier_join))
}

fn is_modifier_token(token: &str) -> bool {
    matches!(token, "ctrl" | "alt" | "shift" | "cmd")
}

fn map_sway_modifier_token(token: &str) -> Option<&'static str> {
    match token {
        "ctrl" => Some("Ctrl"),
        "alt" => Some("Alt"),
        "shift" => Some("Shift"),
        "cmd" => Some("Mod4"),
        _ => None,
    }
}

fn map_hyprland_modifier_token(token: &str) -> Option<&'static str> {
    match token {
        "ctrl" => Some("CTRL"),
        "alt" => Some("ALT"),
        "shift" => Some("SHIFT"),
        "cmd" => Some("SUPER"),
        _ => None,
    }
}

fn modifier_sort_key(token: &str) -> u8 {
    match token {
        "alt" => 0,
        "ctrl" => 1,
        "shift" => 2,
        "cmd" => 3,
        _ => 4,
    }
}

fn map_non_modifier_key(token: &str) -> Option<String> {
    Some(match token {
        "left" => "Left".to_string(),
        "right" => "Right".to_string(),
        "up" => "Up".to_string(),
        "down" => "Down".to_string(),
        "tab" => "Tab".to_string(),
        "escape" => "Escape".to_string(),
        "enter" | "return" => "Return".to_string(),
        "space" => "space".to_string(),
        "minus" => "minus".to_string(),
        "plus" => "plus".to_string(),
        "comma" => "comma".to_string(),
        "period" => "period".to_string(),
        "slash" => "slash".to_string(),
        token => {
            if token.len() == 1 {
                return Some(token.to_ascii_uppercase());
            }

            if token.starts_with('f') && token[1..].chars().all(|char| char.is_ascii_digit()) {
                return Some(token.to_ascii_uppercase());
            }

            let mut chars = token.chars();
            let Some(first) = chars.next() else {
                return None;
            };
            let mut normalized = String::new();
            normalized.push(first.to_ascii_uppercase());
            normalized.push_str(chars.as_str());
            normalized
        }
    })
}

fn run_sway_command(args: &[&str]) -> Result<(), HotkeySystemError> {
    run_command("swaymsg", args, "sway command")
}

fn run_hyprctl_bind_command(combo: &str, command: &str) -> Result<(), HotkeySystemError> {
    run_hyprctl_command(&["keyword", "bind", &format!("{combo},custom_event,{command}")])
}

fn run_hyprctl_unbind_command(combo: &str) -> Result<(), HotkeySystemError> {
    run_hyprctl_command(&["keyword", "unbind", combo])
}

fn run_hyprctl_command(args: &[&str]) -> Result<(), HotkeySystemError> {
    run_command("hyprctl", args, "hyprctl")
}

fn run_command(program: &str, args: &[&str], context: &str) -> Result<(), HotkeySystemError> {
    let status = Command::new(program)
        .args(args)
        .status()
        .map_err(|error| {
            HotkeySystemError::Platform(format!("failed to run {context}: {error}"))
        })?;

    if !status.success() {
        return Err(HotkeySystemError::Platform(format!(
            "{context} failed: exit status {status}"
        )));
    }

    Ok(())
}

fn hyprland_event_socket_path() -> Result<PathBuf, HotkeySystemError> {
    let Some(runtime_dir) = env::var_os("XDG_RUNTIME_DIR").or_else(|| env::var_os("TMPDIR")) else {
        return Err(HotkeySystemError::Platform(
            "no runtime directory found for Hyprland event socket".to_string(),
        ));
    };

    let Some(signature) = env::var_os("HYPRLAND_INSTANCE_SIGNATURE") else {
        return Err(HotkeySystemError::Platform(
            "HYPRLAND_INSTANCE_SIGNATURE is required for Hyprland event socket access".to_string(),
        ));
    };

    Ok(PathBuf::from(runtime_dir)
        .join("hypr")
        .join(signature)
        .join(".socket2.sock"))
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

fn lock_guard<T>(state: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match state.lock() {
        Ok(guard) => guard,
        Err(error) => error.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::ffi::OsString;

    fn set_env(key: &str, value: Option<&str>) {
        if let Some(value) = value {
            unsafe {
                env::set_var(key, value);
            }
        } else {
            unsafe {
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

    fn create_executable(path: &std::path::Path) {
        std::fs::write(path, "#!/bin/sh\n").unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    fn supports_sway_hotkeys_when_sway_env_is_available() {
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
            ("PATH".to_string(), env::var_os("PATH")),
        ];

        let temp_dir =
            std::env::temp_dir().join(format!("window_zones_swaymsg_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        create_executable(&temp_dir.join("swaymsg"));

        set_env("XDG_SESSION_TYPE", Some("wayland"));
        set_env("WAYLAND_DISPLAY", Some("wayland-0"));
        set_env("SWAYSOCK", Some("/tmp/sway"));
        set_env("PATH", Some(temp_dir.to_str().unwrap()));

        assert!(WaylandHotkeySystem::is_available());

        restore_env(&backups);
    }

    #[test]
    fn prefers_sway_backend_when_sway_sock_and_hyprland_present() {
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
            (
                "XDG_CURRENT_DESKTOP".to_string(),
                env::var_os("XDG_CURRENT_DESKTOP"),
            ),
            ("PATH".to_string(), env::var_os("PATH")),
        ];

        let temp_dir =
            std::env::temp_dir().join(format!("window_zones_hypr_land_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        create_executable(&temp_dir.join("swaymsg"));
        create_executable(&temp_dir.join("hyprctl"));

        set_env("XDG_SESSION_TYPE", Some("wayland"));
        set_env("WAYLAND_DISPLAY", Some("wayland-0"));
        set_env("SWAYSOCK", Some("/tmp/sway"));
        set_env("HYPRLAND_INSTANCE_SIGNATURE", Some("sig"));
        set_env("XDG_CURRENT_DESKTOP", Some("Hyprland"));
        set_env("PATH", Some(temp_dir.to_str().unwrap()));

        let result = resolve_wayland_hotkey_backend();
        assert!(matches!(result, Ok(WaylandHotkeyBackend::Sway)));

        restore_env(&backups);
    }

    #[test]
    fn rejects_unsupported_hotkey_syntax() {
        assert!(wayland_binding_from_hotkey("++").is_none());
        assert!(wayland_binding_from_hotkey("alt+").is_none());
    }

    #[test]
    fn maps_common_hotkeys_to_sway_bindings() {
        assert_eq!(
            wayland_binding_from_hotkey("alt+ctrl+left").as_deref(),
            Some("Alt+Ctrl+Left")
        );
        assert_eq!(
            wayland_binding_from_hotkey("ctrl+shift+f1").as_deref(),
            Some("Ctrl+Shift+F1")
        );
    }

    #[test]
    fn maps_named_punctuation_and_function_keys() {
        assert_eq!(
            wayland_binding_from_hotkey("cmd+space").as_deref(),
            Some("Mod4+space")
        );
        assert_eq!(
            wayland_binding_from_hotkey("cmd+minus").as_deref(),
            Some("Mod4+minus")
        );
        assert_eq!(
            wayland_binding_from_hotkey("alt+plus").as_deref(),
            Some("Alt+plus")
        );
        assert_eq!(
            wayland_binding_from_hotkey("shift+f12").as_deref(),
            Some("Shift+F12")
        );
    }

    #[test]
    fn hyprland_hotkeys_are_supported_when_hyprctl_is_available() {
        let backups = [
            (
                "XDG_SESSION_TYPE".to_string(),
                env::var_os("XDG_SESSION_TYPE"),
            ),
            (
                "WAYLAND_DISPLAY".to_string(),
                env::var_os("WAYLAND_DISPLAY"),
            ),
            (
                "HYPRLAND_INSTANCE_SIGNATURE".to_string(),
                env::var_os("HYPRLAND_INSTANCE_SIGNATURE"),
            ),
            (
                "XDG_CURRENT_DESKTOP".to_string(),
                env::var_os("XDG_CURRENT_DESKTOP"),
            ),
            ("PATH".to_string(), env::var_os("PATH")),
        ];

        let temp_dir =
            std::env::temp_dir().join(format!("window_zones_hyprctl_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        create_executable(&temp_dir.join("hyprctl"));

        set_env("XDG_SESSION_TYPE", Some("wayland"));
        set_env("WAYLAND_DISPLAY", Some("wayland-0"));
        set_env("HYPRLAND_INSTANCE_SIGNATURE", Some("sig"));
        set_env("XDG_CURRENT_DESKTOP", Some("Hyprland"));
        set_env("PATH", Some(temp_dir.to_str().unwrap()));

        let result = resolve_wayland_hotkey_backend();
        assert!(matches!(result, Ok(WaylandHotkeyBackend::Hyprland)));

        restore_env(&backups);
    }

    #[test]
    fn maps_common_hotkeys_to_hyprland_binds() {
        assert_eq!(
            hyprland_binding_from_hotkey("alt+ctrl+left").as_deref(),
            Some("ALT_CTRL_Left")
        );
        assert_eq!(
            hyprland_binding_from_hotkey("cmd+space").as_deref(),
            Some("SUPER_space")
        );
        assert_eq!(
            hyprland_binding_from_hotkey("ctrl+shift+f12").as_deref(),
            Some("CTRL_SHIFT_F12")
        );
    }

    #[test]
    fn rejects_hyprland_without_hyprctl() {
        let backups = [
            (
                "XDG_SESSION_TYPE".to_string(),
                env::var_os("XDG_SESSION_TYPE"),
            ),
            (
                "WAYLAND_DISPLAY".to_string(),
                env::var_os("WAYLAND_DISPLAY"),
            ),
            (
                "HYPRLAND_INSTANCE_SIGNATURE".to_string(),
                env::var_os("HYPRLAND_INSTANCE_SIGNATURE"),
            ),
            (
                "XDG_CURRENT_DESKTOP".to_string(),
                env::var_os("XDG_CURRENT_DESKTOP"),
            ),
            ("PATH".to_string(), env::var_os("PATH")),
        ];

        set_env("XDG_SESSION_TYPE", Some("wayland"));
        set_env("WAYLAND_DISPLAY", Some("wayland-0"));
        set_env("HYPRLAND_INSTANCE_SIGNATURE", Some("sig"));
        set_env("XDG_CURRENT_DESKTOP", Some("Hyprland"));

        set_env("PATH", Some(""));

        assert!(matches!(
            resolve_wayland_hotkey_backend(),
            Err(HotkeySystemError::Platform(message))
                if message.contains("hyprctl is required")
        ));

        restore_env(&backups);
    }
}
