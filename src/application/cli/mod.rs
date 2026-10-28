mod help;
mod init;

pub(super) use help::print_help;
pub(super) use init::execute_init;

use std::env;
use std::path::PathBuf;

use super::backends::BackendPreference;

#[derive(Debug, Clone)]
pub(super) enum Command {
    Status,
    Dispatch { hotkey: String },
    Tui,
    Run,
    Init,
}

#[derive(Debug)]
pub(super) struct CliArgs {
    pub(super) command: Command,
    pub(super) config_path: Option<PathBuf>,
    pub(super) perf_log_path: Option<PathBuf>,
    pub(super) log_file_path: Option<PathBuf>,
    pub(super) backend: BackendPreference,
    pub(super) show_tray: bool,
    pub(super) chord: Option<String>,
    pub(super) force: bool,
}

#[derive(Debug)]
pub(super) enum ParseStatus {
    Ok(CliArgs),
    Help,
    Err(String),
}

pub(super) fn parse_args() -> ParseStatus {
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
    let mut log_file_path = None;
    let mut backend = BackendPreference::Auto;
    let mut show_tray = false;
    let mut chord = None;
    let mut force = false;

    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];

        match arg.as_str() {
            "--config" | "--perf-log" | "--log-file" | "--backend" | "--chord" => {
                let Some(value) = args.get(index + 1) else {
                    return ParseStatus::Err(format!("{arg} requires a value"));
                };
                match arg.as_str() {
                    "--config" => config_path = Some(PathBuf::from(value)),
                    "--perf-log" => perf_log_path = Some(PathBuf::from(value)),
                    "--log-file" => log_file_path = Some(PathBuf::from(value)),
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
        log_file_path,
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{CliArgs, Command, ParseStatus, parse_args_from_inputs};

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
}
