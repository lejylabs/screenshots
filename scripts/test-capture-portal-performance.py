#!/usr/bin/env python3
"""Serial optimized generated 4K portal/PipeWire observations on private services.

Pins both images; never connects to the user's PipeWire, desktop, bus or audio.
The finite observer separates application processes from source/oracle services.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import statistics
import subprocess
import tempfile
import time

spec = importlib.util.spec_from_file_location("native_observer", Path(__file__).with_name("test-capture-worker-performance.py"))
native = importlib.util.module_from_spec(spec)
spec.loader.exec_module(native)


def descendants(pid):
    pending, found = [pid], set()
    while pending:
        current = pending.pop()
        if current in found:
            continue
        native.require(len(found) < 64, "Private fixture exceeded its process cap")
        found.add(current)
        pending.extend(native.children(current))
    return found


def run_case(test, binary, report, env, app_identity):
    child = subprocess.Popen([str(test), "--exact", "portal_memory_capture_uses_the_authorized_fd_and_exact_serial",
                              "--ignored", "--nocapture", "--test-threads=1"],
                             env=dict(env, SCREENSHOTS_PORTAL_PERFORMANCE="1", SCREENSHOTS_PORTAL_REPORT=str(report),
                                      SCREENSHOTS_PORTAL_BINARY=str(binary)),
                             stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    maxima = {"application": {}, "whole_private_fixture": {}}
    observed, samples = {}, 0
    deadline = time.monotonic() + 120
    try:
        while child.poll() is None:
            native.require(time.monotonic() < deadline, "Private portal measurement deadline exceeded")
            sums = {key: {} for key in maxima}
            for pid in descendants(child.pid):
                sample = native.process_memory(pid)
                if sample is None:
                    continue
                identity, memory = sample
                # A child legitimately execs/renames between stable samples. Its start time,
                # unlike its executable or comm, must stay unchanged during that lifetime.
                native.require(pid not in observed or observed[pid][0] == identity[0], "Observed private PID was reused")
                observed[pid] = identity
                kinds = ["whole_private_fixture"]
                if tuple(identity[2:]) == app_identity:
                    kinds.append("application")
                for kind in kinds:
                    for key, value in memory.items():
                        sums[kind][key] = sums[kind].get(key, 0) + value
            for kind in maxima:
                for key, value in sums[kind].items():
                    maxima[kind][key] = max(maxima[kind].get(key, 0), value)
            samples += 1
            time.sleep(0.005)  # Explicit finite test observer; production has no idle sampler.
        _, errors = child.communicate(timeout=3)
        native.require(child.returncode == 0, f"Private portal benchmark failed: {errors.decode(errors='replace')[-4000:]}")
        for pid, original in observed.items():
            try:
                native.require(native.identity(pid)[0] != original[0], "Private fixture retained a process")
            except (FileNotFoundError, ProcessLookupError):
                pass
        result = json.loads(report.read_text())
        native.require(result["samples"] == 30 and result["exact_generated_pixels"], "Missing exact portal samples")
        native.require(maxima["application"].get("Rss", 0) > 0 and samples > 0, "Application memory sampling unavailable")
        result.update(observed_memory_samples=samples, observer_period_ms=5, identity_verified=True,
                      maximum_observed_near_simultaneous_sum_kib=maxima, observed_process_count=len(observed),
                      observed_processes_gone=True)
        return result
    finally:
        if child.poll() is None:
            child.kill()
            child.communicate(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--test-binary", required=True)
    parser.add_argument("--report", required=True, type=Path)
    args = parser.parse_args()
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="screenshots-portal-measure-") as temporary, tempfile.TemporaryDirectory(prefix="scpw-", dir="/tmp") as fixtures:
        root = Path(temporary)
        binary, test = root / "worker-image", root / "test-image"
        os.link(Path(args.binary).resolve(), binary)
        os.link(Path(args.test_binary).resolve(), test)
        identity = binary.stat()
        app_identity = (identity.st_dev, identity.st_ino)
        env = dict(os.environ, TMPDIR=fixtures)
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS", "AT_SPI_BUS_ADDRESS", "SCREENSHOTS_TASK_CLIPBOARD"):
            env.pop(key, None)
        runs = [run_case(test, binary, root / f"case-{index}.json", env, app_identity) for index in range(3)]
        result = dict(binary_sha256=native.digest(binary), test_sha256=native.digest(test), cases=runs,
                      hardware=native.hardware(), hardware_acceptance=False, elapsed_seconds=time.monotonic()-started,
                      user_services_used=False, executables_pinned_by_hard_link=True,
                      capture_p50_median_ms=statistics.median(run["capture"]["p50_ms"] for run in runs),
                      capture_p95_median_ms=statistics.median(run["capture"]["p95_ms"] for run in runs),
                      cli_including_png_p95_median_ms=statistics.median(run["cli_including_png"]["p95_ms"] for run in runs),
                      comparison="New portal source; no equivalent previous in-memory portal baseline. No PNG-fallback or reference-hardware comparison claimed.",
                      memory_caveat="Sequential near-simultaneous process sums; sampling can miss peaks and double-counts shared RSS. Application includes CLI+native worker; whole fixture also includes source publisher, private PipeWire/D-Bus and PNG oracle. PSS/RSS are not private-footprint guarantees.")
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(result, indent=2)+"\n")
        print(json.dumps({key:value for key,value in result.items() if key not in ("cases",)}, indent=2))


if __name__ == "__main__":
    main()
