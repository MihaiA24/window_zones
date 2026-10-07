use std::env;
use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use thiserror::Error;

use crate::config::{
    AppConfig, BindingValidationError, ConfigError, normalize_hotkey, parse_config,
};
use crate::dispatcher::{DispatchHotkeyError, dispatch_hotkey};
use crate::executor::WindowHistory;
use crate::hotkey_system::{HotkeyEvent, HotkeySystem, HotkeySystemError};
use crate::window_system::WindowSystem;
const CONFIG_DIRECTORY: &str = "window_zones";
const CONFIG_FILE: &str = "config.toml";
const CONFIG_RELOAD_DEBOUNCE: Duration = Duration::from_millis(150);
/// Delay between registration attempts while hotkeys are unavailable, so an
/// absent companion is not re-probed on every runtime tick.
pub const HOTKEY_RETRY_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ConfigFileSignature(u64);

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
    #[error("hotkeys in {path} were refused: {message}; the previous bindings remain active")]
    HotkeysRejected { path: PathBuf, message: String },
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

/// A runtime state change worth reporting to the user. Events are produced on
/// transitions only, so a surface can print every one without repeating itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeEvent {
    ConfigLoaded {
        bindings: usize,
    },
    ConfigMissing {
        path: PathBuf,
    },
    /// The config could not be read or parsed; the previous bindings stay active.
    ConfigFailed {
        message: String,
    },
    /// The config's hotkey set was refused; the previous config and hotkeys stay active.
    ConfigRejected {
        message: String,
    },
    HotkeysRegistered {
        count: usize,
    },
    /// Hotkeys registered again after a reported registration problem.
    HotkeysRecovered {
        count: usize,
    },
    /// The hotkey set was refused and nothing is registered until the config changes.
    HotkeysRejected {
        message: String,
    },
    /// Hotkeys cannot be registered right now; registration is retried.
    HotkeysUnavailable {
        message: String,
    },
}

impl RuntimeEvent {
    /// Whether the event reports something the user has to act on.
    pub fn is_problem(&self) -> bool {
        matches!(
            self,
            Self::ConfigFailed { .. }
                | Self::ConfigRejected { .. }
                | Self::HotkeysRejected { .. }
                | Self::HotkeysUnavailable { .. }
        )
    }
}

impl fmt::Display for RuntimeEvent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfigLoaded { bindings } => {
                write!(formatter, "Config state: Loaded ({bindings} bindings)")
            }
            Self::ConfigMissing { path } => write!(
                formatter,
                "Config state: Missing; run `window_zones init` to create {}",
                path.display()
            ),
            Self::ConfigFailed { message } => write!(
                formatter,
                "Config reload error: {message}; the previous bindings remain active"
            ),
            Self::ConfigRejected { message } => write!(formatter, "Config reload error: {message}"),
            Self::HotkeysRegistered { count } => write!(formatter, "Hotkeys registered: {count}"),
            Self::HotkeysRecovered { count } => write!(
                formatter,
                "Hotkey registration now recovered: {count} hotkeys registered"
            ),
            Self::HotkeysRejected { message } => write!(
                formatter,
                "Hotkey registration refused: {message}; no hotkeys are active until the config changes"
            ),
            Self::HotkeysUnavailable { message } => {
                write!(formatter, "Hotkey registration failed: {message}; retrying")
            }
        }
    }
}

/// Platform-neutral App state created at process startup.
///
/// Startup always produces an App. A missing config uses an empty AppConfig;
/// discovery, read, and parse failures remain inspectable through config_state.
/// The active config and the hotkey system's registered set change together:
/// a config whose hotkeys the hotkey system refuses is not applied.
#[derive(Debug)]
pub struct App {
    config: AppConfig,
    config_path: Option<PathBuf>,
    config_state: ConfigState,
    dispatch_state: DispatchState,
    last_dispatch_hotkey: Option<String>,
    hotkey_state: HotkeyRegistrationState,
    history: WindowHistory,
    /// The set the hotkey system currently has registered, when known.
    registered_hotkeys: Option<Vec<String>>,
    /// A set the hotkey system refused; not retried until the config changes.
    rejected_hotkeys: Option<Vec<String>>,
    registration_retry_at: Option<Instant>,
    last_config_error: Option<String>,
    last_hotkey_error: Option<String>,
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
        match app.load_config() {
            Some(Ok(Some(config))) => {
                app.config = config;
                app.config_state = ConfigState::Loaded;
            }
            Some(Ok(None)) | None => {}
            Some(Err(error)) => app.config_state = ConfigState::Error(error),
        }
        app.last_config_error = app.config_error_message();
        app.register_config_watcher();
        app
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

    pub fn hotkey_state(&self) -> &HotkeyRegistrationState {
        &self.hotkey_state
    }

    pub fn last_dispatch_hotkey(&self) -> Option<&str> {
        self.last_dispatch_hotkey.as_deref()
    }

    /// Whether the hotkey system holds a registered set events can be read from.
    pub fn hotkeys_registered(&self) -> bool {
        self.registered_hotkeys.is_some()
    }

    /// The event describing the config as loaded at startup.
    pub fn startup_event(&self) -> RuntimeEvent {
        match &self.config_state {
            ConfigState::Loaded => RuntimeEvent::ConfigLoaded {
                bindings: self.config.bindings.len(),
            },
            ConfigState::Missing => RuntimeEvent::ConfigMissing {
                path: self.config_path.clone().unwrap_or_default(),
            },
            ConfigState::Error(error) => RuntimeEvent::ConfigFailed {
                message: error.to_string(),
            },
        }
    }

    /// Picks up config file changes after a short debounce and keeps the hotkey
    /// system's registered set in step with the active config.
    pub fn tick<H: HotkeySystem>(
        &mut self,
        hotkey_system: &mut H,
        now: Instant,
    ) -> Vec<RuntimeEvent> {
        let mut events = Vec::new();
        if let Some(loaded) = self.poll_config_file(now) {
            self.apply_loaded_config(loaded, hotkey_system, now, &mut events);
        }
        self.sync_hotkeys(hotkey_system, now, &mut events);
        events
    }

    /// Reloads the config file now, applying it only if its hotkeys register.
    pub fn reload<H: HotkeySystem>(
        &mut self,
        hotkey_system: &mut H,
        now: Instant,
    ) -> Vec<RuntimeEvent> {
        let mut events = Vec::new();
        if let Some(path) = self.config_path.as_deref() {
            self.last_config_signature = config_file_signature(path).ok().flatten();
            self.reload_deadline = None;
            // A manual reload always reports its outcome, even an unchanged error.
            self.last_config_error = None;
        }
        if let Some(loaded) = self.load_config() {
            self.apply_loaded_config(loaded, hotkey_system, now, &mut events);
        }
        self.sync_hotkeys(hotkey_system, now, &mut events);
        events
    }

    /// Records that the hotkey system stopped delivering events; registration
    /// is retried on the next tick.
    pub fn hotkeys_lost(&mut self, error: HotkeySystemError, now: Instant) -> Option<RuntimeEvent> {
        self.registered_hotkeys = None;
        self.registration_retry_at = Some(now);
        self.hotkey_failed(error)
    }

    pub fn dispatch_hotkey<W: WindowSystem>(
        &mut self,
        hotkey: &str,
        window_system: &mut W,
    ) -> &DispatchState {
        // State names the canonical binding, so a Companion hotkey event and a manual `dispatch`
        // of the same binding report one spelling instead of two.
        self.last_dispatch_hotkey =
            Some(normalize_hotkey(hotkey).unwrap_or_else(|_| hotkey.to_string()));
        self.dispatch_state =
            match dispatch_hotkey(&self.config, hotkey, &mut self.history, window_system) {
                Ok(()) => DispatchState::Succeeded,
                Err(error) => DispatchState::Error(error),
            };
        &self.dispatch_state
    }

    /// Dispatches the next queued hotkey event. `Ok(None)` means the queue is
    /// empty; callers loop until then to drain every press of one poll.
    pub fn dispatch_next_hotkey<W: WindowSystem, H: HotkeySystem>(
        &mut self,
        hotkey_system: &mut H,
        window_system: &mut W,
    ) -> Result<Option<&DispatchState>, HotkeySystemError> {
        let Some(HotkeyEvent::Pressed { hotkey }) = hotkey_system.next_hotkey()? else {
            return Ok(None);
        };

        Ok(Some(self.dispatch_hotkey(&hotkey, window_system)))
    }

    fn load_config(&self) -> Option<Result<Option<AppConfig>, ConfigLoadError>> {
        Some(load_and_normalize_config(self.config_path.as_deref()?))
    }

    /// Returns a load result once a changed config file has been stable for the
    /// debounce interval.
    fn poll_config_file(
        &mut self,
        now: Instant,
    ) -> Option<Result<Option<AppConfig>, ConfigLoadError>> {
        let path = self.config_path.as_deref()?;

        let signature = match config_file_signature(path) {
            Ok(signature) => signature,
            Err(source) => {
                // Once the file is readable again it reloads even if its content is unchanged.
                self.last_config_signature = None;
                return Some(Err(ConfigLoadError::Read {
                    path: path.to_owned(),
                    source,
                }));
            }
        };

        if self.last_config_signature != signature {
            self.last_config_signature = signature;
            self.reload_deadline = Some(now + CONFIG_RELOAD_DEBOUNCE);
        }

        let deadline = self.reload_deadline?;
        if now < deadline {
            return None;
        }
        self.reload_deadline = None;
        self.load_config()
    }

    fn apply_loaded_config<H: HotkeySystem>(
        &mut self,
        loaded: Result<Option<AppConfig>, ConfigLoadError>,
        hotkey_system: &mut H,
        now: Instant,
        events: &mut Vec<RuntimeEvent>,
    ) {
        let candidate = match loaded {
            Ok(candidate) => candidate,
            Err(error) => {
                let message = error.to_string();
                self.config_state = ConfigState::Error(error);
                if self.last_config_error.as_ref() != Some(&message) {
                    self.last_config_error = Some(message.clone());
                    events.push(RuntimeEvent::ConfigFailed { message });
                }
                return;
            }
        };

        let missing = candidate.is_none();
        let config = candidate.unwrap_or_default();
        let hotkeys = hotkeys_of(&config);
        if let Some(registered) = &self.registered_hotkeys
            && *registered != hotkeys
        {
            match hotkey_system.register_hotkeys(&hotkeys) {
                Ok(()) => self.hotkeys_registered_as(hotkeys, events),
                Err(HotkeySystemError::Rejected(message)) => {
                    let error = ConfigLoadError::HotkeysRejected {
                        path: self.config_path.clone().unwrap_or_default(),
                        message,
                    };
                    let message = error.to_string();
                    self.config_state = ConfigState::Error(error);
                    if self.last_config_error.as_ref() != Some(&message) {
                        self.last_config_error = Some(message.clone());
                        events.push(RuntimeEvent::ConfigRejected { message });
                    }
                    return;
                }
                Err(error @ HotkeySystemError::Unavailable(_)) => {
                    self.registered_hotkeys = None;
                    self.registration_retry_at = Some(now + HOTKEY_RETRY_INTERVAL);
                    events.extend(self.hotkey_failed(error));
                }
            }
        }

        self.config_state = if missing {
            ConfigState::Missing
        } else {
            ConfigState::Loaded
        };
        self.last_config_error = None;
        events.push(if missing {
            RuntimeEvent::ConfigMissing {
                path: self.config_path.clone().unwrap_or_default(),
            }
        } else {
            RuntimeEvent::ConfigLoaded {
                bindings: config.bindings.len(),
            }
        });
        self.config = config;
    }

    fn sync_hotkeys<H: HotkeySystem>(
        &mut self,
        hotkey_system: &mut H,
        now: Instant,
        events: &mut Vec<RuntimeEvent>,
    ) {
        let wanted = hotkeys_of(&self.config);
        if self.registered_hotkeys.as_ref() == Some(&wanted)
            || self.rejected_hotkeys.as_ref() == Some(&wanted)
            || self
                .registration_retry_at
                .is_some_and(|retry_at| now < retry_at)
        {
            return;
        }

        match hotkey_system.register_hotkeys(&wanted) {
            Ok(()) => self.hotkeys_registered_as(wanted, events),
            Err(HotkeySystemError::Rejected(message)) => {
                // The hotkey system kept whatever it had registered before.
                self.rejected_hotkeys = Some(wanted);
                self.hotkey_state =
                    HotkeyRegistrationState::Error(HotkeySystemError::Rejected(message.clone()));
                self.last_hotkey_error = Some(message.clone());
                events.push(RuntimeEvent::HotkeysRejected { message });
            }
            Err(error @ HotkeySystemError::Unavailable(_)) => {
                self.registered_hotkeys = None;
                self.registration_retry_at = Some(now + HOTKEY_RETRY_INTERVAL);
                events.extend(self.hotkey_failed(error));
            }
        }
    }

    fn hotkeys_registered_as(&mut self, hotkeys: Vec<String>, events: &mut Vec<RuntimeEvent>) {
        let count = hotkeys.len();
        let recovered = self.last_hotkey_error.take().is_some();
        self.registered_hotkeys = Some(hotkeys);
        self.rejected_hotkeys = None;
        self.registration_retry_at = None;
        self.hotkey_state = HotkeyRegistrationState::Registered;
        if recovered {
            events.push(RuntimeEvent::HotkeysRecovered { count });
        } else if count > 0 {
            events.push(RuntimeEvent::HotkeysRegistered { count });
        }
    }

    fn hotkey_failed(&mut self, error: HotkeySystemError) -> Option<RuntimeEvent> {
        let message = match &error {
            HotkeySystemError::Rejected(message) | HotkeySystemError::Unavailable(message) => {
                message.clone()
            }
        };
        self.hotkey_state = HotkeyRegistrationState::Error(error);
        if self.last_hotkey_error.as_ref() == Some(&message) {
            return None;
        }
        self.last_hotkey_error = Some(message.clone());
        Some(RuntimeEvent::HotkeysUnavailable { message })
    }

    fn config_error_message(&self) -> Option<String> {
        match &self.config_state {
            ConfigState::Error(error) => Some(error.to_string()),
            _ => None,
        }
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
            history: WindowHistory::default(),
            registered_hotkeys: None,
            rejected_hotkeys: None,
            registration_retry_at: None,
            last_config_error: None,
            last_hotkey_error: None,
            last_config_signature: None,
            reload_deadline: None,
        }
    }
}

fn hotkeys_of(config: &AppConfig) -> Vec<String> {
    config
        .bindings
        .iter()
        .map(|binding| binding.hotkey.clone())
        .collect()
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

    let config = parse_config(&input).map_err(|error| match error {
        ConfigError::Validation(source) => ConfigLoadError::Validation {
            path: path.to_owned(),
            source,
        },
        source @ ConfigError::Toml(_) => ConfigLoadError::Parse {
            path: path.to_owned(),
            source,
        },
    })?;
    Ok(Some(config))
}

fn config_file_signature(path: &Path) -> Result<Option<ConfigFileSignature>, io::Error> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };

    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Ok(Some(ConfigFileSignature(hasher.finish())))
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
    #[cfg(any(test, target_os = "macos"))]
    MacOS,
    #[cfg(any(
        test,
        not(any(target_os = "linux", target_os = "windows", target_os = "macos"))
    ))]
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

#[cfg(target_os = "macos")]
fn current_platform() -> Platform {
    Platform::MacOS
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn current_platform() -> Platform {
    Platform::Unsupported(env::consts::OS)
}

fn resolve_config_path_for(
    platform: Platform,
    _get_env: impl Fn(&str) -> Option<OsString>,
) -> Result<PathBuf, ConfigPathError> {
    let config_root: PathBuf = match platform {
        #[cfg(any(test, target_os = "linux"))]
        Platform::Linux => Ok(_get_env("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            // XDG absoluteness is POSIX, not host-defined: Path::is_absolute() is false for "/xdg"
            // when the test suite runs on Windows.
            .filter(|value| value.as_encoded_bytes().starts_with(b"/"))
            .map(PathBuf::from)
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
        #[cfg(any(test, target_os = "macos"))]
        Platform::MacOS => Ok(_get_env("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|home| home.join("Library").join("Application Support"))
            .ok_or(ConfigPathError::MissingEnvironment {
                platform: "macOS",
                variables: "HOME",
            })?),
        #[cfg(any(
            test,
            not(any(target_os = "linux", target_os = "windows", target_os = "macos"))
        ))]
        Platform::Unsupported(platform) => Err(ConfigPathError::UnsupportedPlatform { platform }),
    }?;

    Ok(config_root.join(CONFIG_DIRECTORY).join(CONFIG_FILE))
}

#[cfg(test)]
mod tests {
    use super::{
        App, CONFIG_FILE, CONFIG_RELOAD_DEBOUNCE, ConfigLoadError, ConfigState,
        DispatchHotkeyError, DispatchState, HOTKEY_RETRY_INTERVAL, HotkeyRegistrationState,
        Platform, RuntimeEvent, resolve_config_path_for,
    };
    use crate::{
        Action, Binding, DisplayGeometry, ExecuteActionError, FocusedWindow, HotkeyEvent,
        HotkeySystem, HotkeySystemError, Rect, WindowId, WindowMove, WindowSystem,
        WindowSystemError, ZoneDefinition,
    };
    use std::collections::VecDeque;
    use std::env;
    use std::ffi::OsString;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

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

    /// A fresh config path in a private directory, removed when dropped.
    struct TestConfig {
        directory: PathBuf,
        path: PathBuf,
    }

    impl TestConfig {
        fn new(name: &str) -> Self {
            let directory = env::temp_dir().join(format!(
                "window_zones_runtime_{}_{}",
                std::process::id(),
                name
            ));
            let _ = fs::remove_dir_all(&directory);
            fs::create_dir_all(&directory).unwrap();
            let path = directory.join(CONFIG_FILE);
            Self { directory, path }
        }

        fn write(&self, contents: &str) {
            fs::write(&self.path, contents).unwrap();
        }
    }

    impl Drop for TestConfig {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    fn zone_binding(hotkey: &str, zone: &str) -> String {
        format!(
            "[[bindings]]\nhotkey = \"{hotkey}\"\naction = {{ type = \"move-to-zone\", zone = \"{zone}\" }}\n"
        )
    }

    fn left_half() -> Action {
        Action::MoveToZone {
            zone: "left-half".to_string(),
        }
    }

    fn hotkeys(app: &App) -> Vec<&str> {
        app.config()
            .bindings
            .iter()
            .map(|binding| binding.hotkey.as_str())
            .collect()
    }

    /// Ticks once to notice a file change, then again after the debounce.
    fn settle(
        app: &mut App,
        hotkey_system: &mut FakeHotkeySystem,
        now: Instant,
    ) -> Vec<RuntimeEvent> {
        let mut events = app.tick(hotkey_system, now);
        events.extend(app.tick(hotkey_system, now + CONFIG_RELOAD_DEBOUNCE));
        events
    }

    #[derive(Debug, Default)]
    struct FakeHotkeySystem {
        /// The set the fake platform actually holds; refusals leave it unchanged.
        registered: Vec<String>,
        attempts: usize,
        refuse: Option<String>,
        unavailable: bool,
        events: VecDeque<Result<HotkeyEvent, HotkeySystemError>>,
    }

    impl FakeHotkeySystem {
        fn press(&mut self, hotkey: &str) {
            self.events.push_back(Ok(HotkeyEvent::Pressed {
                hotkey: hotkey.to_string(),
            }));
        }
    }

    impl HotkeySystem for FakeHotkeySystem {
        fn register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), HotkeySystemError> {
            self.attempts += 1;
            if self.unavailable {
                return Err(HotkeySystemError::Unavailable(
                    "companion absent".to_string(),
                ));
            }
            if let Some(refused) = &self.refuse
                && hotkeys.contains(refused)
            {
                return Err(HotkeySystemError::Rejected(format!("{refused} is taken")));
            }
            self.registered = hotkeys.to_vec();
            Ok(())
        }

        fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
            self.events.pop_front().transpose()
        }
    }

    #[derive(Debug)]
    struct FakeWindowSystem {
        focused_window: Option<FocusedWindow>,
        moves: Vec<WindowMove>,
    }

    impl FakeWindowSystem {
        fn with_focus(geometry: Rect) -> Self {
            Self {
                focused_window: Some(FocusedWindow::new(WindowId::new("window"), geometry)),
                moves: Vec::new(),
            }
        }

        fn targets(&self) -> Vec<Rect> {
            self.moves
                .iter()
                .map(|window_move| window_move.target)
                .collect()
        }
    }

    impl WindowSystem for FakeWindowSystem {
        fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
            Ok(self.focused_window.clone())
        }

        fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
            Ok(vec![
                DisplayGeometry::new("left", Rect::new(0, 0, 1920, 1080)),
                DisplayGeometry::new("right", Rect::new(1920, 0, 2560, 1440)),
            ])
        }

        fn move_window(&mut self, window_move: &WindowMove) -> Result<(), WindowSystemError> {
            self.moves.push(window_move.clone());
            Ok(())
        }
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
    fn macos_uses_home_library_support() {
        let entries = [("HOME", "/users/bob")];

        let path = resolve_config_path_for(Platform::MacOS, environment(&entries)).unwrap();

        assert_eq!(
            path,
            PathBuf::from("/users/bob/Library/Application Support/window_zones/config.toml")
        );
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
    fn missing_config_boots_with_empty_bindings_and_points_at_init() {
        let config = TestConfig::new("missing");
        fs::remove_dir_all(&config.directory).unwrap();

        let app = App::start_at(&config.path);

        assert!(matches!(app.config_state(), ConfigState::Missing));
        assert!(app.config().bindings.is_empty());
        assert_eq!(app.config_path(), Some(config.path.as_path()));
        assert!(
            app.startup_event()
                .to_string()
                .contains("window_zones init")
        );
    }

    #[test]
    fn existing_config_is_loaded_at_startup() {
        let config = TestConfig::new("loaded");
        config.write(&zone_binding("Ctrl+Alt+Left", "left-half"));

        let app = App::start_at(&config.path);

        assert!(matches!(app.config_state(), ConfigState::Loaded));
        assert_eq!(
            app.config().bindings,
            vec![Binding {
                hotkey: "alt+ctrl+left".to_string(),
                action: left_half(),
            }]
        );
        assert_eq!(
            app.startup_event(),
            RuntimeEvent::ConfigLoaded { bindings: 1 }
        );
    }

    #[test]
    fn custom_zones_are_loaded_and_dispatched() {
        let config = TestConfig::new("custom_zone");
        config.write(&format!(
            "[zones]\nside = {{ x = 10, y = 0, width = 50, height = 100 }}\n\n{}",
            zone_binding("Ctrl+Alt+Right", "side")
        ));

        let mut app = App::start_at(&config.path);
        let mut window_system = FakeWindowSystem::with_focus(Rect::new(200, 200, 800, 600));

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
        assert_eq!(window_system.targets(), vec![Rect::new(192, 0, 960, 1080)]);
    }

    #[test]
    fn startup_errors_keep_path_and_actionable_diagnostic() {
        let duplicate = TestConfig::new("validation_error");
        duplicate.write(&format!(
            "{}\n[[bindings]]\nhotkey = \"ctrl+alt+left\"\naction = {{ type = \"move-to-next-display\" }}\n",
            zone_binding("Ctrl+Alt+Left", "left-half")
        ));
        let malformed = TestConfig::new("parse_error");
        malformed.write("bindings = [");
        let unreadable = TestConfig::new("read_error");

        let duplicate_app = App::start_at(&duplicate.path);
        let malformed_app = App::start_at(&malformed.path);
        let unreadable_app = App::start_at(&unreadable.directory);

        match duplicate_app.config_state() {
            ConfigState::Error(ConfigLoadError::Validation { path, source }) => {
                assert_eq!(path, &duplicate.path);
                assert_eq!(
                    source.to_string(),
                    "duplicate binding for hotkey alt+ctrl+left"
                );
            }
            state => panic!("expected validation error, got {state:?}"),
        }
        match malformed_app.config_state() {
            ConfigState::Error(ConfigLoadError::Parse { path, source }) => {
                assert_eq!(path, &malformed.path);
                assert!(source.to_string().contains("invalid TOML config"));
            }
            state => panic!("expected parse error, got {state:?}"),
        }
        assert!(matches!(
            unreadable_app.config_state(),
            ConfigState::Error(ConfigLoadError::Read { path, .. }) if path == &unreadable.directory
        ));
        assert!(duplicate_app.config().bindings.is_empty());
        assert!(matches!(
            malformed_app.startup_event(),
            RuntimeEvent::ConfigFailed { .. }
        ));
    }

    #[test]
    fn first_tick_registers_the_configured_hotkeys() {
        let config = TestConfig::new("register_hotkeys");
        config.write(&format!(
            "{}{}",
            zone_binding("Ctrl+Alt+Left", "left-half"),
            zone_binding("Ctrl+Alt+Right", "right-half")
        ));
        let mut app = App::start_at(&config.path);
        let mut hotkey_system = FakeHotkeySystem::default();

        assert_eq!(app.hotkey_state(), &HotkeyRegistrationState::Unregistered);
        let events = app.tick(&mut hotkey_system, Instant::now());

        assert_eq!(events, vec![RuntimeEvent::HotkeysRegistered { count: 2 }]);
        assert_eq!(app.hotkey_state(), &HotkeyRegistrationState::Registered);
        assert!(app.hotkeys_registered());
        assert_eq!(
            hotkey_system.registered,
            ["alt+ctrl+left", "alt+ctrl+right"]
        );
    }

    #[test]
    fn tick_reloads_changed_config_after_the_debounce() {
        let config = TestConfig::new("polling_reload");
        config.write(&zone_binding("Ctrl+Alt+Left", "left-half"));
        let mut app = App::start_at(&config.path);
        let mut hotkey_system = FakeHotkeySystem::default();
        let start = Instant::now();
        app.tick(&mut hotkey_system, start);

        config.write(&zone_binding("Shift+Alt+Right", "right-half"));
        let early = app.tick(&mut hotkey_system, start);
        let early_hotkeys: Vec<String> = hotkeys(&app).into_iter().map(String::from).collect();
        let settled = app.tick(&mut hotkey_system, start + CONFIG_RELOAD_DEBOUNCE);

        assert!(early.is_empty());
        assert_eq!(early_hotkeys, ["alt+ctrl+left"]);
        assert_eq!(
            settled,
            vec![
                RuntimeEvent::HotkeysRegistered { count: 1 },
                RuntimeEvent::ConfigLoaded { bindings: 1 },
            ]
        );
        assert_eq!(hotkeys(&app), ["alt+shift+right"]);
        assert_eq!(hotkey_system.registered, ["alt+shift+right"]);
    }

    #[test]
    fn tick_detects_same_length_edits_with_preserved_mtime() {
        let config = TestConfig::new("polling_preserved_metadata");
        let contents =
            "[[bindings]]\nhotkey = \"ctrl+a\"\naction = { type = \"move-to-next-display\" }\n";
        config.write(contents);
        let modified = fs::metadata(&config.path).unwrap().modified().unwrap();
        let mut app = App::start_at(&config.path);
        let mut hotkey_system = FakeHotkeySystem::default();

        config.write(&contents.replace("ctrl+a", "ctrl+b"));
        fs::File::options()
            .write(true)
            .open(&config.path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        settle(&mut app, &mut hotkey_system, Instant::now());

        assert_eq!(hotkeys(&app), ["ctrl+b"]);
    }

    #[test]
    fn invalid_reload_keeps_last_valid_bindings_and_reports_once() {
        let config = TestConfig::new("reload_parse_error");
        config.write(&zone_binding("Ctrl+Alt+Left", "left-half"));
        let mut app = App::start_at(&config.path);
        let mut hotkey_system = FakeHotkeySystem::default();
        let mut window_system = FakeWindowSystem::with_focus(Rect::new(200, 200, 800, 600));
        let start = Instant::now();
        app.tick(&mut hotkey_system, start);

        config.write("bindings = [");
        let first = settle(&mut app, &mut hotkey_system, start);
        let repeated = settle(&mut app, &mut hotkey_system, start + Duration::from_secs(1));
        let manual = app.reload(&mut hotkey_system, start + Duration::from_secs(2));
        let state = app
            .dispatch_hotkey("Ctrl+Alt+Left", &mut window_system)
            .clone();

        assert!(matches!(
            first.as_slice(),
            [RuntimeEvent::ConfigFailed { .. }]
        ));
        assert!(repeated.is_empty());
        assert!(matches!(
            manual.as_slice(),
            [RuntimeEvent::ConfigFailed { .. }]
        ));
        assert!(matches!(
            app.config_state(),
            ConfigState::Error(ConfigLoadError::Parse { path, .. }) if path == &config.path
        ));
        assert_eq!(hotkeys(&app), ["alt+ctrl+left"]);
        assert_eq!(state, DispatchState::Succeeded);

        config.write(&zone_binding("Ctrl+Alt+Right", "right-half"));
        let recovered = settle(&mut app, &mut hotkey_system, start + Duration::from_secs(3));
        assert!(recovered.contains(&RuntimeEvent::ConfigLoaded { bindings: 1 }));
        assert_eq!(hotkeys(&app), ["alt+ctrl+right"]);
    }

    #[test]
    fn config_with_a_refused_hotkey_is_not_applied() {
        let config = TestConfig::new("reload_refused_hotkey");
        config.write(&zone_binding("Ctrl+Alt+Left", "left-half"));
        let mut app = App::start_at(&config.path);
        let mut hotkey_system = FakeHotkeySystem {
            refuse: Some("alt+ctrl+x".to_string()),
            ..FakeHotkeySystem::default()
        };
        let mut window_system = FakeWindowSystem::with_focus(Rect::new(200, 200, 800, 600));
        let start = Instant::now();
        app.tick(&mut hotkey_system, start);

        config.write(&format!(
            "{}{}",
            zone_binding("Ctrl+Alt+Left", "right-half"),
            zone_binding("Ctrl+Alt+X", "maximize")
        ));
        let refused = settle(&mut app, &mut hotkey_system, start);
        let later = settle(&mut app, &mut hotkey_system, start + Duration::from_secs(5));
        hotkey_system.press("alt+ctrl+left");
        let dispatched = app
            .dispatch_next_hotkey(&mut hotkey_system, &mut window_system)
            .unwrap()
            .cloned();

        assert!(
            matches!(refused.as_slice(), [RuntimeEvent::ConfigRejected { message }]
            if message.contains("alt+ctrl+x is taken"))
        );
        assert!(later.is_empty());
        assert!(matches!(
            app.config_state(),
            ConfigState::Error(ConfigLoadError::HotkeysRejected { .. })
        ));
        assert!(app.hotkeys_registered());
        assert_eq!(hotkey_system.registered, ["alt+ctrl+left"]);
        assert_eq!(dispatched, Some(DispatchState::Succeeded));
        // The previous binding (left-half) still applies.
        assert_eq!(window_system.targets(), vec![Rect::new(0, 0, 960, 1080)]);
    }

    #[test]
    fn startup_set_with_a_refused_hotkey_is_not_retried_until_the_config_changes() {
        let config = TestConfig::new("startup_refused_hotkey");
        config.write(&zone_binding("Ctrl+Alt+X", "maximize"));
        let mut app = App::start_at(&config.path);
        let mut hotkey_system = FakeHotkeySystem {
            refuse: Some("alt+ctrl+x".to_string()),
            ..FakeHotkeySystem::default()
        };
        let start = Instant::now();

        let refused = app.tick(&mut hotkey_system, start);
        let later = app.tick(&mut hotkey_system, start + Duration::from_secs(10));
        let attempts_while_refused = hotkey_system.attempts;
        config.write(&zone_binding("Ctrl+Alt+Y", "maximize"));
        let fixed = settle(
            &mut app,
            &mut hotkey_system,
            start + Duration::from_secs(11),
        );

        assert!(matches!(
            refused.as_slice(),
            [RuntimeEvent::HotkeysRejected { .. }]
        ));
        assert!(later.is_empty());
        assert_eq!(attempts_while_refused, 1);
        assert!(fixed.contains(&RuntimeEvent::HotkeysRecovered { count: 1 }));
        assert_eq!(hotkey_system.registered, ["alt+ctrl+y"]);
    }

    #[test]
    fn unavailable_hotkeys_are_retried_after_the_interval_and_reported_on_transitions() {
        let config = TestConfig::new("unavailable_hotkeys");
        config.write(&zone_binding("Ctrl+Alt+Left", "left-half"));
        let mut app = App::start_at(&config.path);
        let mut hotkey_system = FakeHotkeySystem {
            unavailable: true,
            ..FakeHotkeySystem::default()
        };
        let start = Instant::now();

        let first = app.tick(&mut hotkey_system, start);
        let too_soon = app.tick(&mut hotkey_system, start + HOTKEY_RETRY_INTERVAL / 2);
        let attempts_too_soon = hotkey_system.attempts;
        let retried = app.tick(&mut hotkey_system, start + HOTKEY_RETRY_INTERVAL);
        let attempts_retried = hotkey_system.attempts;
        hotkey_system.unavailable = false;
        let recovered = app.tick(&mut hotkey_system, start + HOTKEY_RETRY_INTERVAL * 2);

        assert!(matches!(
            first.as_slice(),
            [RuntimeEvent::HotkeysUnavailable { .. }]
        ));
        assert!(too_soon.is_empty());
        assert_eq!(attempts_too_soon, 1);
        assert!(retried.is_empty());
        assert_eq!(attempts_retried, 2);
        assert_eq!(recovered, vec![RuntimeEvent::HotkeysRecovered { count: 1 }]);
        assert!(app.hotkeys_registered());
    }

    #[test]
    fn lost_hotkeys_are_re_registered_on_the_next_tick() {
        let config = TestConfig::new("lost_hotkeys");
        config.write(&zone_binding("Ctrl+Alt+Left", "left-half"));
        let mut app = App::start_at(&config.path);
        let mut hotkey_system = FakeHotkeySystem::default();
        let start = Instant::now();
        app.tick(&mut hotkey_system, start);

        let lost = app.hotkeys_lost(
            HotkeySystemError::Unavailable("companion restarted".to_string()),
            start,
        );
        let registered_after_loss = app.hotkeys_registered();
        let recovered = app.tick(&mut hotkey_system, start);

        assert!(matches!(
            lost,
            Some(RuntimeEvent::HotkeysUnavailable { .. })
        ));
        assert!(!registered_after_loss);
        assert_eq!(recovered, vec![RuntimeEvent::HotkeysRecovered { count: 1 }]);
        assert_eq!(hotkey_system.attempts, 2);
    }

    #[test]
    fn unreadable_config_recovers_when_restored_with_identical_content() {
        let config = TestConfig::new("polling_read_error");
        let contents = zone_binding("Ctrl+Alt+Left", "left-half");
        config.write(&contents);
        let mut app = App::start_at(&config.path);
        let mut hotkey_system = FakeHotkeySystem::default();
        let mut window_system = FakeWindowSystem::with_focus(Rect::new(200, 200, 800, 600));
        let start = Instant::now();
        app.tick(&mut hotkey_system, start);

        fs::remove_file(&config.path).unwrap();
        fs::create_dir(&config.path).unwrap();
        let failed = settle(&mut app, &mut hotkey_system, start);
        let state = app
            .dispatch_hotkey("Ctrl+Alt+Left", &mut window_system)
            .clone();
        fs::remove_dir(&config.path).unwrap();
        config.write(&contents);
        settle(&mut app, &mut hotkey_system, start + Duration::from_secs(1));

        assert!(matches!(
            failed.as_slice(),
            [RuntimeEvent::ConfigFailed { .. }]
        ));
        assert_eq!(state, DispatchState::Succeeded);
        assert!(matches!(app.config_state(), ConfigState::Loaded));
        assert_eq!(hotkeys(&app), ["alt+ctrl+left"]);
    }

    #[test]
    fn dispatch_next_hotkey_reports_each_press_once() {
        let config = TestConfig::new("dispatch_queue");
        config.write(&zone_binding("Ctrl+Alt+Left", "left-half"));
        let mut app = App::start_at(&config.path);
        let mut hotkey_system = FakeHotkeySystem::default();
        hotkey_system.press("alt+shift+right");
        hotkey_system.press("alt+ctrl+left");
        let mut window_system = FakeWindowSystem::with_focus(Rect::new(200, 200, 800, 600));

        let mut results = Vec::new();
        while let Some(state) = app
            .dispatch_next_hotkey(&mut hotkey_system, &mut window_system)
            .unwrap()
        {
            results.push(state.clone());
        }
        let after_drain = app
            .dispatch_next_hotkey(&mut hotkey_system, &mut window_system)
            .unwrap()
            .cloned();

        assert_eq!(
            results,
            vec![
                DispatchState::Error(DispatchHotkeyError::NoBindingForHotkey {
                    hotkey: "alt+shift+right".to_string()
                }),
                DispatchState::Succeeded,
            ]
        );
        assert_eq!(after_drain, None);
        assert_eq!(app.last_dispatch_hotkey(), Some("alt+ctrl+left"));
        assert_eq!(window_system.targets(), vec![Rect::new(0, 0, 960, 1080)]);
    }

    #[test]
    fn no_focused_window_remains_an_explicit_dispatch_state() {
        let config = TestConfig::new("no_focus");
        config.write(&zone_binding("Ctrl+Alt+Left", "left-half"));
        let mut app = App::start_at(&config.path);
        let mut window_system = FakeWindowSystem {
            focused_window: None,
            moves: Vec::new(),
        };

        let state = app.dispatch_hotkey("Ctrl+Alt+Left", &mut window_system);

        assert_eq!(
            state,
            &DispatchState::Error(DispatchHotkeyError::ExecuteAction(
                ExecuteActionError::NoFocusedWindow
            ))
        );
    }

    #[test]
    fn restore_uses_history_kept_across_dispatches() {
        let config = TestConfig::new("restore_history");
        config.write(&format!(
            "{}[[bindings]]\nhotkey = \"Ctrl+Alt+Backspace\"\naction = {{ type = \"restore\" }}\n",
            zone_binding("Ctrl+Alt+Left", "left-half")
        ));
        let mut app = App::start_at(&config.path);
        let original = Rect::new(200, 200, 800, 600);
        let mut window_system = FakeWindowSystem::with_focus(original);

        app.dispatch_hotkey("Ctrl+Alt+Left", &mut window_system);
        window_system.focused_window = Some(FocusedWindow::new(
            WindowId::new("window"),
            Rect::new(0, 0, 960, 1080),
        ));
        let state = app.dispatch_hotkey("Ctrl+Alt+Backspace", &mut window_system);

        assert_eq!(state, &DispatchState::Succeeded);
        assert_eq!(window_system.targets().last(), Some(&original));
    }

    #[test]
    fn deleting_the_config_applies_an_empty_set() {
        let config = TestConfig::new("deleted_config");
        config.write(&zone_binding("Ctrl+Alt+Left", "left-half"));
        let mut app = App::start_at(&config.path);
        let mut hotkey_system = FakeHotkeySystem::default();
        let start = Instant::now();
        app.tick(&mut hotkey_system, start);

        fs::remove_file(&config.path).unwrap();
        let events = settle(&mut app, &mut hotkey_system, start);

        assert!(matches!(app.config_state(), ConfigState::Missing));
        assert!(app.config().bindings.is_empty());
        assert!(hotkey_system.registered.is_empty());
        assert!(matches!(
            events.last(),
            Some(RuntimeEvent::ConfigMissing { path }) if path == Path::new(&config.path)
        ));
    }
}
