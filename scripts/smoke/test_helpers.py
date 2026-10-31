#!/usr/bin/env python3
"""Desktop-free regressions for the native smoke runner's safety/proof seams."""

import importlib.util
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

import owned_process
import tui_frame


SCRIPTS = Path(__file__).resolve().parents[1]
GNOME_SPEC = importlib.util.spec_from_file_location("gnome_smoke", SCRIPTS / "gnome-smoke.py")
gnome = importlib.util.module_from_spec(GNOME_SPEC)
GNOME_SPEC.loader.exec_module(gnome)


class SmokeHelpersTest(unittest.TestCase):
    def shell(self, script, **env):
        return subprocess.run(
            ["bash", "-c", script],
            env={**os.environ, "SCRIPT_DIR": str(SCRIPTS), **env},
            text=True,
            capture_output=True,
            timeout=15,
        )

    def test_window_lookup_never_adopts_a_title_only_match(self):
        result = self.shell(r'''
source "$SCRIPT_DIR/smoke/geometry.sh"
source "$SCRIPT_DIR/smoke/gnome-live/observations.sh"
GNOME_XDISPLAY=:123
GNOME_XAUTHORITY=/unused
EDITOR_PID=42
calls=0
owned_process() { calls=$((calls + 1)); (( calls < 2 )); }
xon() {
    if [[ "$2" == search ]]; then printf '111\n'; else printf '777\n'; fi
}
if gnome_live_find_window; then exit 1; fi
[[ -z "$WINDOW_ID" ]] || exit 2
owned_process() { return 0; }
xon() {
    if [[ "$2" == search ]]; then printf '111\n112\n'
    elif [[ "$3" == 112 ]]; then printf '42\n'
    else printf '777\n'; fi
}
gnome_live_find_window && [[ "$WINDOW_ID" == 112 ]]
''')
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_signal_identity_does_not_kill_a_reused_pid(self):
        child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
        start = owned_process.process(child.pid)[0]
        try:
            owned_process.signal_identity(child.pid, start + 1, signal.SIGTERM)
            self.assertIsNone(child.poll())
            owned_process.signal_identity(child.pid, start, signal.SIGTERM)
            self.assertEqual(child.wait(timeout=5), -signal.SIGTERM)
        finally:
            if child.poll() is None:
                owned_process.signal_identity(child.pid, start, signal.SIGKILL)
                child.wait(timeout=5)

    def test_cleanup_finds_restarted_orphan_without_adopting_foreign_process(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            registry = root / "pids"
            registry.touch()
            owner = str(root)
            child_file = root / "restarted-pid"
            foreign = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
            foreign_start = owned_process.process(foreign.pid)[0]
            restarted = None
            start = None
            try:
                launcher = subprocess.Popen(
                    [sys.executable, "-c", '''
import os, pathlib, subprocess, sys
first = subprocess.Popen([sys.executable, "-c", "pass"])
first.wait()
replacement = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
pathlib.Path(sys.argv[1]).write_text(str(replacement.pid))
''', str(child_file)],
                    env={**os.environ, "WINDOW_ZONES_SMOKE_OWNER": owner},
                )
                owned_process.register(registry, owner, os.getpid(), launcher.pid)
                self.assertEqual(launcher.wait(timeout=5), 0)
                restarted = int(child_file.read_text())
                start = owned_process.process(restarted)[0]
                # A historical number now naming an unrelated live process is not ownership.
                with registry.open("a") as stream:
                    stream.write(f"{foreign.pid} {foreign_start + 1}\n")
                owned_process.cleanup(registry, owner, os.getpid())
                self.assertFalse(owned_process.is_alive(restarted, start))
                self.assertIsNone(foreign.poll())
            finally:
                if restarted is not None and start is not None:
                    owned_process.signal_identity(restarted, start, signal.SIGKILL)
                owned_process.signal_identity(foreign.pid, foreign_start, signal.SIGKILL)
                foreign.wait(timeout=5)

    def test_cleanup_keeps_published_identity_when_orphan_environment_is_unreadable(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            registry, publication, ready = (root / name for name in ("pids", "child.pid", "ready"))
            registry.touch(mode=0o600)
            owner = str(root)
            foreign = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
            foreign_start = owned_process.process(foreign.pid)[0]
            child_code = '''
import ctypes, os, pathlib, subprocess, sys, time
registry, owner, publication, ready, helper = sys.argv[1:]
subprocess.run([sys.executable, helper, "publish", registry, owner, "0",
                str(os.getpid()), publication], check=True)
libc = ctypes.CDLL(None, use_errno=True)
if libc.prctl(4, 0, 0, 0, 0) != 0:
    raise OSError(ctypes.get_errno(), "PR_SET_DUMPABLE")
pathlib.Path(ready).touch()
time.sleep(60)
'''
            launcher = subprocess.Popen(
                [sys.executable, "-c", '''
import pathlib, subprocess, sys, time
child = subprocess.Popen([sys.executable, "-c", *sys.argv[1:]])
ready = pathlib.Path(sys.argv[-2])
deadline = time.monotonic() + 5
while not ready.exists():
    if child.poll() is not None or time.monotonic() >= deadline:
        raise RuntimeError("owned child did not publish its protected identity")
    time.sleep(0.01)
''', child_code, str(registry), owner, str(publication), str(ready),
                 str(SCRIPTS / "smoke/owned_process.py")],
                env={**os.environ, "WINDOW_ZONES_SMOKE_OWNER": owner},
            )
            try:
                self.assertEqual(launcher.wait(timeout=10), 0)
                pid, start = map(int, publication.read_text().split())
                self.assertEqual(owned_process.records(registry)[pid], start)
                self.assertFalse(owned_process.has_owner(pid, owner))
                self.assertTrue(owned_process.is_alive(pid, start))
                for expected, status in ((start, 0), (start + 1, 1)):
                    result = subprocess.run(
                        [sys.executable, str(SCRIPTS / "smoke/owned_process.py"), "alive",
                         str(registry), owner, "0", str(pid), str(expected)],
                        capture_output=True, text=True, timeout=5)
                    self.assertEqual(result.returncode, status, result.stderr)
                owned_process.signal_identity(pid, start + 1, signal.SIGTERM)
                self.assertTrue(owned_process.is_alive(pid, start))
                with registry.open("a") as stream:
                    stream.write(f"{foreign.pid} {foreign_start + 1}\n")
                with self.assertRaisesRegex(RuntimeError, "unowned smoke process"):
                    owned_process.register(registry, owner, 0, foreign.pid)
                owned_process.cleanup(registry, owner, os.getpid())
                self.assertFalse(owned_process.is_alive(pid, start))
                self.assertIsNone(foreign.poll())
            finally:
                if launcher.poll() is None:
                    launcher.terminate()
                    launcher.wait(timeout=5)
                owned_process.cleanup(registry, owner, os.getpid())
                owned_process.signal_identity(foreign.pid, foreign_start, signal.SIGKILL)
                foreign.wait(timeout=5)

    def test_nested_cleanup_preserves_outer_owner_and_rescues_private_orphans(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            outer, inner = str(root / "outer"), str(root / "private")
            outer_registry, inner_registry = root / "outer-pids", root / "inner-pids"
            outer_registry.touch()
            inner_registry.touch()
            outer_env = {**os.environ, "WINDOW_ZONES_SMOKE_OWNER": outer}
            outer_env.pop("WINDOW_ZONES_SMOKE_PRIVATE_OWNER", None)
            outer_only = subprocess.Popen(
                [sys.executable, "-c", "import time; time.sleep(60)"], env=outer_env)
            outer_start = owned_process.process(outer_only.pid)[0]
            orphans = []
            def orphan(name):
                pid_file = root / name
                launcher = subprocess.Popen(
                    [sys.executable, "-c", '''
import os, pathlib, signal, subprocess, sys
child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
pathlib.Path(sys.argv[1]).write_text(str(child.pid))
os.kill(os.getpid(), signal.SIGKILL)
''', str(pid_file)],
                    env={**outer_env, "WINDOW_ZONES_SMOKE_PRIVATE_OWNER": inner})
                self.assertEqual(launcher.wait(timeout=5), -signal.SIGKILL)
                pid = int(pid_file.read_text())
                start = owned_process.process(pid)[0]
                orphans.append((pid, start))
                return pid, start
            try:
                first, first_start = orphan("inner-child")
                self.assertTrue(owned_process.has_owner(first, inner))
                self.assertTrue(owned_process.has_owner(first, outer))
                self.assertFalse(owned_process.has_owner(first, str(root)))
                self.assertFalse(owned_process.has_owner(first, inner + "-sibling"))
                owned_process.cleanup(inner_registry, inner, os.getpid())
                self.assertFalse(owned_process.is_alive(first, first_start))
                self.assertIsNone(outer_only.poll())
                second, second_start = orphan("outer-rescued-child")
                # No inner teardown runs after this driver's SIGKILL.
                owned_process.cleanup(outer_registry, outer, os.getpid())
                self.assertFalse(owned_process.is_alive(second, second_start))
                self.assertEqual(outer_only.wait(timeout=5), -signal.SIGTERM)
            finally:
                for pid, start in orphans:
                    owned_process.signal_identity(pid, start, signal.SIGKILL)
                owned_process.signal_identity(outer_only.pid, outer_start, signal.SIGKILL)
                outer_only.wait(timeout=5)

    def test_gnome_registration_keeps_an_already_exited_child_status(self):
        with tempfile.TemporaryDirectory() as directory:
            registry = Path(directory) / "pids"
            registry.touch()
            child = subprocess.Popen([sys.executable, "-c", "raise SystemExit(23)"])
            self.assertEqual(child.wait(timeout=5), 23)
            gnome.register_child(child, registry, directory)
            self.assertEqual(child.returncode, 23)
            self.assertEqual(gnome.owned_process.records(registry), {})

    def test_gnome_registration_stops_a_live_child_without_stable_identity(self):
        with tempfile.TemporaryDirectory() as directory:
            registry = Path(directory) / "pids"
            registry.touch()
            child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
            try:
                with mock.patch.object(gnome.owned_process, "process", return_value=None):
                    with self.assertRaisesRegex(RuntimeError, "stable identity"):
                        gnome.register_child(child, registry, directory)
                self.assertEqual(child.returncode, -signal.SIGTERM)
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait(timeout=5)

    def test_gnome_reduced_motion_is_optional_only_before_shell_51(self):
        schema = mock.Mock()
        schema.has_key.return_value = False
        self.assertFalse(gnome.reduced_motion_supported(50, None))
        self.assertFalse(gnome.reduced_motion_supported(50, schema))
        with self.assertRaisesRegex(RuntimeError, "GNOME 51 requires"):
            gnome.reduced_motion_supported(51, schema)
        with self.assertRaisesRegex(RuntimeError, "GNOME 51 requires"):
            gnome.reduced_motion_supported(51, None)
        schema.has_key.return_value = True
        self.assertTrue(gnome.reduced_motion_supported(51, schema))

    def test_gnome_capture_links_only_exact_owned_process_ports(self):
        graph = [
            {"id": 1, "type": "PipeWire:Interface:Client",
             "info": {"props": {"application.process.id": 111}}},
            {"id": 2, "type": "PipeWire:Interface:Client",
             "info": {"props": {"application.process.id": "222"}}},
            {"id": 3, "type": "PipeWire:Interface:Client",
             "info": {"props": {"application.process.id": 333}}},
            {"id": 10, "type": "PipeWire:Interface:Node",
             "info": {"props": {"client.id": 1, "node.name": "gnome-shell"}}},
            {"id": 11, "type": "PipeWire:Interface:Node",
             "info": {"props": {"client.id": 2, "node.name": "private-capture"}}},
            {"id": 12, "type": "PipeWire:Interface:Node",
             "info": {"props": {"client.id": 3, "node.name": "gst-launch-1.0"}}},
            {"id": 20, "type": "PipeWire:Interface:Port",
             "info": {"direction": "output", "props": {"node.id": 10}}},
            {"id": 21, "type": "PipeWire:Interface:Port",
             "info": {"direction": "input", "props": {"node.id": 11}}},
            {"id": 22, "type": "PipeWire:Interface:Port",
             "info": {"direction": "input", "props": {"node.id": 12}}},
        ]
        self.assertEqual(gnome.capture_endpoints(graph, 10, 111, "private-capture", 222),
                         (10, 11, 20, 21))
        self.assertIsNone(gnome.capture_endpoints(graph, 99, 111, "private-capture", 222))
        self.assertIsNone(gnome.capture_endpoints(graph, 10, 111, "missing-capture", 222))
        with self.assertRaisesRegex(RuntimeError, "owned process"):
            gnome.capture_endpoints(graph, 10, 999, "private-capture", 222)
        with self.assertRaisesRegex(RuntimeError, "owned process"):
            gnome.capture_endpoints(graph, 10, 111, "private-capture", 999)
        duplicate = {"id": 23, "type": "PipeWire:Interface:Port",
                     "info": {"direction": "input", "props": {"node.id": 11}}}
        with self.assertRaisesRegex(RuntimeError, "Ambiguous.*ports"):
            gnome.capture_endpoints(graph + [duplicate], 10, 111, "private-capture", 222)

    def test_signal_exits_instead_of_resuming_the_gate(self):
        for signame, code in (("INT", 130), ("TERM", 143)):
            with self.subTest(signal=signame):
                result = self.shell(r'''
source "$SCRIPT_DIR/smoke/processes.sh"
cleanup() { printf 'cleanup\n'; }
install_smoke_traps
kill -"$SIGNAME" "$$"
printf 'continued\n'
''', SIGNAME=signame)
                self.assertEqual(result.returncode, code, result.stderr)
                self.assertEqual(result.stdout, "cleanup\n")

    def test_tui_response_requires_new_complete_frame_and_keeps_errors(self):
        old = tui_frame.HEADER + "Last action: alt+ctrl+left\nLast error: none\n" + tui_frame.PROMPT
        failed = tui_frame.HEADER + "Last action: ctrl+alt+left\nLast error: dispatch: unavailable\n"
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "tui.log"
            log.write_text(old + failed)
            checkpoint = len(old.encode())
            frame, matched = tui_frame.wait_frame(log, checkpoint, "Last action: alt+ctrl+left", 0)
            self.assertFalse(matched)
            self.assertEqual(frame, "")
            with log.open("a") as stream:
                stream.write(tui_frame.PROMPT)
            frame, matched = tui_frame.wait_frame(log, checkpoint, "Last action: alt+ctrl+left", 0)
            self.assertTrue(matched)
            self.assertIn("Last error: dispatch: unavailable", frame)
            self.assertNotIn("Last error: none", frame)
            log.write_text(old + failed + tui_frame.PROMPT + old)
            frame, matched = tui_frame.wait_frame(log, checkpoint, "Last action: alt+ctrl+left", 0)
            self.assertTrue(matched)
            self.assertIn("Last error: none", frame)

    def test_tui_action_label_cannot_pass_a_failed_dispatch(self):
        with tempfile.TemporaryDirectory() as directory:
            result = self.shell(r'''
source "$SCRIPT_DIR/smoke/assertions.sh"
source "$SCRIPT_DIR/smoke/waits.sh"
source "$SCRIPT_DIR/smoke/tui.sh"
ASSERT_LOG="$TMP_DIR/assertions.log"
ASSERTIONS=0
FAILURES=0
tui_log="$TMP_DIR/tui.log"
: >"$tui_log"
send_app() {
    printf '\033[2J\033[HWindow Zones TUI\nLast action: ctrl+alt+left\nLast error: dispatch: unavailable\nwindow-zones tui> ' >>"$tui_log"
}
tui_dispatch "$tui_log" regression alt+ctrl+left
[[ "$FAILURES" == 1 ]]
''', TMP_DIR=directory)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("FAIL regression completes successfully", result.stdout)

    def test_pinned_application_keeps_helper_builds_outside_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            scratch = root / "scratch"
            scratch.mkdir()
            persistent = root / "cargo-target"
            result = self.shell(r'''
source "$SCRIPT_DIR/smoke/assertions.sh"
source "$SCRIPT_DIR/smoke/seam.sh"
ASSERT_LOG="$TMP_DIR/assertions.log"
ASSERTIONS=0
FAILURES=0
TARGET_DIR=''
WINDOW_ZONES_BINARY=/pinned/window_zones
cargo() {
    if [[ "$1" == metadata ]]; then
        printf '{"target_directory":"%s"}\n' "$PERSISTENT_TARGET"
    else
        printf '%s\n' "$CARGO_TARGET_DIR" >"$BUILD_TARGET_RECORD"
    fi
}
build_seam_helper regression
[[ "$SEAM_HELPER" == "$PERSISTENT_TARGET/smoke-seam-helper/debug/window_zones_smoke_seam" ]]
''', TMP_DIR=str(scratch), PROJECT_ROOT=str(SCRIPTS.parent),
                PERSISTENT_TARGET=str(persistent), BUILD_TARGET_RECORD=str(root / "build-target"))
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual((root / "build-target").read_text().strip(), str(persistent / "smoke-seam-helper"))
            self.assertFalse((scratch / "smoke-seam-helper").exists())


if __name__ == "__main__":
    unittest.main()
