"""Checkout ownership, build freshness, evidence and process lifecycle."""
import contextlib
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import select
import shlex
import shutil
import signal
import subprocess
import time
import uuid


CHECK_EPOCH = 1704067200
VALID = '''[[bindings]]
hotkey = "Alt+Ctrl+Left"
action = { type = "move-to-zone", zone = "left-half" }

[[bindings]]
hotkey = "Alt+Ctrl+Up"
action = { type = "move-to-zone", zone = "center-third" }

[[bindings]]
hotkey = "Alt+Ctrl+Right"
action = { type = "move-to-zone", zone = "left-two-thirds" }

[[bindings]]
hotkey = "Alt+Ctrl+Down"
action = { type = "move-to-next-display" }

[[bindings]]
hotkey = "Alt+Ctrl+Shift+Right"
action = { type = "move-to-previous-display" }
'''
SEEDS = {"valid": VALID, "changed": VALID.split("\n\n")[0].replace("left-half", "right-half") + "\n",
         "invalid": VALID + "\nthis is not valid toml\n", "empty": "", "missing": None,
         "custom": VALID.split("\n\n")[0].replace("left-half", "inset") +
         "\n\n[zones.inset]\nx = 10\ny = 10\nwidth = 50\nheight = 50\n"}
ZONES = ("left-half", "right-half", "left-third", "center-third", "right-third",
         "left-two-thirds", "right-two-thirds", "maximize")


def fail(message):
    raise RuntimeError(message)


def gnome_resource_environment():
    """Capture only installed extension roots and the real capture socket before isolation."""
    data_home = Path(os.environ.get("XDG_DATA_HOME") or Path.home() / ".local/share")
    if not data_home.is_absolute():
        data_home = Path.home() / ".local/share"
    roots = [data_home, *(Path(value) for value in
                         (os.environ.get("XDG_DATA_DIRS") or "/usr/local/share:/usr/share").split(os.pathsep)
                         if value and Path(value).is_absolute())]
    return {"WINDOW_ZONES_SMOKE_GNOME_DATA_DIRS": json.dumps(list(dict.fromkeys(map(str, roots)))),
            "WINDOW_ZONES_SMOKE_PIPEWIRE_RUNTIME_DIR": os.environ.get("PIPEWIRE_RUNTIME_DIR")
            or os.environ.get("XDG_RUNTIME_DIR", ""),
            "WINDOW_ZONES_SMOKE_PIPEWIRE_REMOTE": os.environ.get("PIPEWIRE_REMOTE") or "pipewire-0"}


def load(path):
    try:
        value = json.loads(path.read_text())
        if not isinstance(value, dict):
            raise ValueError("expected a JSON object")
        return value
    except (ValueError, OSError) as error:
        fail(f"Cannot read {path}: {error}. Inspect it; do not borrow another checkout's state.")


def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(value, indent=2) + "\n")
    temporary.replace(path)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def process_identity(pid):
    try:
        fields = (Path("/proc") / str(pid) / "stat").read_text().rsplit(")", 1)[1].split()
        return {"pid": pid, "start": fields[19], "state": fields[0]}
    except (OSError, IndexError):
        return None


def process(pid):
    identity = process_identity(pid)
    if not identity:
        return None
    try:
        proc = Path("/proc") / str(pid)
        environment = (proc / "environ").read_bytes().split(b"\0")
        return {**identity, "cwd": str((proc / "cwd").resolve(strict=True)),
                "command": (proc / "cmdline").read_bytes().replace(b"\0", b" ").decode(errors="replace").strip(),
                "checkout_marker": next((v.decode(errors="replace").split("=", 1)[1]
                                         for v in environment if v.startswith(b"WINDOW_ZONES_VERIFY_CHECKOUT=")), None),
                "runtime_marker": next((v.decode(errors="replace").split("=", 1)[1]
                                        for v in environment if v.startswith(b"XDG_RUNTIME_DIR=")), None)}
    except (OSError, IndexError):
        return None


class Workspace:
    """Checkout-owned paths and runtime effects, shared explicitly by all journeys."""

    def __init__(self, root, *, base=None, scratch=None, binary=None):
        self.root = Path(root).resolve()
        self.key = hashlib.sha256(os.fsencode(self.root)).hexdigest()[:12]
        self.base = Path(base) if base is not None else self.root / "target" / "verification" / self.key
        self.scratch = Path(scratch) if scratch is not None else Path("/tmp") / ("wz-" + self.key)
        self.evidence = self.base / "evidence"
        self.profile = "checkout-" + self.key
        self.display = 200 + int(self.key[:8], 16) % 30000
        self.config = self.scratch / "config" / "window_zones" / "config.toml"
        self.binary = Path(binary) if binary is not None else self.root / "target" / "debug" / "window_zones"
        self.stamp = self.base / "build.json"
        self.skill = self.root / ".agents" / "skills" / "verify-window-zones"
        self.expected = self.skill / "expected.json"
        self.fixed_clock = self.skill / "fixed_clock.c"
        self.owner = {"checkout": str(self.root), "key": self.key, "uid": os.getuid()}

    def inputs(self):
        files = [self.root / "Cargo.toml", self.root / "Cargo.lock",
                 self.skill / "verify", self.expected, self.fixed_clock]
        files.extend((self.skill / "wz_verify").rglob("*.py"))
        for directory in ("src", "gnome-extension", "kwin-script", "scripts", ".cargo"):
            files.extend(p for p in (self.root / directory).rglob("*") if p.is_file() and p.suffix not in (".pyc", ".pyo"))
        hashed = hashlib.sha256()
        for path in sorted(files):
            hashed.update(os.fsencode(path.relative_to(self.root)))
            hashed.update(path.read_bytes())
        toolchain = subprocess.check_output(["rustc", "--version"], text=True).strip()
        build_env = {k: v for k, v in os.environ.items() if k.startswith(("CARGO_", "RUST"))}
        build_env["CARGO_TARGET_DIR"] = str(self.root / "target")
        hashed.update(json.dumps([toolchain, build_env], sort_keys=True).encode())
        return hashed.hexdigest(), toolchain

    def build_status(self):
        if not self.stamp.exists():
            return "missing: run launch"
        stamp = load(self.stamp)
        current, _ = self.inputs()
        if stamp.get("inputs") != current:
            return "stale: source, verifier, toolchain or build environment changed; run launch"
        for name, expected in stamp["binaries"].items():
            path = self.binary.with_name(name)
            if not path.is_file() or digest(path) != expected:
                return f"stale: {path} differs from the recorded build; run launch"
        return "current"

    def state(self, required=True):
        if self.scratch.is_symlink() or (self.scratch.exists() and
                (not self.scratch.is_dir() or self.scratch.stat().st_uid != os.getuid() or self.scratch.stat().st_mode & 0o077)):
            fail(f"Unsafe profile at {self.scratch}; require a private owned directory, not a symlink. Inspect it before launch or cleanup.")
        if not (self.scratch / "owner.json").exists():
            if required:
                fail("No launched verifier profile. Run launch first; doctor explains the expected paths.")
            return None
        value = load(self.scratch / "owner.json")
        if any(value.get(k) != v for k, v in self.owner.items()):
            fail(f"Foreign profile at {self.scratch}; refusing to attach or clean it. Use this checkout's launch.")
        destination = Path(value["evidence"])
        if destination.parent != self.evidence or destination.is_symlink():
            fail("Evidence path is outside this checkout's evidence directory. Inspect owner.json.")
        return value

    def active(self):
        path = self.scratch / "active.json"
        if not path.exists():
            return None
        saved = load(path)
        observed = process(saved["pid"])
        if observed and observed["state"] != "Z" and observed["start"] == saved["start"]:
            if observed["cwd"] != str(self.root) or observed["checkout_marker"] != str(self.root):
                fail(f"PID {saved['pid']} belongs to a foreign instance. Refusing to attach; inspect /proc/{saved['pid']}.")
            return observed
        return None

    def owns_runtime(self, marker):
        if not marker:
            return False
        path = Path(marker)
        try:
            relative = path.relative_to(self.scratch)
        except ValueError:
            return False
        if not relative.parts or ".." in relative.parts:
            return False
        current = self.scratch
        for part in relative.parts:
            current /= part
            if current.is_symlink():
                return False
        return True

    def runtime_processes(self):
        result = []
        for directory in Path("/proc").iterdir():
            if not directory.name.isdigit():
                continue
            try:
                if directory.stat().st_uid != os.getuid():
                    continue
            except OSError:
                continue
            observed = process(int(directory.name))
            if (observed and observed["state"] != "Z" and observed["checkout_marker"] == str(self.root)
                    and self.owns_runtime(observed["runtime_marker"]) and observed["pid"] != os.getpid()):
                result.append(observed)
        return result

    def display_owner(self):
        socket = Path(f"/tmp/.X11-unix/X{self.display}")
        lock = Path(f"/tmp/.X{self.display}-lock")
        owner = None
        if lock.exists():
            try:
                owner = process(int(lock.read_text().strip()))
            except ValueError:
                pass
        return {"display": f":{self.display}", "socket": str(socket), "present": socket.exists() or lock.exists(),
                "owner": owner, "ours": bool(owner and owner["checkout_marker"] == str(self.root)
                                              and owner["cwd"] == str(self.root))}

    def env(self):
        result = dict(os.environ)
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "SWAYSOCK", "HYPRLAND_INSTANCE_SIGNATURE",
                     "DBUS_SESSION_BUS_ADDRESS", "DBUS_STARTER_ADDRESS", "DBUS_STARTER_BUS_TYPE",
                     "XAUTHORITY", "DCONF_PROFILE", "PIPEWIRE_RUNTIME_DIR", "PIPEWIRE_REMOTE",
                     "WINDOW_ZONES_SMOKE_GNOME_DATA_DIRS", "WINDOW_ZONES_SMOKE_PIPEWIRE_RUNTIME_DIR",
                     "WINDOW_ZONES_SMOKE_PIPEWIRE_REMOTE"):
            result.pop(name, None)
        result["CARGO_HOME"] = os.environ.get("CARGO_HOME", str(Path.home() / ".cargo"))
        result["RUSTUP_HOME"] = os.environ.get("RUSTUP_HOME", str(Path.home() / ".rustup"))
        result.update({"HOME": str(self.scratch / "home"), "XDG_CONFIG_HOME": str(self.scratch / "config"),
                       "XDG_DATA_HOME": str(self.scratch / "data"), "XDG_STATE_HOME": str(self.scratch / "state"),
                       "XDG_CACHE_HOME": str(self.scratch / "cache"), "XDG_RUNTIME_DIR": str(self.scratch / "runtime"),
                       "XDG_DATA_DIRS": "/usr/local/share:/usr/share",
                       "WINDOW_ZONES_BINARY": str(self.binary), "CARGO_TARGET_DIR": str(self.root / "target"),
                       "WINDOW_ZONES_VERIFY_CHECKOUT": str(self.root), "XDG_SESSION_TYPE": "x11"})
        return result

    def artifact(self):
        return Path(self.state()["evidence"])

    def event(self, kind, **values):
        with (self.artifact() / "actions.jsonl").open("a") as output:
            output.write(json.dumps({"time": datetime.datetime.now(datetime.timezone.utc).isoformat(),
                                     "kind": kind, **values}) + "\n")

    def snapshot(self):
        return {str(p.relative_to(self.scratch)): digest(p) for p in self.scratch.rglob("*")
                if p.is_file() and p.name not in ("active.json", "owner.json")}

    @contextlib.contextmanager
    def lock(self):
        self.base.mkdir(parents=True, exist_ok=True)
        with (self.base / "control.lock").open("a+") as handle:
            try:
                fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                fail("This checkout already has an active verifier command. Let it finish; run doctor, not a second drive.")
            yield

    def require_ready(self):
        self.state()
        status = self.build_status()
        if status != "current":
            fail(f"Build {status}. Stale builds are not evidence.")
        if self.active():
            fail("A verifier process is already running. Use doctor/logs; cleanup only if this is your abandoned run.")
        owner = self.display_owner()
        if owner["present"] and not owner["ours"]:
            fail(f"Display {owner['display']} is occupied by {owner['owner'] or 'an unknown owner'}. "
                 "Refusing to attach. Stop it from its owning checkout, then retry.")

    def track(self, child):
        if child.poll() is not None:
            return
        # An unreaped direct child is ours; cwd/environ can vanish before it exits.
        details = process_identity(child.pid)
        if not details:
            if child.poll() is not None:
                return
            fail(f"Cannot record ownership of live process {child.pid}. Inspect logs, then cleanup.")
        save(self.scratch / "active.json", details)

    def stop(self, child):
        if child.poll() is None:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=15)
            except subprocess.TimeoutExpired:
                with contextlib.suppress(ProcessLookupError):
                    os.killpg(child.pid, signal.SIGKILL)
                child.wait()
        (self.scratch / "active.json").unlink(missing_ok=True)

    def run(self, command, label, timeout=60, environment=None, timings=None):
        folder = self.artifact()
        stdout, stderr = folder / f"{label}.stdout", folder / f"{label}.stderr"
        self.event("exec", command=shlex.join(command), stdout=str(stdout), stderr=str(stderr))
        with stdout.open("wb") as out, stderr.open("wb") as err:
            started = time.perf_counter_ns()
            child = subprocess.Popen(command, cwd=self.root, env=environment or self.env(), stdout=out, stderr=err,
                                     stdin=subprocess.DEVNULL, start_new_session=True)
            try:
                self.track(child)
                if timings is None:
                    rc = child.wait(timeout=timeout)
                elif child.poll() is not None:
                    rc = child.returncode
                else:
                    finished = os.pidfd_open(child.pid)
                    try:
                        if not select.select([finished], [], [], timeout)[0]:
                            raise subprocess.TimeoutExpired(command, timeout)
                        rc = child.wait()
                    finally:
                        os.close(finished)
                if timings is not None:
                    timings.append(time.perf_counter_ns() - started)
            finally:
                self.stop(child)
        text = stdout.read_text(errors="replace") + stderr.read_text(errors="replace")
        self.event("exit", label=label, code=rc)
        print(text, end="")
        return rc, text

    def set_config(self, seed, zone=None):
        previous = self.config.read_text() if self.config.exists() else None
        text = SEEDS[seed]
        if zone:
            if seed != "valid":
                fail("--zone requires the valid seed. Use drive config valid --zone NAME.")
            text = text.replace('zone = "left-half"', f'zone = "{zone}"', 1)
        self.config.parent.mkdir(parents=True, exist_ok=True)
        if text is None:
            self.config.unlink(missing_ok=True)
        else:
            self.config.write_text(text)
        self.event("config-edit", seed=seed, path=str(self.config), before=previous, after=text)

    def app_args(self, options, arguments):
        flags = ["--dry-run"] if options.backend_flag == "alias" else ["--backend", "dry-run"]
        if not options.discovery:
            flags += ["--config", str(self.config)]
        return [str(self.root / "scripts" / "run.sh"), *flags, *arguments]

    def doctor(self):
        profile = self.state(required=False)
        report = {**self.owner, "profile": self.profile, "data_directory": str(self.scratch / "data"),
                  "config": str(self.config), "tcp_ports": [], "port_owner": "N/A: no TCP service; Xvfb uses -nolisten tcp",
                  "x11": self.display_owner(), "active_process": self.active() if profile else None,
                  "binary": str(self.binary), "build": self.build_status(), "launched": bool(profile),
                  "evidence": profile["evidence"] if profile else None,
                  "lifecycle": "build once; every CLI/TUI drive owns a new short-lived process/PTY"}
        print(json.dumps(report, indent=2))
        owner = report["x11"]
        return 0 if profile and report["build"] == "current" and (not owner["present"] or owner["ours"]) else 1

    def launch(self, options):
        if self.scratch.exists():
            self.state()
            fail("A profile already exists. Run cleanup --dry-run, then cleanup before launch; evidence is retained.")
        owner = self.display_owner()
        if owner["present"]:
            fail(f"Derived display {owner['display']} is occupied: {owner['owner']}. Do not attach; release it from its owner.")
        folder = self.evidence / (datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ-") + uuid.uuid4().hex[:8])
        folder.mkdir(parents=True)
        self.scratch.mkdir(mode=0o700)
        for name in ("home", "config/window_zones", "data", "state", "cache", "runtime"):
            (self.scratch / name).mkdir(parents=True, mode=0o700)
        save(self.scratch / "owner.json", {**self.owner, "evidence": str(folder)})
        try:
            before, toolchain = self.inputs()
            # Build with the user's real Cargo home/toolchain; app drives use the disposable HOME.
            build_env = dict(os.environ, CARGO_TARGET_DIR=str(self.root / "target"), WINDOW_ZONES_VERIFY_CHECKOUT=str(self.root))
            rc, _ = self.run(["cargo", "build", "--locked", "--bins"], "build", timeout=300, environment=build_env)
            if rc:
                fail(f"Build failed. Inspect {folder / 'build.stderr'}; fix the checkout before relaunching.")
            after, _ = self.inputs()
            if before != after:
                fail("Build inputs changed during compilation. Run cleanup, then launch again.")
            names = ("window_zones", "window_zones_kwin")
            save(self.stamp, {**self.owner, "inputs": after, "toolchain": toolchain, "cargo_profile": "debug",
                         "binaries": {name: digest(self.binary.with_name(name)) for name in names}})
            self.set_config("valid")
            rc, text = self.run(self.app_args(options, ["status"]), "ready")
            if rc or "binding count: 5" not in text or "config state: Loaded" not in text:
                fail("Ready probe did not load all five seeded bindings. Inspect ready.stdout/stderr.")
            save(folder / "launch.json", {**self.owner, "build": load(self.stamp), "profile": self.profile,
                                          "config": str(self.config), "display": f":{self.display}"})
            print(f"READY {self.profile}\nEvidence: {folder}")
        except BaseException:
            self.cleanup(False)
            raise

    def cleanup(self, dry_run):
        profile = self.state(required=False)
        if not profile:
            print(f"No profile to remove. Evidence retained: {self.evidence}")
            return
        running = self.active()
        runtime = self.runtime_processes()
        mounts = []
        for directory, children, _ in os.walk(self.scratch, followlinks=False):
            path = Path(directory)
            children[:] = [name for name in children if not (path / name).is_symlink()]
            if path.name == "runtime" and self.owns_runtime(str(path)):
                mounts.extend(path / name for name in ("doc", "gvfs")
                              if not (path / name).is_symlink() and os.path.ismount(path / name))
                children.clear()
        plan = {"terminate": running, "terminate_runtime": runtime, "unmount": list(map(str, mounts)),
                "remove": str(self.scratch), "retain_evidence": profile["evidence"]}
        print(json.dumps(plan, indent=2))
        if dry_run:
            return
        if running:
            os.kill(running["pid"], signal.SIGTERM)
            deadline = time.monotonic() + 20
            while self.active() and time.monotonic() < deadline:
                select.select([], [], [], 0.1)
            if self.active():
                fail(f"PID {running['pid']} did not stop. Inspect it before retrying cleanup; evidence is retained.")
        for sig in (signal.SIGTERM, signal.SIGKILL):
            for observed in self.runtime_processes():
                current = process(observed["pid"])
                if (current and current["start"] == observed["start"]
                        and current["checkout_marker"] == str(self.root)
                        and current["runtime_marker"] == observed["runtime_marker"]
                        and self.owns_runtime(current["runtime_marker"])):
                    with contextlib.suppress(ProcessLookupError):
                        os.kill(observed["pid"], sig)
            deadline = time.monotonic() + 5
            while self.runtime_processes() and time.monotonic() < deadline:
                select.select([], [], [], 0.1)
        remaining = self.runtime_processes()
        if remaining:
            fail(f"Runtime helpers did not stop: {remaining}. Inspect them before retrying cleanup; ownership and evidence are retained.")
        for path in mounts:
            if path.is_symlink() or not self.owns_runtime(str(path.parent)):
                fail(f"Unsafe runtime mount path {path}. Inspect it before retrying cleanup; ownership and evidence are retained.")
            if os.path.ismount(path):
                unmount = subprocess.run(["fusermount3", "-u", str(path)], capture_output=True, text=True)
                if unmount.returncode and os.path.ismount(path):
                    fail(f"Cannot unmount owned scratch {path}: {unmount.stderr.strip()}. Inspect cleanup --dry-run holders, then retry cleanup.")
        self.event("cleanup", removed=str(self.scratch))
        # Keep ownership recoverable if a mounted/read-only child cannot be removed.
        for child in self.scratch.iterdir():
            if child.name != "owner.json":
                if child.is_dir() and not child.is_symlink():
                    shutil.rmtree(child)
                else:
                    child.unlink()
        (self.scratch / "owner.json").unlink()
        try:
            self.scratch.rmdir()
        except OSError:
            save(self.scratch / "owner.json", profile)
            raise
        if not Path(profile["evidence"]).is_dir():
            fail("Evidence disappeared during cleanup. This run is invalid.")
