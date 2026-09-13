use std::env;
use std::fmt::Write as _;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use std::thread;

use std::sync::mpsc;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::sync::mpsc::SyncSender;
use std::time::Duration;

#[cfg(any(target_os = "linux", target_os = "windows"))]
use tray_item::{IconSource, TrayItem};

#[cfg(target_os = "macos")]
use window_zones::MacOSWindowSystem;
#[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
use window_zones::RdevHotkeySystem;
#[cfg(target_os = "windows")]
use window_zones::WindowsWindowSystem;
use window_zones::{
    App, ConfigState, DispatchState, DisplayGeometry, FocusedWindow, HotkeyEvent,
    HotkeyRegistrationState, HotkeySystem, HotkeySystemError, WindowMove, WindowSystem,
};
#[cfg(target_os = "linux")]
use window_zones::{
    GnomeHotkeySystem, GnomeWindowSystem, KwinHotkeySystem, KwinWindowSystem, WaylandBackend,
    WaylandWindowSystem, X11WindowSystem,
};
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
}

#[derive(Debug)]
struct CliArgs {
    command: Command,
    config_path: Option<PathBuf>,
    backend: BackendPreference,
    show_tray: bool,
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

    let mut command = Command::Run;
    let mut config_path = None;
    let mut backend = BackendPreference::Auto;
    let mut show_tray = false;

    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];

        match arg.as_str() {
            "--config" => {
                let Some(next) = args.get(index + 1) else {
                    return ParseStatus::Err("--config requires a path argument".to_string());
                };

                config_path = Some(PathBuf::from(next));
                index += 1;
            }
            "--backend" => {
                let Some(next) = args.get(index + 1) else {
                    return ParseStatus::Err("--backend requires a value".to_string());
                };

                match parse_backend_preference(next) {
                    Ok(value) => backend = value,
                    Err(message) => return ParseStatus::Err(message),
                }

                index += 1;
            }
            "--dry-run" => backend = BackendPreference::DryRun,
            "--tray" => show_tray = true,
            "status" => {
                if !matches!(command, Command::Run) {
                    return ParseStatus::Err("only one command is allowed".to_string());
                }

                command = Command::Status;
            }
            "dispatch" => {
                if !matches!(command, Command::Run) {
                    return ParseStatus::Err("only one command is allowed".to_string());
                }

                command = Command::Dispatch {
                    hotkey: String::new(),
                };
            }
            "tui" => {
                if !matches!(command, Command::Run) {
                    return ParseStatus::Err("only one command is allowed".to_string());
                }

                command = Command::Tui;
            }
            "run" => {
                if !matches!(command, Command::Run) {
                    return ParseStatus::Err("only one command is allowed".to_string());
                }

                command = Command::Run;
            }
            arg if arg.starts_with('-') => {
                return ParseStatus::Err(format!("unknown flag `{arg}`"));
            }
            _ => match &mut command {
                Command::Dispatch { hotkey } if hotkey.is_empty() => {
                    *hotkey = arg.to_string();
                }
                Command::Dispatch { hotkey: _ } => {
                    return ParseStatus::Err(format!(
                        "dispatch command got an extra argument `{}`",
                        arg
                    ));
                }
                _ => {
                    return ParseStatus::Err(format!("unexpected argument `{arg}`"));
                }
            },
        }

        index += 1;
    }

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

    ParseStatus::Ok(CliArgs {
        command,
        config_path,
        backend,
        show_tray,
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
    Wayland,
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
    let identity = session_identity(
        session_type.as_deref(),
        env::var_os("WAYLAND_DISPLAY").is_some(),
        env::var_os("DISPLAY").is_some(),
    );
    resolve_linux_backend_in_session(preference, identity)
}

#[cfg(target_os = "linux")]
fn resolve_linux_backend_in_session(
    preference: BackendPreference,
    identity: SessionIdentity,
) -> Result<RuntimeBackend, String> {
    match preference {
        BackendPreference::DryRun => Ok(RuntimeBackend::DryRun),
        BackendPreference::X11 => {
            if matches!(identity, SessionIdentity::Wayland) {
                Err("X11 backend is unavailable inside a Wayland session; use the native Wayland compositor integration".to_string())
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
        WaylandBackend::Sway | WaylandBackend::Hyprland => Ok(RuntimeBackend::Wayland),
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

impl RuntimeWindowSystem {
    fn with_backend(backend: RuntimeBackend) -> Self {
        match backend {
            RuntimeBackend::DryRun => RuntimeWindowSystem::DryRun(DryRunWindowSystem::new()),
            #[cfg(target_os = "linux")]
            RuntimeBackend::X11 => RuntimeWindowSystem::X11(X11WindowSystem::new()),
            #[cfg(target_os = "linux")]
            RuntimeBackend::Wayland => RuntimeWindowSystem::Wayland(WaylandWindowSystem::new()),
            #[cfg(target_os = "linux")]
            RuntimeBackend::Gnome => RuntimeWindowSystem::Gnome(GnomeWindowSystem::new()),
            #[cfg(target_os = "linux")]
            RuntimeBackend::Kde => RuntimeWindowSystem::Kde(KwinWindowSystem::new()),
            #[cfg(target_os = "windows")]
            RuntimeBackend::Windows => RuntimeWindowSystem::Windows(WindowsWindowSystem::new()),
            #[cfg(target_os = "macos")]
            RuntimeBackend::MacOS => RuntimeWindowSystem::MacOS(MacOSWindowSystem::new()),
            #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
            RuntimeBackend::Unsupported => RuntimeWindowSystem::Unsupported,
        }
    }

    fn last_move(&self) -> Option<WindowMove> {
        match self {
            RuntimeWindowSystem::DryRun(system) => system.last_move,
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::X11(_) => None,
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Wayland(_) => None,
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Gnome(_) => None,
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Kde(_) => None,
            #[cfg(target_os = "windows")]
            RuntimeWindowSystem::Windows(_) => None,
            #[cfg(target_os = "macos")]
            RuntimeWindowSystem::MacOS(_) => None,
            #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
            RuntimeWindowSystem::Unsupported => None,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            RuntimeWindowSystem::DryRun(_) => "dry-run",
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::X11(_) => "x11",
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Wayland(_) => "wayland",
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Gnome(_) => "gnome-wayland",
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Kde(_) => "kde-wayland",
            #[cfg(target_os = "windows")]
            RuntimeWindowSystem::Windows(_) => "windows",
            #[cfg(target_os = "macos")]
            RuntimeWindowSystem::MacOS(_) => "macos",
            #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
            RuntimeWindowSystem::Unsupported => "unsupported",
        }
    }
}

impl WindowSystem for RuntimeWindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, window_zones::WindowSystemError> {
        match self {
            RuntimeWindowSystem::DryRun(system) => system.focused_window(),
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::X11(system) => system.focused_window(),
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Wayland(system) => system.focused_window(),
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Gnome(system) => system.focused_window(),
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Kde(system) => system.focused_window(),
            #[cfg(target_os = "windows")]
            RuntimeWindowSystem::Windows(system) => system.focused_window(),
            #[cfg(target_os = "macos")]
            RuntimeWindowSystem::MacOS(system) => system.focused_window(),
            #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
            RuntimeWindowSystem::Unsupported => Err(window_zones::WindowSystemError::Platform(
                "window-system adapter is unsupported on this platform".to_string(),
            )),
        }
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, window_zones::WindowSystemError> {
        match self {
            RuntimeWindowSystem::DryRun(system) => system.displays(),
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::X11(system) => system.displays(),
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Wayland(system) => system.displays(),
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Gnome(system) => system.displays(),
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Kde(system) => system.displays(),
            #[cfg(target_os = "windows")]
            RuntimeWindowSystem::Windows(system) => system.displays(),
            #[cfg(target_os = "macos")]
            RuntimeWindowSystem::MacOS(system) => system.displays(),
            #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
            RuntimeWindowSystem::Unsupported => Err(window_zones::WindowSystemError::Platform(
                "window-system adapter is unsupported on this platform".to_string(),
            )),
        }
    }

    fn move_focused_window(
        &mut self,
        window_move: WindowMove,
    ) -> Result<(), window_zones::WindowSystemError> {
        match self {
            RuntimeWindowSystem::DryRun(system) => system.move_focused_window(window_move),
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::X11(system) => system.move_focused_window(window_move),
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Wayland(system) => system.move_focused_window(window_move),
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Gnome(system) => system.move_focused_window(window_move),
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Kde(system) => system.move_focused_window(window_move),
            #[cfg(target_os = "windows")]
            RuntimeWindowSystem::Windows(system) => system.move_focused_window(window_move),
            #[cfg(target_os = "macos")]
            RuntimeWindowSystem::MacOS(system) => system.move_focused_window(window_move),
            #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
            RuntimeWindowSystem::Unsupported => Err(window_zones::WindowSystemError::Platform(
                "window-system adapter is unsupported on this platform".to_string(),
            )),
        }
    }
}

#[derive(Debug)]
struct DryRunWindowSystem {
    focused_window: FocusedWindow,
    displays: Vec<DisplayGeometry>,
    last_move: Option<WindowMove>,
}

impl DryRunWindowSystem {
    fn new() -> Self {
        Self {
            focused_window: FocusedWindow::new(window_zones::Rect::new(40, 40, 640, 480)),
            displays: vec![
                DisplayGeometry::new("display-0", window_zones::Rect::new(0, 0, 1920, 1080)),
                DisplayGeometry::new("display-1", window_zones::Rect::new(1920, 0, 1280, 1024)),
            ],
            last_move: None,
        }
    }
}

impl WindowSystem for DryRunWindowSystem {
    fn focused_window(&self) -> Result<Option<FocusedWindow>, window_zones::WindowSystemError> {
        Ok(Some(self.focused_window.clone()))
    }

    fn displays(&self) -> Result<Vec<DisplayGeometry>, window_zones::WindowSystemError> {
        Ok(self.displays.clone())
    }

    fn move_focused_window(
        &mut self,
        window_move: WindowMove,
    ) -> Result<(), window_zones::WindowSystemError> {
        self.last_move = Some(window_move);
        Ok(())
    }
}

enum RuntimeInstruction {
    Empty,
    Status,
    Reload,
    Restart,
    Quit,
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
        "dispatch" => rest.filter(|value| !value.is_empty()).map_or(
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

#[cfg(target_os = "linux")]
struct UnavailableHotkeySystem(String);

#[cfg(target_os = "linux")]
impl HotkeySystem for UnavailableHotkeySystem {
    fn register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), HotkeySystemError> {
        if hotkeys.is_empty() {
            Ok(())
        } else {
            Err(HotkeySystemError::Platform(self.0.clone()))
        }
    }

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
        Ok(None)
    }
}

enum RuntimeHotkeySystem {
    Noop(NoopHotkeySystem),
    #[cfg(target_os = "linux")]
    Unavailable(UnavailableHotkeySystem),
    #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
    Global(RdevHotkeySystem),
    #[cfg(target_os = "linux")]
    Gnome(GnomeHotkeySystem),
    #[cfg(target_os = "linux")]
    Kde(KwinHotkeySystem),
}

impl RuntimeHotkeySystem {
    fn new(backend: RuntimeBackend) -> Self {
        match backend {
            RuntimeBackend::DryRun => Self::Noop(NoopHotkeySystem),
            #[cfg(target_os = "linux")]
            RuntimeBackend::Wayland => Self::Unavailable(UnavailableHotkeySystem(
                "global hotkeys are unavailable on sway/hyprland; bind `window_zones dispatch <hotkey>` in the compositor config".to_string(),
            )),
            #[cfg(target_os = "linux")]
            RuntimeBackend::X11 => Self::Global(RdevHotkeySystem::new()),
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
        match self {
            Self::Noop(system) => system.register_hotkeys(hotkeys),
            #[cfg(target_os = "linux")]
            Self::Unavailable(system) => system.register_hotkeys(hotkeys),
            #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
            Self::Global(system) => system.register_hotkeys(hotkeys),
            #[cfg(target_os = "linux")]
            Self::Gnome(system) => system.register_hotkeys(hotkeys),
            #[cfg(target_os = "linux")]
            Self::Kde(system) => system.register_hotkeys(hotkeys),
        }
    }

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
        match self {
            Self::Noop(system) => system.next_hotkey(),
            #[cfg(target_os = "linux")]
            Self::Unavailable(system) => system.next_hotkey(),
            #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
            Self::Global(system) => system.next_hotkey(),
            #[cfg(target_os = "linux")]
            Self::Gnome(system) => system.next_hotkey(),
            #[cfg(target_os = "linux")]
            Self::Kde(system) => system.next_hotkey(),
        }
    }
}

fn configured_hotkeys_for_runtime(app: &App) -> Vec<String> {
    app.config()
        .bindings
        .iter()
        .map(|binding| binding.hotkey.clone())
        .collect()
}

fn rebind_if_configured_hotkeys_changed<H: HotkeySystem>(
    app: &mut App,
    hotkey_system: &mut H,
    cached_hotkeys: &mut Vec<String>,
    registration_is_valid: &mut bool,
    last_registration_error: &mut Option<String>,
    event_label: &str,
) {
    let target_hotkeys = configured_hotkeys_for_runtime(app);

    if *registration_is_valid && *cached_hotkeys == target_hotkeys {
        return;
    }

    match app.register_hotkeys(hotkey_system) {
        Ok(()) => {
            *cached_hotkeys = target_hotkeys;
            *registration_is_valid = true;
            if last_registration_error.take().is_some() {
                println!("{event_label} recovered.");
            }
        }
        Err(error) => {
            let message = error.to_string();
            if last_registration_error.as_deref() != Some(message.as_str()) {
                println!("{event_label} failed: {message}");
                *last_registration_error = Some(message);
            }
            *registration_is_valid = false;
        }
    }
}

fn runtime_instruction_receiver() -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel::<String>();

    thread::spawn(move || {
        let stdin = io::stdin();
        let mut stdin = stdin.lock();
        let mut line = String::new();

        while stdin.read_line(&mut line).expect("read stdin") > 0 {
            if tx.send(line.clone()).is_err() {
                break;
            }
            line.clear();
        }
    });

    rx
}

fn build_app(config_path: Option<&PathBuf>) -> App {
    match config_path {
        Some(path) => App::start_at(path),
        None => App::start(),
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

    status
}

fn print_status(app: &App) {
    for line in runtime_status_lines(app) {
        println!("{line}");
    }
}

fn print_dispatch_state(state: &DispatchState, window_system: &RuntimeWindowSystem) {
    println!("Dispatch state: {:?}", state);
    if let Some(window_move) = window_system.last_move() {
        println!(
            "  last move: x={} y={} w={} h={}",
            window_move.target.x,
            window_move.target.y,
            window_move.target.width,
            window_move.target.height
        );
    }
}

fn execute_dispatch(
    mut app: App,
    mut window_system: RuntimeWindowSystem,
    hotkey: String,
) -> ExitCode {
    println!("Using runtime window backend: {}", window_system.name());
    print_status(&app);

    let state = app.dispatch_hotkey(&hotkey, &mut window_system);
    print_dispatch_state(state, &window_system);
    if matches!(state, DispatchState::Error(_)) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn execute_status(app: App, backend: RuntimeBackend) {
    let window_system = RuntimeWindowSystem::with_backend(backend);
    println!("Using runtime window backend: {}", window_system.name());
    #[cfg(target_os = "linux")]
    if matches!(backend, RuntimeBackend::Gnome | RuntimeBackend::Kde) {
        let label = if matches!(backend, RuntimeBackend::Gnome) {
            "GNOME"
        } else {
            "KDE"
        };
        let capabilities = match backend {
            RuntimeBackend::Gnome => GnomeWindowSystem::new()
                .capabilities()
                .map_err(|error| error.to_string()),
            RuntimeBackend::Kde => KwinWindowSystem::new()
                .capabilities()
                .map_err(|error| error.to_string()),
            _ => unreachable!("companion label only applies to companion backends"),
        };
        match capabilities {
            Ok(capabilities) => println!("{label} companion capabilities: {capabilities:?}"),
            Err(error) => println!("{label} companion status: {error}"),
        }
    }
    print_status(&app);
}

fn execute_run(
    app: App,
    backend: RuntimeBackend,
    window_system: RuntimeWindowSystem,
    show_tray: bool,
) {
    if show_tray {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        {
            return execute_run_with_tray(app, backend, window_system);
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            eprintln!("Tray mode is not supported on this platform yet.");
        }
    }

    execute_run_cli(app, backend, window_system);
}

fn execute_run_cli(app: App, backend: RuntimeBackend, window_system: RuntimeWindowSystem) {
    execute_runtime_loop(app, backend, window_system, RuntimeSurface::Cli);
}

fn execute_tui(app: App, backend: RuntimeBackend, window_system: RuntimeWindowSystem) {
    execute_runtime_loop(app, backend, window_system, RuntimeSurface::Tui);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuntimeSurface {
    Cli,
    Tui,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TuiCapabilityStatus {
    window: String,
    hotkey: String,
    diagnostic: Option<String>,
}

fn tui_capability_status(_backend: RuntimeBackend) -> TuiCapabilityStatus {
    #[cfg(target_os = "linux")]
    if matches!(_backend, RuntimeBackend::Wayland) {
        return TuiCapabilityStatus {
            window: "focused-window, displays, move-resize".to_string(),
            hotkey: "<none>".to_string(),
            diagnostic: None,
        };
    }
    #[cfg(target_os = "linux")]
    let companion_capabilities = match _backend {
        RuntimeBackend::Gnome => Some(
            GnomeWindowSystem::new()
                .capabilities()
                .map_err(|error| error.to_string()),
        ),
        RuntimeBackend::Kde => Some(
            KwinWindowSystem::new()
                .capabilities()
                .map_err(|error| error.to_string()),
        ),
        _ => None,
    };

    #[cfg(not(target_os = "linux"))]
    let companion_capabilities: Option<Result<Vec<String>, String>> = None;

    tui_capability_status_from(companion_capabilities)
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
    if matches!(backend, RuntimeBackend::DryRun) {
        return "dry-run";
    }

    #[cfg(target_os = "linux")]
    if matches!(backend, RuntimeBackend::Gnome) {
        return "gnome-companion";
    }
    #[cfg(target_os = "linux")]
    if matches!(backend, RuntimeBackend::Kde) {
        return "kwin-companion";
    }
    #[cfg(target_os = "linux")]
    if matches!(backend, RuntimeBackend::Wayland) {
        return "compositor-bound dispatch";
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    if matches!(backend, RuntimeBackend::Unsupported) {
        return "cli";
    }

    "global"
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

    capabilities
        .diagnostic
        .as_deref()
        .unwrap_or("none")
        .to_string()
}

fn tui_frame(
    app: &App,
    backend: RuntimeBackend,
    window_system: &RuntimeWindowSystem,
    capabilities: &TuiCapabilityStatus,
) -> String {
    let mut frame = format!(
        "\x1b[2J\x1b[HWindow Zones TUI\n================\n\
         Window backend: {}\nWindow capabilities: {}\nHotkey mode: {}\n\
         Hotkey capabilities: {}\nHotkey state: {:?}\nConfig: {} ({:?})\nBindings:\n",
        window_system.name(),
        capabilities.window,
        tui_hotkey_mode(backend),
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
        "Last action: {}\nLast error: {}\n\n\
         Commands: reload | restart | status/refresh | dispatch HOTKEY | quit\nwindow-zones tui> ",
        app.last_dispatch_hotkey().unwrap_or("<none>"),
        tui_last_error(app, capabilities),
    )
    .expect("write TUI frame");
    frame
}

fn execute_runtime_loop(
    mut app: App,
    backend: RuntimeBackend,
    mut window_system: RuntimeWindowSystem,
    surface: RuntimeSurface,
) {
    let config_path = app.config_path().map(PathBuf::from);
    let stdin_is_terminal = io::stdin().is_terminal();
    let instruction_rx = runtime_instruction_receiver();

    let mut hotkey_system = RuntimeHotkeySystem::new(backend);
    let mut cached_hotkeys = configured_hotkeys_for_runtime(&app);
    let mut hotkeys_are_valid = false;
    let mut last_registration_error = None;
    rebind_if_configured_hotkeys_changed(
        &mut app,
        &mut hotkey_system,
        &mut cached_hotkeys,
        &mut hotkeys_are_valid,
        &mut last_registration_error,
        "Hotkey registration initially",
    );

    let mut tui_snapshot = String::new();

    if matches!(surface, RuntimeSurface::Cli) {
        println!("Window backend: {}", window_system.name());
        println!("Interactive session started. type `help` for commands.");
        print!("window-zones> ");
        io::stdout().flush().expect("stdout flush");
    } else {
        tui_snapshot = tui_frame(
            &app,
            backend,
            &window_system,
            &tui_capability_status(backend),
        );
        print!("{tui_snapshot}");
        io::stdout().flush().expect("stdout flush");
    }

    let mut hotkey_listener_available = true;

    loop {
        let mut redraw_tui = false;
        let mut prompt_cli = false;

        match instruction_rx.recv_timeout(Duration::from_millis(250)) {
            Ok(line) => {
                redraw_tui = matches!(surface, RuntimeSurface::Tui);
                prompt_cli = matches!(surface, RuntimeSurface::Cli);
                let instruction = match surface {
                    RuntimeSurface::Cli => parse_runtime_input(&line),
                    RuntimeSurface::Tui => parse_tui_input(&line),
                };

                match instruction {
                    RuntimeInstruction::Empty => {}
                    RuntimeInstruction::Status => {
                        if matches!(surface, RuntimeSurface::Cli) {
                            print_status(&app);
                        }
                    }
                    RuntimeInstruction::Reload => {
                        if matches!(surface, RuntimeSurface::Cli) {
                            println!("Reload requested.");
                        }
                        let state = app.poll_config_changes();
                        if matches!(surface, RuntimeSurface::Cli) {
                            match state {
                                ConfigState::Error(error) => {
                                    println!("Config reload error: {error}");
                                }
                                state => println!("Config state: {:?}", state),
                            }
                        }
                    }
                    RuntimeInstruction::Restart => {
                        app = build_app(config_path.as_ref());
                        hotkey_system = RuntimeHotkeySystem::new(backend);
                        hotkey_listener_available = true;
                        cached_hotkeys.clear();
                        hotkeys_are_valid = false;
                        last_registration_error = None;
                        if matches!(surface, RuntimeSurface::Cli) {
                            println!("Runtime restarted.");
                        }
                    }
                    RuntimeInstruction::Quit => break,
                    RuntimeInstruction::Help => {
                        if matches!(surface, RuntimeSurface::Cli) {
                            print_help();
                        }
                    }
                    RuntimeInstruction::Unknown(message) => {
                        if matches!(surface, RuntimeSurface::Cli) {
                            println!("Unknown command: {message}");
                            println!("type `help` for usage.");
                        }
                    }
                    RuntimeInstruction::Dispatch(hotkey) => {
                        let state = app.dispatch_hotkey(&hotkey, &mut window_system);
                        if matches!(surface, RuntimeSurface::Cli) {
                            print_dispatch_state(state, &window_system);
                        }
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if stdin_is_terminal {
                    println!("Input stream closed. Session closed.");
                    return;
                }
                thread::sleep(Duration::from_millis(250));
            }
        }

        let _ = app.poll_config_changes();
        rebind_if_configured_hotkeys_changed(
            &mut app,
            &mut hotkey_system,
            &mut cached_hotkeys,
            &mut hotkeys_are_valid,
            &mut last_registration_error,
            "Hotkey registration now",
        );

        if hotkeys_are_valid {
            match app.dispatch_next_hotkey(&mut hotkey_system, &mut window_system) {
                Ok(state) => {
                    if let DispatchState::Error(error) = state
                        && matches!(surface, RuntimeSurface::Cli)
                    {
                        println!("Dispatch failed: {error}");
                    }
                    hotkey_listener_available = true;
                }
                Err(error) if hotkey_listener_available => {
                    if matches!(surface, RuntimeSurface::Cli) {
                        println!("Dispatch failed: {error}");
                    }
                    hotkeys_are_valid = false;
                    hotkey_listener_available = false;
                    redraw_tui = true;
                }
                Err(_) => {
                    hotkeys_are_valid = false;
                    redraw_tui = true;
                }
            }
        }

        if matches!(surface, RuntimeSurface::Tui) {
            let next_snapshot = tui_frame(
                &app,
                backend,
                &window_system,
                &tui_capability_status(backend),
            );
            if next_snapshot != tui_snapshot {
                redraw_tui = true;
            }
            if redraw_tui {
                print!("{next_snapshot}");
                io::stdout().flush().expect("stdout flush");
                tui_snapshot = next_snapshot;
            }
        } else if prompt_cli {
            print!("window-zones> ");
            io::stdout().flush().expect("stdout flush");
        }
    }

    println!("Session closed.");
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
#[derive(Debug)]
enum RuntimeTrayCommand {
    Reload,
    Restart,
    Status,
    Quit,
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn execute_run_with_tray(
    mut app: App,
    backend: RuntimeBackend,
    mut window_system: RuntimeWindowSystem,
) {
    let config_path = app.config_path().map(PathBuf::from);
    let mut hotkey_system = RuntimeHotkeySystem::new(backend);
    let mut cached_hotkeys = configured_hotkeys_for_runtime(&app);
    let mut hotkeys_are_valid = false;
    let mut last_registration_error = None;
    let mut hotkey_listener_available = true;

    rebind_if_configured_hotkeys_changed(
        &mut app,
        &mut hotkey_system,
        &mut cached_hotkeys,
        &mut hotkeys_are_valid,
        &mut last_registration_error,
        "Hotkey registration initially",
    );

    let (tray_tx, tray_rx) = mpsc::sync_channel::<RuntimeTrayCommand>(16);
    let mut tray = match build_tray_menu(&app, &tray_tx) {
        Ok(tray) => tray,
        Err(error) => {
            eprintln!("Failed to start tray surface: {error}");
            return;
        }
    };
    let mut status_snapshot = runtime_status_lines(&app);

    println!("Window backend: {}", window_system.name());
    println!("Tray menu started. Use tray controls to reload/restart/quit.");

    loop {
        match tray_rx.recv_timeout(Duration::from_millis(250)) {
            Ok(RuntimeTrayCommand::Reload) => {
                println!("Reload requested.");
                match app.poll_config_changes() {
                    ConfigState::Error(error) => {
                        println!("Config reload error: {error}");
                    }
                    state => println!("Config state: {:?}", state),
                }
            }
            Ok(RuntimeTrayCommand::Restart) => {
                app = build_app(config_path.as_ref());
                hotkey_system = RuntimeHotkeySystem::new(backend);
                hotkey_listener_available = true;
                cached_hotkeys.clear();
                hotkeys_are_valid = false;
                last_registration_error = None;
                println!("Runtime restarted.");
            }
            Ok(RuntimeTrayCommand::Status) => print_status(&app),
            Ok(RuntimeTrayCommand::Quit) => {
                println!("Session closed.");
                return;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                println!("Tray channel closed. Session closed.");
                return;
            }
        }

        let _ = app.poll_config_changes();
        rebind_if_configured_hotkeys_changed(
            &mut app,
            &mut hotkey_system,
            &mut cached_hotkeys,
            &mut hotkeys_are_valid,
            &mut last_registration_error,
            "Hotkey registration now",
        );

        if hotkeys_are_valid {
            match app.dispatch_next_hotkey(&mut hotkey_system, &mut window_system) {
                Ok(state) => {
                    if let DispatchState::Error(error) = state {
                        println!("Dispatch failed: {error}");
                    } else if let DispatchState::Succeeded = state {
                        print_dispatch_state(state, &window_system);
                    }
                    hotkey_listener_available = true;
                }
                Err(error) if hotkey_listener_available => {
                    println!("Dispatch failed: {error}");
                    hotkeys_are_valid = false;
                    hotkey_listener_available = false;
                }
                Err(_) => {
                    hotkeys_are_valid = false;
                }
            }
        }

        let next_snapshot = runtime_status_lines(&app);
        if status_snapshot != next_snapshot {
            tray = match build_tray_menu(&app, &tray_tx) {
                Ok(new_tray) => {
                    status_snapshot = next_snapshot;
                    new_tray
                }
                Err(error) => {
                    eprintln!("Failed to refresh tray status: {error}");
                    tray
                }
            };
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn build_tray_menu(app: &App, tx: &SyncSender<RuntimeTrayCommand>) -> Result<TrayItem, String> {
    let mut tray = TrayItem::new("Window Zones", IconSource::Resource(""))
        .map_err(|error| error.to_string())?;

    for line in runtime_status_lines(app) {
        tray.add_label(&line).map_err(|error| error.to_string())?;
    }

    let status_tx = tx.clone();
    tray.add_menu_item("Show status", move || {
        let _ = status_tx.send(RuntimeTrayCommand::Status);
    })
    .map_err(|error| error.to_string())?;

    let reload_tx = tx.clone();
    tray.add_menu_item("Reload", move || {
        let _ = reload_tx.send(RuntimeTrayCommand::Reload);
    })
    .map_err(|error| error.to_string())?;

    let restart_tx = tx.clone();
    tray.add_menu_item("Restart", move || {
        let _ = restart_tx.send(RuntimeTrayCommand::Restart);
    })
    .map_err(|error| error.to_string())?;

    let quit_tx = tx.clone();
    tray.add_menu_item("Quit", move || {
        let _ = quit_tx.send(RuntimeTrayCommand::Quit);
    })
    .map_err(|error| error.to_string())?;

    Ok(tray)
}

fn print_help() {
    println!("window_zones: execute configured window movement actions");
    println!("Usage:");
    println!(
        "  window_zones [--tray] [--config <path>] [--backend <auto|x11|wayland|windows|macos|dry-run>] status"
    );
    println!(
        "  window_zones [--tray] [--config <path>] [--backend <auto|x11|wayland|windows|macos|dry-run>] dispatch <HOTKEY>"
    );
    println!(
        "  window_zones [--tray] [--config <path>] [--backend <auto|x11|wayland|windows|macos|dry-run>] run"
    );
    println!(
        "  window_zones [--config <path>] [--backend <auto|x11|wayland|windows|macos|dry-run>] tui"
    );
    println!("Commands:");
    println!("  status           print runtime state and exit");
    println!("  dispatch         dispatch a single hotkey and exit");
    println!("  run              start an interactive session (reload/restart/quit)");
    println!("  run --tray       start optional tray/menu surface (reload/restart/quit)");
    println!("  tui              start the ANSI runtime dashboard");
    println!("  q/quit/exit      leave interactive session");
}

fn main() -> ExitCode {
    match parse_args() {
        ParseStatus::Help => {
            print_help();
        }
        ParseStatus::Err(message) => {
            eprintln!("Error: {message}");
            print_help();
            std::process::exit(1);
        }
        ParseStatus::Ok(config) => {
            #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
            if !matches!(config.backend, BackendPreference::DryRun) {
                eprintln!("Unsupported host OS for live window-system actions. Use --dry-run.");
                std::process::exit(1);
            }

            let backend = match resolve_runtime_backend(config.backend) {
                Ok(backend) => backend,
                Err(error) => {
                    eprintln!("Backend selection failed: {error}");
                    std::process::exit(1);
                }
            };
            let app = build_app(config.config_path.as_ref());
            let window_system = RuntimeWindowSystem::with_backend(backend);

            match config.command {
                Command::Status => execute_status(app, backend),
                Command::Dispatch { hotkey } => {
                    return execute_dispatch(app, window_system, hotkey);
                }
                Command::Tui => execute_tui(app, backend, window_system),
                Command::Run => {
                    execute_run(app, backend, window_system, config.show_tray);
                }
            }
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, fs, path::PathBuf};

    #[test]
    fn parses_run_with_tray() {
        let ParseStatus::Ok(args) = parse(&["run", "--tray"]) else {
            panic!("expected parsed args");
        };
        assert!(matches!(args.command, Command::Run));
        assert!(args.show_tray);
    }

    #[test]
    fn parse_rejects_tray_with_non_run_command() {
        assert!(matches!(parse(&["status", "--tray"]), ParseStatus::Err(_)));
        assert!(matches!(
            parse(&["dispatch", "Ctrl+Alt+Left", "--tray"]),
            ParseStatus::Err(_)
        ));
    }

    fn parse(raw: &[&str]) -> ParseStatus {
        let args: Vec<String> = raw.iter().map(|value| (*value).to_string()).collect();
        parse_args_from_inputs(&args)
    }

    #[test]
    fn parses_tui_command() {
        let ParseStatus::Ok(args) = parse(&["tui"]) else {
            panic!("expected parsed args");
        };
        assert!(matches!(args.command, Command::Tui));
    }

    #[test]
    fn parse_rejects_tui_with_tray() {
        assert!(matches!(parse(&["tui", "--tray"]), ParseStatus::Err(_)));
    }

    #[test]
    fn tui_refresh_is_status_without_changing_run_input() {
        assert!(matches!(
            parse_tui_input("refresh"),
            RuntimeInstruction::Status
        ));
        assert!(matches!(
            parse_runtime_input("refresh"),
            RuntimeInstruction::Dispatch(hotkey) if hotkey == "refresh"
        ));
    }

    #[test]
    fn parses_status_command() {
        let ParseStatus::Ok(args) = parse(&["status"]) else {
            panic!("expected parsed args");
        };
        assert!(matches!(args.command, Command::Status));
    }

    #[test]
    fn parses_dispatch_command_with_hotkey() {
        let ParseStatus::Ok(args) = parse(&["dispatch", "Ctrl+Alt+Left"]) else {
            panic!("expected parsed args");
        };
        assert!(matches!(
            args.command,
            Command::Dispatch { hotkey } if hotkey == "Ctrl+Alt+Left"
        ));
    }

    #[test]
    fn parse_rejects_dispatch_without_hotkey() {
        assert!(matches!(parse(&["dispatch"]), ParseStatus::Err(_)));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parse_accepts_macos_backend() {
        let ParseStatus::Ok(args) = parse(&["status", "--backend", "macos"]) else {
            panic!("expected parsed args");
        };
        assert!(matches!(args.backend, BackendPreference::MacOS));
    }
    #[test]
    fn parse_rejects_unknown_flags() {
        assert!(matches!(parse(&["--mystery"]), ParseStatus::Err(_)));
    }
    #[derive(Debug)]
    struct FakeHotkeySystem {
        registration_calls: usize,
        registered_hotkeys: Vec<String>,
        outcomes: VecDeque<Result<(), HotkeySystemError>>,
    }

    impl FakeHotkeySystem {
        fn new(outcomes: Vec<Result<(), HotkeySystemError>>) -> Self {
            Self {
                registration_calls: 0,
                registered_hotkeys: Vec::new(),
                outcomes: outcomes.into(),
            }
        }
    }

    impl HotkeySystem for FakeHotkeySystem {
        fn register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), HotkeySystemError> {
            self.registration_calls += 1;
            self.registered_hotkeys = hotkeys.to_vec();
            self.outcomes.pop_front().unwrap_or(Ok(()))
        }

        fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
            Ok(None)
        }
    }

    fn write_test_config(name: &str, contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "window_zones_main_{}_{}.toml",
            std::process::id(),
            name
        ));
        let _ = fs::remove_file(&path);
        fs::write(&path, contents).unwrap();
        path
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
        assert_eq!(degraded.diagnostic, None);

        let unavailable =
            tui_capability_status_from(Some(Err("GNOME companion unavailable".to_string())));
        assert_eq!(unavailable.window, "unavailable");
        assert_eq!(unavailable.hotkey, "unavailable");
        assert_eq!(
            unavailable.diagnostic.as_deref(),
            Some("GNOME companion unavailable")
        );

        let dry_run = tui_capability_status(RuntimeBackend::DryRun);
        assert_eq!(dry_run.window, "focused-window, displays, move-resize");
        assert_eq!(dry_run.hotkey, "hotkeys");
        assert_eq!(dry_run.diagnostic, None);
    }

    #[test]
    fn tui_error_precedence_prefers_runtime_errors_over_companion_diagnostics() {
        let capability_status = TuiCapabilityStatus {
            window: "unavailable".to_string(),
            hotkey: "unavailable".to_string(),
            diagnostic: Some("companion unavailable".to_string()),
        };

        let missing_path = std::env::temp_dir().join(format!(
            "window_zones_main_missing_{}.toml",
            std::process::id()
        ));
        let _ = fs::remove_file(&missing_path);
        let missing_app = App::start_at(missing_path);
        assert_eq!(
            tui_last_error(&missing_app, &capability_status),
            "companion unavailable"
        );

        let config_path = write_test_config("error_precedence", "bindings = [");
        let mut app = App::start_at(&config_path);
        assert!(tui_last_error(&app, &capability_status).starts_with("config: "));

        let mut hotkeys = FakeHotkeySystem::new(vec![Err(HotkeySystemError::Platform(
            "permission denied".to_string(),
        ))]);
        assert!(app.register_hotkeys(&mut hotkeys).is_err());
        assert!(tui_last_error(&app, &capability_status).starts_with("hotkey registration: "));

        let mut window_system = DryRunWindowSystem::new();
        app.dispatch_hotkey("Ctrl+Alt+Left", &mut window_system);
        assert!(tui_last_error(&app, &capability_status).starts_with("dispatch: "));

        fs::remove_file(config_path).unwrap();
    }

    #[test]
    fn rebind_retries_failed_registration_and_skips_unchanged_valid_sets() {
        let config_path = write_test_config(
            "rebind",
            r#"[[bindings]]
hotkey = "Ctrl+Alt+Left"
action = { type = "move-to-zone", zone = "left-half" }
"#,
        );
        let mut app = App::start_at(&config_path);
        let mut hotkeys = FakeHotkeySystem::new(vec![
            Err(HotkeySystemError::Platform("unavailable".to_string())),
            Ok(()),
        ]);
        let mut cached_hotkeys = Vec::new();
        let mut registration_is_valid = false;
        let mut last_registration_error = None;

        rebind_if_configured_hotkeys_changed(
            &mut app,
            &mut hotkeys,
            &mut cached_hotkeys,
            &mut registration_is_valid,
            &mut last_registration_error,
            "test registration",
        );
        assert_eq!(hotkeys.registration_calls, 1);
        assert!(!registration_is_valid);
        assert!(cached_hotkeys.is_empty());
        assert_eq!(
            last_registration_error.as_deref(),
            Some("platform hotkey error: unavailable")
        );

        rebind_if_configured_hotkeys_changed(
            &mut app,
            &mut hotkeys,
            &mut cached_hotkeys,
            &mut registration_is_valid,
            &mut last_registration_error,
            "test registration",
        );
        assert_eq!(hotkeys.registration_calls, 2);
        assert!(registration_is_valid);
        assert_eq!(cached_hotkeys, vec!["alt+ctrl+left".to_string()]);
        assert!(last_registration_error.is_none());
        assert_eq!(app.hotkey_state(), &HotkeyRegistrationState::Registered);

        rebind_if_configured_hotkeys_changed(
            &mut app,
            &mut hotkeys,
            &mut cached_hotkeys,
            &mut registration_is_valid,
            &mut last_registration_error,
            "test registration",
        );
        assert_eq!(hotkeys.registration_calls, 2);

        fs::remove_file(config_path).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn runtime_window_backend_names_identify_native_linux_paths() {
        assert_eq!(
            RuntimeWindowSystem::with_backend(RuntimeBackend::X11).name(),
            "x11"
        );
        assert_eq!(
            RuntimeWindowSystem::with_backend(RuntimeBackend::Wayland).name(),
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
            resolve_linux_backend_in_session(BackendPreference::Auto, SessionIdentity::X11),
            Ok(RuntimeBackend::X11)
        );
        assert_eq!(
            resolve_linux_backend_in_session(BackendPreference::X11, SessionIdentity::X11),
            Ok(RuntimeBackend::X11)
        );
        assert!(
            resolve_linux_backend_in_session(BackendPreference::X11, SessionIdentity::Wayland)
                .is_err()
        );
        assert!(
            resolve_linux_backend_in_session(BackendPreference::Wayland, SessionIdentity::X11)
                .is_err()
        );
        assert_eq!(
            resolve_linux_backend_in_session(BackendPreference::DryRun, SessionIdentity::Wayland),
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
    fn unresolved_sessions_require_explicit_backend_selection() {
        for (session_type, wayland, display) in [
            (None, false, false),
            (Some("x11"), true, false),
            (Some("x11"), true, true),
            (None, true, true),
            (Some("tty"), false, true),
            (Some(""), true, false),
        ] {
            let identity =
                session_identity(session_type.map(std::ffi::OsStr::new), wayland, display);
            assert!(matches!(identity, SessionIdentity::Unresolved(_)));
            let error =
                resolve_linux_backend_in_session(BackendPreference::Auto, identity).unwrap_err();
            for hint in [
                "XDG_SESSION_TYPE",
                "WAYLAND_DISPLAY",
                "DISPLAY",
                "--backend x11|wayland",
            ] {
                assert!(error.contains(hint), "{error}");
            }
            assert_eq!(
                resolve_linux_backend_in_session(
                    BackendPreference::X11,
                    session_identity(session_type.map(std::ffi::OsStr::new), wayland, display),
                ),
                Ok(RuntimeBackend::X11)
            );
        }
    }

    #[test]
    fn dry_run_registration_does_not_need_a_native_listener() {
        let mut hotkeys = RuntimeHotkeySystem::new(RuntimeBackend::DryRun);
        assert_eq!(hotkeys.register_hotkeys(&["ctrl+a".to_string()]), Ok(()));
        assert_eq!(hotkeys.next_hotkey(), Ok(None));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sway_hyprland_registration_requires_compositor_bindings() {
        let mut hotkeys = RuntimeHotkeySystem::new(RuntimeBackend::Wayland);
        assert_eq!(hotkeys.register_hotkeys(&[]), Ok(()));
        assert!(matches!(
            hotkeys.register_hotkeys(&["ctrl+a".to_string()]),
            Err(HotkeySystemError::Platform(_))
        ));
        assert_eq!(hotkeys.next_hotkey(), Ok(None));
    }
}
