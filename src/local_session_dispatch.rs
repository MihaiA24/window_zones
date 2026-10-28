//! Private, bounded local transport for compositor-owned key capture.
//! The worker only forwards requests; the existing runtime owns every action.
use std::fs;
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixDatagram;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::{InstanceLock, RuntimeInstruction, RuntimeRequest, current_uid};

const MAX_REQUEST: usize = 8192;
const MAX_RESPONSE: usize = 65536;
const IO_TIMEOUT: Duration = Duration::from_millis(100);
const ACTION_TIMEOUT: Duration = Duration::from_secs(5);
static CLIENT_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    backend: String,
    config_path: PathBuf,
}

impl Identity {
    pub(super) fn new(backend: &str, config_path: &Path) -> io::Result<Self> {
        Ok(Self {
            backend: backend.to_string(),
            config_path: normalized_path(config_path)?,
        })
    }
}

// Resolve existing symlinks, including parents of a not-yet-created config.
fn normalized_path(path: &Path) -> io::Result<PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    let mut resolved = loop {
        match fs::canonicalize(ancestor) {
            Ok(path) => break path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(ancestor.file_name().ok_or(error)?);
                ancestor = ancestor.parent().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "config has no parent")
                })?;
            }
            Err(error) => return Err(error),
        }
    };
    for part in missing.into_iter().rev() {
        resolved.push(part);
    }
    let mut normalized = PathBuf::new();
    for component in resolved.components() {
        if component == Component::ParentDir {
            normalized.pop();
        } else {
            normalized.push(component);
        }
    }
    Ok(normalized)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    identity: Identity,
    hotkey: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Response {
    pub(super) success: bool,
    pub(super) output: String,
}

impl Response {
    fn error(message: impl std::fmt::Display) -> Self {
        Self {
            success: false,
            output: format!("Error: {message}\n"),
        }
    }
}

#[derive(Debug)]
pub(super) struct Reply {
    pub(super) sender: mpsc::SyncSender<Response>,
    pub(super) deadline: Instant,
}

fn directory(lock_path: &Path) -> PathBuf {
    lock_path.with_extension("dispatch")
}

fn validate_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.uid() != current_uid() || metadata.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} must be an owned, non-symlink directory with mode 0700",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn socket_metadata(path: &Path) -> io::Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_socket() || metadata.uid() != current_uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "refusing non-socket, symlink, or foreign socket {}",
                path.display()
            ),
        ));
    }
    Ok(metadata)
}

struct OwnedSocket {
    socket: UnixDatagram,
    path: PathBuf,
    metadata: fs::Metadata,
}

impl OwnedSocket {
    fn bind(path: PathBuf) -> io::Result<Self> {
        let socket = UnixDatagram::bind(&path)?;
        let metadata = socket_metadata(&path)?;
        let owned = Self {
            socket,
            path,
            metadata,
        };
        fs::set_permissions(&owned.path, fs::Permissions::from_mode(0o600))?;
        owned.socket.set_read_timeout(Some(IO_TIMEOUT))?;
        owned.socket.set_write_timeout(Some(IO_TIMEOUT))?;
        Ok(owned)
    }
}

fn remove_same_socket(path: &Path, original: &fs::Metadata) {
    if let Ok(current) = socket_metadata(path)
        && current.dev() == original.dev()
        && current.ino() == original.ino()
    {
        let _ = fs::remove_file(path);
    }
}

impl Drop for OwnedSocket {
    fn drop(&mut self) {
        remove_same_socket(&self.path, &self.metadata);
    }
}

struct PendingResponse {
    client: PathBuf,
    response: mpsc::Receiver<Response>,
    deadline: Instant,
}

fn send_response(socket: &UnixDatagram, client: &Path, response: Response) {
    let response = serde_json::to_vec(&response).unwrap_or_else(|_| {
        b"{\"success\":false,\"output\":\"Error: cannot encode dispatch response\\n\"}".to_vec()
    });
    if response.len() <= MAX_RESPONSE {
        let _ = socket.send_to(&response, client);
    } else {
        let _ = socket.send_to(
            b"{\"success\":false,\"output\":\"Error: dispatch report exceeds local response limit\\n\"}",
            client,
        );
    }
}

pub(super) struct Server {
    stopping: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Server {
    // A caller must already own the same session lock before cleaning up a stale socket.
    pub(super) fn start(
        lock: &InstanceLock,
        identity: Identity,
        instructions: mpsc::Sender<RuntimeRequest>,
    ) -> io::Result<Self> {
        let dir = directory(&lock.pid_path);
        match fs::DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        validate_directory(&dir)?;
        let path = dir.join("dispatch.sock");
        match socket_metadata(&path) {
            Ok(metadata) => {
                // Datagram connect has no handshake or accept backlog to block on.
                match UnixDatagram::unbound()?.connect(&path) {
                    Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                        remove_same_socket(&path, &metadata);
                    }
                    Ok(()) => {
                        return Err(io::Error::new(
                            io::ErrorKind::AddrInUse,
                            "dispatch socket is still active",
                        ));
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let owned = OwnedSocket::bind(path)?;
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = stopping.clone();
        let worker = thread::Builder::new().name("session-dispatch".to_string()).spawn(move || {
            let mut bytes = [0; MAX_REQUEST + 1];
            let mut pending: Vec<PendingResponse> = Vec::new();
            while !stop.load(Ordering::Acquire) {
                // Receiving a new request must not wait for an earlier action:
                // every queued action gets its deadline before the client times out.
                pending.retain(|request| {
                    let response = match request.response.try_recv() {
                        Ok(response) => response,
                        Err(mpsc::TryRecvError::Disconnected) => Response::error("running session stopped before dispatch"),
                        Err(mpsc::TryRecvError::Empty) if Instant::now() >= request.deadline => {
                            Response::error("dispatch timed out; an action already started may have run; inspect the window before retrying")
                        }
                        Err(mpsc::TryRecvError::Empty) => return true,
                    };
                    send_response(&owned.socket, &request.client, response);
                    false
                });
                let (count, address) = match owned.socket.recv_from(&mut bytes) {
                    Ok(message) => message,
                    Err(error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted) => continue,
                    Err(_) => break,
                };
                let Some(client) = address.as_pathname() else { continue };
                // Never send replies outside the private session directory.
                if client.parent() != Some(dir.as_path()) || socket_metadata(client).is_err() {
                    continue;
                }
                let response = if count > MAX_REQUEST {
                    Response::error("malformed dispatch request: message exceeds 8192 bytes")
                } else {
                    match serde_json::from_slice::<Request>(&bytes[..count]) {
                        Err(error) => Response::error(format!("malformed dispatch request: {error}")),
                        Ok(request) if request.identity != identity => Response::error(format!(
                            "dispatch backend/config mismatch; running session uses {} and {}; use the same --backend and --config, or restart that session",
                            identity.backend, identity.config_path.display()
                        )),
                        Ok(request) => {
                            let (sender, response) = mpsc::sync_channel(1);
                            let deadline = Instant::now() + ACTION_TIMEOUT;
                            let request = RuntimeRequest {
                                instruction: RuntimeInstruction::Dispatch(request.hotkey),
                                reply: Some(Reply { sender, deadline }),
                            };
                            if instructions.send(request).is_err() {
                                Response::error("running session is shutting down; start `window_zones run`")
                            } else {
                                pending.push(PendingResponse {
                                    client: client.to_path_buf(),
                                    response,
                                    deadline,
                                });
                                continue;
                            }
                        }
                    }
                };
                send_response(&owned.socket, client, response);
            }
            for request in pending {
                send_response(&owned.socket, &request.client, Response::error("running session is shutting down"));
            }
        })?;
        Ok(Self {
            stopping,
            worker: Some(worker),
        })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(super) fn dispatch(
    lock_path: &Path,
    identity: Identity,
    hotkey: &str,
) -> Result<Response, String> {
    let dir = directory(lock_path);
    let missing = |error| {
        format!(
            "cannot reach compositor-bound session: {error}; start `window_zones run` (or `tui`) with the same --backend and --config"
        )
    };
    validate_directory(&dir).map_err(missing)?;
    let server = dir.join("dispatch.sock");
    let metadata = socket_metadata(&server).map_err(missing)?;
    if metadata.mode() & 0o777 != 0o600 {
        return Err("refusing dispatch socket without mode 0600".to_string());
    }
    let client = dir.join(format!(
        "c-{}-{}.sock",
        std::process::id(),
        CLIENT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    let owned = OwnedSocket::bind(client).map_err(missing)?;
    owned.socket.connect(server).map_err(missing)?;
    let request = serde_json::to_vec(&Request {
        identity,
        hotkey: hotkey.to_string(),
    })
    .map_err(|error| error.to_string())?;
    if request.len() > MAX_REQUEST {
        return Err("dispatch request exceeds 8192 bytes".to_string());
    }
    owned.socket.send(&request).map_err(missing)?;
    owned
        .socket
        .set_read_timeout(Some(ACTION_TIMEOUT + Duration::from_secs(1)))
        .map_err(missing)?;
    let mut bytes = [0; MAX_RESPONSE + 1];
    let count = owned.socket.recv(&mut bytes).map_err(|error| format!("dispatch response unavailable: {error}; an action may have run; inspect the window before retrying"))?;
    if count > MAX_RESPONSE {
        return Err("dispatch response exceeds local limit".to_string());
    }
    serde_json::from_slice(&bytes[..count])
        .map_err(|error| format!("malformed dispatch response: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CliSurface, Rect, Runtime, RuntimeBackend, run_runtime};
    use std::os::unix::fs::symlink;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            // Short names also fit macOS's smaller Unix socket pathname limit.
            let path = PathBuf::from("/tmp").join(format!(
                "wzd-{}-{}",
                std::process::id(),
                CLIENT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    struct Session {
        instructions: mpsc::Sender<RuntimeRequest>,
        runtime: Option<thread::JoinHandle<Rect>>,
        server: Option<Server>,
        identity: Identity,
        lock_path: PathBuf,
        config: PathBuf,
        log: PathBuf,
        _lock: InstanceLock,
        _directory: TestDirectory,
    }

    impl Session {
        fn new() -> Self {
            let dir = TestDirectory::new();
            let lock_path = dir.0.join("instance.lock");
            let config = dir.0.join("config.toml");
            let log = dir.0.join("perf.jsonl");
            fs::write(&config, window_zones::starter_config("Ctrl+Alt")).unwrap();
            let lock = crate::acquire_instance_lock(&lock_path).unwrap();
            let identity = Identity::new("sway", &config).unwrap();
            let (instructions, receiver) = mpsc::channel();
            let server = Server::start(&lock, identity.clone(), instructions.clone()).unwrap();
            let runtime_config = config.clone();
            let runtime_log = log.clone();
            let runtime = thread::spawn(move || {
                let mut runtime = Runtime::new(
                    Some(runtime_config),
                    RuntimeBackend::DryRun,
                    Some(&runtime_log),
                )
                .unwrap();
                run_runtime(
                    &mut runtime,
                    &mut CliSurface {
                        prompt: false,
                        notifier: None,
                    },
                    receiver,
                    false,
                );
                runtime.window_system.last_move().unwrap().target
            });
            Self {
                instructions,
                runtime: Some(runtime),
                server: Some(server),
                identity,
                lock_path,
                config,
                log,
                _lock: lock,
                _directory: dir,
            }
        }

        fn dispatch(&self, hotkey: &str) -> Response {
            dispatch(&self.lock_path, self.identity.clone(), hotkey).unwrap()
        }

        fn raw(&self, bytes: &[u8]) -> Response {
            let dir = directory(&self.lock_path);
            let client = OwnedSocket::bind(dir.join(format!(
                "raw-{}.sock",
                CLIENT_ID.fetch_add(1, Ordering::Relaxed)
            )))
            .unwrap();
            client
                .socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            client.socket.connect(dir.join("dispatch.sock")).unwrap();
            client.socket.send(bytes).unwrap();
            let mut response = [0; MAX_RESPONSE];
            let count = client.socket.recv(&mut response).unwrap();
            serde_json::from_slice(&response[..count]).unwrap()
        }

        fn stop(&mut self) -> Rect {
            self.instructions
                .send(RuntimeInstruction::Quit.into())
                .unwrap();
            self.server.take();
            self.runtime.take().unwrap().join().unwrap()
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            if let Some(runtime) = self.runtime.take() {
                let _ = self.instructions.send(RuntimeInstruction::Quit.into());
                self.server.take();
                let _ = runtime.join();
            }
        }
    }

    #[test]
    fn separate_clients_share_runtime_history_reload_and_session_logging() {
        let mut session = Session::new();
        session
            .instructions
            .send(RuntimeInstruction::InputClosed.into())
            .unwrap();
        let failed = session.dispatch("Ctrl+Alt+Backspace");
        assert!(!failed.success);
        assert!(
            failed.output.contains("nothing to restore"),
            "{}",
            failed.output
        );
        for (hotkey, geometry) in [
            ("Ctrl+Alt+Left", "last move: x=0 y=0 w=960 h=1080"),
            ("Ctrl+Alt+Right", "last move: x=960 y=0 w=960 h=1080"),
            ("Ctrl+Alt+Backspace", "last move: x=40 y=40 w=640 h=480"),
        ] {
            let response = session.dispatch(hotkey);
            assert!(response.success, "{}", response.output);
            assert!(
                response
                    .output
                    .contains("Using runtime window backend: dry-run")
            );
            assert!(response.output.contains("Runtime status:"));
            assert!(response.output.contains("Dispatch state: Succeeded"));
            assert!(response.output.contains(geometry), "{}", response.output);
        }
        let failed = session.dispatch("Ctrl+Alt+F9");
        assert!(!failed.success);
        assert!(failed.output.contains("no binding configured for hotkey"));

        fs::write(&session.config, window_zones::starter_config("Ctrl+Super")).unwrap();
        session
            .instructions
            .send(RuntimeInstruction::Reload.into())
            .unwrap();
        assert!(session.dispatch("Ctrl+Super+Left").success);
        let restored = session.dispatch("Ctrl+Super+Backspace");
        assert!(restored.success);
        assert!(restored.output.contains("last move: x=40 y=40 w=640 h=480"));
        assert!(
            restored.output.contains("hotkey state: Registered"),
            "{}",
            restored.output
        );
        assert_eq!(session.stop(), Rect::new(40, 40, 640, 480));
        assert!(!directory(&session.lock_path).join("dispatch.sock").exists());
        let rows: Vec<serde_json::Value> = fs::read_to_string(&session.log)
            .unwrap()
            .lines()
            .map(|row| serde_json::from_str(row).unwrap())
            .collect();
        assert_eq!(rows.len(), 8);
        assert_eq!(rows[0]["event"], "session");
        assert_eq!(rows[1]["result"], "error");
        assert!(rows.iter().all(|row| row["pid"] == std::process::id()));
        let missing = dispatch(
            &session.lock_path,
            session.identity.clone(),
            "Ctrl+Super+Left",
        )
        .unwrap_err();
        assert!(missing.contains("start `window_zones run`"), "{missing}");
    }

    #[test]
    fn mismatch_and_malformed_requests_fail_without_dispatching() {
        let session = Session::new();
        for identity in [
            Identity::new("hyprland", &session.config).unwrap(),
            Identity::new("sway", &session.config.with_file_name("other.toml")).unwrap(),
        ] {
            let response = dispatch(&session.lock_path, identity, "Ctrl+Alt+Left").unwrap();
            assert!(!response.success);
            assert!(
                response.output.contains("backend/config mismatch"),
                "{}",
                response.output
            );
        }
        for bytes in [&b"not-json"[..], &b"{\"hotkey\":17}"[..]] {
            let response = session.raw(bytes);
            assert!(!response.success);
            assert!(response.output.contains("malformed dispatch request"));
        }
        #[cfg(target_os = "linux")]
        {
            let response = session.raw(&vec![b'x'; MAX_REQUEST + 1]);
            assert!(!response.success);
            assert!(response.output.contains("exceeds 8192 bytes"));
        }
        let untouched = session.dispatch("Ctrl+Alt+Backspace");
        assert!(!untouched.success);
        assert!(untouched.output.contains("nothing to restore"));
        assert!(session.dispatch("Ctrl+Alt+Left").success);
    }

    #[test]
    fn socket_lifecycle_rejects_symlinks_and_preserves_replacements() {
        let dir = TestDirectory::new();
        let lock_path = dir.0.join("instance.lock");
        let lock = crate::acquire_instance_lock(&lock_path).unwrap();
        let identity = Identity::new("sway", &dir.0.join("config.toml")).unwrap();
        let endpoint = directory(&lock_path);
        let witness = dir.0.join("witness");
        fs::create_dir(&witness).unwrap();
        symlink(&witness, &endpoint).unwrap();
        let (tx, _receiver) = mpsc::channel();
        assert!(Server::start(&lock, identity.clone(), tx.clone()).is_err());
        assert!(
            fs::symlink_metadata(&endpoint)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_file(&endpoint).unwrap();
        fs::DirBuilder::new().mode(0o700).create(&endpoint).unwrap();
        fs::set_permissions(&endpoint, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(Server::start(&lock, identity.clone(), tx.clone()).is_err());
        fs::set_permissions(&endpoint, fs::Permissions::from_mode(0o700)).unwrap();
        let path = endpoint.join("dispatch.sock");
        fs::write(&path, "keep").unwrap();
        assert!(Server::start(&lock, identity.clone(), tx.clone()).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "keep");
        fs::remove_file(&path).unwrap();
        symlink(witness.join("absent"), &path).unwrap();
        assert!(Server::start(&lock, identity.clone(), tx.clone()).is_err());
        assert!(
            fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_file(&path).unwrap();

        let stale = UnixDatagram::bind(&path).unwrap();
        drop(stale);
        let server = Server::start(&lock, identity.clone(), tx.clone()).unwrap();
        assert_eq!(fs::metadata(&endpoint).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(
            dispatch(&lock_path, identity.clone(), "Ctrl+Alt+Left")
                .unwrap_err()
                .contains("without mode 0600")
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(Server::start(&lock, identity, tx).is_err());
        assert!(path.exists());
        fs::remove_file(&path).unwrap();
        symlink(&witness, &path).unwrap();
        drop(server);
        assert!(
            fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(witness.is_dir());
    }

    #[test]
    fn a_pending_action_does_not_delay_receiving_the_next_request() {
        let dir = TestDirectory::new();
        let lock_path = dir.0.join("instance.lock");
        let lock = crate::acquire_instance_lock(&lock_path).unwrap();
        let identity = Identity::new("sway", &dir.0.join("config.toml")).unwrap();
        let (tx, receiver) = mpsc::channel();
        let server = Server::start(&lock, identity.clone(), tx).unwrap();
        let first_path = lock_path.clone();
        let first_identity = identity.clone();
        let first = thread::spawn(move || dispatch(&first_path, first_identity, "Ctrl+Alt+Left"));
        let first_request = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        let second = thread::spawn(move || dispatch(&lock_path, identity, "Ctrl+Alt+Right"));
        let second_request = receiver.recv_timeout(Duration::from_secs(2));
        drop(server);
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();

        assert!(
            second_request.is_ok(),
            "a pending action blocked the next request"
        );
        assert!(matches!(
            first_request.instruction,
            RuntimeInstruction::Dispatch(_)
        ));
    }

    #[test]
    fn shutdown_rejects_a_pending_request_and_removes_only_the_owned_socket() {
        let dir = TestDirectory::new();
        let lock_path = dir.0.join("instance.lock");
        let lock = crate::acquire_instance_lock(&lock_path).unwrap();
        let identity = Identity::new("sway", &dir.0.join("config.toml")).unwrap();
        let (tx, receiver) = mpsc::channel();
        let server = Server::start(&lock, identity.clone(), tx).unwrap();
        let client_lock = lock_path.clone();
        let client = thread::spawn(move || dispatch(&client_lock, identity, "Ctrl+Alt+Left"));
        let pending = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(
            pending.instruction,
            RuntimeInstruction::Dispatch(_)
        ));
        drop(server);
        let response = client.join().unwrap().unwrap();
        assert!(!response.success);
        assert!(
            response.output.contains("shutting down"),
            "{}",
            response.output
        );
        assert!(!directory(&lock_path).join("dispatch.sock").exists());
    }
}

#[cfg(all(test, unix))]
mod deadline_tests {
    use super::*;

    #[test]
    fn expired_queued_dispatch_does_not_run_an_action() {
        let mut runtime = crate::Runtime::new(
            Some(PathBuf::from("/nonexistent/window-zones-expired.toml")),
            crate::RuntimeBackend::DryRun,
            None,
        )
        .unwrap();
        let (instructions, receiver) = mpsc::channel();
        let (sender, response) = mpsc::sync_channel(1);
        instructions
            .send(RuntimeRequest {
                instruction: RuntimeInstruction::Dispatch("Ctrl+Alt+Left".to_string()),
                reply: Some(Reply {
                    sender,
                    deadline: Instant::now() - Duration::from_secs(1),
                }),
            })
            .unwrap();
        instructions.send(RuntimeInstruction::Quit.into()).unwrap();
        crate::run_runtime(
            &mut runtime,
            &mut crate::CliSurface {
                prompt: false,
                notifier: None,
            },
            receiver,
            true,
        );
        let response = response.recv().unwrap();
        assert!(!response.success);
        assert!(response.output.contains("no action was run"));
        assert!(runtime.window_system.last_move().is_none());
        assert!(matches!(
            runtime.app.dispatch_state(),
            crate::DispatchState::Idle
        ));
    }
}
