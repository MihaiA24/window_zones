use std::process::Command;

#[test]
fn script_moves_identity_restores_state_and_connects_geometry_once() {
    let output = Command::new("node")
        .args(["-e", include_str!("fixtures/moves_identity.js")])
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/kwin-script/contents/code/main.js"
        ))
        .output()
        .expect("node must be installed for KWin script contract tests");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn script_releases_native_keys_readds_and_refreshes_reserved_work_areas() {
    let output = Command::new("node")
        .args(["-e", include_str!("fixtures/native_keys_and_work_areas.js")])
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/kwin-script/contents/code/main.js"
        ))
        .output()
        .expect("node must be installed for KWin script contract tests");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
