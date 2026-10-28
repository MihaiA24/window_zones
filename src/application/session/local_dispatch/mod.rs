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

use super::{RuntimeInstruction, RuntimeRequest};
use crate::application::ownership::{InstanceLock, current_uid};

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

#[cfg(all(test, unix))]
mod deadline_tests;
#[cfg(test)]
mod session_tests;
