#!/usr/bin/env python3
"""Run with python3; also included by scripts/test.sh. No third-party test runner."""
import argparse
import contextlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
from unittest.mock import patch

sys.dont_write_bytecode = True

from wz_verify.checks import differences, measurement_summary
from wz_verify.journeys import Terminal, session, smoke_summary
from wz_verify.runtime import Workspace, digest, process, save


def rejected(action, reason):
    try:
        action()
    except RuntimeError as error:
        assert reason in str(error), error
    else:
        raise AssertionError("Unsafe operation was accepted")


assert smoke_summary("Smoke assertions (x11): 7; passed: 6; failed: 0; skipped: 1", "x11") == {
    "assertions": 7, "passed": 6, "failed": 0, "skipped": 1,
}
assert smoke_summary("unrelated failures: 0\nSmoke assertions (gnome): 2; passed: 1; failed: 1; skipped: 0",
                     "gnome")["failed"] == 1
for text in (
    "",
    "Smoke assertions (gnome): 1; passed: 1; failed: 0; skipped: 0",
    "Smoke assertions (x11): 1; passed: 1; failed: 0; skipped: 0\n" * 2,
):
    rejected(lambda: smoke_summary(text, "x11"), "Missing or ambiguous")
rejected(lambda: smoke_summary("Smoke assertions (x11): 2; passed: 1; failed: 0; skipped: 0", "x11"),
         "Inconsistent")


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

    # Popen can return after a short command has already exited and been reaped.
    real_popen = subprocess.Popen
    spawned = []

    def recording_spawn(*args, **kwargs):
        child = real_popen(*args, **kwargs)
        spawned.append(child)
        return child

    def completed_spawn(*args, **kwargs):
        child = recording_spawn(*args, **kwargs)
        child.wait(timeout=10)
        return child

    for timed in (False, True):
        for exit_code in (0, 7):
            timings = [] if timed else None
            command = [sys.executable, "-c",
                       f"import sys; print('completed stdout'); print('completed stderr', file=sys.stderr); sys.exit({exit_code})"]
            with patch("wz_verify.runtime.subprocess.Popen", side_effect=completed_spawn):
                with contextlib.redirect_stdout(io.StringIO()):
                    code, text = workspace.run(command, f"completed-{timed}-{exit_code}", timings=timings)
            assert code == exit_code and text == "completed stdout\ncompleted stderr\n"
            if timed:
                assert len(timings) == 1 and timings[0] > 0, "Completed command lost its timing sample"
            assert not (workspace.scratch / "active.json").exists()

    # Volatile metadata loss must not invalidate an unreaped direct child's identity.
    child = recording_spawn(["sleep", "60"], cwd=workspace.root, env=workspace.env(), start_new_session=True)
    try:
        with patch("wz_verify.runtime.process", return_value=None):
            workspace.track(child)
        assert workspace.active()["pid"] == child.pid, "Direct child lost its recorded start identity"
    finally:
        workspace.stop(child)

    # Losing stable ownership identity for a live command must still fail and reap it.
    try:
        with patch("wz_verify.runtime.subprocess.Popen", side_effect=recording_spawn):
            with patch("wz_verify.runtime.process_identity", return_value=None):
                rejected(lambda: workspace.run(["sleep", "60"], "live-tracking-failure"),
                         "Cannot record ownership of live process")
        assert spawned[-1].returncode is not None, "Tracking failure left a live direct child"
        assert process(spawned[-1].pid) is None, "Tracking failure did not reap the direct child"
    finally:
        for child in spawned:
            if child.poll() is None:
                child.kill()
                child.wait()

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
    for relative in ("wz_verify/new_journey.py", "src/application/session/probe.rs",
                     "scripts/smoke/native/probe.sh", "scripts/smoke/fixtures/probe.js"):
        added = checkout.skill / relative if relative.startswith("wz_verify/") else checkout.root / relative
        added.parent.mkdir(parents=True, exist_ok=True)
        try:
            added.write_bytes(b"initial extracted input\n")
            assert checkout.build_status().startswith("stale:"), f"new {relative} accepted with old proof"
            recorded, _ = checkout.inputs()
            save(checkout.stamp, {"inputs": recorded, "binaries": {checkout.binary.name: digest(checkout.binary)}})
            assert checkout.build_status() == "current"
            added.write_bytes(b"changed extracted input\n")
            assert checkout.build_status().startswith("stale:"), f"changed {relative} accepted with old proof"
            added.unlink()
            assert checkout.build_status().startswith("stale:"), f"removed {relative} accepted with old proof"
        finally:
            added.unlink(missing_ok=True)
            save(checkout.stamp, {"inputs": fingerprint, "binaries": {checkout.binary.name: digest(checkout.binary)}})
        assert checkout.build_status() == "current"

    # Cleaning one workspace must not remove another workspace's profile or proof.
    other = Workspace(workspace.root, base=base / "other", scratch=base / "other-scratch")
    other.scratch.mkdir(mode=0o700)
    other_proof = other.evidence / "proof"
    other_proof.mkdir(parents=True)
    other_survivor = other_proof / "proof.txt"
    other_survivor.write_text("independent workspace")
    save(other.scratch / "owner.json", {**other.owner, "evidence": str(other_proof)})
    # Native helpers may outlive their driver and use a nested private runtime.
    native_runtime = workspace.scratch / "gnome-123" / "gnome" / "runtime"
    sibling_runtime = base / "scratch-sibling" / "gnome-123" / "gnome" / "runtime"
    for runtime in (native_runtime, sibling_runtime):
        runtime.mkdir(mode=0o700, parents=True)
        for name in ("doc", "gvfs"):
            (runtime / name).mkdir()
    linked_runtime = workspace.scratch / "runtime-link"
    linked_runtime.symlink_to(sibling_runtime, target_is_directory=True)
    helpers = []
    ready_files = []
    try:
        for name, runtime, checkout, ignore_term in (
                ("owned", native_runtime, str(workspace.root), True),
                ("foreign", native_runtime, "/another/checkout", False),
                ("sibling", sibling_runtime, str(workspace.root), False),
                ("linked", linked_runtime, str(workspace.root), False),
                ("traversal", workspace.scratch / ".." / "scratch-sibling" / "gnome-123" / "gnome" / "runtime",
                 str(workspace.root), False)):
            ready = base / f"{name}.ready"
            ready_files.append(ready)
            script = ("import signal, time\nfrom pathlib import Path\n"
                      + ("signal.signal(signal.SIGTERM, signal.SIG_IGN)\n" if ignore_term else "")
                      + f"Path({str(ready)!r}).write_text('ready')\ntime.sleep(60)\n")
            helpers.append(subprocess.Popen([sys.executable, "-c", script], cwd=base,
                           env={**workspace.env(), "XDG_RUNTIME_DIR": str(runtime),
                                "WINDOW_ZONES_VERIFY_CHECKOUT": checkout}, start_new_session=True))
        deadline = time.monotonic() + 10
        while not all(path.exists() for path in ready_files) and time.monotonic() < deadline:
            time.sleep(0.01)
        assert all(path.exists() for path in ready_files), "Native helper startup failed"
        assert {p["pid"] for p in workspace.runtime_processes()} == {helpers[0].pid}
        # Exercise mount selection without requiring privileged real mounts.
        mounted = {runtime / name for runtime in (native_runtime, sibling_runtime) for name in ("doc", "gvfs")}
        planned = io.StringIO()
        with patch("wz_verify.runtime.os.path.ismount", side_effect=lambda path: Path(path) in mounted):
            with contextlib.redirect_stdout(planned):
                workspace.cleanup(True)
        assert set(json.loads(planned.getvalue())["unmount"]) == {
            str(native_runtime / "doc"), str(native_runtime / "gvfs")}
        assert all(child.poll() is None for child in helpers), "Dry-run signalled a helper"
        with contextlib.redirect_stdout(io.StringIO()):
            workspace.cleanup(False)
        assert helpers[0].poll() is not None, "Cleanup removed ownership before its native helper stopped"
        assert process(helpers[0].pid) is None, "Owned helper was not reaped by its parent"
        assert all(child.poll() is None for child in helpers[1:]), "Cleanup signalled a foreign runtime helper"
        assert sibling_runtime.is_dir(), "Cleanup followed a runtime symlink or sibling prefix"
        assert not workspace.scratch.exists() and survivor.read_text() == "must survive cleanup"
    finally:
        for child in helpers:
            if child.poll() is None:
                child.kill()
            child.wait()
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
    # Terminal construction must close its PTY and reap an untracked live child.
    descriptors = len(list(Path("/proc/self/fd").iterdir()))
    try:
        with patch("wz_verify.runtime.subprocess.Popen", side_effect=recording_spawn):
            with patch("wz_verify.runtime.process_identity", return_value=None):
                rejected(lambda: Terminal(workspace, ["sleep", "60"], "terminal-tracking-failure"),
                         "Cannot record ownership of live process")
        assert spawned[-1].returncode is not None, "Terminal tracking failure left its direct child alive"
        assert process(spawned[-1].pid) is None, "Terminal tracking failure did not reap its direct child"
        assert len(list(Path("/proc/self/fd").iterdir())) == descriptors, "Terminal constructor leaked a descriptor"
    finally:
        for child in spawned:
            if child.poll() is None:
                child.kill()
                child.wait()

    # A requested action precedes a delayed completed error frame; input is not echoed.
    tui_prompt = "window-zones tui> "
    tui_responder = (
        "import sys, time\n"
        f"prompt = {tui_prompt!r}\n"
        "def render(action, error):\n"
        "    print('Window Zones TUI\\nLast action: ' + action, flush=True)\n"
        "    time.sleep(0.05)\n"
        "    print('Last error: ' + error + '\\n\\nCommands: reload | restart | status/refresh | dispatch HOTKEY | quit\\n' + prompt, end='', flush=True)\n"
        "render('<none>', 'none')\n"
        "for line in sys.stdin:\n"
        "    command = line.strip().lower()\n"
        "    if command in ('quit', 'q', 'exit'):\n"
        "        print('Session closed.', flush=True)\n"
        "        break\n"
        "    if command.endswith('f12'):\n"
        "        render('alt+ctrl+f12', 'dispatch: no binding configured for hotkey alt+ctrl+f12')\n"
        "    else:\n"
        "        render('alt+ctrl+left', 'none')\n"
    )
    tui_options = argparse.Namespace(mode="tui", journey="lifecycle", backend_flag="backend", discovery=False,
                                     command=[], expect=[], quit="quit")
    with patch.object(workspace, "app_args", return_value=[sys.executable, "-u", "-c", tui_responder]):
        for command in ("dispatch Alt+Ctrl+F12", "Alt+Ctrl+F12"):
            for expected in ("Last action: alt+ctrl+f12", "Last error:", "Last error: dispatch:"):
                tui_options.command, tui_options.expect = [command], [expected]
                rejected(lambda: session(workspace, tui_options), "TUI dispatch failed")
                transcript = sorted(workspace.artifact().glob("session-*.txt"))[-1].read_text()
                assert "Last error: dispatch: no binding configured" in transcript
                assert transcript.endswith(tui_prompt), "Failed dispatch was judged before its completed response"
            tui_options.expect = ["Last error: dispatch: no binding configured"]
            session(workspace, tui_options)  # Explicit negative dispatch remains a runnable journey.
            transcript = sorted(workspace.artifact().glob("session-*.txt"))[-1].read_text()
            assert "Session closed." in transcript
        for command in ("dispatch Alt+Ctrl+Left", "Alt+Ctrl+Left"):
            tui_options.command, tui_options.expect = [command], ["Last action: alt+ctrl+left"]
            session(workspace, tui_options)

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
print("PASS verifier: ownership, completed commands/timings, child teardown, nested native cleanup, evidence, freshness and completed TUI results")
