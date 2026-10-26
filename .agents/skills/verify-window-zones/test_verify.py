#!/usr/bin/env python3
"""Run with python3; also included by scripts/test.sh. No third-party test runner."""
import contextlib
import io
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

sys.dont_write_bytecode = True

from wz_verify.checks import differences, measurement_summary
from wz_verify.journeys import Terminal
from wz_verify.runtime import Workspace, digest, process, save


def rejected(action, reason):
    try:
        action()
    except RuntimeError as error:
        assert reason in str(error), error
    else:
        raise AssertionError("Unsafe operation was accepted")


with tempfile.TemporaryDirectory(prefix="window-zones-verifier-check-") as directory:
    base = Path(directory)
    workspace = Workspace(Path(__file__).resolve().parents[3], base=base,
                          scratch=base / "scratch", binary=base / "window_zones")
    workspace.scratch.mkdir(mode=0o700)
    proof = workspace.evidence / "proof"
    proof.mkdir(parents=True)
    survivor = proof / "proof.txt"
    survivor.write_text("must survive cleanup")
    owner = {**workspace.owner, "evidence": str(proof)}
    save(workspace.scratch / "owner.json", {**owner, "checkout": "/another/checkout"})
    rejected(lambda: workspace.cleanup(False), "Foreign profile")
    assert workspace.scratch.is_dir() and survivor.read_text() == "must survive cleanup"
    save(workspace.scratch / "owner.json", owner)
    workspace.scratch.chmod(0o755)
    rejected(lambda: workspace.cleanup(False), "Unsafe profile")
    assert survivor.exists() and (workspace.scratch / "owner.json").exists()
    workspace.scratch.chmod(0o700)
    linked = Workspace(workspace.root, base=base, scratch=base / "linked-profile")
    linked.scratch.symlink_to(workspace.scratch, target_is_directory=True)
    rejected(lambda: linked.cleanup(False), "Unsafe profile")
    assert workspace.scratch.is_dir() and survivor.exists()
    linked.scratch.unlink()
    with contextlib.redirect_stdout(io.StringIO()):
        workspace.cleanup(True)
    assert workspace.scratch.is_dir() and survivor.exists(), "dry-run mutated state"

    # A live PID from another checkout must not be adopted or signalled.
    child = subprocess.Popen(["sleep", "60"], cwd=workspace.root,
                             env=dict(os.environ, WINDOW_ZONES_VERIFY_CHECKOUT="/another/checkout"))
    try:
        details = process(child.pid)
        save(workspace.scratch / "active.json", details)
        rejected(lambda: workspace.cleanup(False), "foreign instance")
        assert child.poll() is None, "cleanup signalled a foreign process"
    finally:
        child.terminate()
        child.wait()
        (workspace.scratch / "active.json").unlink()

    fingerprint, _ = workspace.inputs()
    workspace.binary.write_bytes(b"recorded binary")
    save(workspace.stamp, {"inputs": fingerprint, "binaries": {workspace.binary.name: digest(workspace.binary)}})
    assert workspace.build_status() == "current"
    workspace.binary.write_bytes(b"binary from a different build")
    assert workspace.build_status().startswith("stale:"), "replaced binary accepted as proof"
    workspace.binary.write_bytes(b"recorded binary")
    previous = os.environ.get("RUSTFLAGS")
    try:
        os.environ["RUSTFLAGS"] = (previous or "") + " -C opt-level=1"
        assert workspace.build_status().startswith("stale:"), "changed build flags accepted as proof"
    finally:
        if previous is None:
            os.environ.pop("RUSTFLAGS", None)
        else:
            os.environ["RUSTFLAGS"] = previous

    # Every extracted module, including added/removed modules, invalidates old proof.
    checkout = Workspace(base / "source-checkout", scratch=base / "source-scratch")
    sources = [workspace.root / "Cargo.toml", workspace.root / "Cargo.lock",
               workspace.skill / "verify", workspace.expected, workspace.fixed_clock,
               *sorted((workspace.skill / "wz_verify").rglob("*.py"))]
    for source in sources:
        destination = checkout.root / source.relative_to(workspace.root)
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, destination)
    checkout.binary.parent.mkdir(parents=True)
    checkout.binary.write_bytes(b"recorded binary")
    fingerprint, _ = checkout.inputs()
    save(checkout.stamp, {"inputs": fingerprint, "binaries": {checkout.binary.name: digest(checkout.binary)}})
    assert checkout.build_status() == "current"
    for source in (checkout.skill / "verify", *sorted((checkout.skill / "wz_verify").rglob("*.py"))):
        original = source.read_bytes()
        try:
            source.write_bytes(original + b"\n# changed verification implementation\n")
            assert checkout.build_status().startswith("stale:"), f"changed {source.name} accepted as proof"
            if source.suffix == ".py":
                source.unlink()
                assert checkout.build_status().startswith("stale:"), f"removed {source.name} accepted as proof"
        finally:
            source.write_bytes(original)
    added = checkout.skill / "wz_verify" / "new_journey.py"
    try:
        added.write_text("# newly added verification implementation\n")
        assert checkout.build_status().startswith("stale:"), "new module accepted with old proof"
    finally:
        added.unlink()
    assert checkout.build_status() == "current"

    # Cleaning one workspace must not remove another workspace's profile or proof.
    other = Workspace(workspace.root, base=base / "other", scratch=base / "other-scratch")
    other.scratch.mkdir(mode=0o700)
    other_proof = other.evidence / "proof"
    other_proof.mkdir(parents=True)
    other_survivor = other_proof / "proof.txt"
    other_survivor.write_text("independent workspace")
    save(other.scratch / "owner.json", {**other.owner, "evidence": str(other_proof)})
    with contextlib.redirect_stdout(io.StringIO()):
        workspace.cleanup(False)
    assert not workspace.scratch.exists() and survivor.read_text() == "must survive cleanup"
    assert other.state()["evidence"] == str(other_proof) and other_survivor.read_text() == "independent workspace"
    with contextlib.redirect_stdout(io.StringIO()):
        other.cleanup(False)
    assert other_survivor.read_text() == "independent workspace"
    assert differences({"geometry": [0, 0, 960, 1080]}, {"geometry": [0, 0, 959, 1080]}) == [
        {"path": "$.geometry[2]", "expected": 960, "actual": 959}
    ]
    assert differences({"error": None}, {}) == [
        {"path": "$.error", "expected": None, "actual": {"missing": True}}
    ]
    assert differences({"exit": 0}, {"exit": False}) == [
        {"path": "$.exit", "expected": 0, "actual": False}
    ]
    assert measurement_summary([5_000_000, 1_000_000, 100_000_000, 3_000_000, 7_000_000]) == {
        "runs": 5, "median_ms": 5.0, "min_ms": 1.0, "max_ms": 100.0, "spread_ms": 99.0
    }
    library = base / "clock-scope.so"
    subprocess.run(["cc", "-O2", "-Wall", "-Wextra", "-Werror", "-shared", "-fPIC",
                    str(workspace.fixed_clock), "-o", str(library)], check=True)
    before = int(time.time())
    external = int(subprocess.check_output(["date", "+%s"], env=dict(os.environ, LD_PRELOAD=str(library)), text=True))
    after = int(time.time())
    assert before <= external <= after, "Application clock leaked into external tools and shell deadlines"

    # An input echoed by the PTY must never count as application output.
    workspace.scratch.mkdir(mode=0o700)
    save(workspace.scratch / "owner.json", owner)
    prompt = "window-zones> "
    responder = (
        "import sys\n"
        f"print({prompt!r}, end='', flush=True)\n"
        "for line in sys.stdin:\n"
        "    print('observed: ' + line.strip().lower(), flush=True)\n"
        f"    print({prompt!r}, end='', flush=True)\n"
    )
    terminal = Terminal(workspace, [sys.executable, "-u", "-c", responder], "terminal-response")
    try:
        terminal.wait(prompt)
        rejected(lambda: terminal.send("INPUT_ONLY", "INPUT_ONLY"), "Timed out")
        response = terminal.send("OUTPUT", "observed: output")
        assert response.endswith(prompt), "Assertion returned before the completed response"
        assert "OUTPUT" not in response, "Terminal input leaked into output evidence"
    finally:
        terminal.close()
        with contextlib.redirect_stdout(io.StringIO()):
            workspace.cleanup(False)
print("PASS verifier: ownership, dry-run, freshness, evidence retention, completed PTY output without input echo")
