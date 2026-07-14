use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::config::{AppConfig, ConfigError, parse_config};
use crate::dispatcher::{DispatchHotkeyError, dispatch_hotkey};
use crate::window_system::WindowSystem;

const CONFIG_DIRECTORY: &str = "window_zones";
const CONFIG_FILE: &str = "config.toml";

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
}

impl App {
    pub fn start() -> Self {
        match default_config_path() {
            Ok(path) => Self::start_at(path),
            Err(error) => Self::with_config_state(None, ConfigState::Error(error.into())),
        }
    }

    pub fn start_at(path: impl Into<PathBuf>) -> Self {
        let path = path.into();

        match fs::read_to_string(&path) {
            Ok(input) => match parse_config(&input) {
                Ok(config) => Self {
                    config,
                    config_path: Some(path),
                    config_state: ConfigState::Loaded,
                    dispatch_state: DispatchState::Idle,
                },
                Err(source) => Self::with_config_state(
                    Some(path.clone()),
                    ConfigState::Error(ConfigLoadError::Parse { path, source }),
                ),
            },
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                Self::with_config_state(Some(path), ConfigState::Missing)
            }
            Err(source) => Self::with_config_state(
                Some(path.clone()),
                ConfigState::Error(ConfigLoadError::Read { path, source }),
            ),
        }
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    pub fn config_state(&self) -> &ConfigState {
        &self.config_state
    }

    pub fn dispatch_state(&self) -> &DispatchState {
        &self.dispatch_state
    }

    pub fn dispatch_hotkey<W: WindowSystem>(
        &mut self,
        hotkey: &str,
        window_system: &mut W,
    ) -> &DispatchState {
        self.dispatch_state = match dispatch_hotkey(&self.config, hotkey, window_system) {
            Ok(()) => DispatchState::Succeeded,
            Err(error) => DispatchState::Error(error),
        };
        &self.dispatch_state
    }

    fn with_config_state(config_path: Option<PathBuf>, config_state: ConfigState) -> Self {
        Self {
            config: AppConfig::default(),
            config_path,
            config_state,
            dispatch_state: DispatchState::Idle,
        }
    }
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
    get_env: impl Fn(&str) -> Option<OsString>,
) -> Result<PathBuf, ConfigPathError> {
    let config_root = match platform {
        #[cfg(any(test, target_os = "linux"))]
        Platform::Linux => get_env("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| is_linux_absolute(path))
            .or_else(|| {
                get_env("HOME")
                    .filter(|value| !value.is_empty())
                    .map(PathBuf::from)
                    .map(|home| home.join(".config"))
            })
            .ok_or(ConfigPathError::MissingEnvironment {
                platform: "Linux",
                variables: "XDG_CONFIG_HOME or HOME",
            })?,
        #[cfg(any(test, target_os = "windows"))]
        Platform::Windows => get_env("APPDATA")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or(ConfigPathError::MissingEnvironment {
                platform: "Windows",
                variables: "APPDATA",
            })?,
        #[cfg(any(test, not(any(target_os = "linux", target_os = "windows"))))]
        Platform::Unsupported(platform) => {
            return Err(ConfigPathError::UnsupportedPlatform { platform });
        }
    };

    Ok(config_root.join(CONFIG_DIRECTORY).join(CONFIG_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Action, Binding, BuiltInZone, DisplayGeometry, ExecuteActionError, FocusedWindow,
        WindowMove, WindowSystemError,
    };

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
        assert_eq!(app.config().bindings.len(), 1);
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
            ConfigState::Error(
                error @ ConfigLoadError::Parse {
                    path: error_path, ..
                },
            ) => {
                assert_eq!(error_path, &path);
                assert!(error.to_string().contains(&path.display().to_string()));
                assert!(error.to_string().contains("invalid TOML config"));
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
            ConfigState::Error(error @ ConfigLoadError::Read { path, .. }) => {
                assert_eq!(path, &directory);
                assert!(error.to_string().contains(&directory.display().to_string()));
            }
            state => panic!("expected read error, got {state:?}"),
        }
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
                hotkey: "Ctrl+Alt+Left".to_string(),
                action: Action::MoveToZone {
                    zone: BuiltInZone::LeftHalf,
                },
            }]
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
