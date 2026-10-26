use std::env;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(any(target_os = "linux", target_os = "windows"))]
use tray_item::{IconSource, TrayItem};

#[cfg(target_os = "macos")]
use window_zones::MacOSWindowSystem;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use window_zones::RdevHotkeySystem;
#[cfg(target_os = "windows")]
use window_zones::WindowsWindowSystem;
use window_zones::{
    App, ConfigState, DispatchState, DisplayGeometry, FocusedWindow, HotkeyEvent,
    HotkeyRegistrationState, HotkeySystem, HotkeySystemError, Rect, RuntimeEvent, WindowId,
    WindowMove, WindowSystem, WindowSystemError, default_config_path, parse_config, starter_config,
};
#[cfg(target_os = "linux")]
use window_zones::{
    GnomeHotkeySystem, GnomeWindowSystem, KwinHotkeySystem, KwinWindowSystem, ScriptedCompositor,
    WaylandBackend, WaylandWindowSystem, X11HotkeySystem, X11WindowSystem,
};

#[cfg(any(target_os = "linux", all(test, unix)))]
mod local_session_dispatch;

/// How often queued hotkey presses are read; bounds keypress-to-move latency.
const HOTKEY_POLL_INTERVAL: Duration = Duration::from_millis(50);
/// How often the config file and hotkey registration are reconciled.
const RUNTIME_TICK_INTERVAL: Duration = Duration::from_millis(250);
/// How often the TUI re-reads companion capabilities.
const CAPABILITY_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
/// Upper bound on presses handled in one poll, so input stays responsive.
const MAX_HOTKEYS_PER_POLL: usize = 32;
/// Exit status when another instance already owns the hotkeys (EX_TEMPFAIL);
/// the systemd unit does not restart on it.
const EXIT_ALREADY_RUNNING: u8 = 75;

#[derive(Debug, Clone, Copy)]
enum BackendPreference {
    Auto,
    #[cfg(target_os = "linux")]
    X11,
    #[cfg(target_os = "linux")]
    Wayland,
    #[cfg(target_os = "windows")]
    Windows,
    #[cfg(target_os = "macos")]
    MacOS,
    DryRun,
}

#[derive(Debug, Clone)]
enum Command {
    Status,
    Dispatch { hotkey: String },
    Tui,
    Run,
    Init,
}

#[derive(Debug)]
struct CliArgs {
    command: Command,
    config_path: Option<PathBuf>,
    perf_log_path: Option<PathBuf>,
    backend: BackendPreference,
    show_tray: bool,
    chord: Option<String>,
    force: bool,
}

#[derive(Debug)]
enum ParseStatus {
    Ok(CliArgs),
    Help,
    Err(String),
}

fn parse_args() -> ParseStatus {
    let args: Vec<String> = env::args().skip(1).collect();
    parse_args_from_inputs(&args)
}

fn parse_args_from_inputs(args: &[String]) -> ParseStatus {
    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        return ParseStatus::Help;
    }

    let mut command = None;
    let mut config_path = None;
    let mut perf_log_path = None;
    let mut backend = BackendPreference::Auto;
    let mut show_tray = false;
    let mut chord = None;
    let mut force = false;

    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];

        match arg.as_str() {
            "--config" | "--perf-log" | "--backend" | "--chord" => {
                let Some(value) = args.get(index + 1) else {
                    return ParseStatus::Err(format!("{arg} requires a value"));
                };
                match arg.as_str() {
                    "--config" => config_path = Some(PathBuf::from(value)),
                    "--perf-log" => perf_log_path = Some(PathBuf::from(value)),
                    "--backend" => match parse_backend_preference(value) {
                        Ok(value) => backend = value,
                        Err(message) => return ParseStatus::Err(message),
                    },
                    _ => chord = Some(value.clone()),
                }
                index += 1;
            }
            "--dry-run" => backend = BackendPreference::DryRun,
            "--tray" => show_tray = true,
            "--force" => force = true,
            "status" | "dispatch" | "tui" | "run" | "init" => {
                if command.is_some() {
                    return ParseStatus::Err("only one command is allowed".to_string());
                }
                command = Some(match arg.as_str() {
                    "status" => Command::Status,
                    "dispatch" => Command::Dispatch {
                        hotkey: String::new(),
                    },
                    "tui" => Command::Tui,
                    "init" => Command::Init,
                    _ => Command::Run,
                });
            }
            arg if arg.starts_with('-') => {
                return ParseStatus::Err(format!("unknown flag `{arg}`"));
            }
            _ => match &mut command {
                Some(Command::Dispatch { hotkey }) if hotkey.is_empty() => {
                    *hotkey = arg.to_string();
                }
                Some(Command::Dispatch { .. }) => {
                    return ParseStatus::Err(format!(
                        "dispatch command got an extra argument `{arg}`"
                    ));
                }
                _ => return ParseStatus::Err(format!("unexpected argument `{arg}`")),
            },
        }

        index += 1;
    }

    let command = command.unwrap_or(Command::Run);
    if let Command::Dispatch { hotkey } = &command
        && hotkey.is_empty()
    {
        return ParseStatus::Err(
            "`dispatch` requires a hotkey argument, for example `dispatch Ctrl+Alt+Left`"
                .to_string(),
        );
    }
    if show_tray && !matches!(command, Command::Run) {
        return ParseStatus::Err("--tray is only valid with the `run` command".to_string());
    }
    if (chord.is_some() || force) && !matches!(command, Command::Init) {
        return ParseStatus::Err("--chord and --force are only valid with `init`".to_string());
    }
    if perf_log_path.is_some()
        && !matches!(
            command,
            Command::Dispatch { .. } | Command::Run | Command::Tui
        )
    {
        return ParseStatus::Err(
            "--perf-log is only valid with `dispatch`, `run`, or `tui`".to_string(),
        );
    }

    ParseStatus::Ok(CliArgs {
        command,
        config_path,
        perf_log_path,
        backend,
        show_tray,
        chord,
        force,
    })
}

fn parse_backend_preference(raw: &str) -> Result<BackendPreference, String> {
    match raw {
        "auto" => Ok(BackendPreference::Auto),
        "dry-run" => Ok(BackendPreference::DryRun),
        #[cfg(target_os = "linux")]
        "x11" => Ok(BackendPreference::X11),
        #[cfg(target_os = "linux")]
        "wayland" => Ok(BackendPreference::Wayland),
        #[cfg(target_os = "windows")]
        "windows" => Ok(BackendPreference::Windows),
        #[cfg(target_os = "macos")]
        "macos" => Ok(BackendPreference::MacOS),
        value => Err(format!("unsupported backend `{value}` on this platform")),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeBackend {
    DryRun,
    #[cfg(target_os = "linux")]
    X11,
    #[cfg(target_os = "linux")]
    Wayland(ScriptedCompositor),
    #[cfg(target_os = "linux")]
    Gnome,
    #[cfg(target_os = "linux")]
    Kde,
    #[cfg(target_os = "windows")]
    Windows,
    #[cfg(target_os = "macos")]
    MacOS,
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    Unsupported,
}

fn resolve_runtime_backend(preference: BackendPreference) -> Result<RuntimeBackend, String> {
    #[cfg(target_os = "linux")]
    {
        resolve_linux_backend(preference)
    }

    #[cfg(target_os = "windows")]
    {
        Ok(match preference {
            BackendPreference::DryRun => RuntimeBackend::DryRun,
            BackendPreference::Windows | BackendPreference::Auto => RuntimeBackend::Windows,
        })
    }

    #[cfg(target_os = "macos")]
    {
        Ok(match preference {
            BackendPreference::DryRun => RuntimeBackend::DryRun,
            BackendPreference::MacOS | BackendPreference::Auto => RuntimeBackend::MacOS,
        })
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        match preference {
            BackendPreference::DryRun => Ok(RuntimeBackend::DryRun),
            BackendPreference::Auto => Ok(RuntimeBackend::Unsupported),
        }
    }
}

#[cfg(target_os = "linux")]
fn resolve_linux_backend(preference: BackendPreference) -> Result<RuntimeBackend, String> {
    let session_type = env::var_os("XDG_SESSION_TYPE");
    let wayland_display = env::var_os("WAYLAND_DISPLAY").is_some();
    let identity = session_identity(
        session_type.as_deref(),
        wayland_display,
        env::var_os("DISPLAY").is_some(),
    );
    let wayland_indicated = wayland_display || session_type.as_deref() == Some("wayland".as_ref());
    resolve_linux_backend_in_session(preference, identity, wayland_indicated)
}

/// `wayland_indicated`: any Wayland signal is present, even one the session
/// identity could not reconcile. X11 is never used then: inside a Wayland
/// session it would only reach XWayland windows.
#[cfg(target_os = "linux")]
fn resolve_linux_backend_in_session(
    preference: BackendPreference,
    identity: SessionIdentity,
    wayland_indicated: bool,
) -> Result<RuntimeBackend, String> {
    match preference {
        BackendPreference::DryRun => Ok(RuntimeBackend::DryRun),
        BackendPreference::X11 => {
            if wayland_indicated {
                Err("X11 backend is unavailable while WAYLAND_DISPLAY or XDG_SESSION_TYPE=wayland is set; use the native Wayland compositor integration".to_string())
            } else {
                Ok(RuntimeBackend::X11)
            }
        }
        BackendPreference::Wayland => {
            if matches!(identity, SessionIdentity::X11) {
                return Err(
                    "Wayland backend requires XDG_SESSION_TYPE=wayland or WAYLAND_DISPLAY"
                        .to_string(),
                );
            }
            resolve_wayland_runtime_backend()
        }
        BackendPreference::Auto => match identity {
            SessionIdentity::Wayland => resolve_wayland_runtime_backend(),
            SessionIdentity::X11 => Ok(RuntimeBackend::X11),
            SessionIdentity::Unresolved(reason) => Err(format!(
                "cannot resolve desktop session from XDG_SESSION_TYPE, WAYLAND_DISPLAY, and DISPLAY: {reason}; use --backend x11|wayland"
            )),
        },
    }
}

#[cfg(target_os = "linux")]
fn resolve_wayland_runtime_backend() -> Result<RuntimeBackend, String> {
    match window_zones::resolve_wayland_backend().map_err(|error| error.to_string())? {
        WaylandBackend::Gnome => Ok(RuntimeBackend::Gnome),
        WaylandBackend::Kde => Ok(RuntimeBackend::Kde),
        WaylandBackend::Sway => Ok(RuntimeBackend::Wayland(ScriptedCompositor::Sway)),
        WaylandBackend::Hyprland => Ok(RuntimeBackend::Wayland(ScriptedCompositor::Hyprland)),
    }
}

#[cfg(target_os = "linux")]
#[derive(Debug, PartialEq, Eq)]
enum SessionIdentity {
    X11,
    Wayland,
    Unresolved(String),
}

#[cfg(target_os = "linux")]
fn session_identity(
    session_type: Option<&std::ffi::OsStr>,
    wayland_display: bool,
    display: bool,
) -> SessionIdentity {
    match session_type {
        Some(value) if value == "wayland" => SessionIdentity::Wayland,
        Some(value) if value == "x11" => {
            if wayland_display {
                SessionIdentity::Unresolved(
                    "conflicting XDG_SESSION_TYPE=x11 and WAYLAND_DISPLAY".to_string(),
                )
            } else {
                SessionIdentity::X11
            }
        }
        Some(value) => {
            SessionIdentity::Unresolved(format!("unrecognized XDG_SESSION_TYPE={value:?}"))
        }
        None => match (wayland_display, display) {
            (true, false) => SessionIdentity::Wayland,
            (false, true) => SessionIdentity::X11,
            (true, true) => SessionIdentity::Unresolved(
                "conflicting WAYLAND_DISPLAY and DISPLAY without XDG_SESSION_TYPE".to_string(),
            ),
            (false, false) => SessionIdentity::Unresolved(
                "XDG_SESSION_TYPE, WAYLAND_DISPLAY, and DISPLAY are unset".to_string(),
            ),
        },
    }
}

#[derive(Debug)]
enum RuntimeWindowSystem {
    DryRun(DryRunWindowSystem),
    #[cfg(target_os = "linux")]
    X11(X11WindowSystem),
    #[cfg(target_os = "linux")]
    Wayland(WaylandWindowSystem),
    #[cfg(target_os = "linux")]
    Gnome(GnomeWindowSystem),
    #[cfg(target_os = "linux")]
    Kde(KwinWindowSystem),
    #[cfg(target_os = "windows")]
    Windows(WindowsWindowSystem),
    #[cfg(target_os = "macos")]
    MacOS(MacOSWindowSystem),
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    Unsupported,
}

/// Runs `$body` with `$system` bound to whichever adapter is active.
macro_rules! with_window_system {
    ($runtime_system:expr, $system:ident => $body:expr) => {
        match $runtime_system {
            RuntimeWindowSystem::DryRun($system) => $body,
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::X11($system) => $body,
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Wayland($system) => $body,
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Gnome($system) => $body,
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Kde($system) => $body,
            #[cfg(target_os = "windows")]
            RuntimeWindowSystem::Windows($system) => $body,
            #[cfg(target_os = "macos")]
            RuntimeWindowSystem::MacOS($system) => $body,
            #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
            RuntimeWindowSystem::Unsupported => Err(WindowSystemError::Platform(
                "window-system adapter is unsupported on this platform".to_string(),
            )),
        }
    };
}

impl RuntimeWindowSystem {
    fn with_backend(backend: RuntimeBackend) -> Self {
        match backend {
            RuntimeBackend::DryRun => Self::DryRun(DryRunWindowSystem::new()),
            #[cfg(target_os = "linux")]
            RuntimeBackend::X11 => Self::X11(X11WindowSystem::new()),
            #[cfg(target_os = "linux")]
            RuntimeBackend::Wayland(compositor) => {
                Self::Wayland(WaylandWindowSystem::new(compositor))
            }
            #[cfg(target_os = "linux")]
            RuntimeBackend::Gnome => Self::Gnome(GnomeWindowSystem::new()),
            #[cfg(target_os = "linux")]
            RuntimeBackend::Kde => Self::Kde(KwinWindowSystem::new()),
            #[cfg(target_os = "windows")]
            RuntimeBackend::Windows => Self::Windows(WindowsWindowSystem::new()),
            #[cfg(target_os = "macos")]
            RuntimeBackend::MacOS => Self::MacOS(MacOSWindowSystem::new()),
            #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
            RuntimeBackend::Unsupported => Self::Unsupported,
        }
    }

    fn last_move(&self) -> Option<&WindowMove> {
        match self {
            Self::DryRun(system) => system.last_move.as_ref(),
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Self::DryRun(_) => "dry-run",
            #[cfg(target_os = "linux")]
            Self::X11(_) => "x11",
            #[cfg(target_os = "linux")]
            Self::Wayland(_) => "wayland",
            #[cfg(target_os = "linux")]
            Self::Gnome(_) => "gnome-wayland",
            #[cfg(target_os = "linux")]
            Self::Kde(_) => "kde-wayland",
            #[cfg(target_os = "windows")]
            Self::Windows(_) => "windows",
            #[cfg(target_os = "macos")]
            Self::MacOS(_) => "macos",
            #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
            Self::Unsupported => "unsupported",
        }
    }

    /// The companion label and its capability report, for companion backends.
    fn companion_capabilities(&self) -> Option<(&'static str, Result<Vec<String>, String>)> {
        match self {
            #[cfg(target_os = "linux")]
            Self::Gnome(system) => Some((
                "GNOME",
                system.capabilities().map_err(|error| error.to_string()),
            )),
            #[cfg(target_os = "linux")]
            Self::Kde(system) => Some((
                "KDE",
                system.capabilities().map_err(|error| error.to_string()),
            )),
            #[allow(unreachable_patterns)]
            _ => None,
        }
    }
}

impl WindowSystem for RuntimeWindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
        with_window_system!(self, system => system.focused_window())
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        with_window_system!(self, system => system.displays())
    }

    fn move_window(&mut self, window_move: &WindowMove) -> Result<(), WindowSystemError> {
        with_window_system!(self, system => system.move_window(window_move))
    }

    fn transition_metrics(
        &self,
    ) -> Result<
        Option<window_zones::window_system::TransitionMetrics>,
        window_zones::WindowSystemError,
    > {
        with_window_system!(self, system => system.transition_metrics())
    }
}

/// Simulated desktop for `--backend dry-run`: one window on two displays that
/// moves when asked, so actions and their sequences can be checked safely.
#[derive(Debug)]
struct DryRunWindowSystem {
    focused_window: FocusedWindow,
    displays: Vec<DisplayGeometry>,
    last_move: Option<WindowMove>,
}

impl DryRunWindowSystem {
    fn new() -> Self {
        Self {
            focused_window: FocusedWindow::new(
                WindowId::new("dry-run-window"),
                Rect::new(40, 40, 640, 480),
            ),
            displays: vec![
                DisplayGeometry::new("display-0", Rect::new(0, 0, 1920, 1080)),
                DisplayGeometry::new("display-1", Rect::new(1920, 0, 1280, 1024)),
            ],
            last_move: None,
        }
    }
}

impl WindowSystem for DryRunWindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, WindowSystemError> {
        Ok(Some(self.focused_window.clone()))
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, WindowSystemError> {
        Ok(self.displays.clone())
    }

    fn move_window(&mut self, window_move: &WindowMove) -> Result<(), WindowSystemError> {
        if window_move.window != self.focused_window.id {
            return Err(WindowSystemError::WindowGone(window_move.window.clone()));
        }
        self.focused_window.geometry = window_move.target;
        self.last_move = Some(window_move.clone());
        Ok(())
    }
}

#[derive(Debug, Default)]
struct NoopHotkeySystem;

impl HotkeySystem for NoopHotkeySystem {
    fn register_hotkeys(&mut self, _hotkeys: &[String]) -> Result<(), HotkeySystemError> {
        Ok(())
    }

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
        Ok(None)
    }
}

/// Hotkeys on compositors whose own config binds keys to `window_zones dispatch`.
#[cfg(target_os = "linux")]
struct CompositorBoundHotkeySystem;

#[cfg(target_os = "linux")]
impl HotkeySystem for CompositorBoundHotkeySystem {
    fn register_hotkeys(&mut self, _hotkeys: &[String]) -> Result<(), HotkeySystemError> {
        // The compositor owns key capture; the persistent runtime owns bindings and history.
        Ok(())
    }

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
        Ok(None)
    }
}

enum RuntimeHotkeySystem {
    Noop(NoopHotkeySystem),
    #[cfg(target_os = "linux")]
    CompositorBound(CompositorBoundHotkeySystem),
    #[cfg(target_os = "linux")]
    X11(X11HotkeySystem),
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    Global(RdevHotkeySystem),
    #[cfg(target_os = "linux")]
    Gnome(GnomeHotkeySystem),
    #[cfg(target_os = "linux")]
    Kde(KwinHotkeySystem),
}

/// Runs `$body` with `$system` bound to whichever hotkey system is active.
macro_rules! with_hotkey_system {
    ($runtime_system:expr, $system:ident => $body:expr) => {
        match $runtime_system {
            RuntimeHotkeySystem::Noop($system) => $body,
            #[cfg(target_os = "linux")]
            RuntimeHotkeySystem::CompositorBound($system) => $body,
            #[cfg(target_os = "linux")]
            RuntimeHotkeySystem::X11($system) => $body,
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            RuntimeHotkeySystem::Global($system) => $body,
            #[cfg(target_os = "linux")]
            RuntimeHotkeySystem::Gnome($system) => $body,
            #[cfg(target_os = "linux")]
            RuntimeHotkeySystem::Kde($system) => $body,
        }
    };
}

impl RuntimeHotkeySystem {
    fn new(backend: RuntimeBackend) -> Self {
        match backend {
            RuntimeBackend::DryRun => Self::Noop(NoopHotkeySystem),
            #[cfg(target_os = "linux")]
            RuntimeBackend::Wayland(_) => Self::CompositorBound(CompositorBoundHotkeySystem),
            #[cfg(target_os = "linux")]
            RuntimeBackend::X11 => Self::X11(X11HotkeySystem::new()),
            #[cfg(target_os = "linux")]
            RuntimeBackend::Gnome => Self::Gnome(GnomeHotkeySystem::new()),
            #[cfg(target_os = "linux")]
            RuntimeBackend::Kde => Self::Kde(KwinHotkeySystem::new()),
            #[cfg(target_os = "windows")]
            RuntimeBackend::Windows => Self::Global(RdevHotkeySystem::new()),
            #[cfg(target_os = "macos")]
            RuntimeBackend::MacOS => Self::Global(RdevHotkeySystem::new()),
            #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
            RuntimeBackend::Unsupported => Self::Noop(NoopHotkeySystem),
        }
    }
}

impl HotkeySystem for RuntimeHotkeySystem {
    fn register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), HotkeySystemError> {
        with_hotkey_system!(self, system => system.register_hotkeys(hotkeys))
    }

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
        with_hotkey_system!(self, system => system.next_hotkey())
    }
}

fn build_app(
    config_path: Option<&PathBuf>,
    perf_log_path: Option<&PathBuf>,
) -> Result<App, String> {
    let mut app = match config_path {
        Some(path) => App::start_at(path),
        None => App::start(),
    };
    if let Some(path) = perf_log_path {
        app.enable_perf_log(path).map_err(|error| {
            format!("Cannot open performance log at {}: {error}", path.display())
        })?;
    }
    Ok(app)
}

/// A hotkey press and what dispatching it did.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DispatchReport {
    hotkey: String,
    state: DispatchState,
}

/// The running App with its adapters: one engine shared by the CLI, TUI, and
/// tray surfaces.
struct Runtime {
    app: App,
    backend: RuntimeBackend,
    window_system: RuntimeWindowSystem,
    hotkey_system: RuntimeHotkeySystem,
    next_tick: Instant,
}

impl Runtime {
    fn new(
        config_path: Option<PathBuf>,
        backend: RuntimeBackend,
        perf_log_path: Option<&PathBuf>,
    ) -> Result<Self, String> {
        Ok(Self {
            app: build_app(config_path.as_ref(), perf_log_path)?,
            backend,
            window_system: RuntimeWindowSystem::with_backend(backend),
            hotkey_system: RuntimeHotkeySystem::new(backend),
            next_tick: Instant::now(),
        })
    }

    /// Reconciles config and registration when due, then dispatches every
    /// queued hotkey press.
    fn poll(&mut self, now: Instant) -> (Vec<RuntimeEvent>, Vec<DispatchReport>) {
        let mut events = Vec::new();
        if now >= self.next_tick {
            events = self.app.tick(&mut self.hotkey_system, now);
            self.next_tick = now + RUNTIME_TICK_INTERVAL;
        }

        let mut dispatches = Vec::new();
        if !self.app.hotkeys_registered() {
            return (events, dispatches);
        }
        for _ in 0..MAX_HOTKEYS_PER_POLL {
            match self
                .app
                .dispatch_next_hotkey(&mut self.hotkey_system, &mut self.window_system)
            {
                Ok(Some(state)) => {
                    let state = state.clone();
                    dispatches.push(DispatchReport {
                        hotkey: self
                            .app
                            .last_dispatch_hotkey()
                            .unwrap_or_default()
                            .to_string(),
                        state,
                    });
                }
                Ok(None) => break,
                Err(error) => {
                    events.extend(self.app.hotkeys_lost(error, now));
                    self.next_tick = now;
                    break;
                }
            }
        }
        (events, dispatches)
    }

    fn reload(&mut self, now: Instant) -> Vec<RuntimeEvent> {
        self.app.reload(&mut self.hotkey_system, now)
    }

    /// Rebuilds the App and adapters from scratch, releasing the old grabs first.
    fn restart(&mut self, now: Instant) -> Vec<RuntimeEvent> {
        self.hotkey_system = RuntimeHotkeySystem::Noop(NoopHotkeySystem);
        self.app.restart();
        self.window_system = RuntimeWindowSystem::with_backend(self.backend);
        self.hotkey_system = RuntimeHotkeySystem::new(self.backend);
        self.next_tick = now + RUNTIME_TICK_INTERVAL;
        let mut events = vec![self.app.startup_event()];
        events.extend(self.app.tick(&mut self.hotkey_system, now));
        events
    }

    fn dispatch(&mut self, hotkey: &str) -> DispatchState {
        self.app
            .dispatch_hotkey(hotkey, &mut self.window_system)
            .clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RuntimeInstruction {
    Empty,
    Status,
    Reload,
    Restart,
    Quit,
    InputClosed,
    Help,
    Dispatch(String),
    Unknown(String),
}

fn parse_runtime_input(line: &str) -> RuntimeInstruction {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return RuntimeInstruction::Empty;
    }

    let mut parts = trimmed.splitn(2, ' ');
    let command = parts.next().unwrap_or("");
    let rest = parts.next().map(str::trim).filter(|rest| !rest.is_empty());

    match command.to_ascii_lowercase().as_str() {
        "status" => RuntimeInstruction::Status,
        "reload" => RuntimeInstruction::Reload,
        "restart" => RuntimeInstruction::Restart,
        "quit" | "exit" | "q" => RuntimeInstruction::Quit,
        "help" | "?" => RuntimeInstruction::Help,
        "dispatch" => rest.map_or(
            RuntimeInstruction::Unknown("missing hotkey argument".to_string()),
            |hotkey| RuntimeInstruction::Dispatch(hotkey.to_string()),
        ),
        _ => RuntimeInstruction::Dispatch(trimmed.to_string()),
    }
}

fn parse_tui_input(line: &str) -> RuntimeInstruction {
    if line.trim().eq_ignore_ascii_case("refresh") {
        RuntimeInstruction::Status
    } else {
        parse_runtime_input(line)
    }
}

/// One instruction queue serves stdin, tray callbacks, and local compositor dispatch.
#[derive(Debug)]
struct RuntimeRequest {
    instruction: RuntimeInstruction,
    #[cfg(any(target_os = "linux", all(test, unix)))]
    reply: Option<local_session_dispatch::Reply>,
}

impl From<RuntimeInstruction> for RuntimeRequest {
    fn from(instruction: RuntimeInstruction) -> Self {
        Self {
            instruction,
            #[cfg(any(target_os = "linux", all(test, unix)))]
            reply: None,
        }
    }
}

/// EOF is an instruction, since local dispatch may keep the queue connected.
fn spawn_stdin_reader(parse: fn(&str) -> RuntimeInstruction, tx: mpsc::Sender<RuntimeRequest>) {
    thread::spawn(move || {
        for line in io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(parse(&line).into()).is_err() {
                return;
            }
        }
        let _ = tx.send(RuntimeInstruction::InputClosed.into());
    });
}

/// What a runtime surface is told to show.
enum Report {
    Event(RuntimeEvent),
    HotkeyDispatch(DispatchReport),
    ManualDispatch(DispatchState),
    Status,
    Help,
    Unknown(String),
    ReloadRequested,
    Restarted,
    InputClosed,
}

trait Surface {
    fn start(&mut self, runtime: &Runtime);
    fn report(&mut self, runtime: &Runtime, report: Report);
    /// Called once per loop iteration after all reports; `input` is whether an
    /// instruction was handled in this iteration.
    fn refresh(&mut self, runtime: &Runtime, now: Instant, input: bool);
    fn finish(&mut self);
}

/// Drives a surface until Quit. With `quit_on_input_end`, the end of the
/// instruction stream ends the session (interactive terminals); otherwise the
/// runtime keeps serving hotkeys (services with stdin closed).
fn run_runtime(
    runtime: &mut Runtime,
    surface: &mut impl Surface,
    instructions: mpsc::Receiver<RuntimeRequest>,
    quit_on_input_end: bool,
) {
    surface.start(runtime);
    surface.report(runtime, Report::Event(runtime.app.startup_event()));

    let mut input_open = true;
    loop {
        let instruction = if input_open {
            match instructions.recv_timeout(HOTKEY_POLL_INTERVAL) {
                Ok(instruction) => Some(instruction),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    input_open = false;
                    if quit_on_input_end {
                        surface.report(runtime, Report::InputClosed);
                        break;
                    }
                    None
                }
            }
        } else {
            thread::sleep(HOTKEY_POLL_INTERVAL);
            None
        };

        let now = Instant::now();
        let handled_input = instruction.is_some();
        if let Some(request) = instruction {
            #[cfg(any(target_os = "linux", all(test, unix)))]
            if let Some(reply) = &request.reply
                && now >= reply.deadline
            {
                let _ = reply.sender.try_send(local_session_dispatch::Response {
                    success: false,
                    output: "Error: dispatch expired before execution; no action was run\n"
                        .to_string(),
                });
                continue;
            }
            match request.instruction {
                RuntimeInstruction::Quit => break,
                RuntimeInstruction::InputClosed if quit_on_input_end => {
                    surface.report(runtime, Report::InputClosed);
                    break;
                }
                RuntimeInstruction::InputClosed | RuntimeInstruction::Empty => {}
                RuntimeInstruction::Status => surface.report(runtime, Report::Status),
                RuntimeInstruction::Help => surface.report(runtime, Report::Help),
                RuntimeInstruction::Unknown(message) => {
                    surface.report(runtime, Report::Unknown(message));
                }
                RuntimeInstruction::Reload => {
                    surface.report(runtime, Report::ReloadRequested);
                    for event in runtime.reload(now) {
                        surface.report(runtime, Report::Event(event));
                    }
                }
                RuntimeInstruction::Restart => {
                    let events = runtime.restart(now);
                    surface.report(runtime, Report::Restarted);
                    for event in events {
                        surface.report(runtime, Report::Event(event));
                    }
                }
                RuntimeInstruction::Dispatch(hotkey) => {
                    #[cfg(any(target_os = "linux", all(test, unix)))]
                    let mut output = request.reply.as_ref().map(|_| {
                        let mut output = Vec::new();
                        let _ = writeln!(
                            output,
                            "Using runtime window backend: {}",
                            runtime.window_system.name()
                        );
                        for line in runtime_status_lines(&runtime.app) {
                            let _ = writeln!(output, "{line}");
                        }
                        output
                    });
                    let state = runtime.dispatch(&hotkey);
                    #[cfg(any(target_os = "linux", all(test, unix)))]
                    if let Some(reply) = request.reply {
                        let output = output.as_mut().expect("remote report initialized");
                        let _ = write_dispatch_state(output, &state, &runtime.window_system);
                        let _ = reply.sender.try_send(local_session_dispatch::Response {
                            success: !matches!(state, DispatchState::Error(_)),
                            output: String::from_utf8(std::mem::take(output))
                                .expect("formatted report is UTF-8"),
                        });
                    }
                    surface.report(runtime, Report::ManualDispatch(state));
                }
            }
        }

        let (events, dispatches) = runtime.poll(now);
        for event in events {
            surface.report(runtime, Report::Event(event));
        }
        for dispatch in dispatches {
            surface.report(runtime, Report::HotkeyDispatch(dispatch));
        }
        surface.refresh(runtime, now, handled_input);
    }

    surface.finish();
}

/// Line-oriented session for terminals and background services.
struct CliSurface {
    prompt: bool,
    notifier: Option<Notifier>,
}

impl Surface for CliSurface {
    fn start(&mut self, runtime: &Runtime) {
        println!("Window backend: {}", runtime.window_system.name());
        println!("Interactive session started. type `help` for commands.");
        if self.prompt {
            print_prompt();
        }
    }

    fn report(&mut self, runtime: &Runtime, report: Report) {
        match report {
            Report::Event(event) => {
                println!("{event}");
                if let Some(notifier) = &mut self.notifier {
                    notifier.event(&event, Instant::now());
                }
            }
            Report::HotkeyDispatch(DispatchReport {
                hotkey,
                state: DispatchState::Error(error),
            }) => println!("Dispatch failed for {hotkey}: {error}"),
            Report::HotkeyDispatch(_) => {}
            Report::ManualDispatch(state) => print_dispatch_state(&state, &runtime.window_system),
            Report::Status => print_status(&runtime.app),
            Report::Help => print_help(),
            Report::Unknown(message) => {
                println!("Unknown command: {message}");
                println!("type `help` for usage.");
            }
            Report::ReloadRequested => println!("Reload requested."),
            Report::Restarted => println!("Runtime restarted."),
            Report::InputClosed => println!("Input stream closed."),
        }
        let _ = io::stdout().flush();
    }

    fn refresh(&mut self, _runtime: &Runtime, now: Instant, input: bool) {
        if let Some(notifier) = &mut self.notifier {
            notifier.tick(now);
        }
        if input && self.prompt {
            print_prompt();
        }
    }

    fn finish(&mut self) {
        println!("Session closed.");
    }
}

fn print_prompt() {
    print!("window-zones> ");
    let _ = io::stdout().flush();
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TuiCapabilityStatus {
    window: String,
    hotkey: String,
    diagnostic: Option<String>,
}

fn tui_capability_status(
    backend: RuntimeBackend,
    window_system: &RuntimeWindowSystem,
) -> TuiCapabilityStatus {
    #[cfg(target_os = "linux")]
    if matches!(backend, RuntimeBackend::Wayland(_)) {
        return TuiCapabilityStatus {
            window: "focused-window, displays, move-resize".to_string(),
            hotkey: "<none>".to_string(),
            diagnostic: None,
        };
    }
    let _ = backend;
    tui_capability_status_from(
        window_system
            .companion_capabilities()
            .map(|(_, capabilities)| capabilities),
    )
}

fn tui_capability_status_from(
    companion_capabilities: Option<Result<Vec<String>, String>>,
) -> TuiCapabilityStatus {
    let mut status = TuiCapabilityStatus {
        window: "focused-window, displays, move-resize".to_string(),
        hotkey: "hotkeys".to_string(),
        diagnostic: None,
    };

    let Some(capabilities) = companion_capabilities else {
        return status;
    };

    match capabilities {
        Ok(capabilities) => {
            let window_capabilities = capabilities
                .iter()
                .filter(|capability| capability.as_str() != "hotkeys")
                .cloned()
                .collect::<Vec<_>>();
            status.window = if window_capabilities.is_empty() {
                "<none>".to_string()
            } else {
                window_capabilities.join(", ")
            };
            status.hotkey = if capabilities
                .iter()
                .any(|capability| capability == "hotkeys")
            {
                "hotkeys".to_string()
            } else {
                "<none>".to_string()
            };
        }
        Err(error) => {
            status.window = "unavailable".to_string();
            status.hotkey = "unavailable".to_string();
            status.diagnostic = Some(error);
        }
    }

    status
}

fn tui_hotkey_mode(backend: RuntimeBackend) -> &'static str {
    match backend {
        RuntimeBackend::DryRun => "dry-run",
        #[cfg(target_os = "linux")]
        RuntimeBackend::Gnome => "gnome-companion",
        #[cfg(target_os = "linux")]
        RuntimeBackend::Kde => "kwin-companion",
        #[cfg(target_os = "linux")]
        RuntimeBackend::Wayland(_) => "compositor-bound dispatch",
        #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
        RuntimeBackend::Unsupported => "cli",
        #[allow(unreachable_patterns)]
        _ => "global",
    }
}

fn tui_last_error(app: &App, capabilities: &TuiCapabilityStatus) -> String {
    if let DispatchState::Error(error) = app.dispatch_state() {
        return format!("dispatch: {error}");
    }
    if let HotkeyRegistrationState::Error(error) = app.hotkey_state() {
        return format!("hotkey registration: {error}");
    }
    if let ConfigState::Error(error) = app.config_state() {
        return format!("config: {error}");
    }
    if let Some(error) = app.perf_log_error() {
        return error.to_string();
    }

    capabilities
        .diagnostic
        .as_deref()
        .unwrap_or("none")
        .to_string()
}

fn tui_frame(
    runtime: &Runtime,
    capabilities: &TuiCapabilityStatus,
    last_event: Option<&str>,
) -> String {
    let app = &runtime.app;
    let mut frame = format!(
        "\x1b[2J\x1b[HWindow Zones TUI\n================\n\
         Window backend: {}\nWindow capabilities: {}\nHotkey mode: {}\n\
         Hotkey capabilities: {}\nHotkey state: {:?}\nConfig: {} ({:?})\nBindings:\n",
        runtime.window_system.name(),
        capabilities.window,
        tui_hotkey_mode(runtime.backend),
        capabilities.hotkey,
        app.hotkey_state(),
        app.config_path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "<unresolved>".to_string()),
        app.config_state(),
    );
    if app.config().bindings.is_empty() {
        frame.push_str("  <none>\n");
    } else {
        for binding in &app.config().bindings {
            writeln!(frame, "  {} -> {:?}", binding.hotkey, binding.action)
                .expect("write TUI frame");
        }
    }
    write!(
        frame,
        "Last action: {}\nLast error: {}\nLast event: {}\n\n\
         Commands: reload | restart | status/refresh | dispatch HOTKEY | quit\nwindow-zones tui> ",
        app.last_dispatch_hotkey().unwrap_or("<none>"),
        tui_last_error(app, capabilities),
        last_event.unwrap_or("none"),
    )
    .expect("write TUI frame");
    frame
}

/// Full-screen ANSI dashboard redrawn whenever its content changes.
struct TuiSurface {
    snapshot: String,
    capabilities: TuiCapabilityStatus,
    capabilities_due: Instant,
    last_event: Option<String>,
    dirty: bool,
}

impl TuiSurface {
    fn new() -> Self {
        Self {
            snapshot: String::new(),
            capabilities: tui_capability_status_from(None),
            capabilities_due: Instant::now(),
            last_event: None,
            dirty: true,
        }
    }

    fn draw(&mut self, runtime: &Runtime, force: bool) {
        let frame = tui_frame(runtime, &self.capabilities, self.last_event.as_deref());
        if force || frame != self.snapshot {
            print!("{frame}");
            let _ = io::stdout().flush();
            self.snapshot = frame;
        }
        self.dirty = false;
    }
}

impl Surface for TuiSurface {
    fn start(&mut self, runtime: &Runtime) {
        self.capabilities = tui_capability_status(runtime.backend, &runtime.window_system);
        self.capabilities_due = Instant::now() + CAPABILITY_REFRESH_INTERVAL;
        self.draw(runtime, true);
    }

    fn report(&mut self, _runtime: &Runtime, report: Report) {
        self.dirty = true;
        match report {
            Report::Event(event) => self.last_event = Some(event.to_string()),
            Report::Unknown(message) => {
                self.last_event = Some(format!("unknown command: {message}"));
            }
            Report::Status | Report::Restarted => self.capabilities_due = Instant::now(),
            _ => {}
        }
    }

    fn refresh(&mut self, runtime: &Runtime, now: Instant, input: bool) {
        if now >= self.capabilities_due {
            let capabilities = tui_capability_status(runtime.backend, &runtime.window_system);
            self.dirty |= self.capabilities != capabilities;
            self.capabilities = capabilities;
            self.capabilities_due = now + CAPABILITY_REFRESH_INTERVAL;
        }
        if self.dirty || input {
            self.draw(runtime, input);
        }
    }

    fn finish(&mut self) {
        println!("\nSession closed.");
    }
}

/// Fixed tray status lines, updated in place.
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn tray_status_lines(runtime: &Runtime) -> [String; 4] {
    let app = &runtime.app;
    let config = match app.config_state() {
        ConfigState::Loaded => format!("Config: {} bindings", app.config().bindings.len()),
        ConfigState::Missing => "Config: missing (run `window_zones init`)".to_string(),
        ConfigState::Error(error) => format!("Config error: {error}"),
    };
    let hotkeys = match app.hotkey_state() {
        HotkeyRegistrationState::Registered => "Hotkeys: registered".to_string(),
        HotkeyRegistrationState::Unregistered => "Hotkeys: not registered".to_string(),
        HotkeyRegistrationState::Error(error) => format!("Hotkeys: {error}"),
    };
    let last = match (app.last_dispatch_hotkey(), app.dispatch_state()) {
        (Some(hotkey), DispatchState::Error(error)) => format!("Last: {hotkey} failed: {error}"),
        (Some(hotkey), _) => format!("Last: {hotkey}"),
        (None, _) => "Last: <none>".to_string(),
    };
    [
        format!("Backend: {}", runtime.window_system.name()),
        config,
        hotkeys,
        last,
    ]
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
struct TraySurface {
    tray: TrayItem,
    label_ids: Vec<u32>,
    labels: Vec<String>,
    cli: CliSurface,
    dirty: bool,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl TraySurface {
    fn new(runtime: &Runtime, tx: &mpsc::Sender<RuntimeRequest>) -> Result<Self, String> {
        #[cfg(target_os = "linux")]
        let icon = IconSource::Resource("view-grid-symbolic");
        #[cfg(target_os = "windows")]
        let icon = IconSource::Resource("");
        let mut tray = TrayItem::new("Window Zones", icon).map_err(|error| error.to_string())?;

        let labels = tray_status_lines(runtime).to_vec();
        let mut label_ids = Vec::with_capacity(labels.len());
        for line in &labels {
            #[cfg(target_os = "linux")]
            let id = {
                let tx = tx.clone();
                tray.inner_mut().add_menu_item_with_id(line, move || {
                    let _ = tx.send(RuntimeInstruction::Status.into());
                })
            };
            #[cfg(target_os = "windows")]
            let id = tray.inner_mut().add_label_with_id(line);
            label_ids.push(id.map_err(|error| error.to_string())?);
        }

        for (label, instruction) in [
            ("Show status", RuntimeInstruction::Status),
            ("Reload", RuntimeInstruction::Reload),
            ("Restart", RuntimeInstruction::Restart),
            ("Quit", RuntimeInstruction::Quit),
        ] {
            let tx = tx.clone();
            tray.add_menu_item(label, move || {
                let _ = tx.send(instruction.clone().into());
            })
            .map_err(|error| error.to_string())?;
        }

        Ok(Self {
            tray,
            label_ids,
            labels,
            dirty: false,
            cli: CliSurface {
                prompt: false,
                notifier: Notifier::new(),
            },
        })
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl Surface for TraySurface {
    fn start(&mut self, runtime: &Runtime) {
        println!("Window backend: {}", runtime.window_system.name());
        println!("Tray menu started. Use tray controls to reload/restart/quit.");
    }

    fn report(&mut self, runtime: &Runtime, report: Report) {
        self.dirty = true;
        self.cli.report(runtime, report);
    }

    fn refresh(&mut self, runtime: &Runtime, now: Instant, _input: bool) {
        self.cli.refresh(runtime, now, false);
        if !self.dirty {
            return;
        }
        self.dirty = false;
        for (index, line) in tray_status_lines(runtime).into_iter().enumerate() {
            if self.labels[index] == line {
                continue;
            }
            #[cfg(target_os = "linux")]
            let updated = self
                .tray
                .inner_mut()
                .set_menu_item_label(&line, self.label_ids[index]);
            #[cfg(target_os = "windows")]
            let updated = self
                .tray
                .inner_mut()
                .set_label(&line, self.label_ids[index]);
            match updated {
                Ok(()) => self.labels[index] = line,
                Err(error) => {
                    self.dirty = true;
                    eprintln!("Failed to refresh tray status: {error}");
                }
            }
        }
    }

    fn finish(&mut self) {
        self.cli.finish();
    }
}

/// How long hotkeys may stay unavailable before a notification is shown. Short
/// outages fix themselves: at login the App can start before GNOME Shell has
/// loaded the companion, and a Shell or companion restart reconnects within seconds.
#[cfg(any(target_os = "linux", target_os = "macos"))]
const HOTKEY_OUTAGE_GRACE: Duration = Duration::from_secs(20);

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProblemKind {
    Config,
    Hotkeys,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Debug, PartialEq, Eq)]
enum Notification {
    Problem(String),
    Resolved(String),
}

/// Decides which runtime events become desktop notifications. Config problems
/// and refused hotkeys are shown at once; unavailable hotkeys only once the
/// outage outlasts `HOTKEY_OUTAGE_GRACE`. A resolution is shown only for the
/// problem currently on screen.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Debug, Default)]
struct NotificationPolicy {
    pending_outage: Option<(Instant, String)>,
    shown: Option<ProblemKind>,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl NotificationPolicy {
    fn event(&mut self, event: &RuntimeEvent, now: Instant) -> Option<Notification> {
        let resolved = match event {
            RuntimeEvent::HotkeysUnavailable { .. } => {
                self.pending_outage = Some((now, event.to_string()));
                return None;
            }
            RuntimeEvent::ConfigFailed { .. } | RuntimeEvent::ConfigRejected { .. } => {
                return Some(self.problem(ProblemKind::Config, event));
            }
            RuntimeEvent::HotkeysRejected { .. } => {
                self.pending_outage = None;
                return Some(self.problem(ProblemKind::Hotkeys, event));
            }
            RuntimeEvent::ConfigLoaded { .. } => ProblemKind::Config,
            RuntimeEvent::HotkeysRegistered { .. } | RuntimeEvent::HotkeysRecovered { .. } => {
                self.pending_outage = None;
                ProblemKind::Hotkeys
            }
            RuntimeEvent::ConfigMissing { .. } => return None,
        };
        if self.shown == Some(resolved) {
            self.shown = None;
            return Some(Notification::Resolved(event.to_string()));
        }
        None
    }

    /// Shows an outage that has lasted past the grace period.
    fn tick(&mut self, now: Instant) -> Option<Notification> {
        let (since, _) = self.pending_outage.as_ref()?;
        if now.duration_since(*since) < HOTKEY_OUTAGE_GRACE {
            return None;
        }
        let (_, message) = self.pending_outage.take()?;
        self.shown = Some(ProblemKind::Hotkeys);
        Some(Notification::Problem(message))
    }

    fn problem(&mut self, kind: ProblemKind, event: &RuntimeEvent) -> Notification {
        self.shown = Some(kind);
        Notification::Problem(event.to_string())
    }
}

/// Desktop notifications for problems while no terminal is attached.
/// Linux updates one notification in place; macOS uses Notification Center.
#[cfg(any(target_os = "linux", target_os = "macos"))]
struct Notifier {
    policy: NotificationPolicy,
    #[cfg(target_os = "linux")]
    connection: Option<dbus::blocking::Connection>,
    #[cfg(target_os = "linux")]
    notification_id: u32,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl Notifier {
    fn new() -> Option<Self> {
        Some(Self {
            policy: NotificationPolicy::default(),
            #[cfg(target_os = "linux")]
            connection: None,
            #[cfg(target_os = "linux")]
            notification_id: 0,
        })
    }

    fn event(&mut self, event: &RuntimeEvent, now: Instant) {
        if let Some(notification) = self.policy.event(event, now) {
            self.show(notification);
        }
    }

    fn tick(&mut self, now: Instant) {
        if let Some(notification) = self.policy.tick(now) {
            self.show(notification);
        }
    }

    #[cfg(target_os = "linux")]
    fn show(&mut self, notification: Notification) {
        let (summary, body) = match &notification {
            Notification::Problem(body) => ("Window Zones needs attention", body),
            Notification::Resolved(body) => ("Window Zones", body),
        };
        if self.connection.is_none() {
            self.connection = dbus::blocking::Connection::new_session().ok();
        }
        let Some(connection) = &self.connection else {
            return;
        };
        let proxy = connection.with_proxy(
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            Duration::from_millis(500),
        );
        let hints = dbus::arg::PropMap::new();
        let result: Result<(u32,), dbus::Error> = proxy.method_call(
            "org.freedesktop.Notifications",
            "Notify",
            (
                "Window Zones",
                self.notification_id,
                "preferences-system-windows",
                summary,
                body.as_str(),
                Vec::<String>::new(),
                hints,
                -1_i32,
            ),
        );
        match result {
            Ok((id,)) => self.notification_id = id,
            Err(error) => {
                eprintln!("Desktop notification failed: {error}");
                self.connection = None;
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn show(&mut self, notification: Notification) {
        let (summary, body) = match notification {
            Notification::Problem(body) => ("Window Zones needs attention", body),
            Notification::Resolved(body) => ("Window Zones", body),
        };
        // argv transports user/config text without interpreting it as a script.
        // Delivery runs off the hotkey thread and its child is always reaped.
        thread::spawn(move || {
            let result = std::process::Command::new("osascript")
                .args([
                    "-l", "JavaScript", "-e",
                    "function run(argv) { const app = Application.currentApplication(); app.includeStandardAdditions = true; app.displayNotification(argv[1], {withTitle: argv[0]}); }",
                    "--", summary, &body,
                ])
                .status();
            match result {
                Ok(status) if status.success() => {}
                Ok(status) => eprintln!("Desktop notification failed: {status}"),
                Err(error) => eprintln!("Desktop notification failed: {error}"),
            }
        });
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
struct Notifier;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
impl Notifier {
    fn new() -> Option<Self> {
        None
    }

    fn event(&mut self, _event: &RuntimeEvent, _now: Instant) {}

    fn tick(&mut self, _now: Instant) {}
}

/// Held for the lifetime of a `run`/`tui` session: only one App instance owns
/// the hotkeys at a time.
#[derive(Debug)]
struct InstanceLock {
    _file: File,
    pid_path: PathBuf,
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // Keep the lock held while removing its sidecar; never unlink the lock
        // file, or another process could lock a different inode.
        let _ = fs::remove_file(&self.pid_path);
    }
}

#[derive(Debug)]
enum InstanceLockError {
    AlreadyRunning(String),
    Io(String),
}

fn instance_lock_path() -> PathBuf {
    #[cfg(target_os = "linux")]
    if let Some(runtime_dir) = env::var_os("XDG_RUNTIME_DIR").filter(|value| !value.is_empty()) {
        return PathBuf::from(runtime_dir).join("window_zones.lock");
    }
    let user = env::var("USER")
        .or_else(|_| env::var("USERNAME"))
        .unwrap_or_default();
    env::temp_dir().join(format!("window_zones-{user}.lock"))
}

fn acquire_instance_lock(path: &Path) -> Result<InstanceLock, InstanceLockError> {
    let file = File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|error| {
            InstanceLockError::Io(format!(
                "cannot open instance lock {}: {error}",
                path.display()
            ))
        })?;
    // The pid sits beside the lock: a Windows lock also blocks reading the locked file.
    let pid_path = path.with_extension("pid");
    match file.try_lock() {
        Ok(()) => {
            // The pid is informational, for the message another instance prints.
            let _ = fs::write(&pid_path, std::process::id().to_string());
            Ok(InstanceLock {
                _file: file,
                pid_path,
            })
        }
        Err(fs::TryLockError::WouldBlock) => {
            let owner = fs::read_to_string(&pid_path).unwrap_or_default();
            let owner = owner.trim();
            Err(InstanceLockError::AlreadyRunning(format!(
                "another window_zones session{} already owns the hotkeys; stop it first (for the systemd service: `systemctl --user stop window-zones`) or use `window_zones dispatch <hotkey>`",
                if owner.is_empty() {
                    String::new()
                } else {
                    format!(" (pid {owner})")
                }
            )))
        }
        Err(fs::TryLockError::Error(error)) => Err(InstanceLockError::Io(format!(
            "cannot lock {}: {error}",
            path.display()
        ))),
    }
}

fn runtime_status_lines(app: &App) -> Vec<String> {
    let mut status = vec![
        "Runtime status:".to_string(),
        format!(
            "  config path: {}",
            app.config_path()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "<unresolved>".to_string())
        ),
        format!("  binding count: {}", app.config().bindings.len()),
        format!("  config state: {:?}", app.config_state()),
        format!("  hotkey state: {:?}", app.hotkey_state()),
        format!(
            "  last action: {}",
            app.last_dispatch_hotkey().unwrap_or("<none>")
        ),
        format!("  dispatch state: {:?}", app.dispatch_state()),
    ];

    if let ConfigState::Error(error) = app.config_state() {
        status.push(format!("  config error: {error}"));
    }
    if let DispatchState::Error(error) = app.dispatch_state() {
        status.push(format!("  dispatch error: {error}"));
    }
    if let Some(error) = app.perf_log_error() {
        status.push(format!("  performance log error: {error}"));
    }

    status
}

fn print_status(app: &App) {
    for line in runtime_status_lines(app) {
        println!("{line}");
    }
}

fn print_dispatch_state(state: &DispatchState, window_system: &RuntimeWindowSystem) {
    let _ = write_dispatch_state(&mut io::stdout().lock(), state, window_system);
}

fn write_dispatch_state(
    output: &mut impl io::Write,
    state: &DispatchState,
    window_system: &RuntimeWindowSystem,
) -> io::Result<()> {
    writeln!(output, "Dispatch state: {state:?}")?;
    match state {
        DispatchState::Error(error) => writeln!(output, "  error: {error}")?,
        DispatchState::Succeeded => {
            if let Some(window_move) = window_system.last_move() {
                writeln!(
                    output,
                    "  last move: x={} y={} w={} h={}",
                    window_move.target.x,
                    window_move.target.y,
                    window_move.target.width,
                    window_move.target.height
                )?;
            }
        }
        DispatchState::Idle => {}
    }
    Ok(())
}

fn execute_dispatch(
    mut app: App,
    mut window_system: RuntimeWindowSystem,
    hotkey: &str,
) -> ExitCode {
    println!("Using runtime window backend: {}", window_system.name());
    print_status(&app);

    let state = app.dispatch_hotkey(hotkey, &mut window_system).clone();
    print_dispatch_state(&state, &window_system);
    if matches!(state, DispatchState::Error(_)) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn execute_status(app: &App, window_system: &RuntimeWindowSystem) {
    println!("Using runtime window backend: {}", window_system.name());
    match window_system.companion_capabilities() {
        Some((label, Ok(capabilities))) => {
            println!("{label} companion capabilities: {capabilities:?}");
        }
        Some((label, Err(error))) => println!("{label} companion status: {error}"),
        None => {}
    }
    print_status(app);
}

fn execute_session(
    config_path: Option<PathBuf>,
    backend: RuntimeBackend,
    command: &Command,
    show_tray: bool,
    perf_log_path: Option<&PathBuf>,
) -> ExitCode {
    let _lock = if matches!(backend, RuntimeBackend::DryRun) {
        None
    } else {
        match acquire_instance_lock(&instance_lock_path()) {
            Ok(lock) => Some(lock),
            Err(error) => {
                let (message, code) = match error {
                    InstanceLockError::AlreadyRunning(message) => {
                        (message, ExitCode::from(EXIT_ALREADY_RUNNING))
                    }
                    InstanceLockError::Io(message) => (message, ExitCode::FAILURE),
                };
                eprintln!("Error: {message}");
                return code;
            }
        }
    };

    let mut runtime = match Runtime::new(config_path, backend, perf_log_path) {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let (tx, instructions) = mpsc::channel();
    #[cfg(target_os = "linux")]
    let _dispatch_server = if matches!(backend, RuntimeBackend::Wayland(_)) {
        let server = (|| {
            let path = runtime.app.config_path().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "cannot resolve session config path",
                )
            })?;
            let identity =
                local_session_dispatch::Identity::new(compositor_dispatch_backend(backend), path)?;
            local_session_dispatch::Server::start(
                _lock.as_ref().expect("live session holds instance lock"),
                identity,
                tx.clone(),
            )
        })();
        match server {
            Ok(server) => Some(server),
            Err(error) => {
                eprintln!("Cannot start compositor-bound dispatch: {error}");
                return ExitCode::FAILURE;
            }
        }
    } else {
        None
    };
    let interactive = io::stdin().is_terminal();

    if matches!(command, Command::Tui) {
        spawn_stdin_reader(parse_tui_input, tx);
        run_runtime(&mut runtime, &mut TuiSurface::new(), instructions, true);
        return ExitCode::SUCCESS;
    }

    if show_tray {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        {
            let mut surface = match TraySurface::new(&runtime, &tx) {
                Ok(surface) => surface,
                Err(error) => {
                    eprintln!("Failed to start tray surface: {error}");
                    return ExitCode::FAILURE;
                }
            };
            drop(tx);
            run_runtime(&mut runtime, &mut surface, instructions, true);
            return ExitCode::SUCCESS;
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        eprintln!("Tray mode is not supported on this platform yet; continuing without it.");
    }

    let mut surface = CliSurface {
        prompt: interactive,
        // Without a terminal nobody reads stdout live: surface problems on the desktop.
        notifier: if io::stdout().is_terminal() || matches!(backend, RuntimeBackend::DryRun) {
            None
        } else {
            Notifier::new()
        },
    };
    spawn_stdin_reader(parse_runtime_input, tx);
    run_runtime(&mut runtime, &mut surface, instructions, interactive);
    ExitCode::SUCCESS
}

/// Default modifier chord for the starter config, chosen to avoid the desktop's
/// own bindings: GNOME uses Ctrl+Alt+arrows for workspaces and Super+arrows
/// for tiling; Plasma uses Ctrl+Meta+arrows for desktops.
fn default_chord(current_desktop: Option<&str>) -> &'static str {
    let desktop_is = |name: &str| {
        current_desktop.is_some_and(|desktops| {
            desktops
                .split(':')
                .any(|desktop| desktop.eq_ignore_ascii_case(name))
        })
    };
    if desktop_is("GNOME") {
        "Ctrl+Super"
    } else if desktop_is("KDE") {
        "Ctrl+Alt+Super"
    } else {
        "Ctrl+Alt"
    }
}

#[derive(Debug, PartialEq, Eq)]
enum InitOutcome {
    Written,
    AlreadyExists,
}

fn write_starter_config(path: &Path, chord: &str, force: bool) -> Result<InitOutcome, String> {
    if path.exists() && !force {
        return Ok(InitOutcome::AlreadyExists);
    }
    let contents = starter_config(chord);
    parse_config(&contents)
        .map_err(|error| format!("--chord `{chord}` does not form valid hotkeys: {error}"))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let mut file = match File::options()
        .write(true)
        .create(force)
        .create_new(!force)
        .truncate(force)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if !force && error.kind() == io::ErrorKind::AlreadyExists => {
            return Ok(InitOutcome::AlreadyExists);
        }
        Err(error) => return Err(format!("cannot write {}: {error}", path.display())),
    };
    file.write_all(contents.as_bytes())
        .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
    Ok(InitOutcome::Written)
}

fn execute_init(config_path: Option<PathBuf>, chord: Option<&str>, force: bool) -> ExitCode {
    let path = match config_path.map_or_else(default_config_path, Ok) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("Error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let current_desktop = env::var("XDG_CURRENT_DESKTOP").ok();
    let chord = chord.unwrap_or_else(|| default_chord(current_desktop.as_deref()));

    match write_starter_config(&path, chord, force) {
        Ok(InitOutcome::Written) => {
            println!("Wrote starter config to {}", path.display());
            println!(
                "Hotkeys use {chord}+<key>: arrows for halves (repeat Left/Right to cross to an adjacent display), U/I/J/K corners, D/F/G thirds, E/T two-thirds, Return maximize, C center, Backspace restore, Shift+Left/Right other display."
            );
            println!("Edit it any time; a running window_zones applies changes automatically.");
            ExitCode::SUCCESS
        }
        Ok(InitOutcome::AlreadyExists) => {
            println!(
                "Config already exists at {}; pass --force to replace it",
                path.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn print_help() {
    println!("window_zones: move and resize the focused window into zones with hotkeys");
    println!("Usage:");
    println!(
        "  window_zones [--config <path>] [--backend <auto|x11|wayland|windows|macos|dry-run>] status"
    );
    println!(
        "  window_zones [--perf-log <path>] [--config <path>] [--backend <...>] dispatch <HOTKEY>"
    );
    println!("  window_zones [--tray] [--perf-log <path>] [--config <path>] [--backend <...>] run");
    println!("  window_zones [--perf-log <path>] [--config <path>] [--backend <...>] tui");
    println!("  window_zones [--config <path>] init [--chord <MODIFIERS>] [--force]");
    println!("Commands:");
    println!("  init             write a starter config (default chord depends on the desktop)");
    println!("  status           print runtime state and exit");
    println!(
        "  dispatch         run the action bound to HOTKEY once and exit (non-zero on failure)"
    );
    println!("                   Sway/Hyprland dispatch requires an existing run/tui session");
    println!("  run              serve hotkeys until quit; keeps running without a terminal");
    println!("  run --tray       serve hotkeys with a tray status menu");
    println!("  tui              serve hotkeys with the ANSI dashboard");
    println!("Session commands: status, reload, restart, dispatch HOTKEY, help, q/quit/exit");
    println!("Options:");
    println!("  --perf-log PATH  append JSONL dispatch timings and app-process memory");
    println!(
        "                   Linux RSS/peak RSS exclude GNOME Shell; unavailable values are null"
    );
    println!("                   Sway/Hyprland logging belongs to run/tui, not dispatch clients");
}

fn main() -> ExitCode {
    let args = match parse_args() {
        ParseStatus::Help => {
            print_help();
            return ExitCode::SUCCESS;
        }
        ParseStatus::Err(message) => {
            eprintln!("Error: {message}");
            print_help();
            return ExitCode::FAILURE;
        }
        ParseStatus::Ok(args) => args,
    };

    if matches!(args.command, Command::Init) {
        return execute_init(args.config_path, args.chord.as_deref(), args.force);
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    if !matches!(args.backend, BackendPreference::DryRun) {
        eprintln!("Unsupported host OS for live window-system actions. Use --dry-run.");
        return ExitCode::FAILURE;
    }

    let backend = match resolve_runtime_backend(args.backend) {
        Ok(backend) => backend,
        Err(error) => {
            eprintln!("Backend selection failed: {error}");
            return ExitCode::FAILURE;
        }
    };

    if matches!(args.command, Command::Run | Command::Tui) {
        return execute_session(
            args.config_path,
            backend,
            &args.command,
            args.show_tray,
            args.perf_log_path.as_ref(),
        );
    }
    #[cfg(target_os = "linux")]
    if let Command::Dispatch { hotkey } = &args.command
        && matches!(backend, RuntimeBackend::Wayland(_))
    {
        return execute_compositor_dispatch(
            args.config_path,
            backend,
            args.perf_log_path.as_ref(),
            hotkey,
        );
    }
    let app = match build_app(args.config_path.as_ref(), args.perf_log_path.as_ref()) {
        Ok(app) => app,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };

    match &args.command {
        Command::Status => {
            execute_status(&app, &RuntimeWindowSystem::with_backend(backend));
            ExitCode::SUCCESS
        }
        Command::Dispatch { hotkey } => {
            execute_dispatch(app, RuntimeWindowSystem::with_backend(backend), hotkey)
        }
        Command::Run | Command::Tui | Command::Init => {
            unreachable!("handled before one-shot App construction")
        }
    }
}

#[cfg(target_os = "linux")]
fn compositor_dispatch_backend(backend: RuntimeBackend) -> &'static str {
    match backend {
        RuntimeBackend::Wayland(ScriptedCompositor::Sway) => "sway",
        RuntimeBackend::Wayland(ScriptedCompositor::Hyprland) => "hyprland",
        _ => unreachable!("only compositor-bound backends use local dispatch"),
    }
}

#[cfg(target_os = "linux")]
fn execute_compositor_dispatch(
    config_path: Option<PathBuf>,
    backend: RuntimeBackend,
    perf_log_path: Option<&PathBuf>,
    hotkey: &str,
) -> ExitCode {
    if perf_log_path.is_some() {
        eprintln!(
            "Error: compositor-bound dispatch uses session-owned performance logging; set --perf-log on `window_zones run` or `tui`, not on dispatch clients"
        );
        return ExitCode::FAILURE;
    }
    let response = (|| {
        let path = config_path
            .map_or_else(default_config_path, Ok)
            .map_err(|error| error.to_string())?;
        let identity =
            local_session_dispatch::Identity::new(compositor_dispatch_backend(backend), &path)
                .map_err(|error| format!("cannot resolve dispatch config path: {error}"))?;
        local_session_dispatch::dispatch(&instance_lock_path(), identity, hotkey)
    })();
    match response {
        Ok(response) => {
            print!("{}", response.output);
            if response.success {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            eprintln!("Error: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_perf_log_for_dispatch_run_tui_and_tray() {
        for command in [
            vec!["dispatch", "Ctrl+Alt+Left"],
            vec!["run"],
            vec!["tui"],
            vec!["run", "--tray"],
        ] {
            let mut inputs = vec!["--perf-log", "/tmp/window-zones.jsonl"];
            inputs.extend(command);
            let ParseStatus::Ok(args) = parse(&inputs) else {
                panic!("expected performance-log args");
            };
            assert_eq!(
                args.perf_log_path,
                Some(PathBuf::from("/tmp/window-zones.jsonl"))
            );
        }
        assert!(matches!(parse(&["run", "--perf-log"]), ParseStatus::Err(_)));
        assert!(matches!(
            parse(&["status", "--perf-log", "/tmp/window-zones.jsonl"]),
            ParseStatus::Err(_)
        ));
        assert!(matches!(
            parse(&["init", "--perf-log", "/tmp/window-zones.jsonl"]),
            ParseStatus::Err(_)
        ));
    }

    fn parse(raw: &[&str]) -> ParseStatus {
        let args: Vec<String> = raw.iter().map(|value| (*value).to_string()).collect();
        parse_args_from_inputs(&args)
    }

    fn temp_path(name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!("window_zones_main_{}_{name}", std::process::id()));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir_all(&path);
        path
    }

    #[test]
    fn unified_runtime_keeps_performance_logging_through_restart() {
        let config_path = temp_path("runtime_perf_config.toml");
        let log_path = temp_path("runtime_perf.jsonl");
        fs::write(&config_path, starter_config("Ctrl+Alt")).unwrap();
        let mut runtime = Runtime::new(
            Some(config_path.clone()),
            RuntimeBackend::DryRun,
            Some(&log_path),
        )
        .unwrap();
        let (tx, instructions) = mpsc::channel();
        for instruction in [
            RuntimeInstruction::Dispatch("Ctrl+Alt+Right".to_string()),
            RuntimeInstruction::Restart,
            RuntimeInstruction::Dispatch("Ctrl+Alt+Left".to_string()),
            RuntimeInstruction::Quit,
        ] {
            tx.send(instruction.into()).unwrap();
        }
        drop(tx);
        run_runtime(
            &mut runtime,
            &mut CliSurface {
                prompt: false,
                notifier: None,
            },
            instructions,
            true,
        );
        let rows: Vec<serde_json::Value> = fs::read_to_string(&log_path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["event"], "session");
        assert_eq!(rows[1]["hotkey"], "alt+ctrl+right");
        assert_eq!(rows[2]["hotkey"], "alt+ctrl+left");
        assert_eq!(rows[1]["result"], "ok");
        assert_eq!(rows[2]["result"], "ok");
        assert!(runtime.app.hotkeys_registered());
        assert!(runtime.app.perf_log_error().is_none());
        drop(runtime);
        fs::remove_file(config_path).unwrap();
        fs::remove_file(log_path).unwrap();
    }

    #[test]
    fn runtime_reports_performance_log_open_failure_before_dispatch() {
        let log_path = temp_path("runtime_perf_directory");
        fs::create_dir_all(&log_path).unwrap();
        let result = Runtime::new(
            Some(temp_path("runtime_perf_missing_config.toml")),
            RuntimeBackend::DryRun,
            Some(&log_path),
        );
        assert!(matches!(result, Err(error) if error.contains("Cannot open performance log")));
        fs::remove_dir(log_path).unwrap();
    }

    #[test]
    fn tui_skips_idle_frames_and_redraws_runtime_events() {
        let runtime = Runtime::new(
            Some(temp_path("idle_tui_missing_config.toml")),
            RuntimeBackend::DryRun,
            None,
        )
        .unwrap();
        let now = Instant::now();
        let mut surface = TuiSurface::new();
        surface.snapshot = "untouched".to_string();
        surface.dirty = false;
        surface.capabilities_due = now + CAPABILITY_REFRESH_INTERVAL;
        surface.refresh(&runtime, now, false);
        assert_eq!(surface.snapshot, "untouched");
        surface.report(&runtime, Report::Event(runtime.app.startup_event()));
        surface.refresh(&runtime, now, false);
        assert!(surface.snapshot.contains("Window Zones TUI"));
        assert!(!surface.dirty);
    }

    #[test]
    fn interactive_runtime_exits_on_input_end() {
        struct InputEndSurface {
            closed: bool,
            finished: bool,
        }
        impl Surface for InputEndSurface {
            fn start(&mut self, _: &Runtime) {}
            fn report(&mut self, _: &Runtime, report: Report) {
                self.closed |= matches!(report, Report::InputClosed);
            }
            fn refresh(&mut self, _: &Runtime, _: Instant, _: bool) {
                panic!("interactive EOF must exit before polling");
            }
            fn finish(&mut self) {
                self.finished = true;
            }
        }
        let mut runtime = Runtime::new(
            Some(temp_path("eof_missing_config.toml")),
            RuntimeBackend::DryRun,
            None,
        )
        .unwrap();
        let (tx, instructions) = mpsc::channel();
        drop(tx);
        let mut surface = InputEndSurface {
            closed: false,
            finished: false,
        };
        run_runtime(&mut runtime, &mut surface, instructions, true);
        assert!(surface.closed);
        assert!(surface.finished);
    }

    #[test]
    fn runtime_dry_run_rejects_a_closed_window_id_without_moving_the_focus() {
        let mut system = RuntimeWindowSystem::with_backend(RuntimeBackend::DryRun);
        let before = system.focused_window().unwrap();
        let window_move =
            WindowMove::new(WindowId::new("closed-window"), Rect::new(0, 0, 100, 100));
        assert_eq!(
            system.move_window(&window_move),
            Err(WindowSystemError::WindowGone(window_move.window.clone()))
        );
        assert_eq!(system.focused_window().unwrap(), before);
        assert!(system.last_move().is_none());
    }

    #[test]
    fn parses_commands_and_rejects_misplaced_flags() {
        assert!(matches!(
            parse(&["run", "--tray"]),
            ParseStatus::Ok(CliArgs {
                command: Command::Run,
                show_tray: true,
                ..
            })
        ));
        assert!(matches!(
            parse(&[]),
            ParseStatus::Ok(CliArgs {
                command: Command::Run,
                ..
            })
        ));
        assert!(matches!(
            &parse(&["dispatch", "Ctrl+Alt+Left"]),
            ParseStatus::Ok(CliArgs { command: Command::Dispatch { hotkey }, .. })
                if hotkey == "Ctrl+Alt+Left"
        ));
        assert!(matches!(
            &parse(&["init", "--chord", "Ctrl+Super", "--force"]),
            ParseStatus::Ok(CliArgs { command: Command::Init, chord: Some(chord), force: true, .. })
                if chord == "Ctrl+Super"
        ));
        for invalid in [
            &["status", "--tray"][..],
            &["tui", "--tray"],
            &["dispatch"],
            &["dispatch", "a", "b"],
            &["run", "--force"],
            &["status", "--chord", "Ctrl+Alt"],
            &["init", "--chord"],
            &["status", "run"],
            &["--mystery"],
        ] {
            assert!(matches!(parse(invalid), ParseStatus::Err(_)), "{invalid:?}");
        }
    }

    #[test]
    fn tui_refresh_is_status_without_changing_run_input() {
        assert_eq!(parse_tui_input("refresh"), RuntimeInstruction::Status);
        assert_eq!(
            parse_runtime_input("refresh"),
            RuntimeInstruction::Dispatch("refresh".to_string())
        );
        assert_eq!(
            parse_runtime_input("dispatch"),
            RuntimeInstruction::Unknown("missing hotkey argument".to_string())
        );
    }

    #[test]
    fn tui_capability_status_distinguishes_complete_degraded_and_unavailable_companions() {
        let complete = tui_capability_status_from(Some(Ok(vec![
            "focused-window".to_string(),
            "displays".to_string(),
            "move-resize".to_string(),
            "hotkeys".to_string(),
        ])));
        assert_eq!(complete.window, "focused-window, displays, move-resize");
        assert_eq!(complete.hotkey, "hotkeys");
        assert_eq!(complete.diagnostic, None);

        let degraded = tui_capability_status_from(Some(Ok(vec!["focused-window".to_string()])));
        assert_eq!(degraded.window, "focused-window");
        assert_eq!(degraded.hotkey, "<none>");

        let unavailable =
            tui_capability_status_from(Some(Err("GNOME companion unavailable".to_string())));
        assert_eq!(unavailable.window, "unavailable");
        assert_eq!(unavailable.hotkey, "unavailable");
        assert_eq!(
            unavailable.diagnostic.as_deref(),
            Some("GNOME companion unavailable")
        );
    }

    #[test]
    fn tui_error_precedence_prefers_runtime_errors_over_companion_diagnostics() {
        struct RefusingHotkeys;
        impl HotkeySystem for RefusingHotkeys {
            fn register_hotkeys(&mut self, _: &[String]) -> Result<(), HotkeySystemError> {
                Err(HotkeySystemError::Rejected("taken".to_string()))
            }
            fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
                Ok(None)
            }
        }
        let capability_status = TuiCapabilityStatus {
            window: "unavailable".to_string(),
            hotkey: "unavailable".to_string(),
            diagnostic: Some("companion unavailable".to_string()),
        };

        let missing_app = App::start_at(temp_path("missing.toml"));
        let config_path = temp_path("error_precedence.toml");
        fs::write(&config_path, "bindings = [").unwrap();
        let mut app = App::start_at(&config_path);
        let config_error = tui_last_error(&app, &capability_status);
        app.tick(&mut RefusingHotkeys, Instant::now());
        let hotkey_error = tui_last_error(&app, &capability_status);
        app.dispatch_hotkey("Ctrl+Alt+Left", &mut DryRunWindowSystem::new());
        let dispatch_error = tui_last_error(&app, &capability_status);
        fs::remove_file(config_path).unwrap();

        assert_eq!(
            tui_last_error(&missing_app, &capability_status),
            "companion unavailable"
        );
        assert!(config_error.starts_with("config: "), "{config_error}");
        assert!(
            hotkey_error.starts_with("hotkey registration: "),
            "{hotkey_error}"
        );
        assert!(dispatch_error.starts_with("dispatch: "), "{dispatch_error}");
    }

    #[test]
    fn dry_run_runtime_crosses_displays_and_restores_the_simulated_window() {
        let config_path = temp_path("dry_run_crossing.toml");
        fs::write(&config_path, starter_config("Ctrl+Alt")).unwrap();
        let mut runtime =
            Runtime::new(Some(config_path.clone()), RuntimeBackend::DryRun, None).unwrap();

        let mut targets = Vec::new();
        for hotkey in [
            "ctrl+alt+right",
            "ctrl+alt+right",
            "ctrl+alt+left",
            "ctrl+alt+backspace",
        ] {
            assert_eq!(
                runtime.dispatch(hotkey),
                DispatchState::Succeeded,
                "{hotkey}"
            );
            targets.push(runtime.window_system.last_move().unwrap().target);
        }
        fs::remove_file(config_path).unwrap();

        assert_eq!(
            targets,
            vec![
                Rect::new(960, 0, 960, 1080),
                Rect::new(1920, 0, 640, 1024),
                Rect::new(960, 0, 960, 1080),
                Rect::new(40, 40, 640, 480),
            ]
        );
    }

    #[test]
    fn second_instance_lock_is_refused_until_the_first_is_released() {
        let path = temp_path("instance.lock");

        let first = acquire_instance_lock(&path).unwrap();
        let InstanceLockError::AlreadyRunning(second) = acquire_instance_lock(&path).unwrap_err()
        else {
            panic!("expected lock contention");
        };
        drop(first);
        assert!(!path.with_extension("pid").exists());
        let third = acquire_instance_lock(&path).unwrap();
        drop(third);
        assert!(!path.with_extension("pid").exists());
        fs::remove_file(&path).unwrap();

        assert!(
            second.contains(&format!("pid {}", std::process::id())),
            "{second}"
        );
    }

    #[test]
    fn instance_lock_io_failures_are_not_reported_as_contention() {
        let path = temp_path("missing_lock_parent").join("instance.lock");
        assert!(matches!(
            acquire_instance_lock(&path),
            Err(InstanceLockError::Io(message)) if message.contains("cannot open")
        ));
    }

    #[test]
    fn init_writes_a_valid_config_once_and_rejects_bad_chords() {
        let directory = temp_path("init");
        let path = directory.join("nested").join("config.toml");

        let written = write_starter_config(&path, "Ctrl+Super", false);
        fs::write(&path, "# user edits\n").unwrap();
        let kept = write_starter_config(&path, "Ctrl+Super", false);
        let kept_contents = fs::read_to_string(&path).unwrap();
        let bad_chord = write_starter_config(&path, "Ctrl+Hyper", true);
        let forced = write_starter_config(&path, "Ctrl+Alt", true);
        let forced_config = parse_config(&fs::read_to_string(&path).unwrap()).unwrap();
        fs::remove_dir_all(&directory).unwrap();

        assert_eq!(written, Ok(InitOutcome::Written));
        assert_eq!(kept, Ok(InitOutcome::AlreadyExists));
        assert_eq!(kept_contents, "# user edits\n");
        assert!(bad_chord.unwrap_err().contains("Ctrl+Hyper"));
        assert_eq!(forced, Ok(InitOutcome::Written));
        assert_eq!(forced_config.bindings[0].hotkey, "alt+ctrl+left");
    }

    #[test]
    fn default_chord_avoids_desktop_bindings() {
        assert_eq!(default_chord(Some("GNOME")), "Ctrl+Super");
        assert_eq!(default_chord(Some("ubuntu:GNOME")), "Ctrl+Super");
        assert_eq!(default_chord(Some("KDE")), "Ctrl+Alt+Super");
        assert_eq!(default_chord(Some("XFCE")), "Ctrl+Alt");
        assert_eq!(default_chord(None), "Ctrl+Alt");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn runtime_window_backend_names_identify_native_linux_paths() {
        assert_eq!(
            RuntimeWindowSystem::with_backend(RuntimeBackend::X11).name(),
            "x11"
        );
        assert_eq!(
            RuntimeWindowSystem::with_backend(RuntimeBackend::Wayland(ScriptedCompositor::Sway))
                .name(),
            "wayland"
        );
        assert_eq!(
            RuntimeWindowSystem::with_backend(RuntimeBackend::Gnome).name(),
            "gnome-wayland"
        );
        assert_eq!(
            RuntimeWindowSystem::with_backend(RuntimeBackend::Kde).name(),
            "kde-wayland"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_backend_routing_never_crosses_session_protocols() {
        assert_eq!(
            resolve_linux_backend_in_session(BackendPreference::Auto, SessionIdentity::X11, false),
            Ok(RuntimeBackend::X11)
        );
        assert_eq!(
            resolve_linux_backend_in_session(BackendPreference::X11, SessionIdentity::X11, false),
            Ok(RuntimeBackend::X11)
        );
        assert!(
            resolve_linux_backend_in_session(
                BackendPreference::X11,
                SessionIdentity::Wayland,
                true
            )
            .is_err()
        );
        assert!(
            resolve_linux_backend_in_session(
                BackendPreference::Wayland,
                SessionIdentity::X11,
                false
            )
            .is_err()
        );
        assert_eq!(
            resolve_linux_backend_in_session(
                BackendPreference::DryRun,
                SessionIdentity::Wayland,
                true
            ),
            Ok(RuntimeBackend::DryRun)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn session_identity_uses_unambiguous_protocol_signals() {
        for (session_type, wayland, display, expected) in [
            (None, true, false, SessionIdentity::Wayland),
            (None, false, true, SessionIdentity::X11),
            (Some("wayland"), false, true, SessionIdentity::Wayland),
            (Some("wayland"), true, true, SessionIdentity::Wayland),
            (Some("x11"), false, false, SessionIdentity::X11),
        ] {
            assert_eq!(
                session_identity(session_type.map(std::ffi::OsStr::new), wayland, display),
                expected,
                "{session_type:?}, WAYLAND_DISPLAY={wayland}, DISPLAY={display}"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unresolved_sessions_require_explicit_backend_and_x11_needs_no_wayland_signal() {
        for (session_type, wayland, display) in [
            (None, false, false),
            (Some("x11"), true, false),
            (Some("x11"), true, true),
            (None, true, true),
            (Some("tty"), false, true),
            (Some(""), true, false),
        ] {
            let identity =
                || session_identity(session_type.map(std::ffi::OsStr::new), wayland, display);
            assert!(matches!(identity(), SessionIdentity::Unresolved(_)));
            let error =
                resolve_linux_backend_in_session(BackendPreference::Auto, identity(), wayland)
                    .unwrap_err();
            for hint in [
                "XDG_SESSION_TYPE",
                "WAYLAND_DISPLAY",
                "DISPLAY",
                "--backend x11|wayland",
            ] {
                assert!(error.contains(hint), "{error}");
            }
            let explicit_x11 =
                resolve_linux_backend_in_session(BackendPreference::X11, identity(), wayland);
            assert_eq!(
                explicit_x11.is_ok(),
                !wayland,
                "{session_type:?} {wayland} {display}"
            );
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn short_hotkey_outages_never_notify_and_long_ones_notify_once() {
        let unavailable = RuntimeEvent::HotkeysUnavailable {
            message: "companion absent".to_string(),
        };
        let recovered = RuntimeEvent::HotkeysRecovered { count: 18 };
        let start = Instant::now();

        // Login race: the companion appears a few seconds after the App.
        let mut login = NotificationPolicy::default();
        let login_notifications = [
            login.event(&RuntimeEvent::ConfigLoaded { bindings: 18 }, start),
            login.event(&unavailable, start),
            login.tick(start + Duration::from_secs(3)),
            login.event(&recovered, start + Duration::from_secs(4)),
            login.tick(start + HOTKEY_OUTAGE_GRACE * 2),
        ];

        // The companion stays away: one notification, then its resolution.
        let mut outage = NotificationPolicy::default();
        outage.event(&unavailable, start);
        let before_grace = outage.tick(start + HOTKEY_OUTAGE_GRACE - Duration::from_secs(1));
        let after_grace = outage.tick(start + HOTKEY_OUTAGE_GRACE);
        let again = outage.tick(start + HOTKEY_OUTAGE_GRACE * 3);
        let resolution = outage.event(&recovered, start + HOTKEY_OUTAGE_GRACE * 4);

        assert_eq!(login_notifications, [None, None, None, None, None]);
        assert_eq!(before_grace, None);
        assert_eq!(
            after_grace,
            Some(Notification::Problem(unavailable.to_string()))
        );
        assert_eq!(again, None);
        assert_eq!(
            resolution,
            Some(Notification::Resolved(recovered.to_string()))
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn config_problems_notify_at_once_and_only_their_own_fix_resolves_them() {
        let start = Instant::now();
        let failed = RuntimeEvent::ConfigFailed {
            message: "bad toml".to_string(),
        };
        let refused = RuntimeEvent::HotkeysRejected {
            message: "ctrl+super+c is taken".to_string(),
        };
        let loaded = RuntimeEvent::ConfigLoaded { bindings: 18 };
        let mut policy = NotificationPolicy::default();

        let config_problem = policy.event(&failed, start);
        let config_fixed = policy.event(&loaded, start);
        let hotkey_problem = policy.event(&refused, start);
        // A config reload alone does not claim the refused hotkeys are fixed.
        let unrelated_reload = policy.event(&loaded, start);
        let hotkeys_fixed = policy.event(&RuntimeEvent::HotkeysRegistered { count: 18 }, start);

        assert_eq!(
            config_problem,
            Some(Notification::Problem(failed.to_string()))
        );
        assert_eq!(
            config_fixed,
            Some(Notification::Resolved(loaded.to_string()))
        );
        assert_eq!(
            hotkey_problem,
            Some(Notification::Problem(refused.to_string()))
        );
        assert_eq!(unrelated_reload, None);
        assert!(matches!(hotkeys_fixed, Some(Notification::Resolved(_))));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sway_hyprland_accept_compositor_owned_hotkeys() {
        let mut hotkeys =
            RuntimeHotkeySystem::new(RuntimeBackend::Wayland(ScriptedCompositor::Hyprland));
        assert_eq!(hotkeys.register_hotkeys(&[]), Ok(()));
        assert_eq!(hotkeys.register_hotkeys(&["ctrl+a".to_string()]), Ok(()));
        assert_eq!(hotkeys.next_hotkey(), Ok(None));
    }
}
