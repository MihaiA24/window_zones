use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use window_zones::DispatchState;

use super::{Identity, Reply};
use crate::application::backends::RuntimeBackend;
use crate::application::session::runner::run_runtime;
use crate::application::session::{Runtime, RuntimeInstruction, RuntimeRequest};
use crate::application::surfaces::CliSurface;

#[test]
fn expired_queued_dispatch_does_not_run_an_action() {
    let mut runtime = Runtime::new(
        Some(PathBuf::from("/nonexistent/window-zones-expired.toml")),
        RuntimeBackend::DryRun,
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
                identity: Identity::new(
                    "sway",
                    &PathBuf::from("/nonexistent/window-zones-expired.sock"),
                    &PathBuf::from("/nonexistent/window-zones-expired.toml"),
                )
                .unwrap()
                .into(),
            }),
        })
        .unwrap();
    instructions.send(RuntimeInstruction::Quit.into()).unwrap();
    run_runtime(
        &mut runtime,
        &mut CliSurface {
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
    assert!(matches!(runtime.app.dispatch_state(), DispatchState::Idle));
}

#[test]
fn a_compositor_replaced_after_queueing_cannot_receive_the_action() {
    use std::fs;
    use std::os::unix::net::UnixDatagram;

    use crate::application::test_support::temp_path;

    let config_path = temp_path("queued_compositor_config.toml");
    let socket_path = temp_path("queued_compositor.sock");
    fs::write(&config_path, window_zones::starter_config("Ctrl+Alt")).unwrap();
    let original = UnixDatagram::bind(&socket_path).unwrap();
    let identity = Identity::new("sway", &socket_path, &config_path).unwrap();
    let mut runtime =
        Runtime::new(Some(config_path.clone()), RuntimeBackend::DryRun, None).unwrap();
    let (instructions, receiver) = mpsc::channel();
    let (sender, response) = mpsc::sync_channel(1);
    instructions
        .send(RuntimeRequest {
            instruction: RuntimeInstruction::Dispatch("Ctrl+Alt+Left".to_string()),
            reply: Some(Reply {
                sender,
                deadline: Instant::now() + Duration::from_secs(30),
                identity: identity.into(),
            }),
        })
        .unwrap();
    fs::remove_file(&socket_path).unwrap();
    let replacement = UnixDatagram::bind(&socket_path).unwrap();
    instructions.send(RuntimeInstruction::Quit.into()).unwrap();
    run_runtime(
        &mut runtime,
        &mut CliSurface {
            prompt: false,
            notifier: None,
        },
        receiver,
        true,
    );
    let response = response.recv().unwrap();
    assert!(!response.success);
    assert!(
        response
            .output
            .contains("compositor instance changed before execution")
    );
    assert!(runtime.window_system.last_move().is_none());
    assert!(matches!(runtime.app.dispatch_state(), DispatchState::Idle));
    drop((original, replacement));
    fs::remove_file(socket_path).unwrap();
    fs::remove_file(config_path).unwrap();
}
