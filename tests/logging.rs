#![cfg(target_os = "linux")]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
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
    wait_output(child)
}

fn wait_output(mut child: Child) -> Output {
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

mod tray {
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;
    use std::thread::{self, JoinHandle};

    use dbus::blocking::LocalConnection;
    use dbus_tree::{Factory, MethodErr};

    use super::*;

    struct Process(Option<Child>);

    impl Drop for Process {
        fn drop(&mut self) {
            if let Some(child) = self.0.as_mut() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    struct Bus {
        process: Process,
        address: String,
    }

    impl Bus {
        fn new(directory: &Directory) -> Self {
            let mut child = Command::new("dbus-daemon")
                .args(["--session", "--nofork", "--print-address=1"])
                .arg(format!(
                    "--address=unix:path={}",
                    directory.0.join("bus").display()
                ))
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("dbus-daemon is required for Linux tray contract tests");
            let mut address = String::new();
            BufReader::new(child.stdout.take().unwrap())
                .read_line(&mut address)
                .unwrap();
            assert!(!address.trim().is_empty());
            Self {
                process: Process(Some(child)),
                address: address.trim().into(),
            }
        }
    }

    struct Watcher {
        stop: mpsc::Sender<()>,
        worker: Option<JoinHandle<()>>,
    }

    impl Watcher {
        fn new(address: &str, reject: bool) -> Self {
            let address = address.to_owned();
            let (stop, stopped) = mpsc::channel();
            let (ready, started) = mpsc::channel();
            let worker = thread::spawn(move || {
                let connection = LocalConnection::new_address(&address).unwrap();
                connection
                    .request_name("org.kde.StatusNotifierWatcher", false, false, true)
                    .unwrap();
                let factory = Factory::new_fn::<()>();
                let tree = factory.tree(()).add(
                    factory.object_path("/StatusNotifierWatcher", ()).add(
                        factory
                            .interface("org.kde.StatusNotifierWatcher", ())
                            .add_m(factory.method(
                                "RegisterStatusNotifierItem",
                                (),
                                move |method| {
                                    let _: String = method.msg.read1()?;
                                    if reject {
                                        Err(MethodErr::failed("tray registration refused"))
                                    } else {
                                        Ok(vec![method.msg.method_return()])
                                    }
                                },
                            )),
                    ),
                );
                tree.start_receive(&connection);
                ready.send(()).unwrap();
                while stopped.try_recv().is_err()
                    && connection.process(Duration::from_millis(10)).is_ok()
                {}
            });
            started.recv_timeout(Duration::from_secs(3)).unwrap();
            Self {
                stop,
                worker: Some(worker),
            }
        }
    }

    impl Drop for Watcher {
        fn drop(&mut self) {
            let _ = self.stop.send(());
            if let Some(worker) = self.worker.take() {
                worker.join().unwrap();
            }
        }
    }

    fn tray_command(directory: &Directory, address: &str) -> Command {
        let config = directory.0.join("config.toml");
        fs::write(&config, window_zones::starter_config("Ctrl+Alt")).unwrap();
        let mut command = super::command();
        command
            .arg("--config")
            .arg(config)
            .args(["--tray", "run"])
            .env("DBUS_SESSION_BUS_ADDRESS", address)
            .env_remove("DBUS_STARTER_ADDRESS")
            .stderr(Stdio::piped());
        command
    }

    fn assert_startup_failure(result: Output, expected: &str) {
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert_eq!(result.status.code(), Some(1), "{stderr}");
        assert!(stderr.contains(expected), "{stderr}");
        assert!(!stderr.contains("panicked"), "{stderr}");
        assert!(
            !String::from_utf8_lossy(&result.stdout).contains("Tray menu started"),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
    }

    #[test]
    fn unavailable_bus_fails_tray_startup_without_panicking_or_advertising_ready() {
        let directory = Directory::new();
        let address = format!("unix:path={}", directory.0.join("missing-bus").display());
        assert_startup_failure(
            output(tray_command(&directory, &address), None),
            "DBUS_SESSION_BUS_ADDRESS",
        );
    }

    #[test]
    fn tray_requires_a_watcher_and_successful_registration() {
        let directory = Directory::new();
        let bus = Bus::new(&directory);
        assert_startup_failure(
            output(tray_command(&directory, &bus.address), None),
            "StatusNotifierWatcher is unavailable",
        );
        let _watcher = Watcher::new(&bus.address, true);
        assert_startup_failure(
            output(tray_command(&directory, &bus.address), None),
            "tray registration refused",
        );
    }

    #[test]
    fn tray_exits_with_failure_when_its_watcher_or_worker_bus_is_lost() {
        for lose_bus in [false, true] {
            let directory = Directory::new();
            let mut bus = Bus::new(&directory);
            let watcher = Watcher::new(&bus.address, false);
            let log = directory.0.join("tray.log");
            let mut command = tray_command(&directory, &bus.address);
            command
                .stdin(Stdio::piped())
                .stdout(fs::File::create(&log).unwrap());
            let mut app = Process(Some(command.spawn().unwrap()));
            let child = app.0.as_mut().unwrap();
            child.stdin.take().unwrap().write_all(b"quit\n").unwrap();
            let deadline = Instant::now() + Duration::from_secs(3);
            while !fs::read_to_string(&log)
                .unwrap()
                .contains("Tray menu started")
            {
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "tray exited before ready"
                );
                assert!(Instant::now() < deadline, "tray never registered");
                thread::sleep(Duration::from_millis(10));
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "tray must not read stdin"
            );
            if lose_bus {
                let daemon = bus.process.0.as_mut().unwrap();
                daemon.kill().unwrap();
                daemon.wait().unwrap();
            }
            drop(watcher);
            let result = wait_output(app.0.take().unwrap());
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert_eq!(result.status.code(), Some(1), "{stderr}");
            assert!(!stderr.contains("panicked"), "{stderr}");
            let expected = if lose_bus {
                "Linux tray service failed"
            } else {
                "StatusNotifierWatcher is unavailable"
            };
            assert!(stderr.contains(expected), "{stderr}");
        }
    }
}
