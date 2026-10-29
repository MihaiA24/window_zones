#!/usr/bin/env python3
"""Signal smoke-owned Linux processes through PID/start identities and pidfds."""

import os
from pathlib import Path
import signal
import sys
import time


def process(pid):
    try:
        path = Path(f"/proc/{pid}")
        if path.stat().st_uid != os.getuid():
            return None
        fields = (path / "stat").read_text().rsplit(")", 1)[1].split()
        return int(fields[19]), int(fields[1]), fields[0]
    except (OSError, ValueError, IndexError):
        return None


def records(path):
    result = {}
    for line in Path(path).read_text().splitlines():
        pid, start = map(int, line.split())
        result[pid] = start
    return result


def is_alive(pid, start):
    current = process(pid)
    return current is not None and current[0] == start and current[2] != "Z"


def signal_identity(pid, start, signum):
    # Checking before kill(pid) leaves a PID-reuse race. Open first, then check the
    # identity, and signal the descriptor rather than whichever process has pid later.
    try:
        fd = os.pidfd_open(pid)
    except ProcessLookupError:
        return
    try:
        if is_alive(pid, start):
            signal.pidfd_send_signal(fd, signum)
    except ProcessLookupError:
        pass
    finally:
        os.close(fd)


def has_owner(pid, owner):
    try:
        environment = Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
        return any(f"{name}={owner}".encode() in environment for name in (
            "WINDOW_ZONES_SMOKE_OWNER", "WINDOW_ZONES_SMOKE_PRIVATE_OWNER"))
    except OSError:
        return False


def descendants(snapshot, owned):
    changed = True
    while changed:
        changed = False
        for pid, (start, parent, state) in snapshot.items():
            if pid in owned or state == "Z":
                continue
            parent_start = owned.get(parent)
            if parent_start is not None and snapshot.get(parent, (None,))[0] == parent_start:
                if start >= parent_start:
                    owned[pid] = start
                    changed = True
    return owned


def discover(known, owner, runner):
    snapshot = {}
    for entry in Path("/proc").iterdir():
        if entry.name.isdigit():
            pid = int(entry.name)
            current = process(pid)
            if current is not None:
                snapshot[pid] = current
    owned = {pid: start for pid, start in known.items() if is_alive(pid, start)}
    # The inherited outer marker rescues orphaned helpers if a nested driver is
    # killed; its private marker lets inner cleanup leave that outer runner alone.
    for pid, (start, _, state) in snapshot.items():
        if state != "Z" and has_owner(pid, owner):
            owned[pid] = start
    descendants(snapshot, owned)
    owned.pop(os.getpid(), None)
    owned.pop(runner, None)
    return owned


def register(path, owner, runner, pid):
    current = process(pid)
    if current is None:
        return
    # A FIFO child may not have execed (and inherited its exported environment)
    # yet. Its direct parent still proves ownership. Published descendant PIDs
    # must carry this run's marker, never just be a currently allocated number.
    if current[1] != runner and not has_owner(pid, owner):
        raise RuntimeError(f"refusing unowned smoke process {pid}")
    with Path(path).open("a") as stream:
        stream.write(f"{pid} {current[0]}\n")


def cleanup(path, owner, runner):
    known = records(path)
    deadline = time.monotonic() + 5
    while True:
        owned = discover(known, owner, runner)
        known.update(owned)
        if not owned:
            return
        signum = signal.SIGTERM if time.monotonic() < deadline else signal.SIGKILL
        for pid, start in owned.items():
            signal_identity(pid, start, signum)
        if time.monotonic() >= deadline + 5:
            remaining = {pid: start for pid, start in known.items() if is_alive(pid, start)}
            if remaining:
                raise RuntimeError(f"smoke processes did not stop: {remaining}")
            return
        time.sleep(0.1)


def main():
    command, path, owner, runner, *args = sys.argv[1:]
    runner = int(runner)
    if command == "require":
        fd = os.pidfd_open(os.getpid())
        try:
            signal.pidfd_send_signal(fd, 0)
        finally:
            os.close(fd)
    elif command == "register":
        register(path, owner, runner, int(args[0]))
    elif command == "cleanup":
        cleanup(path, owner, runner)
    else:
        pid = int(args[0])
        start = records(path).get(pid)
        if command == "alive":
            return 0 if start is not None and is_alive(pid, start) else 1
        if command == "signal":
            if start is not None:
                signal_identity(pid, start, getattr(signal, f"SIG{args[1]}"))
        else:
            raise ValueError(f"unknown process operation: {command}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
