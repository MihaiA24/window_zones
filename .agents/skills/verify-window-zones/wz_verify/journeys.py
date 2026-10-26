"""Real terminal and native desktop journeys using one workspace."""
import errno
import os
from pathlib import Path
import pty
import re
import select
import shlex
import shutil
import subprocess
import termios
import time

from .runtime import fail


ANSI = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")


class Terminal:
    def __init__(self, workspace, command, label, prompt="window-zones> "):
        self.workspace = workspace
        self.prompt = prompt
        self.master, slave = pty.openpty()
        attributes = termios.tcgetattr(slave)
        attributes[3] &= ~termios.ECHO
        termios.tcsetattr(slave, termios.TCSANOW, attributes)
        self.raw = (self.workspace.artifact() / f"{label}.ansi").open("wb")
        self.text = ""
        self.workspace.event("pty-launch", command=shlex.join(command), transcript=self.raw.name)
        self.child = subprocess.Popen(command, cwd=self.workspace.root, env=self.workspace.env(), stdin=slave, stdout=slave, stderr=slave,
                                      start_new_session=True)
        os.close(slave)
        self.workspace.track(self.child)

    def read(self, timeout):
        if select.select([self.master], [], [], timeout)[0]:
            try:
                data = os.read(self.master, 65536)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                data = b""
            self.raw.write(data)
            self.raw.flush()
            self.text += ANSI.sub("", data.decode(errors="replace")).replace("\r", "")

    def wait(self, expected, offset=0, timeout=15):
        deadline = time.monotonic() + timeout
        while expected not in self.text[offset:]:
            if time.monotonic() >= deadline:
                fail(f"Timed out waiting for {expected!r}. Inspect {self.raw.name}, then run doctor and cleanup.")
            self.read(min(0.1, max(0, deadline - time.monotonic())))
            if self.child.poll() is not None and expected not in self.text[offset:]:
                fail(f"App exited {self.child.returncode} before {expected!r}. Inspect {self.raw.name}.")
        return self.text[offset:]

    def send(self, command, expected):
        self.read(0)
        offset = len(self.text)
        self.workspace.event("terminal-input", command=command, expect=expected)
        os.write(self.master, (command + "\n").encode())
        observed = self.wait(expected, offset)
        if expected != "Session closed.":
            end = offset + observed.index(expected) + len(expected)
            observed = self.wait(self.prompt, end)
            observed = self.text[offset:]
        self.workspace.event("terminal-result", command=command, observed=observed)
        return observed

    def status(self, expected, mode):
        heading = "Window Zones TUI" if mode == "tui" else "Runtime status:"
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            observed = self.send("status", heading)
            if expected in observed:
                return observed
            self.read(0.1)  # Poll output while waiting for the observable, debounced config state.
        fail(f"Status never reached {expected!r}. Inspect {self.raw.name}; do not replace this check with a sleep.")

    def close(self):
        self.workspace.stop(self.child)
        os.close(self.master)
        self.raw.close()
        Path(self.raw.name).with_suffix(".txt").write_text(self.text)


def session(workspace, options):
    prompt = "window-zones tui> " if options.mode == "tui" else "window-zones> "
    heading = "Window Zones TUI" if options.mode == "tui" else "Runtime status:"
    terminal = Terminal(workspace, workspace.app_args(options, [options.mode]), f"session-{time.time_ns()}", prompt)
    try:
        terminal.wait(prompt)
        if options.command:
            if len(options.command) != len(options.expect):
                fail("Pair each --command with one --expect observable result; see drive session --help.")
            for command, expected in zip(options.command, options.expect):
                if command.lower() in ("quit", "q", "exit"):
                    fail("Use --quit for exit paths; --command must leave the app running.")
                terminal.send(command, expected)
        else:
            terminal.send("status", heading)
            if options.mode == "tui":
                terminal.send("refresh", heading)
            observed = terminal.send("dispatch Alt+Ctrl+Left", "Last action: alt+ctrl+left" if options.mode == "tui"
                                     else "Dispatch state: Succeeded")
            if "Last error: dispatch:" in observed:
                fail("TUI dispatch failed despite recording the requested hotkey. Inspect the completed response.")
            if options.journey in ("reload", "watcher"):
                for seed, expected in (("changed", "right-half" if options.mode == "tui" else "binding count: 1"),
                                       ("invalid", "(Error" if options.mode == "tui" else "config state: Error"),
                                       ("empty", "Bindings:\n  <none>" if options.mode == "tui" else "binding count: 0"),
                                       ("missing", "(Missing)" if options.mode == "tui" else "config state: Missing"),
                                       ("valid", "left-half" if options.mode == "tui" else "binding count: 5")):
                    workspace.set_config(seed)
                    if options.journey == "reload":
                        terminal.send("reload", heading if options.mode == "tui" else "Reload requested.")
                    observed = terminal.status(expected, options.mode)
                    if seed == "invalid":
                        retained = "right-half" if options.mode == "tui" else "binding count: 1"
                        if retained not in observed:
                            fail("Invalid config did not retain the last valid binding. Keep evidence and report this failure.")
                        observed = terminal.send("dispatch Alt+Ctrl+Left", "Last action: alt+ctrl+left" if options.mode == "tui"
                                                 else "Dispatch state: Succeeded")
                        if "Last error: dispatch:" in observed:
                            fail("Retained TUI binding failed to dispatch. Inspect the completed response.")
            terminal.send("reload", heading if options.mode == "tui" else "Reload requested.")
            terminal.send("restart", heading if options.mode == "tui" else "Runtime restarted.")
            observed = terminal.status("Loaded", options.mode)
            if ("Last action: <none>" if options.mode == "tui" else "last action: <none>") not in observed:
                fail("Restart did not clear runtime action history. Inspect the completed status.")
        if options.quit == "eof":
            workspace.event("terminal-input", command="<Ctrl-D>", expect="Session closed.")
            offset = len(terminal.text)
            os.write(terminal.master, b"\x04")
            terminal.wait("Session closed.", offset)
        else:
            terminal.send(options.quit, "Session closed.")
        rc = terminal.child.wait(timeout=10)
        workspace.event("pty-exit", code=rc)
        if rc:
            fail(f"Session exited {rc}. Inspect the transcript.")
    finally:
        terminal.close()


def smoke(workspace, options):
    required = {"x11": ["Xvfb", "openbox", "xdotool", "xrandr", "xprop", "xdpyinfo", "gnome-text-editor", "ffmpeg", "dbus-run-session"],
                "gnome": ["gnome-shell", "gjs", "dbus-run-session", "gnome-text-editor"],
                "kde-live": ["kwin_wayland", "kpackagetool6", "kwriteconfig6", "busctl", "gjs", "xdotool",
                             "xprop", "dbus-run-session", "python", "setsid", "gnome-text-editor"]}
    missing = [tool for tool in required[options.gate] if not shutil.which(tool)]
    if missing:
        fail(f"Missing {', '.join(missing)}. Install the prerequisites in docs/runbooks/testing.md; do not substitute a fake compositor.")
    environment = workspace.env()
    label = options.gate + "-" + str(time.time_ns())
    native_state = workspace.scratch / label
    native_proof = workspace.artifact() / label
    environment.update(WINDOW_ZONES_SMOKE_ROOT=str(native_state), WINDOW_ZONES_SMOKE_DISPLAY=str(workspace.display),
                       CARGO_NET_OFFLINE="true")
    command = [str(workspace.root / "scripts/smoke.sh"), "--keep-logs", options.gate]
    if options.gate == "x11":
        command = ["dbus-run-session", "--", *command]
    try:
        rc, text = workspace.run(command, "smoke-" + label, timeout=600, environment=environment)
    finally:
        # Preserve proof files, not compositor sockets, profiles or application data.
        for path in native_state.rglob("*"):
            relative = path.relative_to(native_state)
            if path.is_file() and not any(part in {"home", "config", "data", "runtime", "cache", "state"}
                                          for part in relative.parts[:-1]):
                destination = native_proof / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(path, destination)
        workspace.event("native-evidence", path=str(native_proof))
    if rc or "failures: 0" not in text:
        fail(f"{options.gate} smoke failed. Keep its assertions/screenshots; fix the reported precondition or app behavior before retrying.")
    return native_proof
