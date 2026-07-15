use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use thiserror::Error;

use crate::config::{
    AppConfig, BindingValidationError, ConfigError, parse_config, validate_and_normalize_app_config,
};
use crate::dispatcher::{DispatchHotkeyError, dispatch_hotkey};
use crate::hotkey_system::{HotkeyEvent, HotkeySystem, HotkeySystemError};
use crate::window_system::WindowSystem;
const CONFIG_DIRECTORY: &str = "window_zones";
const CONFIG_FILE: &str = "config.toml";
const CONFIG_RELOAD_DEBOUNCE: Duration = Duration::from_millis(150);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConfigFileSignature {
    modified: SystemTime,
    len: u64,
}

#[derive(Debug, Error)]
pub enum ConfigPathError {
    #[error("cannot resolve the {platform} config directory: set {variables}")]
    MissingEnvironment {
        platform: &'static str,
        variables: &'static str,
    },
    #[error("config discovery is not supported on {platform}")]
    UnsupportedPlatform { platform: &'static str },
}

#[derive(Debug, Error)]
pub enum ConfigLoadError {
    #[error(transparent)]
    Path(#[from] ConfigPathError),
    #[error("failed to read config at {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to parse config at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: ConfigError,
    },
    #[error("failed to validate config at {path}: {source}")]
    Validation {
        path: PathBuf,
        #[source]
        source: BindingValidationError,
    },
}

#[derive(Debug)]
pub enum ConfigState {
    Loaded,
    Missing,
    Error(ConfigLoadError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchState {
    Idle,
    Succeeded,
    Error(DispatchHotkeyError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyRegistrationState {
    Unregistered,
    Registered,
    Error(HotkeySystemError),
}

/// Platform-neutral App state created at process startup.
///
/// Startup always produces an App. A missing config uses an empty AppConfig;
/// discovery, read, and parse failures remain inspectable through config_state.
#[derive(Debug)]
pub struct App {
    config: AppConfig,
    config_path: Option<PathBuf>,
    config_state: ConfigState,
    dispatch_state: DispatchState,
    last_dispatch_hotkey: Option<String>,
    hotkey_state: HotkeyRegistrationState,
    last_config_signature: Option<ConfigFileSignature>,
    reload_deadline: Option<Instant>,
}

impl App {
    pub fn start() -> Self {
        match default_config_path() {
            Ok(path) => Self::start_at(path),
            Err(error) => {
                Self::with_config_state(None, ConfigState::Error(ConfigLoadError::Path(error)))
            }
        }
    }

    pub fn start_at(path: impl Into<PathBuf>) -> Self {
        let mut app = Self::with_config_state(Some(path.into()), ConfigState::Missing);
        app.reload_config();
        app.register_config_watcher();
        app
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    pub fn reload_config(&mut self) -> &ConfigState {
        let Some(path) = self.config_path.as_deref() else {
            return &self.config_state;
        };

        match load_and_normalize_config(path) {
            Ok(Some(config)) => {
                self.config = config;
                self.config_state = ConfigState::Loaded;
            }
            Ok(None) => {
                self.config = AppConfig::default();
                self.config_state = ConfigState::Missing;
            }
            Err(error) => {
                self.config_state = ConfigState::Error(error);
            }
        }

        &self.config_state
    }

    pub fn poll_config_changes(&mut self) -> &ConfigState {
        let Some(path) = self.config_path.as_deref() else {
            return &self.config_state;
        };

        let now = Instant::now();
        let signature = match config_file_signature(path) {
            Ok(signature) => signature,
            Err(source) => {
                self.config_state = ConfigState::Error(ConfigLoadError::Read {
                    path: path.to_owned(),
                    source,
                });
                return &self.config_state;
            }
        };

        if self.last_config_signature != signature {
            self.last_config_signature = signature;
            self.reload_deadline = Some(now + CONFIG_RELOAD_DEBOUNCE);
        }

        if let Some(deadline) = self.reload_deadline {
            if now >= deadline {
                self.reload_config();
                self.reload_deadline = None;
            }
        } else if self.last_config_signature.is_none() {
            self.last_config_signature = signature;
        }

        &self.config_state
    }

    pub fn config_state(&self) -> &ConfigState {
        &self.config_state
    }

    pub fn dispatch_state(&self) -> &DispatchState {
        &self.dispatch_state
    }

    pub fn hotkey_state(&self) -> &HotkeyRegistrationState {
        &self.hotkey_state
    }
    pub fn last_dispatch_hotkey(&self) -> Option<&str> {
        self.last_dispatch_hotkey.as_deref()
    }

    pub fn register_hotkeys<H: HotkeySystem>(
        &mut self,
        hotkey_system: &mut H,
    ) -> Result<(), HotkeySystemError> {
        let hotkeys: Vec<String> = self
            .config
            .bindings
            .iter()
            .map(|binding| binding.hotkey.clone())
            .collect();

        let result = hotkey_system.register_hotkeys(&hotkeys);
        self.hotkey_state = match &result {
            Ok(()) => HotkeyRegistrationState::Registered,
            Err(source) => HotkeyRegistrationState::Error(source.clone()),
        };
        result
    }

    pub fn dispatch_hotkey<W: WindowSystem>(
        &mut self,
        hotkey: &str,
        window_system: &mut W,
    ) -> &DispatchState {
        self.last_dispatch_hotkey = Some(hotkey.to_string());
        self.dispatch_state = match dispatch_hotkey(&self.config, hotkey, window_system) {
            Ok(()) => DispatchState::Succeeded,
            Err(error) => DispatchState::Error(error),
        };
        &self.dispatch_state
    }

    pub fn dispatch_next_hotkey<W: WindowSystem, H: HotkeySystem>(
        &mut self,
        hotkey_system: &mut H,
        window_system: &mut W,
    ) -> Result<&DispatchState, HotkeySystemError> {
        let Some(HotkeyEvent::Pressed { hotkey }) = hotkey_system.next_hotkey()? else {
            return Ok(&self.dispatch_state);
        };

        Ok(self.dispatch_hotkey(&hotkey, window_system))
    }

    fn register_config_watcher(&mut self) {
        if let Some(path) = self.config_path.as_deref() {
            self.last_config_signature = config_file_signature(path).ok().flatten();
            self.reload_deadline = None;
        }
    }

    fn with_config_state(config_path: Option<PathBuf>, config_state: ConfigState) -> Self {
        Self {
            config: AppConfig::default(),
            config_path,
            config_state,
            dispatch_state: DispatchState::Idle,
            last_dispatch_hotkey: None,
            hotkey_state: HotkeyRegistrationState::Unregistered,
            last_config_signature: None,
            reload_deadline: None,
        }
    }
}

fn load_and_normalize_config(path: &Path) -> Result<Option<AppConfig>, ConfigLoadError> {
    let input = match fs::read_to_string(path) {
        Ok(input) => input,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ConfigLoadError::Read {
                path: path.to_owned(),
                source,
            });
        }
    };

    let config = parse_config(&input).map_err(|source| ConfigLoadError::Parse {
        path: path.to_owned(),
        source,
    })?;
    let config = validate_and_normalize_app_config(config).map_err(|source| {
        ConfigLoadError::Validation {
            path: path.to_owned(),
            source,
        }
    })?;
    Ok(Some(config))
}

fn config_file_signature(path: &Path) -> Result<Option<ConfigFileSignature>, io::Error> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };

    Ok(Some(ConfigFileSignature {
        modified: metadata.modified()?,
        len: metadata.len(),
    }))
}

pub fn default_config_path() -> Result<PathBuf, ConfigPathError> {
    resolve_config_path_for(current_platform(), |name| env::var_os(name))
}

#[derive(Debug, Clone, Copy)]
enum Platform {
    #[cfg(any(test, target_os = "linux"))]
    Linux,
    #[cfg(any(test, target_os = "windows"))]
    Windows,
    #[cfg(any(test, not(any(target_os = "linux", target_os = "windows"))))]
    Unsupported(&'static str),
}

#[cfg(target_os = "linux")]
fn current_platform() -> Platform {
    Platform::Linux
}

#[cfg(target_os = "windows")]
fn current_platform() -> Platform {
    Platform::Windows
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn current_platform() -> Platform {
    Platform::Unsupported(env::consts::OS)
}

fn is_linux_absolute(path: &Path) -> bool {
    path.as_os_str().as_encoded_bytes().starts_with(b"/")
}

fn resolve_config_path_for(
    platform: Platform,
    _get_env: impl Fn(&str) -> Option<OsString>,
) -> Result<PathBuf, ConfigPathError> {
    let config_root: PathBuf = match platform {
        #[cfg(any(test, target_os = "linux"))]
        Platform::Linux => Ok(_get_env("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| is_linux_absolute(path))
            .or_else(|| {
                _get_env("HOME")
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                    .map(|home| home.join(".config"))
            })
            .ok_or(ConfigPathError::MissingEnvironment {
                platform: "Linux",
                variables: "XDG_CONFIG_HOME or HOME",
            })?),
        #[cfg(any(test, target_os = "windows"))]
        Platform::Windows => Ok(_get_env("APPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or(ConfigPathError::MissingEnvironment {
                platform: "Windows",
                variables: "APPDATA",
            })?),
        #[cfg(any(test, not(any(target_os = "linux", target_os = "windows"))))]
        Platform::Unsupported(platform) => Err(ConfigPathError::UnsupportedPlatform { platform }),
    }?;

    Ok(config_root.join(CONFIG_DIRECTORY).join(CONFIG_FILE))
}

#[cfg(test)]
mod tests {
    use super::{
        App, CONFIG_FILE, CONFIG_RELOAD_DEBOUNCE, ConfigLoadError, ConfigState,
        DispatchHotkeyError, DispatchState, HotkeyRegistrationState, Platform,
        resolve_config_path_for,
    };
    use crate::{
        Action, Binding, DisplayGeometry, ExecuteActionError, FocusedWindow, HotkeyEvent,
        HotkeySystem, HotkeySystemError, Rect, WindowMove, WindowSystem, WindowSystemError,
        ZoneDefinition,
    };
    use std::collections::VecDeque;
    use std::env;
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;
    use std::thread;
    use std::time::Duration;

    fn environment<'a>(
        entries: &'a [(&'a str, &'a str)],
    ) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            entries
                .iter()
                .find(|(entry_name, _)| *entry_name == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    fn test_directory(name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!(
            "window_zones_runtime_{}_{}",
            std::process::id(),
            name
        ));
        let _ = fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn linux_prefers_absolute_xdg_config_home() {
        let entries = [("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/alice")];

        let path = resolve_config_path_for(Platform::Linux, environment(&entries)).unwrap();

        assert_eq!(path, PathBuf::from("/xdg/window_zones/config.toml"));
    }

    #[test]
    fn linux_falls_back_to_home_for_relative_xdg_config_home() {
        let entries = [("XDG_CONFIG_HOME", "relative"), ("HOME", "/home/alice")];

        let path = resolve_config_path_for(Platform::Linux, environment(&entries)).unwrap();

        assert_eq!(
            path,
            PathBuf::from("/home/alice/.config/window_zones/config.toml")
        );
    }

    #[test]
    fn windows_uses_roaming_app_data() {
        let entries = [("APPDATA", "/roaming")];

        let path = resolve_config_path_for(Platform::Windows, environment(&entries)).unwrap();

        assert_eq!(path, PathBuf::from("/roaming/window_zones/config.toml"));
    }

    #[test]
    fn unsupported_platform_has_an_explicit_error() {
        let error =
            resolve_config_path_for(Platform::Unsupported("plan9"), environment(&[])).unwrap_err();

        assert_eq!(
            error.to_string(),
            "config discovery is not supported on plan9"
        );
    }

    #[test]
    fn missing_config_boots_with_empty_bindings() {
        let directory = test_directory("missing");
        let path = directory.join(CONFIG_FILE);

        let app = App::start_at(&path);

        assert!(matches!(app.config_state(), ConfigState::Missing));
        assert!(app.config().bindings.is_empty());
        assert_eq!(app.config_path(), Some(path.as_path()));
    }
    #[test]
    fn dispatch_state_starts_without_last_dispatch_hotkey() {
        let directory = test_directory("no_last_action");
        let path = directory.join(CONFIG_FILE);

        let app = App::start_at(&path);

        assert_eq!(app.last_dispatch_hotkey(), None);
    }

    #[test]
    fn existing_config_is_loaded_at_startup() {
        let directory = test_directory("loaded");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }
"#,
        )
        .unwrap();

        let app = App::start_at(&path);

        assert!(matches!(app.config_state(), ConfigState::Loaded));
        assert_eq!(
            app.config().bindings,
            vec![Binding {
                hotkey: "alt+ctrl+left".to_string(),
                action: Action::MoveToZone {
                    zone: "left-half".to_string()
                },
            }]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn custom_zones_are_loaded_from_config() {
        let directory = test_directory("custom_zone");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"[zones]
side = { x = 10, y = 0, width = 50, height = 100 }

[[bindings]]
hotkey = "Ctrl+Alt+Right"
action = { type = "move-to-zone", zone = "side" }
"#,
        )
        .unwrap();

        let mut app = App::start_at(&path);
        let mut window_system = FakeWindowSystem::with_focus("left", Rect::new(200, 200, 800, 600));

        assert_eq!(
            app.config().zones,
            std::collections::BTreeMap::from([(
                "side".to_string(),
                ZoneDefinition {
                    x: 10,
                    y: 0,
                    width: 50,
                    height: 100,
                },
            )])
        );

        let state = app.dispatch_hotkey("Ctrl+Alt+Right", &mut window_system);
        assert_eq!(state, &DispatchState::Succeeded);
        assert_eq!(
            window_system.moves,
            vec![WindowMove::new(Rect::new(192, 0, 960, 1080))]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn duplicate_bindings_are_rejected_at_startup() {
        let directory = test_directory("validation_error");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }

[[bindings]]
hotkey = "ctrl+alt+left"
action = { type = "move-to-next-display" }
"#,
        )
        .unwrap();

        let app = App::start_at(&path);

        match app.config_state() {
            ConfigState::Error(ConfigLoadError::Validation {
                path: error_path,
                source,
            }) => {
                assert_eq!(error_path, &path);
                assert_eq!(
                    source.to_string(),
                    "duplicate binding for hotkey alt+ctrl+left"
                );
            }
            state => panic!("expected validation error, got {state:?}"),
        }
        assert!(app.config().bindings.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn parse_failure_keeps_path_and_actionable_diagnostic() {
        let directory = test_directory("parse_error");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(&path, "bindings = [").unwrap();

        let app = App::start_at(&path);

        match app.config_state() {
            ConfigState::Error(ConfigLoadError::Parse {
                path: error_path,
                source,
            }) => {
                assert_eq!(error_path, &path);
                assert!(source.to_string().contains("invalid TOML config"));
            }
            state => panic!("expected parse error, got {state:?}"),
        }
        assert!(app.config().bindings.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn read_failure_keeps_path_and_os_diagnostic() {
        let directory = test_directory("read_error");
        fs::create_dir_all(&directory).unwrap();

        let app = App::start_at(&directory);

        match app.config_state() {
            ConfigState::Error(ConfigLoadError::Read { path, .. }) => {
                assert_eq!(path, &directory);
            }
            state => panic!("expected read error, got {state:?}"),
        }
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn reload_config_from_path_replaces_bindings_on_success() {
        let directory = test_directory("reload_success");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }
"#,
        )
        .unwrap();

        let mut app = App::start_at(&path);
        assert_eq!(
            app.config().bindings,
            vec![Binding {
                hotkey: "alt+ctrl+left".to_string(),
                action: Action::MoveToZone {
                    zone: "left-half".to_string()
                },
            }]
        );

        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Shift+Alt+Right"
action = { type = "move-to-next-display" }
"#,
        )
        .unwrap();

        let state = app.reload_config();
        assert!(matches!(state, ConfigState::Loaded));
        assert_eq!(
            app.config().bindings,
            vec![Binding {
                hotkey: "alt+shift+right".to_string(),
                action: Action::MoveToNextDisplay,
            }]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reload_config_keeps_last_valid_bindings_after_parse_error() {
        let directory = test_directory("reload_parse_error");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }
"#,
        )
        .unwrap();

        let mut app = App::start_at(&path);
        fs::write(&path, "bindings = [").unwrap();

        let state = app.reload_config();
        match state {
            ConfigState::Error(ConfigLoadError::Parse {
                path: error_path, ..
            }) => assert_eq!(error_path, &path),
            state => panic!("expected parse error, got {state:?}"),
        }
        assert_eq!(
            app.config().bindings,
            vec![Binding {
                hotkey: "alt+ctrl+left".to_string(),
                action: Action::MoveToZone {
                    zone: "left-half".to_string()
                },
            }]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reload_config_keeps_last_valid_bindings_after_validation_error() {
        let directory = test_directory("reload_validation_error");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }
"#,
        )
        .unwrap();

        let mut app = App::start_at(&path);
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "right-half" }

[[bindings]]
hotkey = "Alt+Ctrl+Left"
action = { type = "move-to-next-display" }
"#,
        )
        .unwrap();

        let state = app.reload_config();
        match state {
            ConfigState::Error(ConfigLoadError::Validation {
                path: error_path,
                source,
            }) => {
                assert_eq!(error_path, &path);
                assert_eq!(
                    source.to_string(),
                    "duplicate binding for hotkey alt+ctrl+left"
                );
            }
            state => panic!("expected validation error, got {state:?}"),
        }
        assert_eq!(
            app.config().bindings,
            vec![Binding {
                hotkey: "alt+ctrl+left".to_string(),
                action: Action::MoveToZone {
                    zone: "left-half".to_string()
                },
            }]
        );
        fs::remove_dir_all(directory).unwrap();
    }
    #[test]
    fn reload_config_updates_bindings_for_subsequent_dispatches() {
        let directory = test_directory("reload_dispatches");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }
"#,
        )
        .unwrap();

        let mut app = App::start_at(&path);
        let mut window_system = FakeWindowSystem::with_focus("left", Rect::new(200, 200, 800, 600));

        let state = app.dispatch_hotkey("Ctrl+Alt+Left", &mut window_system);
        assert_eq!(state, &DispatchState::Succeeded);
        assert_eq!(
            window_system.moves,
            vec![WindowMove::new(Rect::new(0, 0, 960, 1080))]
        );

        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Right"
action = { type = "move-to-zone", zone = "right-half" }
"#,
        )
        .unwrap();

        let state = app.reload_config();
        assert!(matches!(state, ConfigState::Loaded));
        assert_eq!(
            app.config().bindings,
            vec![Binding {
                hotkey: "alt+ctrl+right".to_string(),
                action: Action::MoveToZone {
                    zone: "right-half".to_string()
                },
            }]
        );

        let state = app.dispatch_hotkey("Ctrl+Alt+Right", &mut window_system);
        assert_eq!(state, &DispatchState::Succeeded);
        assert_eq!(
            window_system.moves,
            vec![
                WindowMove::new(Rect::new(0, 0, 960, 1080)),
                WindowMove::new(Rect::new(960, 0, 960, 1080)),
            ]
        );
        let state = app.dispatch_hotkey("Ctrl+Alt+Left", &mut window_system);
        assert_eq!(
            state,
            &DispatchState::Error(DispatchHotkeyError::NoBindingForHotkey {
                hotkey: "Ctrl+Alt+Left".to_string()
            })
        );
        assert_eq!(window_system.moves.len(), 2);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn reload_parse_error_keeps_last_known_bindings_and_dispatches_them() {
        let directory = test_directory("reload_parse_error_dispatched");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }
"#,
        )
        .unwrap();

        let mut app = App::start_at(&path);
        let mut window_system = FakeWindowSystem::with_focus("left", Rect::new(200, 200, 800, 600));
        fs::write(&path, "bindings = [").unwrap();

        let state = app.reload_config();
        match state {
            ConfigState::Error(ConfigLoadError::Parse {
                path: error_path,
                source,
            }) => {
                assert_eq!(error_path, &path);
                assert!(source.to_string().contains("invalid TOML config"));
            }
            state => panic!("expected parse error, got {state:?}"),
        }

        let state = app.dispatch_hotkey("Ctrl+Alt+Left", &mut window_system);
        assert_eq!(state, &DispatchState::Succeeded);
        let state = app.dispatch_hotkey("Ctrl+Alt+Right", &mut window_system);
        assert_eq!(
            state,
            &DispatchState::Error(DispatchHotkeyError::NoBindingForHotkey {
                hotkey: "Ctrl+Alt+Right".to_string()
            })
        );
        assert_eq!(
            window_system.moves,
            vec![WindowMove::new(Rect::new(0, 0, 960, 1080))]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn poll_config_changes_reloads_and_updates_bindings() {
        let directory = test_directory("polling_reload_success");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }
"#,
        )
        .unwrap();

        let mut app = App::start_at(&path);
        let mut window_system = FakeWindowSystem::with_focus("left", Rect::new(200, 200, 800, 600));

        let state = app.dispatch_hotkey("Ctrl+Alt+Left", &mut window_system);
        assert_eq!(state, &DispatchState::Succeeded);
        assert_eq!(
            app.config().bindings,
            vec![Binding {
                hotkey: "alt+ctrl+left".to_string(),
                action: Action::MoveToZone {
                    zone: "left-half".to_string()
                },
            }]
        );

        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Shift+Alt+Right"
action = { type = "move-to-next-display" }
"#,
        )
        .unwrap();

        let state = app.poll_config_changes();
        assert!(matches!(state, ConfigState::Loaded));
        assert_eq!(
            app.config().bindings,
            vec![Binding {
                hotkey: "alt+ctrl+left".to_string(),
                action: Action::MoveToZone {
                    zone: "left-half".to_string()
                },
            }]
        );

        let state = app.dispatch_hotkey("Ctrl+Alt+Left", &mut window_system);
        assert_eq!(state, &DispatchState::Succeeded);

        thread::sleep(CONFIG_RELOAD_DEBOUNCE + Duration::from_millis(50));
        let state = app.poll_config_changes();
        assert!(matches!(state, ConfigState::Loaded));
        assert_eq!(
            app.config().bindings,
            vec![Binding {
                hotkey: "alt+shift+right".to_string(),
                action: Action::MoveToNextDisplay,
            }]
        );

        let state = app.dispatch_hotkey("Shift+Alt+Right", &mut window_system);
        assert_eq!(state, &DispatchState::Succeeded);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn poll_config_changes_debounces_rapid_successive_writes() {
        let directory = test_directory("polling_debounce");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }
"#,
        )
        .unwrap();

        let mut app = App::start_at(&path);

        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Right"
action = { type = "move-to-zone", zone = "right-half" }
"#,
        )
        .unwrap();
        app.poll_config_changes();

        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Shift+Alt+Right"
action = { type = "move-to-next-display" }
"#,
        )
        .unwrap();
        app.poll_config_changes();

        assert_eq!(
            app.config().bindings,
            vec![Binding {
                hotkey: "alt+ctrl+left".to_string(),
                action: Action::MoveToZone {
                    zone: "left-half".to_string()
                },
            }]
        );

        thread::sleep(CONFIG_RELOAD_DEBOUNCE + Duration::from_millis(50));
        let state = app.poll_config_changes();
        assert!(matches!(state, ConfigState::Loaded));
        assert_eq!(
            app.config().bindings,
            vec![Binding {
                hotkey: "alt+shift+right".to_string(),
                action: Action::MoveToNextDisplay,
            }]
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn poll_config_changes_survives_read_errors_without_terminating_runtime() {
        let directory = test_directory("polling_read_error");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }
"#,
        )
        .unwrap();

        let mut app = App::start_at(&path);
        let mut window_system = FakeWindowSystem::with_focus("left", Rect::new(200, 200, 800, 600));

        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();

        app.poll_config_changes();
        thread::sleep(CONFIG_RELOAD_DEBOUNCE + Duration::from_millis(50));
        let state = app.poll_config_changes();
        match state {
            ConfigState::Error(ConfigLoadError::Read {
                path: error_path, ..
            }) => {
                assert_eq!(error_path, &path);
            }
            state => panic!("expected read error, got {state:?}"),
        }

        let state = app.dispatch_hotkey("Ctrl+Alt+Left", &mut window_system);
        assert_eq!(state, &DispatchState::Succeeded);

        fs::remove_dir_all(&path).unwrap();
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Right"
action = { type = "move-to-zone", zone = "right-half" }
"#,
        )
        .unwrap();

        app.poll_config_changes();
        thread::sleep(CONFIG_RELOAD_DEBOUNCE + Duration::from_millis(50));
        let state = app.poll_config_changes();
        assert!(matches!(state, ConfigState::Loaded));
        assert_eq!(
            app.config().bindings,
            vec![Binding {
                hotkey: "alt+ctrl+right".to_string(),
                action: Action::MoveToZone {
                    zone: "right-half".to_string()
                },
            }]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn malformed_reload_does_not_corrupt_hotkey_registration_or_dispatch() {
        let directory = test_directory("reload_malformed_does_not_corrupt_runtime");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }
"#,
        )
        .unwrap();

        let mut app = App::start_at(&path);
        let mut hotkey_system = FakeHotkeySystem::new(vec![Ok(HotkeyEvent::Pressed {
            hotkey: "Ctrl+Alt+Left".to_string(),
        })]);
        app.register_hotkeys(&mut hotkey_system).unwrap();
        assert_eq!(
            hotkey_system.registered_hotkeys,
            vec!["alt+ctrl+left".to_string()]
        );
        assert_eq!(app.hotkey_state(), &HotkeyRegistrationState::Registered);

        fs::write(&path, "bindings = [").unwrap();
        let state = app.reload_config();
        match state {
            ConfigState::Error(ConfigLoadError::Parse {
                path: error_path, ..
            }) => {
                assert_eq!(error_path, &path);
            }
            state => panic!("expected parse error, got {state:?}"),
        }

        let mut window_system = FakeWindowSystem::with_focus("left", Rect::new(200, 200, 800, 600));
        let dispatch_state = app
            .dispatch_next_hotkey(&mut hotkey_system, &mut window_system)
            .unwrap();
        assert_eq!(dispatch_state, &DispatchState::Succeeded);
        assert_eq!(
            window_system.moves,
            vec![WindowMove::new(Rect::new(0, 0, 960, 1080))]
        );

        app.register_hotkeys(&mut hotkey_system).unwrap();
        assert_eq!(app.hotkey_state(), &HotkeyRegistrationState::Registered);
        assert_eq!(
            hotkey_system.registered_hotkeys,
            vec!["alt+ctrl+left".to_string()]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[derive(Debug)]
    struct NoFocusedWindow;

    impl WindowSystem for NoFocusedWindow {
        fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
            Ok(None)
        }

        fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
            Ok(Vec::new())
        }

        fn move_focused_window(
            &mut self,
            _window_move: WindowMove,
        ) -> Result<(), WindowSystemError> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct FakeHotkeySystem {
        registered_hotkeys: Vec<String>,
        register_error: Option<HotkeySystemError>,
        events: VecDeque<Result<HotkeyEvent, HotkeySystemError>>,
    }

    impl FakeHotkeySystem {
        fn new(events: Vec<Result<HotkeyEvent, HotkeySystemError>>) -> Self {
            Self {
                registered_hotkeys: Vec::new(),
                register_error: None,
                events: events.into_iter().collect(),
            }
        }

        fn with_registration_error(error: HotkeySystemError) -> Self {
            Self {
                registered_hotkeys: Vec::new(),
                register_error: Some(error),
                events: VecDeque::new(),
            }
        }
    }

    impl HotkeySystem for FakeHotkeySystem {
        fn register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), HotkeySystemError> {
            self.registered_hotkeys = hotkeys.to_vec();
            match &self.register_error {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            }
        }

        fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
            self.events.pop_front().transpose()
        }
    }

    #[derive(Debug)]
    struct FakeWindowSystem {
        focused_window: Result<Option<FocusedWindow>, WindowSystemError>,
        displays: Result<Vec<DisplayGeometry>, WindowSystemError>,
        moves: Vec<WindowMove>,
        move_error: Option<WindowSystemError>,
    }

    impl FakeWindowSystem {
        fn with_focus(display_id: &str, geometry: Rect) -> Self {
            Self {
                focused_window: Ok(Some(FocusedWindow::new(display_id, geometry))),
                displays: Ok(vec![
                    DisplayGeometry::new("left", Rect::new(0, 0, 1920, 1080)),
                    DisplayGeometry::new("right", Rect::new(1920, 0, 2560, 1440)),
                ]),
                moves: Vec::new(),
                move_error: None,
            }
        }
    }

    impl WindowSystem for FakeWindowSystem {
        fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
            self.focused_window.clone()
        }

        fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
            self.displays.clone()
        }

        fn move_focused_window(
            &mut self,
            window_move: WindowMove,
        ) -> Result<(), WindowSystemError> {
            if let Some(error) = self.move_error.clone() {
                return Err(error);
            }

            self.moves.push(window_move);
            Ok(())
        }
    }

    #[test]
    fn registers_hotkeys_with_hotkey_adapter() {
        let directory = test_directory("register_hotkeys");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }
[[bindings]]
hotkey = "Ctrl+Alt+Shift+Right"
action = { type = "move-to-next-display" }
"#,
        )
        .unwrap();

        let mut app = App::start_at(&path);
        let mut hotkey_system = FakeHotkeySystem::new(Vec::new());

        assert_eq!(app.hotkey_state(), &HotkeyRegistrationState::Unregistered);
        app.register_hotkeys(&mut hotkey_system).unwrap();
        assert_eq!(app.hotkey_state(), &HotkeyRegistrationState::Registered);
        assert_eq!(
            hotkey_system.registered_hotkeys,
            vec![
                "alt+ctrl+left".to_string(),
                "alt+ctrl+shift+right".to_string()
            ]
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn registration_errors_are_exposed_from_hotkey_adapter() {
        let directory = test_directory("register_hotkeys_failed");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }
"#,
        )
        .unwrap();

        let mut app = App::start_at(&path);
        let mut hotkey_system = FakeHotkeySystem::with_registration_error(
            HotkeySystemError::Platform("permission denied".to_string()),
        );
        let error = app.register_hotkeys(&mut hotkey_system).unwrap_err();

        assert_eq!(
            app.hotkey_state(),
            &HotkeyRegistrationState::Error(HotkeySystemError::Platform(
                "permission denied".to_string()
            ))
        );
        assert_eq!(
            error.to_string(),
            "platform hotkey error: permission denied"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn dispatch_next_hotkey_uses_system_events_and_dispatches_known_bindings() {
        let directory = test_directory("dispatch_from_hotkeys");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }
"#,
        )
        .unwrap();

        let mut app = App::start_at(path);
        let mut hotkey_system = FakeHotkeySystem::new(vec![Ok(HotkeyEvent::Pressed {
            hotkey: "Ctrl+Alt+Left".to_string(),
        })]);
        let mut window_system = FakeWindowSystem::with_focus("left", Rect::new(200, 200, 800, 600));

        let state = app
            .dispatch_next_hotkey(&mut hotkey_system, &mut window_system)
            .unwrap();

        assert_eq!(state, &DispatchState::Succeeded);
        assert_eq!(
            window_system.moves,
            vec![WindowMove::new(Rect::new(0, 0, 960, 1080))]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn dispatch_next_hotkey_ignores_unknown_binding_without_move() {
        let directory = test_directory("dispatch_unknown_hotkey");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }
"#,
        )
        .unwrap();

        let mut app = App::start_at(path);
        let mut hotkey_system = FakeHotkeySystem::new(vec![Ok(HotkeyEvent::Pressed {
            hotkey: "Alt+Shift+Right".to_string(),
        })]);
        let mut window_system = FakeWindowSystem::with_focus("left", Rect::new(200, 200, 800, 600));

        let state = app
            .dispatch_next_hotkey(&mut hotkey_system, &mut window_system)
            .unwrap();

        assert_eq!(
            state,
            &DispatchState::Error(DispatchHotkeyError::NoBindingForHotkey {
                hotkey: "Alt+Shift+Right".to_string()
            })
        );
        assert!(window_system.moves.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn dispatch_next_hotkey_tracks_last_hotkey_on_success() {
        let directory = test_directory("dispatch_last_action_success");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }
"#,
        )
        .unwrap();

        let mut app = App::start_at(path);
        let mut hotkey_system = FakeHotkeySystem::new(vec![Ok(HotkeyEvent::Pressed {
            hotkey: "Ctrl+Alt+Left".to_string(),
        })]);
        let mut window_system = FakeWindowSystem::with_focus("left", Rect::new(200, 200, 800, 600));

        let state = app
            .dispatch_next_hotkey(&mut hotkey_system, &mut window_system)
            .unwrap();

        assert_eq!(state, &DispatchState::Succeeded);
        assert_eq!(app.last_dispatch_hotkey(), Some("Ctrl+Alt+Left"));
        assert_eq!(
            window_system.moves,
            vec![WindowMove::new(Rect::new(0, 0, 960, 1080))]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn dispatch_next_hotkey_tracks_last_hotkey_on_error() {
        let directory = test_directory("dispatch_last_action_error");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }
"#,
        )
        .unwrap();

        let mut app = App::start_at(path);
        let mut hotkey_system = FakeHotkeySystem::new(vec![Ok(HotkeyEvent::Pressed {
            hotkey: "Alt+Shift+Right".to_string(),
        })]);
        let mut window_system = FakeWindowSystem::with_focus("left", Rect::new(200, 200, 800, 600));

        let state = app
            .dispatch_next_hotkey(&mut hotkey_system, &mut window_system)
            .unwrap();

        assert_eq!(
            state,
            &DispatchState::Error(DispatchHotkeyError::NoBindingForHotkey {
                hotkey: "Alt+Shift+Right".to_string()
            })
        );
        assert_eq!(app.last_dispatch_hotkey(), Some("Alt+Shift+Right"));
        assert!(window_system.moves.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn no_focused_window_remains_an_explicit_dispatch_state() {
        let directory = test_directory("no_focus");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }
"#,
        )
        .unwrap();
        let mut app = App::start_at(path);
        let mut window_system = NoFocusedWindow;

        let state = app.dispatch_hotkey("Ctrl+Alt+Left", &mut window_system);

        assert_eq!(
            state,
            &DispatchState::Error(DispatchHotkeyError::ExecuteAction(
                ExecuteActionError::NoFocusedWindow
            ))
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn runtime_dispatches_loaded_bindings_without_reparsing() {
        let directory = test_directory("dispatch");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(CONFIG_FILE);
        fs::write(
            &path,
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }
"#,
        )
        .unwrap();
        let app = App::start_at(path);

        assert_eq!(
            app.config().bindings,
            vec![Binding {
                hotkey: "alt+ctrl+left".to_string(),
                action: Action::MoveToZone {
                    zone: "left-half".to_string(),
                },
            }]
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
