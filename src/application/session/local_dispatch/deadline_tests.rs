use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use window_zones::DispatchState;

use super::Reply;
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
