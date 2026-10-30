use std::fs;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt, symlink};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use window_zones::Rect;

#[cfg(target_os = "linux")]
use super::MAX_REQUEST;
use super::{
    CLIENT_ID, Identity, MAX_RESPONSE, OwnedSocket, Response, Server, directory, dispatch,
};
use crate::application::backends::RuntimeBackend;
use crate::application::ownership::{InstanceLock, acquire_instance_lock};
use crate::application::session::runner::run_runtime;
use crate::application::session::{Runtime, RuntimeInstruction, RuntimeRequest};
use crate::application::surfaces::CliSurface;

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
        Self::with_config(|path| {
            fs::write(path, window_zones::starter_config("Ctrl+Alt")).unwrap();
        })
    }

    fn with_config(prepare: impl FnOnce(&Path)) -> Self {
        let dir = TestDirectory::new();
        let lock_path = dir.0.join("instance.lock");
        let config = dir.0.join("config.toml");
        let log = dir.0.join("perf.jsonl");
        prepare(&config);
        let lock = acquire_instance_lock(&lock_path).unwrap();
        let identity = Identity::new("sway", &dir.0.join("sway.sock"), &config).unwrap();
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
        self.dispatch_as(self.identity.clone(), hotkey)
    }

    fn dispatch_as(&self, identity: Identity, hotkey: &str) -> Response {
        dispatch(&self.lock_path, identity, hotkey).unwrap()
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
        Identity::new(
            "hyprland",
            &session.identity.compositor_socket,
            &session.config,
        )
        .unwrap(),
        Identity::new(
            "sway",
            &session.identity.compositor_socket,
            &session.config.with_file_name("other.toml"),
        )
        .unwrap(),
        Identity::new(
            "sway",
            &session.config.with_file_name("other-sway.sock"),
            &session.config,
        )
        .unwrap(),
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
fn symlink_retargets_and_restart_keep_the_watched_path_routable() {
    let session = Session::with_config(|path| {
        let first = path.with_file_name("first.toml");
        fs::write(&first, window_zones::starter_config("Ctrl+Alt")).unwrap();
        fs::write(
            path.with_file_name("second.toml"),
            window_zones::starter_config("Ctrl+Super"),
        )
        .unwrap();
        symlink(first, path).unwrap();
    });
    let first = session.config.with_file_name("first.toml");
    let second = session.config.with_file_name("second.toml");
    let identity =
        |path: &Path| Identity::new("sway", &session.identity.compositor_socket, path).unwrap();
    assert!(
        session
            .dispatch_as(identity(&first), "Ctrl+Alt+Left")
            .success
    );

    fs::remove_file(&session.config).unwrap();
    symlink(&second, &session.config).unwrap();
    session
        .instructions
        .send(RuntimeInstruction::Reload.into())
        .unwrap();
    assert!(
        session
            .dispatch_as(identity(&session.config), "Ctrl+Super+Left")
            .success
    );
    assert!(
        session
            .dispatch_as(identity(&second), "Ctrl+Super+Right")
            .success
    );
    let stale_target = session.dispatch_as(identity(&first), "Ctrl+Super+Left");
    assert!(!stale_target.success);
    assert!(stale_target.output.contains("backend/config mismatch"));

    session
        .instructions
        .send(RuntimeInstruction::Restart.into())
        .unwrap();
    assert!(
        session
            .dispatch_as(identity(&session.config), "Ctrl+Super+Left")
            .success
    );
}

#[test]
fn a_missing_config_can_appear_without_changing_dispatch_identity() {
    let session = Session::with_config(|_| {});
    let initial = session.dispatch("Ctrl+Alt+Left");
    assert!(!initial.success);
    assert!(initial.output.contains("no binding configured for hotkey"));
    fs::write(&session.config, window_zones::starter_config("Ctrl+Alt")).unwrap();
    session
        .instructions
        .send(RuntimeInstruction::Reload.into())
        .unwrap();
    let identity =
        Identity::new("sway", &session.identity.compositor_socket, &session.config).unwrap();
    assert!(
        session
            .dispatch_as(identity.clone(), "Ctrl+Alt+Left")
            .success
    );
    session
        .instructions
        .send(RuntimeInstruction::Restart.into())
        .unwrap();
    assert!(session.dispatch_as(identity, "Ctrl+Alt+Right").success);
}

#[test]
fn replacing_the_compositor_socket_refuses_old_and_new_client_identities() {
    let dir = TestDirectory::new();
    let lock_path = dir.0.join("instance.lock");
    let lock = acquire_instance_lock(&lock_path).unwrap();
    let socket_path = dir.0.join("sway.sock");
    let old_socket = UnixDatagram::bind(&socket_path).unwrap();
    let config = dir.0.join("config.toml");
    let old_identity = Identity::new("sway", &socket_path, &config).unwrap();
    let (tx, receiver) = mpsc::channel();
    let _server = Server::start(&lock, old_identity.clone(), tx).unwrap();
    fs::remove_file(&socket_path).unwrap();
    let _replacement = UnixDatagram::bind(&socket_path).unwrap();
    let new_identity = Identity::new("sway", &socket_path, &config).unwrap();
    assert_ne!(old_identity, new_identity);
    for identity in [old_identity, new_identity] {
        let response = dispatch(&lock_path, identity, "Ctrl+Alt+Left").unwrap();
        assert!(!response.success);
        assert!(response.output.contains("compositor instance mismatch"));
    }
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
    drop(old_socket);
}

#[test]
fn retargeting_a_compositor_socket_symlink_refuses_clients_of_the_original_socket() {
    let dir = TestDirectory::new();
    let lock_path = dir.0.join("instance.lock");
    let lock = acquire_instance_lock(&lock_path).unwrap();
    let first = dir.0.join("first.sock");
    let second = dir.0.join("second.sock");
    let _first = UnixDatagram::bind(&first).unwrap();
    let _second = UnixDatagram::bind(&second).unwrap();
    let watched = dir.0.join("current.sock");
    symlink(&first, &watched).unwrap();
    let config = dir.0.join("config.toml");
    let identity = Identity::new("sway", &watched, &config).unwrap();
    let original_client = Identity::new("sway", &first, &config).unwrap();
    assert!(identity.matches(&original_client));
    let (tx, receiver) = mpsc::channel();
    let _server = Server::start(&lock, identity, tx).unwrap();
    fs::remove_file(&watched).unwrap();
    symlink(&second, &watched).unwrap();
    let response = dispatch(&lock_path, original_client, "Ctrl+Alt+Left").unwrap();
    assert!(!response.success);
    assert!(response.output.contains("compositor instance mismatch"));
    assert!(matches!(
        receiver.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));
}

#[test]
fn socket_lifecycle_rejects_symlinks_and_preserves_replacements() {
    let dir = TestDirectory::new();
    let lock_path = dir.0.join("instance.lock");
    let lock = acquire_instance_lock(&lock_path).unwrap();
    let identity =
        Identity::new("sway", &dir.0.join("sway.sock"), &dir.0.join("config.toml")).unwrap();
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
    let lock = acquire_instance_lock(&lock_path).unwrap();
    let identity =
        Identity::new("sway", &dir.0.join("sway.sock"), &dir.0.join("config.toml")).unwrap();
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
    let lock = acquire_instance_lock(&lock_path).unwrap();
    let identity =
        Identity::new("sway", &dir.0.join("sway.sock"), &dir.0.join("config.toml")).unwrap();
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
