use std::collections::VecDeque;
use std::env;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, SyncSender};
use std::thread;
use std::time::Duration;

mod native_hotkey_system;
#[cfg(target_os = "linux")]
mod wayland_hotkey_system;
use native_hotkey_system::NativeHotkeySystem;
#[cfg(target_os = "linux")]
use wayland_hotkey_system::WaylandHotkeySystem;

#[cfg(any(target_os = "linux", target_os = "windows"))]
use tray_item::{IconSource, TrayItem};

use window_zones::{
    App, ConfigState, DispatchState, DisplayGeometry, FocusedWindow, HotkeyEvent, HotkeySystem,
    HotkeySystemError, WindowMove, WindowSystem,
};

#[cfg(target_os = "macos")]
use window_zones::MacOSWindowSystem;
#[cfg(target_os = "windows")]
use window_zones::WindowsWindowSystem;
#[cfg(target_os = "linux")]
use window_zones::{WaylandWindowSystem, X11WindowSystem};
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
        "auto" | "native" => Ok(BackendPreference::Auto),
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

#[derive(Debug)]
enum RuntimeWindowSystem {
    DryRun(DryRunWindowSystem),
    #[cfg(target_os = "linux")]
    X11(X11WindowSystem),
    #[cfg(target_os = "linux")]
    Wayland(WaylandWindowSystem),
    #[cfg(target_os = "windows")]
    Windows(WindowsWindowSystem),
    #[cfg(target_os = "macos")]
    MacOS(MacOSWindowSystem),
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    Unsupported,
}

impl RuntimeWindowSystem {
    fn with_preference(preference: BackendPreference) -> Self {
        match preference {
            BackendPreference::DryRun => RuntimeWindowSystem::DryRun(DryRunWindowSystem::new()),
            #[cfg(target_os = "linux")]
            BackendPreference::X11 => RuntimeWindowSystem::X11(X11WindowSystem::new()),
            #[cfg(target_os = "linux")]
            BackendPreference::Wayland => RuntimeWindowSystem::Wayland(WaylandWindowSystem::new()),
            #[cfg(target_os = "linux")]
            BackendPreference::Auto => {
                if is_wayland_session() {
                    RuntimeWindowSystem::Wayland(WaylandWindowSystem::new())
                } else {
                    RuntimeWindowSystem::X11(X11WindowSystem::new())
                }
            }
            #[cfg(target_os = "windows")]
            BackendPreference::Windows => RuntimeWindowSystem::Windows(WindowsWindowSystem::new()),
            #[cfg(target_os = "windows")]
            BackendPreference::Auto => RuntimeWindowSystem::Windows(WindowsWindowSystem::new()),
            #[cfg(target_os = "macos")]
            BackendPreference::MacOS => RuntimeWindowSystem::MacOS(MacOSWindowSystem::new()),
            #[cfg(target_os = "macos")]
            BackendPreference::Auto => RuntimeWindowSystem::MacOS(MacOSWindowSystem::new()),
            #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
            BackendPreference::Auto => RuntimeWindowSystem::Unsupported,
        }
    }

    fn last_move(&self) -> Option<WindowMove> {
        match self {
            RuntimeWindowSystem::DryRun(system) => system.last_move,
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::X11(_) => None,
            #[cfg(target_os = "linux")]
            RuntimeWindowSystem::Wayland(_) => None,
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
            focused_window: FocusedWindow::new(
                "display-0",
                window_zones::Rect::new(40, 40, 640, 480),
            ),
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

#[derive(Debug, Default)]
struct CliHotkeySystem {
    registered_hotkeys: Vec<String>,
    events: VecDeque<Result<HotkeyEvent, HotkeySystemError>>,
}

#[derive(Debug)]
enum RuntimeHotkeySystem {
    Cli(CliHotkeySystem),
    Native(NativeHotkeySystem),
    #[cfg(target_os = "linux")]
    Wayland(WaylandHotkeySystem),
}

#[derive(Debug, Clone, Copy)]
enum RuntimeHotkeyMode {
    Cli,
    Native,
    CliFallback,
}

fn should_fallback_to_cli_hotkeys(backend: BackendPreference) -> bool {
    #[cfg(target_os = "linux")]
    {
        should_use_wayland_hotkeys_by_preference(backend) && !WaylandHotkeySystem::is_available()
    }

    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

fn should_use_wayland_hotkey_system(backend: BackendPreference) -> bool {
    #[cfg(target_os = "linux")]
    {
        should_use_wayland_hotkeys_by_preference(backend) && WaylandHotkeySystem::is_available()
    }

    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(target_os = "linux")]
fn should_use_wayland_hotkeys_by_preference(backend: BackendPreference) -> bool {
    is_wayland_session()
        && matches!(
            backend,
            BackendPreference::Auto | BackendPreference::Wayland
        )
}

#[cfg(not(target_os = "linux"))]
fn should_use_wayland_hotkeys_by_preference(_backend: BackendPreference) -> bool {
    false
}

fn initial_hotkey_mode(backend: BackendPreference) -> RuntimeHotkeyMode {
    if matches!(backend, BackendPreference::DryRun) {
        RuntimeHotkeyMode::Cli
    } else if should_fallback_to_cli_hotkeys(backend) {
        RuntimeHotkeyMode::CliFallback
    } else {
        RuntimeHotkeyMode::Native
    }
}

fn build_hotkey_system(backend: BackendPreference) -> (RuntimeHotkeySystem, RuntimeHotkeyMode) {
    match initial_hotkey_mode(backend) {
        RuntimeHotkeyMode::Cli => (
            RuntimeHotkeySystem::Cli(CliHotkeySystem::default()),
            RuntimeHotkeyMode::Cli,
        ),
        RuntimeHotkeyMode::CliFallback => (
            RuntimeHotkeySystem::Cli(CliHotkeySystem::default()),
            RuntimeHotkeyMode::CliFallback,
        ),
        RuntimeHotkeyMode::Native => {
            #[cfg(target_os = "linux")]
            {
                if should_use_wayland_hotkey_system(backend) {
                    (
                        RuntimeHotkeySystem::Wayland(WaylandHotkeySystem::new()),
                        RuntimeHotkeyMode::Native,
                    )
                } else {
                    (
                        RuntimeHotkeySystem::Native(NativeHotkeySystem::new()),
                        RuntimeHotkeyMode::Native,
                    )
                }
            }

            #[cfg(not(target_os = "linux"))]
            {
                (
                    RuntimeHotkeySystem::Native(NativeHotkeySystem::new()),
                    RuntimeHotkeyMode::Native,
                )
            }
        }
    }
}

impl HotkeySystem for RuntimeHotkeySystem {
    fn register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), HotkeySystemError> {
        match self {
            Self::Cli(system) => system.register_hotkeys(hotkeys),
            Self::Native(system) => system.register_hotkeys(hotkeys),
            #[cfg(target_os = "linux")]
            Self::Wayland(system) => system.register_hotkeys(hotkeys),
        }
    }

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
        match self {
            Self::Cli(system) => system.next_hotkey(),
            Self::Native(system) => system.next_hotkey(),
            #[cfg(target_os = "linux")]
            Self::Wayland(system) => system.next_hotkey(),
        }
    }
}

impl CliHotkeySystem {
    fn queue_hotkey(&mut self, hotkey: String) {
        self.events.push_back(Ok(HotkeyEvent::Pressed { hotkey }));
    }
}

impl HotkeySystem for CliHotkeySystem {
    fn register_hotkeys(&mut self, hotkeys: &[String]) -> Result<(), HotkeySystemError> {
        self.registered_hotkeys = hotkeys.to_vec();
        Ok(())
    }

    fn next_hotkey(&mut self) -> Result<Option<HotkeyEvent>, HotkeySystemError> {
        match self.events.pop_front() {
            Some(event) => event.map(Some),
            None => Ok(None),
        }
    }
}

fn build_app(config_path: Option<&PathBuf>) -> App {
    match config_path {
        Some(path) => App::start_at(path),
        None => App::start(),
    }
}

#[cfg(target_os = "linux")]
fn is_wayland_session() -> bool {
    env::var_os("XDG_SESSION_TYPE")
        .and_then(|value| value.to_str().map(str::to_ascii_lowercase))
        .is_some_and(|value| value == "wayland")
        || env::var_os("WAYLAND_DISPLAY").is_some()
}

fn hotkey_backend_label(backend: RuntimeHotkeyMode) -> &'static str {
    match backend {
        RuntimeHotkeyMode::Cli => "cli",
        RuntimeHotkeyMode::Native => "native",
        RuntimeHotkeyMode::CliFallback => "cli (fallback: native unavailable)",
    }
}

fn register_hotkeys_with_fallback(
    app: &mut App,
    backend: BackendPreference,
    hotkey_system: &mut RuntimeHotkeySystem,
    hotkey_mode: &mut RuntimeHotkeyMode,
    context: &str,
) {
    if let Err(error) = app.register_hotkeys(hotkey_system) {
        if matches!(hotkey_mode, RuntimeHotkeyMode::Native)
            && should_use_wayland_hotkeys_by_preference(backend)
        {
            println!("Native hotkey listener is unavailable in this session: {error}");
            println!("Falling back to CLI hotkey mode for this run.");

            *hotkey_system = RuntimeHotkeySystem::Cli(CliHotkeySystem::default());
            *hotkey_mode = RuntimeHotkeyMode::CliFallback;

            if let Err(error) = app.register_hotkeys(hotkey_system) {
                println!("{context} hotkey registration failed in fallback mode: {error}");
                return;
            }
        } else {
            println!("{context} hotkey registration failed: {error}");
            return;
        }
    }

    println!("Registered {} hotkeys.", app.config().bindings.len());
}

fn fallback_to_cli_hotkeys_from_dispatch(
    app: &mut App,
    backend: BackendPreference,
    hotkey_system: &mut RuntimeHotkeySystem,
    hotkey_mode: &mut RuntimeHotkeyMode,
    context: &str,
    error: &HotkeySystemError,
) -> bool {
    if !matches!(hotkey_mode, RuntimeHotkeyMode::Native)
        || !should_use_wayland_hotkeys_by_preference(backend)
    {
        return false;
    }

    println!("Native hotkey listener errored during {context}: {error}");
    println!("Falling back to CLI hotkey mode for this run.");

    *hotkey_system = RuntimeHotkeySystem::Cli(CliHotkeySystem::default());
    *hotkey_mode = RuntimeHotkeyMode::CliFallback;

    if let Err(error) = app.register_hotkeys(hotkey_system) {
        println!("{context} hotkey registration failed in fallback mode: {error}");
        return false;
    }

    true
}

fn runtime_status_lines(app: &App, hotkey_mode: RuntimeHotkeyMode) -> Vec<String> {
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
        format!("  hotkey backend: {}", hotkey_backend_label(hotkey_mode)),
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

fn print_status(app: &App, hotkey_mode: RuntimeHotkeyMode) {
    for line in runtime_status_lines(app, hotkey_mode) {
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

fn execute_dispatch(mut app: App, mut window_system: RuntimeWindowSystem, hotkey: String) {
    println!("Using runtime window backend: {}", window_system.name());
    print_status(&app, RuntimeHotkeyMode::Cli);

    let mut hotkey_system = CliHotkeySystem::default();
    if let Err(error) = app.register_hotkeys(&mut hotkey_system) {
        println!("hotkey registration error: {error}");
    }

    hotkey_system.queue_hotkey(hotkey);
    match app.dispatch_next_hotkey(&mut hotkey_system, &mut window_system) {
        Ok(state) => print_dispatch_state(state, &window_system),
        Err(error) => println!("Dispatch failed: {error}"),
    }
}

fn execute_status(app: App, backend: BackendPreference) {
    let hotkey_mode = initial_hotkey_mode(backend);
    println!(
        "Using runtime hotkey backend: {}",
        hotkey_backend_label(hotkey_mode)
    );
    print_status(&app, hotkey_mode);
}

fn execute_run(
    app: App,
    window_system: RuntimeWindowSystem,
    show_tray: bool,
    backend: BackendPreference,
) {
    if show_tray {
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        {
            return execute_run_with_tray(app, window_system, backend);
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            eprintln!("Tray mode is not supported on this platform yet.");
        }
    }

    execute_run_cli(app, window_system, backend);
}

fn execute_run_cli(
    mut app: App,
    mut window_system: RuntimeWindowSystem,
    backend: BackendPreference,
) {
    let config_path = app.config_path().map(PathBuf::from);
    let (mut hotkey_system, mut hotkey_mode) = build_hotkey_system(backend);

    if matches!(hotkey_mode, RuntimeHotkeyMode::CliFallback) {
        println!("Native hotkey listener is unavailable in this Wayland session; using CLI mode.");
    }

    register_hotkeys_with_fallback(
        &mut app,
        backend,
        &mut hotkey_system,
        &mut hotkey_mode,
        "Hotkey registration initially",
    );

    println!("Window backend: {}", window_system.name());
    println!("Hotkey backend: {}", hotkey_backend_label(hotkey_mode));
    println!("Interactive session started. type `help` for commands.");

    let (input_tx, input_rx) = mpsc::sync_channel::<String>(16);
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut stdin = stdin.lock();
        let mut line = String::new();

        loop {
            line.clear();
            match stdin.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            };

            if input_tx.send(line.clone()).is_err() {
                break;
            }
        }
    });

    let print_prompt = || {
        print!("window-zones> ");
        io::stdout().flush().expect("stdout flush");
    };

    print_prompt();

    loop {
        match input_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                match parse_runtime_input(&line) {
                    RuntimeInstruction::Empty => {}
                    RuntimeInstruction::Status => print_status(&app, hotkey_mode),
                    RuntimeInstruction::Reload => {
                        println!("Reload requested.");
                        match app.poll_config_changes() {
                            ConfigState::Error(error) => {
                                println!("Config reload error: {error}");
                            }
                            state => println!("Config state: {:?}", state),
                        }
                    }
                    RuntimeInstruction::Restart => {
                        app = build_app(config_path.as_ref());
                        println!("Runtime restarted.");
                        register_hotkeys_with_fallback(
                            &mut app,
                            backend,
                            &mut hotkey_system,
                            &mut hotkey_mode,
                            "Hotkey registration now",
                        );
                    }
                    RuntimeInstruction::Quit => break,
                    RuntimeInstruction::Help => print_help(),
                    RuntimeInstruction::Unknown(message) => {
                        println!("Unknown command: {message}");
                        println!("type `help` for usage.");
                    }
                    RuntimeInstruction::Dispatch(hotkey) => {
                        let state = app.dispatch_hotkey(&hotkey, &mut window_system);
                        print_dispatch_state(state, &window_system);
                    }
                }

                print_prompt();
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                println!("Input channel closed. Session closed.");
                break;
            }
        }

        match app.dispatch_next_hotkey(&mut hotkey_system, &mut window_system) {
            Ok(DispatchState::Idle) => {}
            Ok(state) => {
                if let DispatchState::Error(error) = state {
                    println!("Dispatch failed: {error}");
                } else {
                    print_dispatch_state(state, &window_system);
                }
            }
            Err(error) => {
                if !fallback_to_cli_hotkeys_from_dispatch(
                    &mut app,
                    backend,
                    &mut hotkey_system,
                    &mut hotkey_mode,
                    "dispatch polling",
                    &error,
                ) {
                    println!("Dispatch failed: {error}");
                }
            }
        }

        let _ = app.poll_config_changes();
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
    mut window_system: RuntimeWindowSystem,
    backend: BackendPreference,
) {
    let config_path = app.config_path().map(PathBuf::from);
    let (mut hotkey_system, mut hotkey_mode) = build_hotkey_system(backend);

    if matches!(hotkey_mode, RuntimeHotkeyMode::CliFallback) {
        println!("Native hotkey listener is unavailable in this Wayland session; using CLI mode.");
    }

    register_hotkeys_with_fallback(
        &mut app,
        backend,
        &mut hotkey_system,
        &mut hotkey_mode,
        "Hotkey registration initially",
    );

    let (tray_tx, tray_rx) = mpsc::sync_channel::<RuntimeTrayCommand>(16);
    let mut tray = match build_tray_menu(&app, &tray_tx, hotkey_mode) {
        Ok(tray) => tray,
        Err(error) => {
            eprintln!("Failed to start tray surface: {error}");
            return;
        }
    };
    let mut status_snapshot = runtime_status_lines(&app, hotkey_mode);

    println!("Window backend: {}", window_system.name());
    println!("Tray menu started. Use tray controls to reload/restart/quit.");
    println!("Hotkey backend: {}", hotkey_backend_label(hotkey_mode));

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
                println!("Runtime restarted.");
                register_hotkeys_with_fallback(
                    &mut app,
                    backend,
                    &mut hotkey_system,
                    &mut hotkey_mode,
                    "Hotkey registration now",
                );
            }
            Ok(RuntimeTrayCommand::Status) => print_status(&app, hotkey_mode),
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

        match app.dispatch_next_hotkey(&mut hotkey_system, &mut window_system) {
            Ok(DispatchState::Idle) => {}
            Ok(state) => {
                if let DispatchState::Error(error) = state {
                    println!("Dispatch failed: {error}");
                } else if let DispatchState::Succeeded = state {
                    print_dispatch_state(state, &window_system);
                }
            }
            Err(error) => {
                if !fallback_to_cli_hotkeys_from_dispatch(
                    &mut app,
                    backend,
                    &mut hotkey_system,
                    &mut hotkey_mode,
                    "dispatch polling",
                    &error,
                ) {
                    println!("Dispatch failed: {error}");
                }
            }
        }

        let next_snapshot = runtime_status_lines(&app, hotkey_mode);
        if status_snapshot != next_snapshot {
            tray = match build_tray_menu(&app, &tray_tx, hotkey_mode) {
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

        let _ = app.poll_config_changes();
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn build_tray_menu(
    app: &App,
    tx: &SyncSender<RuntimeTrayCommand>,
    hotkey_mode: RuntimeHotkeyMode,
) -> Result<TrayItem, String> {
    let mut tray = TrayItem::new("Window Zones", IconSource::Resource(""))
        .map_err(|error| error.to_string())?;

    for line in runtime_status_lines(app, hotkey_mode) {
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
        "  window_zones [--tray] [--config <path>] [--backend <auto|native|x11|wayland|windows|macos|dry-run>] status"
    );
    println!(
        "  window_zones [--tray] [--config <path>] [--backend <auto|native|x11|wayland|windows|macos|dry-run>] dispatch <HOTKEY>"
    );
    println!(
        "  window_zones [--tray] [--config <path>] [--backend <auto|native|x11|wayland|windows|macos|dry-run>] run"
    );
    println!("Commands:");
    println!("  status           print runtime state and exit");
    println!("  dispatch         dispatch a single hotkey and exit");
    println!("  run              start an interactive session (reload/restart/quit)");
    println!("  run --tray       start optional tray/menu surface (reload/restart/quit)");
    println!("  q/quit/exit      leave interactive session");
}

fn main() {
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

            let app = build_app(config.config_path.as_ref());
            let window_system = RuntimeWindowSystem::with_preference(config.backend);

            match config.command {
                Command::Status => execute_status(app, config.backend),
                Command::Dispatch { hotkey } => execute_dispatch(app, window_system, hotkey),
                Command::Run => {
                    execute_run(app, window_system, config.show_tray, config.backend);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn parse_accepts_native_backend_alias() {
        let ParseStatus::Ok(args) = parse(&["status", "--backend", "native"]) else {
            panic!("expected parsed args");
        };
        assert!(matches!(args.backend, BackendPreference::Auto));
    }

    #[test]
    fn parse_rejects_unknown_flags() {
        assert!(matches!(parse(&["--mystery"]), ParseStatus::Err(_)));
    }
}
