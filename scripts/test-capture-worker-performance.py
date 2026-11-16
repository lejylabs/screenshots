#!/usr/bin/env python3
"""Compare inline/isolated 4K capture using only an authenticated private Xvfb.

Needs the optimized screenshots executable and its unit-test executable, Xvfb,
xauth, xdpyinfo and libX11. No user display, input, clipboard or OS registration.
"""
import argparse
import ctypes as c
import hashlib
import json
import os
import platform
from pathlib import Path
import secrets
import shutil
import statistics
import subprocess
import tempfile
import time


def require(value, message):
    if not value:
        raise RuntimeError(message)


def executable(value):
    path = shutil.which(value)
    require(path, f"Executable unavailable: {value}")
    return str(Path(path).resolve())


def digest(path):
    result = hashlib.sha256()
    with open(path, "rb") as file:
        for chunk in iter(lambda: file.read(65536), b""):
            result.update(chunk)
    return result.hexdigest()


def hardware():
    cpu = next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines()
                if line.startswith("model name")), "unavailable")
    memory = {line.split(":", 1)[0]: int(line.split()[1]) for line in Path("/proc/meminfo").read_text().splitlines()
              if line.startswith(("MemTotal:", "SwapTotal:"))}
    governor = Path("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")
    return dict(os=platform.platform(), cpu=cpu, logical_cpus=os.cpu_count(), memory_kib=memory,
                cpu0_governor=governor.read_text().strip() if governor.exists() else "unavailable")


def identity(pid):
    root = Path(f"/proc/{pid}")
    stat = (root / "stat").read_text().rsplit(")", 1)[1].split()
    image = (root / "exe").stat()
    return (int(stat[19]), (root / "comm").read_text().strip(), image.st_dev, image.st_ino)


def process_memory(pid):
    try:
        before = identity(pid)
        values = {}
        for line in Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines():
            if line.startswith(("Rss:", "Pss:", "Private_Clean:", "Private_Dirty:", "Swap:", "SwapPss:")):
                values[line.split(":", 1)[0]] = int(line.split()[1])
        if before != identity(pid):
            return None
        return before, values
    except (FileNotFoundError, ProcessLookupError, PermissionError):
        return None


def children(pid):
    result = set()
    try:
        for thread in Path(f"/proc/{pid}/task").iterdir():
            try:
                result.update(int(value) for value in (thread / "children").read_text().split())
            except (FileNotFoundError, ProcessLookupError):
                pass
    except (FileNotFoundError, ProcessLookupError):
        pass
    return result


def run_case(test, mode, env):
    child = subprocess.Popen([test, "--exact", "capture_worker::tests::native_worker_4k_comparison",
                              "--ignored", "--nocapture", "--test-threads=1"], env=dict(env, SCREENSHOTS_NATIVE_BENCH_MODE=mode),
                             stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    maximum = {key: 0 for key in ("Rss", "Pss", "Private_Clean", "Private_Dirty", "Swap", "SwapPss")}
    observed = {}
    parent_identity = None
    samples = 0
    deadline = time.monotonic() + 90
    try:
        while child.poll() is None:
            require(time.monotonic() < deadline, "Private 4K benchmark exceeded its deadline")
            parent = process_memory(child.pid)
            if parent:
                if parent_identity is None:
                    parent_identity = parent[0]
                require(parent_identity == parent[0], "Benchmark process identity changed")
                group = dict(parent[1])
                for pid in children(child.pid):
                    worker = process_memory(pid)
                    if worker:
                        previous = observed.get(pid)
                        require(previous is None or previous == worker[0], "Sampled child PID was reused")
                        observed[pid] = worker[0]
                        for key, value in worker[1].items():
                            group[key] = group.get(key, 0) + value
                for key, value in group.items():
                    maximum[key] = max(maximum[key], value)
                samples += 1
            time.sleep(0.005)  # Only the finite observer samples; production has no idle sampler.
        output, errors = child.communicate(timeout=3)
        require(child.returncode == 0, f"Private benchmark failed: {errors.decode(errors='replace')}")
        values = []
        for line in output.decode().splitlines():
            start = line.find('{')
            if start >= 0:
                try:
                    value = json.loads(line[start:])
                    if value.get("mode") == mode:
                        values.append(value)
                except json.JSONDecodeError:
                    pass
        require(len(values) == 1 and values[0]["samples"] == 100, "Missing exact benchmark result")
        require(samples > 0 and parent_identity is not None, "Process memory observation was unavailable")
        for pid, original in observed.items():
            try:
                require(identity(pid) != original, "Native benchmark retained a child process")
            except (FileNotFoundError, ProcessLookupError):
                pass
        result = values[0]
        result.update(observed_memory_samples=samples, maximum_observed_near_simultaneous_sum_kib=maximum,
                      observed_child_count=len(observed), parent_identity=parent_identity,
                      identity_verified=True, observer_period_ms=5)
        return result
    finally:
        if child.poll() is None:
            child.kill()
            child.communicate(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--test-binary", required=True)
    parser.add_argument("--xvfb", required=True)
    parser.add_argument("--report", required=True, type=Path)
    args = parser.parse_args()
    binary, test, xvfb = executable(args.binary), executable(args.test_binary), executable(args.xvfb)
    xauth, xdpyinfo = executable("xauth"), executable("xdpyinfo")
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="screenshots-native-4k-") as temporary:
        root = Path(temporary)
        worker, runner = root / "worker-image", root / "test-image"
        # Pin both exact images without copying binaries; TMPDIR must share their filesystem.
        os.link(binary, worker)
        os.link(test, runner)
        worker_hash, runner_hash = digest(worker), digest(runner)
        display = next((f":{number}" for number in range(220, 280) if not Path(f"/tmp/.X{number}-lock").exists()
                        and not Path(f"/tmp/.X11-unix/X{number}").exists()), None)
        require(display, "No unused private display")
        env = dict(os.environ)
        for key in ("WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS", "AT_SPI_BUS_ADDRESS"):
            env.pop(key, None)
        state = ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME")
        env.update({key: str(root / key) for key in state})
        auth = root / "authority"
        env.update(DISPLAY=display, XAUTHORITY=str(auth), XDG_SESSION_TYPE="x11", MESA_SHADER_CACHE_DISABLE="true",
                   SCREENSHOTS_NATIVE_BENCH_PRIVATE="1", SCREENSHOTS_NATIVE_BENCH_BINARY=str(worker))
        subprocess.run([xauth, "-f", str(auth), "add", display, ".", secrets.token_hex(16)], env=env,
                       check=True, capture_output=True, timeout=5)
        with (root / "xvfb.log").open("wb") as log:
            server = subprocess.Popen([xvfb, display, "-screen", "0", "3840x2160x24", "-auth", str(auth),
                                       "-nolisten", "tcp", "-noreset"], env=env, stdout=log, stderr=subprocess.STDOUT)
        connection = None
        try:
            for _ in range(100):
                require(server.poll() is None, "Private 4K Xvfb exited")
                if subprocess.run([xdpyinfo, "-display", display], env=env, capture_output=True, timeout=2).returncode == 0:
                    break
                time.sleep(0.05)
            else:
                raise RuntimeError("Private 4K Xvfb startup deadline")
            x = c.CDLL("libX11.so.6")
            for name, inputs, output in [("XOpenDisplay", [c.c_char_p], c.c_void_p),
                ("XDefaultScreen", [c.c_void_p], c.c_int), ("XRootWindow", [c.c_void_p, c.c_int], c.c_ulong),
                ("XSetWindowBackground", [c.c_void_p, c.c_ulong, c.c_ulong], c.c_int),
                ("XClearWindow", [c.c_void_p, c.c_ulong], c.c_int), ("XSync", [c.c_void_p, c.c_int], c.c_int),
                ("XCloseDisplay", [c.c_void_p], c.c_int)]:
                function = getattr(x, name)
                function.argtypes, function.restype = inputs, output
            previous = os.environ.get("XAUTHORITY")
            os.environ["XAUTHORITY"] = str(auth)
            try:
                connection = x.XOpenDisplay(display.encode())
            finally:
                if previous is None:
                    os.environ.pop("XAUTHORITY", None)
                else:
                    os.environ["XAUTHORITY"] = previous
            require(connection, "Cannot open own authenticated Xvfb")
            desktop = x.XRootWindow(connection, x.XDefaultScreen(connection))
            x.XSetWindowBackground(connection, desktop, 0x111D47)
            x.XClearWindow(connection, desktop)
            x.XSync(connection, 0)
            listed = subprocess.run([worker, "list", "displays"], env=env, check=True, capture_output=True, timeout=10)
            displays = json.loads(listed.stdout)
            require(len(displays) == 1 and (displays[0]["width"], displays[0]["height"]) == (3840, 2160), "Unexpected private 4K display")
            env["SCREENSHOTS_NATIVE_BENCH_DISPLAY"] = displays[0]["id"]
            cases = [run_case(str(runner), mode, env) for mode in ("inline", "worker", "worker", "inline", "inline", "worker")]
            p50 = {mode: statistics.median(case["p50_ms"] for case in cases if case["mode"] == mode) for mode in ("inline", "worker")}
            p95 = {mode: statistics.median(case["p95_ms"] for case in cases if case["mode"] == mode) for mode in ("inline", "worker")}
            require(all(not (root / key).exists() for key in state), "Benchmark created user-state directories")
            require(digest(worker) == worker_hash and digest(runner) == runner_hash, "Pinned benchmark images changed")
            report = {"worker_sha256": worker_hash, "test_sha256": runner_hash, "width":3840,"height":2160,
                      "modes_p50_median_ms":p50,"modes_p95_median_ms":p95,
                      "relative_p95_change_percent":(p95["worker"]/p95["inline"]-1)*100,
                      "cases":cases,"elapsed_seconds":time.monotonic()-started,
                      "hardware":hardware(),
                      "memory_caveat":"smaps sums are sequential near-simultaneous observations; shared RSS is counted in each process, sampling can miss a peak. Rusage parent/child maxima are independent, not simultaneous or private-footprint measurements.",
                      "hardware_acceptance":False,"source":"private authenticated TCP-disabled Xvfb; generated pixels only",
                      "user_state_unchanged":True,"executables_pinned_by_hard_link":True}
            args.report.parent.mkdir(parents=True, exist_ok=True)
            args.report.write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps({key: report[key] for key in ("worker_sha256","test_sha256","modes_p50_median_ms","modes_p95_median_ms","relative_p95_change_percent","elapsed_seconds")}, indent=2))
        finally:
            if connection:
                x.XCloseDisplay(connection)
            server.terminate()
            try:
                server.wait(timeout=3)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=3)


if __name__ == "__main__":
    main()
