#!/usr/bin/env python3
"""Read a completed TUI response strictly after a command's log checkpoint."""

from pathlib import Path
import sys
import time

HEADER = "\x1b[2J\x1b[HWindow Zones TUI\n"
PROMPT = "window-zones tui> "


def completed_frames(text):
    return [
        "Window Zones TUI\n" + frame.split(PROMPT, 1)[0] + PROMPT
        for frame in text.split(HEADER)[1:]
        if PROMPT in frame
    ]


def frame_matches(frame, expected):
    if expected.startswith("Last action: "):
        wanted = sorted(expected.removeprefix("Last action: ").split("+"))
        return any(
            sorted(line.removeprefix("Last action: ").split("+")) == wanted
            for line in frame.splitlines()
            if line.startswith("Last action: ")
        )
    return expected in frame


def wait_frame(path, offset, expected, timeout):
    deadline = time.monotonic() + timeout
    latest = ""
    while True:
        with Path(path).open("rb") as stream:
            stream.seek(offset)
            frames = completed_frames(stream.read().decode("utf-8", errors="replace"))
        if frames:
            latest = frames[-1]
            if frame_matches(latest, expected):
                return latest, True
        if time.monotonic() >= deadline:
            return latest, False
        time.sleep(0.1)


if __name__ == "__main__":
    frame, matched = wait_frame(Path(sys.argv[1]), int(sys.argv[2]), sys.argv[3], float(sys.argv[4]))
    print(frame, end="")
    sys.exit(0 if matched else 1)
