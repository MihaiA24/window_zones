"""Fixed-oracle observations, scenarios, performance measurements and reports."""
import argparse
import contextlib
import json
import math
import os
from pathlib import Path
import re
import statistics
import tarfile
import sys
import uuid

from .runtime import CHECK_EPOCH, VALID, ZONES, digest, fail, load, save
from .journeys import ANSI, session, smoke


def differences(expected, actual, path="$"):
    if type(expected) is type(actual) and isinstance(expected, dict):
        result = []
        for key in sorted(expected.keys() | actual.keys()):
            if key not in expected or key not in actual:
                result.append({"path": f"{path}.{key}", "expected": expected.get(key, {"missing": True}),
                               "actual": actual.get(key, {"missing": True})})
            else:
                result.extend(differences(expected[key], actual[key], f"{path}.{key}"))
        return result
    if type(expected) is type(actual) and isinstance(expected, list):
        result = []
        for index in range(max(len(expected), len(actual))):
            if index >= len(expected) or index >= len(actual):
                result.append({"path": f"{path}[{index}]", "expected": expected[index] if index < len(expected) else {"missing": True},
                               "actual": actual[index] if index < len(actual) else {"missing": True}})
            else:
                result.extend(differences(expected[index], actual[index], f"{path}[{index}]"))
        return result
    return [] if type(expected) is type(actual) and expected == actual else [
        {"path": path, "expected": expected, "actual": actual}
    ]


def dispatch_observation(code, text, config):
    def field(pattern):
        match = re.search(pattern, text, re.MULTILINE)
        return match.group(1) if match else None
    count = field(r"^  binding count: (\d+)$")
    geometry = re.search(r"^  last move: x=(-?\d+) y=(-?\d+) w=(\d+) h=(\d+)$", text, re.MULTILINE)
    config_path = field(r"^  config path: (.+)$")
    return {"exit_code": code, "backend": field(r"^Using runtime window backend: (.+)$"),
            "config_path": "$CONFIG" if config_path == str(config) else config_path,
            "config_state": field(r"^  config state: (\w+)"), "bindings": int(count) if count else None,
            "dispatch": field(r"^Dispatch state: (\w+)"),
            "error": field(r"^Dispatch state: Error\((\w+)"),
            "geometry": list(map(int, geometry.groups())) if geometry else None}


def check_seed(workspace, seed, zone=None):
    workspace.set_config(seed, zone)
    if workspace.config.exists():
        os.utime(workspace.config, (CHECK_EPOCH, CHECK_EPOCH))


def clock_environment(workspace, prefix, golden):
    library, probe = workspace.scratch / "fixed-clock.so", workspace.scratch / "clock-probe"
    for flags, destination, name in ((["-shared", "-fPIC"], library, "clock-build"),
                                     (["-DCLOCK_PROBE"], probe, "probe-build")):
        code, _ = workspace.run(["cc", "-O2", "-Wall", "-Wextra", "-Werror", *flags, str(workspace.fixed_clock), "-o", str(destination)],
                      f"{prefix}/{name}")
        if code:
            fail("Clock helper did not compile. Install a working C compiler and inspect clock-build/probe-build.stderr.")
    fixed = {"LD_PRELOAD": str(library), "TZ": "UTC", "LC_ALL": "C.UTF-8", "PYTHONHASHSEED": "0",
             "SOURCE_DATE_EPOCH": str(CHECK_EPOCH)}
    code, text = workspace.run([str(probe)], f"{prefix}/clock-probe", environment={**workspace.env(), **fixed})
    observed = json.loads(text) if code == 0 else {"exit_code": code}
    delta = differences(golden["clock"], observed, "$.clock")
    if delta:
        fail(f"Fixed clock probe mismatched: {json.dumps(delta)}. Inspect clock-probe.stdout; do not run unfrozen checks.")
    return fixed, observed


def check_behaviors(workspace, prefix):
    result = {}
    cases = [(zone, "valid", zone, "Alt+Ctrl+Left") for zone in ZONES]
    cases += [("custom", "custom", None, "Alt+Ctrl+Left"), ("next-display", "valid", None, "Alt+Ctrl+Down"),
              ("previous-display", "valid", None, "Alt+Ctrl+Shift+Right"), ("unbound", "valid", None, "Alt+Ctrl+F12")]
    cases += [(seed, seed, None, "Alt+Ctrl+Left") for seed in ("empty", "missing", "invalid")]
    for flag in ("backend", "alias"):
        for discovery in (False, True):
            variant = f"{flag}-{'discovery' if discovery else 'explicit'}"
            result[variant] = {}
            options = argparse.Namespace(backend_flag=flag, discovery=discovery)
            for name, seed, zone, hotkey in cases:
                check_seed(workspace, seed, zone)
                code, text = workspace.run(workspace.app_args(options, ["dispatch", hotkey]), f"{prefix}/{variant}-{name}")
                result[variant][name] = dispatch_observation(code, text, workspace.config)
    check_seed(workspace, "valid")
    return result


def check_sessions(workspace, prefix, golden):
    expected, observed = {}, {}
    for mode in ("run", "tui"):
        for flag_index, flag in enumerate(("backend", "alias")):
            for discovery in (False, True):
                ending = ("quit", "q", "exit", "eof")[2 * flag_index + int(discovery)]
                for journey in ("reload", "watcher", "cancel"):
                    name = f"{mode}-{flag}-{'discovery' if discovery else 'explicit'}-{journey}-{ending}"
                    expected[name] = {"persisted_dispatch": golden["dispatch"], "config_unchanged": True}
                    if workspace.doctor():
                        fail(f"Health check failed before {name}; inspect doctor before driving again.")
                    check_seed(workspace, "valid")
                    before = digest(workspace.config)
                    options = argparse.Namespace(mode=mode, backend_flag=flag, discovery=discovery,
                                                 journey=journey, command=[], expect=[], quit=ending)
                    if journey == "cancel":
                        options.command = ["status"]
                        options.expect = ["Config:" if mode == "tui" else "binding count: 5"]
                    workspace.event("session-case", name=name, options=vars(options))
                    session(workspace, options)
                    code, text = workspace.run(workspace.app_args(options, ["dispatch", "Alt+Ctrl+Left"]), f"{prefix}/{name}-persisted")
                    observed[name] = {"persisted_dispatch": dispatch_observation(code, text, workspace.config),
                                      "config_unchanged": digest(workspace.config) == before}
    return expected, observed


def check_e2e(workspace, prefix):
    native = smoke(workspace, argparse.Namespace(gate="x11"))
    assertions = (native / "assertions.log").read_text()
    native_run = (native / "x11/run.log").read_text()
    tui = ANSI.sub("", (native / "x11-x11-tui.log").read_text())
    geometry = {}
    for match in re.finditer(r"^PASS ([^:\n]+): expected '[^']*' observed '(-?\d+),(-?\d+),(\d+),(\d+)'", assertions, re.MULTILINE):
        name = match.group(1)
        if name in ("x11-left-half", "x11-center-third", "x11-left-two-thirds", "x11-next-display",
                    "x11-previous-display", "X11 real hotkey moves window"):
            geometry[name] = list(map(int, match.groups()[1:]))
    summary = re.search(r"Smoke assertions: \d+; failures: (\d+)", assertions)
    after_error = native_run.partition("Config reload error:")[2]
    retained = re.search(r"binding count: (\d+)", after_error)
    action = re.findall(r"last action: (alt\+ctrl\+\w+)", native_run)
    tui_action = re.findall(r"Last action: (alt\+ctrl\+\w+)", tui)
    captures = sorted(p.name for p in native.glob("*.png")
                      if p.name in {"x11-x11-left-half.png", "x11-x11-center-third.png", "x11-x11-left-two-thirds.png",
                                    "x11-x11-next-display.png", "x11-x11-previous-display.png", "x11-x11-real-hotkey.png"}
                      and p.read_bytes()[:8] == b"\x89PNG\r\n\x1a\n")
    # A new user-facing process must read the file the native runtime left on disk.
    persisted = native / "x11/config.toml"
    code, text = workspace.run([str(workspace.root / "scripts/run.sh"), "--backend", "dry-run", "--config", str(persisted),
                      "dispatch", "Alt+Ctrl+Left"], f"{prefix}/persisted-dispatch")
    before_cancel = workspace.config.read_bytes()
    session(workspace, argparse.Namespace(mode="tui", backend_flag="backend", discovery=False, journey="lifecycle",
                               command=["status"], expect=["Config:"], quit="eof"))
    return {"failures": int(summary.group(1)) if summary else None, "geometry": geometry, "captured_png": captures,
            "native_backend": "x11" if "Window backend: x11" in native_run else None,
            "hotkey_action": action[-1] if action else None, "retained_binding_count": int(retained.group(1)) if retained else None,
            "config_recovered": "Config state: Loaded" in after_error,
            "tui_action": tui_action[-1] if tui_action else None, "run_closed": "Session closed." in native_run,
            "tui_closed": "Session closed." in tui, "cancel_config_unchanged": workspace.config.read_bytes() == before_cancel,
            "persisted_dispatch": dispatch_observation(code, text, persisted)}


def measurement_summary(samples):
    return {"runs": len(samples), "median_ms": statistics.median(samples) / 1_000_000,
            "min_ms": min(samples) / 1_000_000, "max_ms": max(samples) / 1_000_000,
            "spread_ms": (max(samples) - min(samples)) / 1_000_000}


def check_performance(workspace, options, prefix, golden):
    if options.runs < 5:
        fail("--runs must be at least 5 per side. Use --runs 5 or more.")
    if not math.isfinite(options.max_regression_percent) or options.max_regression_percent < 0:
        fail("--max-regression-percent must be a finite nonnegative budget, such as 25.")
    code, text = workspace.run(["git", "rev-parse", "--verify", "--end-of-options", options.baseline_ref + "^{commit}"],
                     f"{prefix}/baseline-ref")
    commit = text.strip()
    if code or not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", commit):
        fail(f"Cannot resolve baseline ref {options.baseline_ref!r}. Supply an existing local commit, branch or tag; no fetch is performed.")
    source = workspace.scratch / ("baseline-" + uuid.uuid4().hex[:8])
    source.mkdir()
    archive_path = source / "source.tar"
    code, _ = workspace.run(["git", "archive", "--format=tar", "--output", str(archive_path), commit], f"{prefix}/baseline-archive")
    if code:
        fail("Baseline archive failed. Inspect baseline-archive.stderr; the working tree and git refs were not changed.")
    with tarfile.open(archive_path) as archive:
        archive.extractall(source, filter="data")
    archive_path.unlink()
    target = source / "target"
    # Compilation is outside the sample window and must use the real toolchain/cache clock.
    build_env = dict(os.environ, CARGO_TARGET_DIR=str(target), CARGO_NET_OFFLINE="true",
                     WINDOW_ZONES_VERIFY_CHECKOUT=str(workspace.root))
    build_env.pop("LD_PRELOAD", None)
    code, _ = workspace.run(["cargo", "build", "--locked", "--manifest-path", str(source / "Cargo.toml"), "--bin", "window_zones"],
                  f"{prefix}/baseline-build", timeout=300, environment=build_env)
    if code:
        fail("Baseline build failed. Inspect baseline-build.stderr; make its locked dependencies available locally, then relaunch. No sample was counted.")
    baseline_binary = target / "debug/window_zones"
    baseline_hash = digest(baseline_binary)
    commands = {
        "baseline": ([str(source / "scripts/run.sh"), "--backend", "dry-run", "--config", str(workspace.config),
                      "dispatch", "Alt+Ctrl+Left"], {**workspace.env(), "WINDOW_ZONES_BINARY": str(baseline_binary)}),
        "candidate": (workspace.app_args(argparse.Namespace(backend_flag="backend", discovery=False), ["dispatch", "Alt+Ctrl+Left"]), workspace.env()),
    }
    samples = {"baseline": [], "candidate": []}
    observations, delta = [], []
    for round_number in range(options.runs + 1):
        for side in ("baseline", "candidate"):
            label = f"{prefix}/{side}-{'warmup' if round_number == 0 else round_number}"
            elapsed = []
            command, environment = commands[side]
            code, text = workspace.run(command, label, environment=environment, timings=elapsed)
            observed = dispatch_observation(code, text, workspace.config)
            trial_delta = differences(golden["dispatch"], observed, f"$.{side}[{round_number}].dispatch")
            delta.extend(trial_delta)
            observations.append({"side": side, "round": round_number, "warmup": round_number == 0,
                                 "elapsed_ns": elapsed[0], "work_confirmed": not trial_delta,
                                 "result": observed, "stdout": str(workspace.artifact() / (label + ".stdout")),
                                 "stderr": str(workspace.artifact() / (label + ".stderr"))})
            if round_number:
                samples[side].append(elapsed[0])
    summary = {side: measurement_summary(values) for side, values in samples.items()}
    baseline_median = statistics.median(samples["baseline"])
    candidate_median = statistics.median(samples["candidate"])
    maximum = baseline_median * (1 + options.max_regression_percent / 100)
    if candidate_median > maximum:
        delta.append({"path": "$.candidate.median_ms", "expected": {"maximum": maximum / 1_000_000},
                      "actual": candidate_median / 1_000_000})
    if digest(baseline_binary) != baseline_hash:
        delta.append({"path": "$.baseline.binary_sha256", "expected": baseline_hash, "actual": digest(baseline_binary)})
    return {"dispatch": golden["dispatch"], "runs_per_side": options.runs,
            "max_regression_percent": options.max_regression_percent}, {
                "baseline_ref": options.baseline_ref, "baseline_commit": commit, "baseline_binary_sha256": baseline_hash,
                "metric": "debug process startup + dispatch through each revision's scripts/run.sh; pidfd completion, capture and ownership overhead included",
                "excluded": ["compilation", "one confirmed warmup per side", "result parsing and comparison"],
                "summary": summary, "regression_percent": (candidate_median / baseline_median - 1) * 100,
                "samples": observations, "work_confirmed": all(trial["work_confirmed"] for trial in observations),
                "fixed_input_unchanged": workspace.config.read_text() == VALID and workspace.config.stat().st_mtime_ns == CHECK_EPOCH * 1_000_000_000,
            }, delta


def check_command(workspace, options):
    label = f"{options.scenario}-{uuid.uuid4().hex[:8]}"
    folder = workspace.base / "check-errors" / label
    report = {"scenario": options.scenario, "status": "error", "difference": [],
              "fixture": {"seed": 0, "wall_time_epoch": CHECK_EPOCH, "timezone": "UTC",
                          "monotonic_clock": "real for deadlines and measurements; not a golden value"}}
    owned = False
    guard = contextlib.ExitStack()
    original_env = dict(os.environ)
    try:
        if options.dry_run:
            print(json.dumps({**report, "status": "planned", "evidence_path": None, "options": vars(options)}))
            return 0
        guard.enter_context(workspace.lock())
        with open(os.devnull, "w") as quiet, contextlib.redirect_stdout(quiet):
            workspace.require_ready()
            owned = True
            folder = workspace.artifact() / "checks" / label
            folder.mkdir(parents=True)
            prefix = str(folder.relative_to(workspace.artifact()))
            golden_path = Path(options.expected_file).resolve()
            golden = load(golden_path)
            report.update(build=load(workspace.stamp), expected_file=str(golden_path), expected_sha256=digest(golden_path))
            fixed, observed_clock = clock_environment(workspace, prefix, golden)
            report["clock_probe"] = observed_clock
            os.environ.update(fixed)
            check_seed(workspace, "valid")
            if options.scenario == "behaviors":
                observed = check_behaviors(workspace, prefix)
                expected = {variant: {name: {**golden["dispatch"], **overrides}
                                      for name, overrides in golden["behaviors"].items()}
                            for variant in ("backend-explicit", "backend-discovery", "alias-explicit", "alias-discovery")}
                delta = differences(expected, observed)
            elif options.scenario == "sessions":
                expected, observed = check_sessions(workspace, prefix, golden)
                delta = differences(expected, observed)
            elif options.scenario == "e2e":
                expected, observed = golden["e2e"], check_e2e(workspace, prefix)
                delta = differences(expected, observed)
            else:
                expected, observed, delta = check_performance(workspace, options, prefix, golden)
                if not observed["work_confirmed"] or not observed["fixed_input_unchanged"]:
                    delta.append({"path": "$.confirmed_fixed_work", "expected": True,
                                  "actual": observed["work_confirmed"] and observed["fixed_input_unchanged"]})
            report.update(expected=expected, actual=observed, difference=delta)
            if workspace.build_status() != "current":
                report["difference"].append({"path": "$.build", "expected": "current", "actual": workspace.build_status()})
            report["status"] = "fail" if report["difference"] else "pass"
            workspace.event("check-result", scenario=options.scenario, status=report["status"], report=str(folder / "report.json"))
    except (Exception, KeyboardInterrupt) as error:
        report["status"] = "error"
        report["difference"].append({"path": "$.execution", "expected": "completed", "actual": str(error)})
    finally:
        os.environ.clear()
        os.environ.update(original_env)
        if owned:
            try:
                with contextlib.redirect_stdout(sys.stderr):
                    if report["status"] == "pass":
                        check_seed(workspace, "valid")
                    else:
                        workspace.cleanup(False)
            except (RuntimeError, OSError) as error:
                report["status"] = "error"
                report["difference"].append({"path": "$.cleanup", "expected": "restored or removed scratch", "actual": str(error)})
        guard.close()
    report["evidence_path"] = str(folder / "report.json")
    save(folder / "report.json", report)
    summary = {key: report[key] for key in ("scenario", "status", "difference", "evidence_path")}
    if options.scenario == "performance" and "actual" in report:
        summary["measurements"] = {key: value for key, value in report["actual"].items() if key != "samples"}
    print(json.dumps(summary, indent=2))
    return 0 if report["status"] == "pass" else 1
