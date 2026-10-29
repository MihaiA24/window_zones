"""Checkout-owned Window Zones verification; Python stdlib, repository runners."""
import argparse
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

from .runtime import SEEDS, ZONES, Workspace, fail, load
from .journeys import session, smoke
from .checks import check_command


def parser(workspace):
    p = argparse.ArgumentParser(description=__doc__, epilog="Examples: verify launch; verify doctor; verify drive session --mode tui; "
                                "verify drive smoke x11; verify logs; verify cleanup --dry-run; verify cleanup",
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = p.add_subparsers(dest="action", required=True)
    commands.add_parser("doctor", help="Read-only ownership, process, display/port and source/build freshness report.")
    start = commands.add_parser("launch", help="Build both Rust binaries, create an isolated seeded profile and prove usable status.")
    start.add_argument("--dry-run", action="store_true", help="Print paths/build plan without creating files or starting processes.")
    logs = commands.add_parser("logs", help="Show evidence location and the action transcript; survives cleanup.")
    logs.add_argument("--path-only", action="store_true", help="Print only the latest evidence directory.")
    clean = commands.add_parser("cleanup", help="Stop only an owned recorded PID; remove scratch state, retain all evidence.")
    clean.add_argument("--dry-run", action="store_true", help="List exact PID/path effects without changing anything.")
    drive = commands.add_parser("drive", help="Drive real CLI/TUI user input or the repository's compositor smoke runner.")
    drivers = drive.add_subparsers(dest="driver", required=True)
    config = drivers.add_parser("config", help="Edit only the disposable user's TOML, recording before/after content.")
    config.add_argument("seed", choices=SEEDS)
    config.add_argument("--zone", choices=ZONES, help="Set the Left binding to any built-in zone; valid seed only.")
    cli = drivers.add_parser("cli", help="Run one CLI command through scripts/run.sh; arguments follow --.",
                             epilog="Example: verify drive cli --expect 'last move: x=0 y=0 w=960 h=1080' -- dispatch Alt+Ctrl+Left")
    cli.add_argument("--expect", action="append", default=[], help="Required literal output; repeat for multiple observations.")
    cli.add_argument("--expect-exit", type=int, default=0, help="Expected process exit (config/dispatch errors can still exit 0).")
    cli.add_argument("arguments", nargs=argparse.REMAINDER, help="status, dispatch HOTKEY, --help, or parser-error arguments; use session for run/tui.")
    interactive = drivers.add_parser("session", help="New isolated PTY; wait for actual prompts/status, capture every input and result.",
                                    epilog="Default lifecycle: status, TUI refresh, dispatch, reload, restart, quit. "
                                    "--journey reload edits valid/invalid/empty/missing configs and verifies retention/recovery; "
                                    "--journey watcher performs those edits without explicit reload. "
                                    "--journey starter verifies all 18 starter bindings and window history in run mode; "
                                    "--journey read-error verifies directory-read errors, identical recovery and missing-file recovery. "
                                    "Custom: --command 'dispatch Alt+Ctrl+Left' --expect 'Last action: alt+ctrl+left'.")
    interactive.add_argument("--mode", choices=("run", "tui"), default="tui")
    interactive.add_argument("--journey", choices=("lifecycle", "reload", "watcher", "starter", "read-error"), default="lifecycle")
    interactive.add_argument("--command", action="append", default=[], help="Literal user line; pair each with --expect. Overrides the built-in journey.")
    interactive.add_argument("--expect", action="append", default=[],
                             help="Literal result for the paired command. TUI dispatch also requires completed success; "
                                  "intentional failures must expect 'Last error: dispatch: …'.")
    interactive.add_argument("--quit", choices=("quit", "q", "exit", "eof"), default="quit", help="Exit/cancel entry point, including terminal Ctrl-D.")
    native = drivers.add_parser("smoke", help="Run existing synthetic compositor gate, keeping assertions, logs and screenshots.",
                               epilog="gnome is seatless; kde-live is nested despite its name. Neither proves seated accelerator behavior. "
                               "gnome-live is deliberately excluded: it attaches to a shared user session.")
    native.add_argument("gate", choices=("x11", "gnome", "kde-live"))
    drivers.add_parser("tests", help="Run the repository's existing scripts/test.sh, not a parallel test framework.")
    for child in (config, cli, interactive, native, drivers.choices["tests"]):
        child.add_argument("--dry-run", action="store_true", help="Print the planned drive; do not mutate config, move windows, build, or start processes.")
    for child in (start, cli, interactive):
        child.add_argument("--backend-flag", choices=("backend", "alias"), default="backend",
                           help="Exercise --backend dry-run or the app's --dry-run alias (separate from verifier --dry-run).")
        child.add_argument("--discovery", action="store_true", help="Exercise XDG config discovery instead of explicit --config.")
    checks = commands.add_parser("check", help="Fixed-input behavioral and native end-to-end checks; JSON differences and retained proof.")
    scenarios = checks.add_subparsers(dest="scenario", required=True)
    for name, help_text in (("behaviors", "Compare eight built-in zones, a custom zone, next/previous display moves and error states through both CLI flag/config variants."),
                            ("sessions", "Run the fixed CLI/TUI reload, watcher and cancel matrix with all flag/config and quit variants."),
                            ("e2e", "Reuse the native X11 smoke gate; compare observed geometry, real hotkey, reload, persistence and cancel."),
                            ("performance", "Alternate baseline/candidate startup+dispatch samples; compare confirmed work and a median regression budget.")):
        child = scenarios.add_parser(name, help=help_text)
        child.add_argument("--expected-file", default=str(workspace.expected), help="Reviewed literal result oracle; overrides are hashed into the report. Never bless actual output.")
        child.add_argument("--dry-run", action="store_true", help="Print a JSON plan without building, seeding, running or deleting anything.")
        if name == "performance":
            child.add_argument("--baseline-ref", required=True, help="Existing local git ref; resolved to a commit, archived and built separately. No checkout, fetch or ref mutation.")
            child.add_argument("--runs", type=int, default=5, help="Measured runs PER SIDE, at least 5; baseline/candidate alternate. One extra warmup each is excluded.")
            child.add_argument("--max-regression-percent", type=float, default=25, help="Fail when candidate median exceeds baseline by this percent (default 25); spread is max minus min.")
    return p


def main():
    workspace = Workspace(Path(__file__).resolve().parents[4])
    options = parser(workspace).parse_args()
    try:
        if options.action == "check":
            return check_command(workspace, options)
        if options.action == "doctor":
            return workspace.doctor()
        if options.action == "logs":
            folders = sorted(workspace.evidence.glob("*"))
            if not folders:
                fail("No evidence yet. Run launch, then a drive command.")
            folder = workspace.artifact() if workspace.state(False) else folders[-1]
            print(folder)
            if not options.path_only:
                print((folder / "actions.jsonl").read_text())
            return 0
        if options.action == "cleanup" and options.dry_run:
            workspace.cleanup(True)
            return 0
        if getattr(options, "dry_run", False):
            workspace.state(False)
            print(json.dumps({"plan": vars(options), "checkout": str(workspace.root), "profile": workspace.profile,
                              "scratch": str(workspace.scratch), "display": f":{workspace.display}", "tcp_ports": [],
                              "runner": str(workspace.root / "scripts/run.sh"), "build": "cargo build --locked --bins",
                              "evidence_root": str(workspace.evidence)}, indent=2))
            return 0
        with workspace.lock():
            if options.action == "cleanup":
                workspace.cleanup(options.dry_run)
            elif options.action == "launch":
                workspace.launch(options)
            else:
                workspace.require_ready()
                before = workspace.snapshot()
                workspace.event("drive-start", options=vars(options), build=load(workspace.stamp), before=before)
                try:
                    if options.driver == "config":
                        workspace.set_config(options.seed, options.zone)
                    elif options.driver == "cli":
                        args = options.arguments
                        if args and args[0] == "--":
                            args = args[1:]
                        if not args or any(a in ("run", "tui", "--config", "--backend") for a in args):
                            fail("Use drive session for run/tui, --discovery for discovery, and --backend-flag for safe backend variants. See drive cli --help.")
                        rc, text = workspace.run(workspace.app_args(options, args), "cli-" + str(time.time_ns()))
                        if rc != options.expect_exit or any(value not in text for value in options.expect):
                            fail(f"CLI result did not match exit {options.expect_exit} / {options.expect}. Inspect stdout/stderr; do not weaken the assertion.")
                    elif options.driver == "session":
                        session(workspace, options)
                    elif options.driver == "smoke":
                        smoke(workspace, options)
                    elif options.driver == "tests":
                        # Test/dev feature unification must not replace the pinned app.
                        rc, _ = workspace.run([str(workspace.root / "scripts/test.sh")], "tests-" + str(time.time_ns()), timeout=600,
                                    environment=dict(os.environ, WINDOW_ZONES_VERIFY_CHECKOUT=str(workspace.root), CARGO_TARGET_DIR=str(workspace.root / "target" / "verification-tests")))
                        if rc:
                            fail("Repository tests failed. Inspect the test logs and fix the reported failure.")
                    if workspace.build_status() != "current":
                        fail("Build became stale during the drive. This is not evidence; cleanup and launch again.")
                    workspace.event("drive-result", result="PASS", after=workspace.snapshot())
                    print(f"PASS {options.driver}\nEvidence: {workspace.artifact()}")
                except BaseException as error:
                    workspace.event("drive-result", result="FAIL", error=str(error), after=workspace.snapshot())
                    workspace.cleanup(False)
                    raise
        return 0
    except (RuntimeError, OSError, subprocess.SubprocessError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1


def entry_point():
    def interrupted(signum, frame):
        raise KeyboardInterrupt(f"signal {signum}")
    signal.signal(signal.SIGTERM, interrupted)
    try:
        return main()
    except KeyboardInterrupt:
        print("Interrupted. Owned child teardown ran; run cleanup to remove any remaining profile. Evidence is retained.", file=sys.stderr)
        return 130

