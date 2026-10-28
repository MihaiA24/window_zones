#!/usr/bin/env python3
"""Desktop-free regressions for the native smoke runner's safety/proof seams."""

import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import unittest

import owned_process
import tui_frame


SCRIPTS = Path(__file__).resolve().parents[1]


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
