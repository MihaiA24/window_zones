"""Real terminal and native desktop journeys using one workspace."""
import contextlib
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

from .runtime import fail, gnome_resource_environment


ANSI = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]")


def tui_dispatch_action(observed):
    completed = observed.rsplit("Window Zones TUI", 1)[-1].replace("\r", "")
    result = re.search(r"^Last action: ([^\n]+)\nLast error: ([^\n]*)\n", completed, re.MULTILINE)
    if not result or "window-zones tui> " not in completed[result.end():]:
        fail("TUI dispatch response is incomplete. Inspect the completed response and prompt.")
    action, error = result.groups()
    if action == "<none>" or error.startswith("dispatch:"):
        fail("TUI dispatch failed despite recording the requested hotkey. Inspect the completed response.")
    return action


class Terminal:
    def __init__(self, workspace, command, label, prompt="window-zones> "):
        self.workspace = workspace
        self.prompt = prompt
        self.text = ""
        with contextlib.ExitStack() as failed:
            self.master, slave = pty.openpty()
            failed.callback(os.close, self.master)
            try:
                attributes = termios.tcgetattr(slave)
                attributes[3] &= ~termios.ECHO
                termios.tcsetattr(slave, termios.TCSANOW, attributes)
                self.raw = failed.enter_context((self.workspace.artifact() / f"{label}.ansi").open("wb"))
                self.workspace.event("pty-launch", command=shlex.join(command), transcript=self.raw.name)
                self.child = subprocess.Popen(command, cwd=self.workspace.root, env=self.workspace.env(),
                                              stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
                failed.callback(self.workspace.stop, self.child)
                self.workspace.track(self.child)
            finally:
                os.close(slave)
            failed.pop_all()

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
            self.wait(self.prompt, end)
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
    starter = options.journey == "starter" and not options.command
    if starter:
        if options.mode != "run":
            fail("The starter journey requires --mode run: TUI does not expose exact dry-run geometry.")
        code, text = workspace.run(workspace.app_args(options, ["init", "--force", "--chord", "Ctrl+Alt"]),
                                   f"starter-init-{time.time_ns()}")
        if code or "Wrote starter config to " not in text:
            fail("Starter initialization failed. Inspect the recorded init result.")
    elif options.journey == "read-error" and not options.command:
        workspace.set_config("valid")
    terminal = Terminal(workspace, workspace.app_args(options, [options.mode]), f"session-{time.time_ns()}", prompt)
    try:
        terminal.wait(prompt)
        if starter:
            observed = terminal.status("binding count: 18", options.mode)
            if "config state: Loaded" not in observed:
                fail("Starter configuration did not load.")
            terminal.send("Ctrl+Alt+C", "last move: x=640 y=300 w=640 h=480")
            for key, target in (
                ("Up", "x=0 y=0 w=1920 h=540"),
                ("Down", "x=0 y=540 w=1920 h=540"),
                ("U", "x=0 y=0 w=960 h=540"),
                ("I", "x=960 y=0 w=960 h=540"),
                ("J", "x=0 y=540 w=960 h=540"),
                ("K", "x=960 y=540 w=960 h=540"),
                ("D", "x=0 y=0 w=640 h=1080"),
                ("F", "x=640 y=0 w=640 h=1080"),
                ("G", "x=1280 y=0 w=640 h=1080"),
                ("E", "x=0 y=0 w=1280 h=1080"),
                ("T", "x=640 y=0 w=1280 h=1080"),
                ("Return", "x=0 y=0 w=1920 h=1080"),
                ("Left", "x=0 y=0 w=960 h=1080"),
                ("Right", "x=960 y=0 w=960 h=1080"),
                ("Right", "x=1920 y=0 w=640 h=1024"),
                ("Right", "x=2560 y=0 w=640 h=1024"),
                ("Right", "x=2560 y=0 w=640 h=1024"),
                ("Left", "x=1920 y=0 w=640 h=1024"),
                ("Left", "x=960 y=0 w=960 h=1080"),
                ("Left", "x=0 y=0 w=960 h=1080"),
                ("Left", "x=0 y=0 w=960 h=1080"),
                ("Backspace", "x=40 y=40 w=640 h=480"),
                ("Left", "x=0 y=0 w=960 h=1080"),
                ("Shift+Right", "x=1920 y=0 w=640 h=1024"),
                ("Shift+Left", "x=0 y=0 w=960 h=1080"),
                ("Backspace", "x=40 y=40 w=640 h=480"),
            ):
                terminal.send("dispatch Ctrl+Alt+" + key, "last move: " + target)
            terminal.send("dispatch Ctrl+Alt+Backspace", "Dispatch state: Error(ExecuteAction(NothingToRestore))")
            terminal.send("dispatch Ctrl+Alt+Left", "last move: x=0 y=0 w=960 h=1080")
            terminal.send("restart", "Runtime restarted.")
            terminal.send("dispatch Ctrl+Alt+Backspace", "Dispatch state: Error(ExecuteAction(NothingToRestore))")
        elif options.command:
            if len(options.command) != len(options.expect):
                fail("Pair each --command with one --expect observable result; see drive session --help.")
            for command, expected in zip(options.command, options.expect):
                if command.lower() in ("quit", "q", "exit"):
                    fail("Use --quit for exit paths; --command must leave the app running.")
                observed = terminal.send(command, expected)
                control = command.strip().split(" ", 1)[0].lower()
                if (options.mode == "tui" and control not in ("", "status", "refresh", "reload", "restart", "help", "?")
                        and command.strip().lower() != "dispatch"
                        and not re.match(r"Last error: dispatch:\s*\S", expected)):
                    tui_dispatch_action(observed)
        else:
            terminal.send("status", heading)
            if options.mode == "tui":
                terminal.send("refresh", heading)
            observed = terminal.send("dispatch Alt+Ctrl+Left", "Last action: alt+ctrl+left" if options.mode == "tui"
                                     else "Dispatch state: Succeeded")
            if options.mode == "tui":
                tui_dispatch_action(observed)
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
                        if options.mode == "tui":
                            tui_dispatch_action(observed)
            elif options.journey == "read-error":
                for recovery in ("identical", "missing"):
                    original = workspace.config.read_bytes()
                    workspace.config.unlink()
                    workspace.config.mkdir()
                    workspace.event("config-edit", seed="read-error", path=str(workspace.config),
                                    before=original.decode(), after={"type": "directory"})
                    observed = terminal.status("(Error" if options.mode == "tui" else "config state: Error", options.mode)
                    if ("left-half" if options.mode == "tui" else "binding count: 5") not in observed:
                        fail("A config read error discarded the active bindings.")
                    observed = terminal.send("dispatch Alt+Ctrl+Left", "Last action: alt+ctrl+left" if options.mode == "tui"
                                             else "last move: x=0 y=0 w=960 h=1080")
                    if options.mode == "tui":
                        tui_dispatch_action(observed)
                    workspace.config.rmdir()
                    workspace.event("config-edit", seed="missing", path=str(workspace.config),
                                    before={"type": "directory"}, after=None)
                    if recovery == "missing":
                        observed = terminal.status("(Missing)" if options.mode == "tui" else "config state: Missing", options.mode)
                        if ("Bindings:\n  <none>" if options.mode == "tui" else "binding count: 0") not in observed:
                            fail("A missing config retained bindings after a read error.")
                        terminal.send("dispatch Alt+Ctrl+Left", "Last error: dispatch: no binding configured" if options.mode == "tui"
                                      else "Dispatch state: Error(NoBindingForHotkey")
                    workspace.config.write_bytes(original)
                    workspace.event("config-edit", seed="identical-recovery", path=str(workspace.config),
                                    before=None, after=original.decode())
                    observed = terminal.status("(Loaded)" if options.mode == "tui" else "config state: Loaded", options.mode)
                    if ("left-half" if options.mode == "tui" else "binding count: 5") not in observed:
                        fail("Config read-error recovery did not reload the original bindings.")
                    observed = terminal.send("dispatch Alt+Ctrl+Left", "Last action: alt+ctrl+left" if options.mode == "tui"
                                             else "last move: x=0 y=0 w=960 h=1080")
                    if options.mode == "tui":
                        tui_dispatch_action(observed)
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
    if starter:
        code, text = workspace.run(workspace.app_args(options, ["status"]), f"starter-persisted-{time.time_ns()}")
        if code or any(value not in text for value in
                       ("config state: Loaded", "binding count: 18", "last action: <none>", "dispatch state: Idle")):
            fail("A fresh process did not load the persisted starter configuration with clear action history.")
        workspace.set_config("valid")


def smoke_summary(text, gate):
    matches = re.findall(
        rf"^Smoke assertions \({re.escape(gate)}\): (\d+); passed: (\d+); failed: (\d+); skipped: (\d+)$",
        text, re.MULTILINE,
    )
    if len(matches) != 1:
        fail(f"Missing or ambiguous native smoke summary for {gate}.")
    summary = dict(zip(("assertions", "passed", "failed", "skipped"), map(int, matches[0])))
    if summary["assertions"] != summary["passed"] + summary["failed"] + summary["skipped"]:
        fail(f"Inconsistent native smoke summary for {gate}.")
    return summary


def smoke(workspace, options):
    required = {"x11": ["Xvfb", "openbox", "xdotool", "xrandr", "xprop", "xdpyinfo", "gnome-text-editor", "ffmpeg", "dbus-run-session"],
                "gnome": ["gnome-shell", "dbus-daemon", "/usr/bin/python3", "gst-launch-1.0",
                          "gst-inspect-1.0", "gsettings", "Xwayland"],
                "kde-live": ["kwin_wayland", "kpackagetool6", "kwriteconfig6", "busctl", "gjs", "xdotool",
                             "xprop", "dbus-run-session", "python", "setsid", "gnome-text-editor"]}
    missing = [tool for tool in required[options.gate] if not shutil.which(tool)]
    if missing:
        fail(f"Missing {', '.join(missing)}. Install the prerequisites in docs/runbooks/testing.md; do not substitute a fake compositor.")
    resources = gnome_resource_environment() if options.gate == "gnome" else {}
    environment = workspace.env()
    environment.update(resources)
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
            if path.is_file() and not any(part in {"home", "config", "data", "runtime", "cache", "state", "target"}
                                          for part in relative.parts[:-1]):
                destination = native_proof / relative
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(path, destination)
        workspace.event("native-evidence", path=str(native_proof))
    if rc or smoke_summary(text, options.gate)["failed"]:
        fail(f"{options.gate} smoke failed. Keep its assertions/screenshots; fix the reported precondition or app behavior before retrying.")
    return native_proof
