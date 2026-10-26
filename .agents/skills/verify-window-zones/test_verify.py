#!/usr/bin/env python3
"""Run with python3; also included by scripts/test.sh. No third-party test runner."""
import contextlib
import importlib.machinery
import importlib.util
import io
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

sys.dont_write_bytecode = True
loader = importlib.machinery.SourceFileLoader("verifier", str(Path(__file__).with_name("verify")))
spec = importlib.util.spec_from_loader(loader.name, loader)
v = importlib.util.module_from_spec(spec)
loader.exec_module(v)


def rejected(action, reason):
    try:
        action()
    except RuntimeError as error:
        assert reason in str(error), error
    else:
        raise AssertionError("Unsafe operation was accepted")


with tempfile.TemporaryDirectory(prefix="window-zones-verifier-check-") as directory:
    v.BASE = Path(directory)
    v.SCRATCH = v.BASE / "scratch"
    v.EVIDENCE = v.BASE / "evidence"
    v.STAMP = v.BASE / "build.json"
    v.BINARY = v.BASE / "window_zones"
    v.SCRATCH.mkdir(mode=0o700)
    proof = v.EVIDENCE / "proof"
    proof.mkdir(parents=True)
    survivor = proof / "proof.txt"
    survivor.write_text("must survive cleanup")
    owner = {**v.OWNER, "evidence": str(proof)}
    v.save(v.SCRATCH / "owner.json", {**owner, "checkout": "/another/checkout"})
    rejected(lambda: v.cleanup(False), "Foreign profile")
    assert v.SCRATCH.is_dir() and survivor.read_text() == "must survive cleanup"
    v.save(v.SCRATCH / "owner.json", owner)
    v.SCRATCH.chmod(0o755)
    rejected(lambda: v.cleanup(False), "Unsafe profile")
    assert survivor.exists() and (v.SCRATCH / "owner.json").exists()
    v.SCRATCH.chmod(0o700)
    private = v.SCRATCH
    v.SCRATCH = v.BASE / "linked-profile"
    v.SCRATCH.symlink_to(private, target_is_directory=True)
    rejected(lambda: v.cleanup(False), "Unsafe profile")
    assert private.is_dir() and survivor.exists()
    v.SCRATCH.unlink()
    v.SCRATCH = private
    with contextlib.redirect_stdout(io.StringIO()):
        v.cleanup(True)
    assert v.SCRATCH.is_dir() and survivor.exists(), "dry-run mutated state"

    # A live PID from another checkout must not be adopted or signalled.
    child = subprocess.Popen(["sleep", "60"], cwd=v.ROOT,
                             env=dict(os.environ, WINDOW_ZONES_VERIFY_CHECKOUT="/another/checkout"))
    try:
        details = v.process(child.pid)
        v.save(v.SCRATCH / "active.json", details)
        rejected(lambda: v.cleanup(False), "foreign instance")
        assert child.poll() is None, "cleanup signalled a foreign process"
    finally:
        child.terminate()
        child.wait()
        (v.SCRATCH / "active.json").unlink()

    fingerprint, _ = v.inputs()
    v.BINARY.write_bytes(b"recorded binary")
    v.save(v.STAMP, {"inputs": fingerprint, "binaries": {v.BINARY.name: v.digest(v.BINARY)}})
    assert v.build_status() == "current"
    v.BINARY.write_bytes(b"binary from a different build")
    assert v.build_status().startswith("stale:"), "replaced binary accepted as proof"
    v.BINARY.write_bytes(b"recorded binary")
    previous = os.environ.get("RUSTFLAGS")
    try:
        os.environ["RUSTFLAGS"] = (previous or "") + " -C opt-level=1"
        assert v.build_status().startswith("stale:"), "changed build flags accepted as proof"
    finally:
        if previous is None:
            os.environ.pop("RUSTFLAGS", None)
        else:
            os.environ["RUSTFLAGS"] = previous
    with contextlib.redirect_stdout(io.StringIO()):
        v.cleanup(False)
    assert not v.SCRATCH.exists() and survivor.read_text() == "must survive cleanup"
    assert v.differences({"geometry": [0, 0, 960, 1080]}, {"geometry": [0, 0, 959, 1080]}) == [
        {"path": "$.geometry[2]", "expected": 960, "actual": 959}
    ]
    assert v.differences({"error": None}, {}) == [
        {"path": "$.error", "expected": None, "actual": {"missing": True}}
    ]
    assert v.differences({"exit": 0}, {"exit": False}) == [
        {"path": "$.exit", "expected": 0, "actual": False}
    ]
    assert v.measurement_summary([5_000_000, 1_000_000, 100_000_000, 3_000_000, 7_000_000]) == {
        "runs": 5, "median_ms": 5.0, "min_ms": 1.0, "max_ms": 100.0, "spread_ms": 99.0
    }
    library = v.BASE / "clock-scope.so"
    subprocess.run(["cc", "-O2", "-Wall", "-Wextra", "-Werror", "-shared", "-fPIC",
                    str(v.FIXED_CLOCK), "-o", str(library)], check=True)
    before = int(time.time())
    external = int(subprocess.check_output(["date", "+%s"], env=dict(os.environ, LD_PRELOAD=str(library)), text=True))
    after = int(time.time())
    assert before <= external <= after, "Application clock leaked into external tools and shell deadlines"
print("PASS verifier: foreign checkout/PID refusal, destructive dry-run, stale builds, evidence retention")
