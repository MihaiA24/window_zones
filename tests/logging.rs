#![cfg(target_os = "linux")]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

fn command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_window_zones"));
    command
        .arg("--dry-run")
        .env_remove("HOME")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_STATE_HOME")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .stdout(Stdio::piped())
        .stderr(OpenOptions::new().write(true).open("/dev/full").unwrap());
    command
}

fn output(mut command: Command, input: Option<&str>) -> Output {
    command.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let mut child = command.spawn().unwrap();
    if let Some(input) = input {
        let written = child.stdin.take().unwrap().write_all(input.as_bytes());
        if let Err(error) = written {
            let _ = child.kill();
            let _ = child.wait();
            panic!("cannot send runtime commands: {error}");
        }
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let result = child.wait_with_output().unwrap();
            panic!(
                "command timed out: {}",
                String::from_utf8_lossy(&result.stdout)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "window-zones-logging-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn invalid_arguments_keep_the_error_exit_code_when_stderr_is_unwritable() {
    let mut command = command();
    command.args(["--unknown-option", "status"]);
    let result = output(command, None);
    assert_eq!(result.status.code(), Some(1));
}

#[test]
fn log_open_failure_does_not_abort_status_when_stderr_is_unwritable() {
    let mut command = command();
    command.args(["--log-file", "/dev/null/window_zones.log", "status"]);
    let result = output(command, None);
    assert!(result.status.success(), "{}", result.status);
    assert!(
        String::from_utf8(result.stdout)
            .unwrap()
            .contains("Runtime status:")
    );
}

#[test]
fn log_write_failure_does_not_abort_status_when_stderr_is_unwritable() {
    let mut command = command();
    command.args(["--log-file", "/dev/full", "status"]);
    let result = output(command, None);
    assert!(result.status.success(), "{}", result.status);
    assert!(
        String::from_utf8(result.stdout)
            .unwrap()
            .contains("Runtime status:")
    );
}

#[test]
fn runtime_accepts_explicit_quit_when_stderr_is_unwritable() {
    let mut command = command();
    command.arg("run");
    let result = output(command, Some("quit\n"));
    assert!(result.status.success(), "{}", result.status);
    assert!(
        String::from_utf8(result.stdout)
            .unwrap()
            .contains("Session closed.")
    );
}

#[test]
fn performance_log_open_failure_remains_fatal_when_stderr_is_unwritable() {
    let mut command = command();
    command.args([
        "--perf-log",
        "/dev/null/window_zones.jsonl",
        "dispatch",
        "Ctrl+Alt+Left",
    ]);
    let result = output(command, None);
    assert_eq!(result.status.code(), Some(1));
    assert!(
        !String::from_utf8(result.stdout)
            .unwrap()
            .contains("Dispatch state:")
    );
}

#[test]
fn session_log_appends_outcomes_and_survives_runtime_restart() {
    let directory = Directory::new();
    let config = directory.0.join("config.toml");
    let log = directory.0.join("session.log");
    fs::write(&config, window_zones::starter_config("Ctrl+Alt")).unwrap();
    fs::write(&log, "earlier session\n").unwrap();

    let mut status = command();
    status
        .arg("--config")
        .arg(&config)
        .arg("--log-file")
        .arg(&log)
        .arg("status");
    let result = output(status, None);
    assert!(result.status.success(), "{}", result.status);
    let before = fs::read_to_string(&log).unwrap();
    assert!(before.starts_with("earlier session\n"));
    assert!(before.contains("session start: status"));
    assert!(before.contains("session end: status"));

    let mut runtime = command();
    runtime
        .arg("--config")
        .arg(&config)
        .arg("--log-file")
        .arg(&log)
        .arg("run");
    let result = output(
        runtime,
        Some(
            "dispatch Ctrl+Alt+Left\ndispatch Ctrl+Alt+F12\nrestart\ndispatch Ctrl+Alt+Right\nquit\n",
        ),
    );
    assert!(result.status.success(), "{}", result.status);
    let text = fs::read_to_string(&log).unwrap();
    assert!(
        text.starts_with(&before),
        "the next process truncated its predecessor's log"
    );
    let events: Vec<_> = text[before.len()..]
        .lines()
        .map(|line| line.split_once(' ').unwrap().1)
        .collect();
    assert_eq!(events.first(), Some(&"session start: run"));
    assert_eq!(events.last(), Some(&"session end: run"));
    for expected in [
        "dispatch alt+ctrl+left succeeded",
        "dispatch alt+ctrl+f12 failed: no binding configured for hotkey: Ctrl+Alt+F12",
        "runtime restarted",
        "dispatch alt+ctrl+right succeeded",
    ] {
        assert!(
            events.contains(&expected),
            "missing {expected:?} in {events:?}"
        );
    }
    assert_eq!(
        events
            .iter()
            .filter(|event| **event == "session start: run")
            .count(),
        1
    );
}

#[test]
fn seated_receiver_proof_rejects_cached_success_and_wrong_actions() {
    let directory = Directory::new();
    let mut shell = Command::new("timeout");
    shell
        .args(["8s", "bash", "-euo", "pipefail", "-c"])
        .arg(
            r#"
source "$PROJECT_ROOT/scripts/smoke/assertions.sh"
source "$PROJECT_ROOT/scripts/smoke/waits.sh"
source "$PROJECT_ROOT/scripts/smoke/processes.sh"
source "$PROJECT_ROOT/scripts/smoke/gnome-live/placement.sh"
SCRIPT_DIR="$PROJECT_ROOT/scripts"
PID_FILE="$TMP_DIR/pids"
CURRENT_GATE=receiver-proof
ASSERT_LOG="$TMP_DIR/assertions.log"
ASSERTIONS=0 FAILURES=0 SKIPPED=0 KEEP_LOGS=1
GNOME_LIVE_ACTIVE=0 GNOME_LIVE_RESTORE_FAILURE=0
export WINDOW_ZONES_SMOKE_OWNER="$TMP_DIR"
trap cleanup EXIT
trap 'exit 143' TERM
: >"$PID_FILE"
: >"$ASSERT_LOG"
CONFIG_PATH="$TMP_DIR/config.toml"
printf '[[bindings]]\nhotkey = "ctrl+cmd+left"\naction = { type = "move-to-zone", zone = "left-half" }\n' >"$CONFIG_PATH"
GNOME_SAFE_LEFT_HOTKEY=ctrl+cmd+left
GNOME_LIVE_EXPECTED_ACTION='{"type":"move-to-zone","zone":"left-half"}'
GNOME_LIVE_PERF_LOG="$TMP_DIR/dispatch.jsonl"
run_log="$TMP_DIR/run.log"
start_fifo_app run dry-run "$run_log" --perf-log "$GNOME_LIVE_PERF_LOG"
send_app 'dispatch ctrl+cmd+left'
wait_for_text "$run_log" 'Dispatch state: Succeeded' 2
GNOME_LIVE_DISPATCH_OFFSET=$(stat -c '%s' "$GNOME_LIVE_PERF_LOG")
if gnome_live_placement_result "$run_log" 'cached success'; then exit 41; fi
[[ "$FAILURES" -eq 1 ]]
GNOME_LIVE_EXPECTED_ACTION='{"type":"move-to-zone","zone":"bottom-half"}'
GNOME_LIVE_DISPATCH_OFFSET=$(stat -c '%s' "$GNOME_LIVE_PERF_LOG")
send_app 'dispatch ctrl+cmd+left'
if gnome_live_placement_result "$run_log" 'wrong action'; then exit 42; fi
[[ "$FAILURES" -eq 2 ]]
GNOME_LIVE_EXPECTED_ACTION='{"type":"move-to-zone","zone":"left-half"}'
GNOME_LIVE_DISPATCH_OFFSET=$(stat -c '%s' "$GNOME_LIVE_PERF_LOG")
send_app 'dispatch ctrl+cmd+left'
gnome_live_placement_result "$run_log" 'fresh matching action'
stop_app_cleanly 'receiver proof fixture exits' "$run_log"
[[ "$FAILURES" -eq 2 ]]
"#,
        )
        .env("PROJECT_ROOT", env!("CARGO_MANIFEST_DIR"))
        .env("TMP_DIR", &directory.0)
        .env("BIN_PATH", env!("CARGO_BIN_EXE_window_zones"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let result = output(shell, None);
    assert!(
        result.status.success(),
        "{}\n{}\n{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
