#!/usr/bin/env python3
"""Run with python3; also included by scripts/test.sh. No third-party test runner."""
import argparse
import contextlib
import io
import importlib.util
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from unittest.mock import patch

sys.dont_write_bytecode = True

from wz_verify.checks import differences, measurement_summary
from wz_verify.journeys import Terminal, session, smoke, smoke_summary
from wz_verify.runtime import Workspace, digest, gnome_resource_environment, process, save


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
    # Native resources are captured before app isolation, not rediscovered in its empty HOME.
    driver_spec = importlib.util.spec_from_file_location("gnome_smoke", workspace.root / "scripts/gnome-smoke.py")
    gnome = importlib.util.module_from_spec(driver_spec)
    driver_spec.loader.exec_module(gnome)
    host_home = base / "host-home"
    user_data = host_home / "custom-data"
    system_data = base / "system-data"
    host_runtime = base / "host-runtime"
    host_runtime.mkdir()
    indicator_uuid = "appindicatorsupport@rgcjonas.gmail.com"

    def indicator(data, versions):
        extension = data / "gnome-shell/extensions" / indicator_uuid
        extension.mkdir(parents=True)
        (extension / "metadata.json").write_text(json.dumps({"uuid": indicator_uuid, "shell-version": versions}))
        (extension / "extension.js").write_text("// installed resource\n")
        return extension

    user_indicator = indicator(user_data, ["50", "51"])
    system_indicator = indicator(system_data, ["51"])
    resources_env = {"HOME": str(host_home), "XDG_DATA_HOME": str(user_data),
                     "XDG_DATA_DIRS": str(system_data), "XDG_RUNTIME_DIR": str(host_runtime),
                     "PIPEWIRE_REMOTE": "user-pipewire", "DISPLAY": ":7", "WAYLAND_DISPLAY": "wayland-host",
                     "SWAYSOCK": "/host/sway.sock", "HYPRLAND_INSTANCE_SIGNATURE": "host-hyprland",
                     "KDE_FULL_SESSION": "true", "KDE_SESSION_VERSION": "6",
                     "XDG_SESSION_DESKTOP": "KDE", "DESKTOP_SESSION": "sway",
                     "DBUS_SESSION_BUS_ADDRESS": "unix:path=/host/bus",
                     "DBUS_STARTER_ADDRESS": "unix:path=/host/starter", "DCONF_PROFILE": "host-profile"}
    with socket.socket(socket.AF_UNIX) as pipewire, socket.socket(socket.AF_UNIX) as default_pipewire:
        pipewire.bind(str(host_runtime / "user-pipewire"))
        default_pipewire.bind(str(host_runtime / "pipewire-0"))
        with patch.dict(os.environ, resources_env, clear=True):
            resource_inputs = gnome_resource_environment()
            environment = workspace.env()
            assert not any(name in environment for name in ("PIPEWIRE_RUNTIME_DIR", "PIPEWIRE_REMOTE",
                                                            "DBUS_SESSION_BUS_ADDRESS", "DBUS_STARTER_ADDRESS"))
            environment.update(resource_inputs)
            roots, runtime, remote = gnome.resource_sources(environment)
            assert roots == [user_data, system_data] and runtime == host_runtime and remote == "user-pipewire"
            assert gnome.find_appindicator(roots, "50") == user_indicator
            assert gnome.find_appindicator(roots, "51") == user_indicator
            (user_indicator / "metadata.json").write_text(json.dumps({"uuid": indicator_uuid, "shell-version": ["49"]}))
            assert gnome.find_appindicator(roots, "51") == system_indicator
            rejected(lambda: gnome.find_appindicator(roots, "50"), "compatible with GNOME 50")
            (user_indicator / "metadata.json").write_text(json.dumps({"uuid": indicator_uuid, "shell-version": ["50", "51"]}))
            native_env = gnome.isolated_environment(base / "private-gnome", runtime, remote, environment)
            for name, directory_name in (("HOME", "home"), ("XDG_CONFIG_HOME", "config"),
                                         ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"),
                                         ("XDG_CACHE_HOME", "cache"), ("XDG_RUNTIME_DIR", "runtime")):
                assert native_env[name] == str(base / "private-gnome" / directory_name)
            assert native_env["PIPEWIRE_RUNTIME_DIR"] == str(host_runtime)
            assert native_env["PIPEWIRE_REMOTE"] == "user-pipewire"
            assert not any(name in native_env for name in ("DBUS_SESSION_BUS_ADDRESS", "DBUS_STARTER_ADDRESS",
                                                           "DCONF_PROFILE", "DISPLAY", "WAYLAND_SOCKET",
                                                           "SWAYSOCK", "HYPRLAND_INSTANCE_SIGNATURE",
                                                           "KDE_FULL_SESSION", "KDE_SESSION_VERSION",
                                                           "XDG_SESSION_DESKTOP", "DESKTOP_SESSION",
                                                           *resource_inputs))
        with patch.dict(os.environ, {"HOME": str(host_home), "XDG_RUNTIME_DIR": str(host_runtime)}, clear=True):
            roots, runtime, remote = gnome.resource_sources(gnome_resource_environment())
            assert roots == [host_home / ".local/share", Path("/usr/local/share"), Path("/usr/share")]
            assert runtime == host_runtime and remote == "pipewire-0"
        override = base / "pipewire-override"
        override.mkdir()
        with patch.dict(os.environ, {**resources_env, "PIPEWIRE_RUNTIME_DIR": str(override),
                                    "PIPEWIRE_REMOTE": str(host_runtime / "user-pipewire")}, clear=True):
            roots, runtime, remote = gnome.resource_sources(gnome_resource_environment())
            assert runtime == override and remote == str(host_runtime / "user-pipewire")
    rejected(lambda: gnome.resource_sources({**resource_inputs, "WINDOW_ZONES_SMOKE_PIPEWIRE_REMOTE": "missing"}),
             "PipeWire socket")
    rejected(lambda: gnome.resource_sources({**resource_inputs, "WINDOW_ZONES_SMOKE_GNOME_DATA_DIRS": '["relative"]'}),
             "absolute")
    complete_frame = "Window Zones TUI\nLast action: alt+ctrl+left\nLast error: none\nLast event: none\nwindow-zones tui> "
    assert gnome.completed_tui_frame(complete_frame) == complete_frame
    assert not gnome.completed_tui_frame(complete_frame + "\x1b[2JWindow Zones TUI\nLast action: alt+ctrl+right\n")
    assert gnome.completed_tui_frame(complete_frame + "\x1b[2JWindow Zones TUI\nLast action: alt+ctrl+right\n",
                                     first=True) == complete_frame
    assert not gnome.completed_tui_frame("Last action: alt+ctrl+left\nwindow-zones tui> ")

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
    def native_launch(command, label, *, timeout, environment):
        assert environment["HOME"] == str(workspace.scratch / "home")
        assert environment["XDG_RUNTIME_DIR"] == str(workspace.scratch / "runtime")
        assert not environment.get("DBUS_SESSION_BUS_ADDRESS")
        assert json.loads(environment["WINDOW_ZONES_SMOKE_GNOME_DATA_DIRS"]) == [str(user_data), str(system_data)]
        roots, runtime, remote = gnome.resource_sources(environment)
        assert gnome.find_appindicator(roots, "51") == user_indicator
        assert runtime == host_runtime and remote == "user-pipewire"
        native_state = Path(environment["WINDOW_ZONES_SMOKE_ROOT"])
        for relative in ("gnome/results.json", "gnome/native.png", "gnome/target/debug/build-output.log",
                         "gnome/home/private.log", "gnome/data/private.log"):
            path = native_state / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("proof" if relative in ("gnome/results.json", "gnome/native.png") else "not proof")
        return 0, "Smoke assertions (gnome): 1; passed: 1; failed: 0; skipped: 0"

    with patch.dict(os.environ, resources_env, clear=True):
        with patch("wz_verify.journeys.shutil.which", return_value="/installed/tool"):
            with patch.object(workspace, "run", side_effect=native_launch):
                native_proof = smoke(workspace, argparse.Namespace(gate="gnome"))
        with patch("wz_verify.journeys.shutil.which", side_effect=lambda tool: None if tool == "gst-launch-1.0" else "/installed/tool"):
            with patch.object(workspace, "run") as launch:
                rejected(lambda: smoke(workspace, argparse.Namespace(gate="gnome")), "gst-launch-1.0")
                launch.assert_not_called()
    assert sorted(str(path.relative_to(native_proof)) for path in native_proof.rglob("*") if path.is_file()) == [
        "gnome/native.png", "gnome/results.json",
    ], "Private app data or build outputs leaked into native proof"

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

    cache = checkout.root / "scripts" / "smoke" / "__pycache__" / "probe.cpython-314.pyc"
    cache.parent.mkdir(parents=True)
    cache.write_bytes(b"generated interpreter cache")
    assert checkout.build_status() == "current", "Python bytecode invalidated source proof"
    cache.unlink()
    assert checkout.build_status() == "current", "removing Python bytecode invalidated source proof"

    # Cleaning one workspace must not remove another workspace's profile or proof.
    other = Workspace(workspace.root, base=base / "other", scratch=base / "other-scratch")
    other.scratch.mkdir(mode=0o700)
    other_proof = other.evidence / "proof"
    other_proof.mkdir(parents=True)
    other_survivor = other_proof / "proof.txt"
    other_survivor.write_text("independent workspace")
    save(other.scratch / "owner.json", {**other.owner, "evidence": str(other_proof)})
    # Native helpers may outlive their driver and use a nested private runtime.
    native_root = workspace.scratch / "gnome-123"
    native_runtime = native_root / "gnome" / "runtime"
    sibling_runtime = base / "scratch-sibling" / "gnome-123" / "gnome" / "runtime"
    for runtime in (native_runtime, sibling_runtime):
        runtime.mkdir(mode=0o700, parents=True)
        for name in ("doc", "gvfs"):
            (runtime / name).mkdir()
    native_root.chmod(0o700)
    native_registry = native_root / "pids"
    native_registry.touch(mode=0o600)
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
                      + ("signal.signal(signal.SIGTERM, signal.SIG_IGN)\n" if ignore_term else ""))
            if name == "owned":
                script += ("import ctypes, os, subprocess, sys\n"
                           f"subprocess.run([sys.executable, {str(workspace.root / 'scripts/smoke/owned_process.py')!r}, "
                           f"'publish', {str(native_registry)!r}, {str(native_root)!r}, '0', str(os.getpid()), "
                           f"{str(native_root / 'owned.pid')!r}], check=True)\n"
                           "libc = ctypes.CDLL(None, use_errno=True)\n"
                           "if libc.prctl(4, 0, 0, 0, 0) != 0:\n"
                           "    raise OSError(ctypes.get_errno(), 'PR_SET_DUMPABLE')\n")
            script += f"Path({str(ready)!r}).write_text('ready')\ntime.sleep(60)\n"
            helpers.append(subprocess.Popen([sys.executable, "-c", script], cwd=base,
                           env={**workspace.env(), "XDG_RUNTIME_DIR": str(runtime),
                                **({"WINDOW_ZONES_SMOKE_OWNER": str(native_root)} if name == "owned" else {}),
                                "WINDOW_ZONES_VERIFY_CHECKOUT": checkout}, start_new_session=True))
        deadline = time.monotonic() + 10
        while not all(path.exists() for path in ready_files) and time.monotonic() < deadline:
            time.sleep(0.01)
        assert all(path.exists() for path in ready_files), "Native helper startup failed"
        assert process(helpers[0].pid) is None, "Protected helper environment remained readable"
        foreign_start = gnome.owned_process.process(helpers[1].pid)[0]
        with native_registry.open("a") as registry:
            registry.write(f"{helpers[1].pid} {foreign_start + 1}\n")
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
        native_registry.chmod(0o644)
        with contextlib.redirect_stdout(io.StringIO()):
            rejected(lambda: workspace.cleanup(True), "Unsafe native ownership")
        native_registry.chmod(0o600)
        saved_registry = native_registry.with_suffix(".saved")
        native_registry.rename(saved_registry)
        native_registry.symlink_to(saved_registry)
        with contextlib.redirect_stdout(io.StringIO()):
            rejected(lambda: workspace.cleanup(True), "Unsafe native ownership")
        native_registry.unlink()
        saved_registry.rename(native_registry)
        with patch("wz_verify.runtime.signal.pidfd_send_signal", side_effect=PermissionError("denied")):
            try:
                with contextlib.redirect_stdout(io.StringIO()):
                    workspace.cleanup(False)
            except PermissionError:
                pass
            else:
                raise AssertionError("Cleanup ignored a helper signalling failure")
        assert (workspace.scratch / "owner.json").exists() and native_registry.exists()
        assert helpers[0].poll() is None, "Failed cleanup lost its live helper"
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
        "    if command == 'dispatch':\n"
        "        render('<none>', 'none')\n"
        "    elif command.endswith('f12'):\n"
        "        render('alt+ctrl+f12', 'dispatch: no binding configured for hotkey alt+ctrl+f12')\n"
        "    else:\n"
        "        render('alt+ctrl+left', 'none')\n"
    )
    tui_options = argparse.Namespace(mode="tui", journey="lifecycle", backend_flag="backend", discovery=False,
                                     command=[], expect=[], quit="quit")
    with patch.object(workspace, "app_args", return_value=[sys.executable, "-u", "-c", tui_responder]):
        tui_options.command, tui_options.expect = ["dispatch"], ["Last action: <none>"]
        session(workspace, tui_options)  # Missing argument reports a control error, not a failed action.
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
