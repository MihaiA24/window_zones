use std::io::{self, BufRead};
use std::sync::mpsc;
use std::thread;

#[cfg(any(target_os = "linux", all(test, unix)))]
use super::local_dispatch;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::application) enum RuntimeInstruction {
    Empty,
    Status,
    Reload,
    Restart,
    Quit,
    InputClosed,
    #[cfg(target_os = "linux")]
    SurfaceFailed(String),
    Help,
    Dispatch(String),
    Unknown(String),
}

pub(super) fn parse_runtime_input(line: &str) -> RuntimeInstruction {
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

pub(super) fn parse_tui_input(line: &str) -> RuntimeInstruction {
    if line.trim().eq_ignore_ascii_case("refresh") {
        RuntimeInstruction::Status
    } else {
        parse_runtime_input(line)
    }
}

/// One instruction queue serves stdin, tray callbacks, and local compositor dispatch.
#[derive(Debug)]
pub(in crate::application) struct RuntimeRequest {
    pub(super) instruction: RuntimeInstruction,
    #[cfg(any(target_os = "linux", all(test, unix)))]
    pub(super) reply: Option<local_dispatch::Reply>,
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
pub(super) fn spawn_stdin_reader(
    parse: fn(&str) -> RuntimeInstruction,
    tx: mpsc::Sender<RuntimeRequest>,
) {
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

#[cfg(test)]
mod tests {
    use super::{RuntimeInstruction, parse_runtime_input, parse_tui_input};

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
}
