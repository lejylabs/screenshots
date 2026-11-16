#!/usr/bin/env python3
"""Private executable scheduling fixtures; no desktop, clipboard or credential access."""
import argparse
import ctypes as c
import shutil
import secrets
import hashlib
import json
import os
import platform
from pathlib import Path
import selectors
import signal
import socketserver
import struct
import subprocess
import sys
import tempfile
import threading
import time
import zlib
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler


def require(value, message):
    if not value:
        raise RuntimeError(message)


def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xffffffff)


def png():
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 2, 1, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(b"\0\x11\x22\x33\xff\x44\x55\x66\x80")) + chunk(b"IEND", b""))


def utc(seconds):
    return (datetime.now(timezone.utc) + timedelta(seconds=seconds)).isoformat(timespec="milliseconds").replace("+00:00", "Z")


class Fixture:
    def __init__(self, binary, root):
        self.binary, self.root = binary, root
        self.env = dict(os.environ)
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS", "SCREENSHOTS_TASK_CLIPBOARD_BRIDGE"):
            self.env.pop(name, None)
        # Only this fixture's private application directories are available.
        for name in ("XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "APPDATA", "LOCALAPPDATA", "USERPROFILE"):
            self.env[name] = str(root / name)
        self.children = []
        (root / "input.png").write_bytes(png())

    def command(self, *args, success=True):
        result = subprocess.run([self.binary, "schedule", *map(str, args)], env=self.env,
                                capture_output=True, timeout=10)
        require((result.returncode == 0) == success, f"Unexpected scheduler exit: {result.stderr.decode(errors='replace')}")
        return json.loads(result.stdout) if success else result

    def profile(self, name, actions=None):
        path = self.root / (name + ".json")
        path.write_text(json.dumps({"version": 1, "source": {"type": "file", "path": "input.png"},
                                   "actions": actions or [{"type": "save", "path": name + "-{uuid}.png"}]}))
        return path

    def start(self, database):
        child = subprocess.Popen([self.binary, "schedule", "serve", str(database)], env=self.env,
                                 stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.children.append(child)
        selector = selectors.DefaultSelector()
        try:
            selector.register(child.stdout, selectors.EVENT_READ)
            require(selector.select(8), "Scheduler did not publish readiness")
            line = child.stdout.readline()
            if not line:
                raise RuntimeError("Scheduler failed before readiness: " + child.stderr.read().decode(errors="replace"))
            require(json.loads(line)["type"] == "schedule_ready", "Unexpected scheduler readiness")
        finally:
            selector.close()
        return child

    def stop(self, child):
        child.send_signal(signal.SIGTERM)
        stdout, stderr = child.communicate(timeout=8)
        require(child.returncode == 0, "Scheduler did not cancel a native wait: " + stderr.decode(errors="replace"))
        require(stdout == b'{"type":"schedule_stopped"}\n', "Unattended action payload or unexpected receipt appeared")

    def wait(self, predicate, seconds=8):
        until = time.monotonic() + seconds
        while time.monotonic() < until:
            if predicate():
                return
            time.sleep(.02)  # Fixture-only active observation, never production waiting.
        raise RuntimeError("Private scheduling fixture timed out")

    def close(self):
        for child in self.children:
            if child.poll() is None:
                child.kill()
                child.wait(timeout=8)


def snapshot(pid):
    root = Path("/proc") / str(pid)
    stat = (root / "stat").read_text().rsplit(")", 1)[1].split()
    status = dict(line.split(":", 1) for line in (root / "status").read_text().splitlines() if ":" in line)
    smaps = dict(line.split(":", 1) for line in (root / "smaps_rollup").read_text().splitlines() if ":" in line)
    io = dict(line.split(":", 1) for line in (root / "io").read_text().splitlines())
    switches = 0
    for thread in (root / "task").iterdir():
        values = dict(line.split(":", 1) for line in (thread / "status").read_text().splitlines() if ":" in line)
        switches += int(values["voluntary_ctxt_switches"]) + int(values["nonvoluntary_ctxt_switches"])
    executable = (root / "exe").stat()
    return {"pid": pid, "starttime": stat[19], "exe_device": executable.st_dev, "exe_inode": executable.st_ino,
            "cpu_ticks": int(stat[11]) + int(stat[12]), "threads": int(status["Threads"]), "switches": switches,
            "rss_kib": int(smaps["Rss"].split()[0]), "pss_kib": int(smaps["Pss"].split()[0]),
            "private_resident_kib": sum(int(smaps[k].split()[0]) for k in ("Private_Clean", "Private_Dirty")),
            "swap_kib": int(smaps["Swap"].split()[0]), "read_bytes": int(io["read_bytes"]), "write_bytes": int(io["write_bytes"])}


def native_x11(fixture, report):
    tools = {name: shutil.which(name) for name in ("Xvfb", "xauth", "xdpyinfo", "xrandr", "ffmpeg")}
    require(all(tools.values()), "Private X11 fixture requires Xvfb/xauth/xdpyinfo/xrandr/ffmpeg")
    root = fixture.root
    display = next((f":{n}" for n in range(190, 250) if not Path(f"/tmp/.X{n}-lock").exists()
                    and not Path(f"/tmp/.X11-unix/X{n}").exists()), None)
    require(display, "No unused private X11 display")
    auth = root / "xauthority"
    env = dict(fixture.env, DISPLAY=display, XAUTHORITY=str(auth), XDG_SESSION_TYPE="x11")
    subprocess.run([tools["xauth"], "-f", str(auth), "add", display, ".", secrets.token_hex(16)], check=True, capture_output=True, timeout=5)
    server = subprocess.Popen([tools["Xvfb"], display, "-screen", "0", "128x96x24", "-auth", str(auth), "-nolisten", "tcp", "-noreset"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    x = c.CDLL("libX11.so.6")
    x.XOpenDisplay.argtypes, x.XOpenDisplay.restype = [c.c_char_p], c.c_void_p
    x.XDefaultScreen.argtypes, x.XDefaultScreen.restype = [c.c_void_p], c.c_int
    x.XRootWindow.argtypes, x.XRootWindow.restype = [c.c_void_p, c.c_int], c.c_ulong
    x.XCreateSimpleWindow.argtypes = [c.c_void_p, c.c_ulong, c.c_int, c.c_int, c.c_uint, c.c_uint, c.c_uint, c.c_ulong, c.c_ulong]
    x.XCreateSimpleWindow.restype = c.c_ulong
    x.XInternAtom.argtypes, x.XInternAtom.restype = [c.c_void_p, c.c_char_p, c.c_int], c.c_ulong
    x.XChangeProperty.argtypes = [c.c_void_p, c.c_ulong, c.c_ulong, c.c_ulong, c.c_int, c.c_int, c.c_void_p, c.c_int]
    x.XMapWindow.argtypes, x.XSetInputFocus.argtypes = [c.c_void_p, c.c_ulong], [c.c_void_p, c.c_ulong, c.c_int, c.c_ulong]
    x.XSync.argtypes, x.XCloseDisplay.argtypes = [c.c_void_p, c.c_int], [c.c_void_p]
    connection = None
    previous_env = fixture.env
    try:
        for _ in range(80):
            require(server.poll() is None, "Private Xvfb exited")
            if subprocess.run([tools["xdpyinfo"], "-display", display], env=env, capture_output=True, timeout=2).returncode == 0:
                break
            time.sleep(.025)
        else:
            raise RuntimeError("Private Xvfb did not start")
        previous_auth = os.environ.get("XAUTHORITY")
        os.environ["XAUTHORITY"] = str(auth)
        try:
            connection = x.XOpenDisplay(display.encode())
        finally:
            if previous_auth is None:
                os.environ.pop("XAUTHORITY", None)
            else:
                os.environ["XAUTHORITY"] = previous_auth
        require(connection, "Could not open authenticated private display")
        subprocess.run([tools["xrandr"], "--display", display, "--output", "screen", "--primary"], env=env, check=True, capture_output=True, timeout=5)
        root_window = x.XRootWindow(connection, x.XDefaultScreen(connection))
        window = x.XCreateSimpleWindow(connection, root_window, 20, 20, 32, 24, 0, 0, 0x111D47)
        def prop(target, name, kind, values):
            data = (c.c_ulong * len(values))(*values)
            x.XChangeProperty(connection, target, x.XInternAtom(connection, name.encode(), 0), x.XInternAtom(connection, kind.encode(), 0), 32, 0, data, len(values))
        prop(window, "_NET_WM_PID", "CARDINAL", [os.getpid()])
        prop(window, "_NET_WM_STATE", "ATOM", [])
        for atom in ("_NET_WM_STATE_HIDDEN", "_NET_WM_STATE_MAXIMIZED_VERT", "_NET_WM_STATE_MAXIMIZED_HORZ"):
            x.XInternAtom(connection, atom.encode(), 0)
        prop(root_window, "_NET_CLIENT_LIST_STACKING", "WINDOW", [window])
        prop(root_window, "_NET_ACTIVE_WINDOW", "WINDOW", [window])
        x.XMapWindow(connection, window)
        x.XSetInputFocus(connection, window, 2, 0)
        x.XSync(connection, 0)
        def run(*args):
            result = subprocess.run([fixture.binary, *map(str, args)], env=env, capture_output=True, timeout=10)
            require(result.returncode == 0, "Private native command failed: " + result.stderr.decode(errors="replace"))
            return json.loads(result.stdout)
        displays = run("list", "displays")
        require(len(displays) == 1, "Unexpected private display count")
        selected = displays[0]["id"]
        catalog = root / "native-catalog.json"
        run("capture-catalog", "bind", catalog, "crop", selected, 20, 20, 2, 1, "--connector", "true")
        sources = {"primary": {"type": "primary_display"}, "active-display": {"type": "active_display"},
                   "active-window": {"type": "active_window"}, "display": {"type": "display", "id": selected},
                   "window": {"type": "window", "id": str(window)},
                   "region": {"type": "region", "display": selected, "x": 20, "y": 20, "width": 2, "height": 1},
                   "template": {"type": "template", "catalog": str(catalog), "name": "crop"}}
        database = root / "native.sqlite"
        fixture.env = env
        start = utc(.6)
        for name, source in sources.items():
            profile = root / ("native-" + name + ".json")
            profile.write_text(json.dumps({"version": 1, "source": source, "actions": [{"type": "save", "path": "native-" + name + "-{uuid}.png"}]}))
            fixture.command("add", database, name, profile, "--at", start, "--every-ms", 150, "--count", 2, "--missed", "once", "--grace-ms", 5000)
        child = fixture.start(database)
        fixture.wait(lambda: all(s["state"] == "complete" for s in fixture.command("list", database)["schedules"]), seconds=15)
        fixture.stop(child)
        for name in sources:
            images = list(root.glob("native-" + name + "-*.png"))
            require(len(images) == 2, "Native finite sequence count changed")
            if name in ("window", "active-window", "region", "template"):
                for image in images:
                    pixels = subprocess.run([tools["ffmpeg"], "-v", "error", "-i", str(image), "-f", "rawvideo", "-pix_fmt", "rgba", "-"], capture_output=True, check=True, timeout=5).stdout
                    require(pixels and pixels == bytes([17, 29, 71, 255]) * (len(pixels) // 4), "Native sequence captured unexpected private pixels")
        report["checks"].append("private authenticated Xvfb: seven native source forms, two captures each, exact window/region/template pixels")
    finally:
        fixture.env = previous_env
        if connection:
            x.XCloseDisplay(connection)
        server.terminate()
        try:
            server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--report", required=True)
    parser.add_argument("--idle-seconds", type=int, default=0)
    parser.add_argument("--native-x11", action="store_true")
    args = parser.parse_args()
    binary = str(Path(args.binary).resolve(strict=True))
    report = {"binary_sha256": hashlib.file_digest(open(binary, "rb"), "sha256").hexdigest(), "checks": [],
              "environment": {"os": platform.platform(), "machine": platform.machine(), "cpu_count": os.cpu_count(),
                              "cpu_model": next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")), "unknown"),
                              "memory_total_kib": next(int(line.split()[1]) for line in Path("/proc/meminfo").read_text().splitlines() if line.startswith("MemTotal:")),
                              "power_mode": "not established", "build": "optimized release-fast; isolated executable has debug sections stripped",
                              "display": "only an authenticated private Xvfb 128x96 when --native-x11; idle service captures nothing"}}
    with tempfile.TemporaryDirectory(prefix="screenshots-schedules-") as temporary:
        root = Path(temporary)
        fixture = Fixture(binary, root)
        try:
            database = root / "finite.sqlite"
            profile = fixture.profile("finite")
            fixture.command("add", database, "finite", profile, "--at", utc(.3), "--every-ms", 150,
                            "--count", 3, "--missed", "once", "--grace-ms", 5000)
            child = fixture.start(database)
            fixture.wait(lambda: len(list(root.glob("finite-*.png"))) == 3)
            fixture.wait(lambda: fixture.command("list", database)["schedules"][0]["state"] == "complete")
            fixture.command("pause", database, "finite", success=False)
            fixture.command("serve", database, success=False)
            fixture.stop(child)
            require(len(list(root.glob("finite-*.png"))) == 3, "Finite sequence ran extra captures")
            report["checks"].append("three serial dynamic outputs, exact count, readonly status and lifetime ownership")

            replaced = root / "changed.sqlite"
            profile = fixture.profile("changed")
            fixture.command("add", replaced, "changed", profile, "--at", utc(.25), "--missed", "once")
            profile.write_text(json.dumps({"version": 1, "source": {"type": "file", "path": "input.png"},
                                           "actions": [{"type": "save", "path": "injected.png"}]}))
            child = fixture.start(replaced)
            fixture.wait(lambda: fixture.command("list", replaced)["schedules"][0]["paused"])
            fixture.stop(child)
            require(not (root / "injected.png").exists() and not list(root.glob("changed-*.png")), "Replaced profile had effects")
            report["checks"].append("changed Task pauses before capture/output")

            for policy, expected in (("skip", 0), ("once", 1), ("pause", 0)):
                database = root / (policy + ".sqlite")
                fixture.command("add", database, policy, fixture.profile(policy), "--at", utc(-10),
                                "--every-ms", 100, "--count", 10, "--missed", policy, "--grace-ms", 0)
                child = fixture.start(database)
                fixture.wait(lambda: (fixture.command("list", database)["schedules"][0]["paused"] if policy == "pause"
                                      else fixture.command("list", database)["schedules"][0]["state"] == "complete"))
                fixture.stop(child)
                require(len(list(root.glob(policy + "-*.png"))) == expected, "Missed policy replayed a backlog")
                if policy == "pause":
                    fixture.command("resume", database, policy)
                    require(fixture.command("list", database)["schedules"][0]["state"] == "complete", "Explicit resume did not discard elapsed finite slots")
            report["checks"].append("all missed policies coalesce/skip/pause without backlog")

            requests = []
            class Handler(BaseHTTPRequestHandler):
                def log_message(self, *unused):
                    pass
                def do_POST(self):
                    length = int(self.headers["Content-Length"])
                    require(0 < length <= 1024 * 1024, "Unexpected private upload size")
                    requests.append(self.rfile.read(length))
                    body = b'{"url":"https://example.invalid/private-schedule-result"}'
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
            with socketserver.TCPServer(("127.0.0.1", 0), Handler) as server:
                worker = threading.Thread(target=server.serve_forever, daemon=True)
                worker.start()
                try:
                    upload = root / "upload.json"
                    upload.write_text(json.dumps({"url": f"http://127.0.0.1:{server.server_address[1]}/upload", "allow_http": True, "json_pointer": "/url"}))
                    database = root / "uncertain.sqlite"
                    profile = fixture.profile("uncertain", [{"type": "upload", "profile": str(upload)},
                                                            {"type": "command", "program": "/usr/bin/false", "timeout_ms": 1000}])
                    fixture.command("add", database, "uncertain", profile, "--at", utc(.2), "--every-ms", 100,
                                    "--count", 2, "--missed", "once", "--grace-ms", 5000)
                    child = fixture.start(database)
                    stdout, stderr = child.communicate(timeout=10)
                    require(child.returncode != 0 and stdout == b"" and b"uncertain" in stderr, "Uncertain task was not fail-closed or leaked a receipt")
                    state = fixture.command("list", database)
                    require(state["uncertain_run"] and state["schedules"][0]["state"] == "uncertain" and len(requests) == 1, "Uncertain upload intent was lost")
                    fixture.command("serve", database, success=False)
                    require(len(requests) == 1, "Uncertain upload was automatically retried")
                    fixture.command("resolve", database, state["uncertain_run"]["id"], "skip")
                    fixture.command("pause", database, "uncertain")
                    child = fixture.start(database)
                    fixture.stop(child)
                    require(len(requests) == 1, "Explicit resolution unexpectedly retried an upload")
                    report["checks"].append("confirmed private upload followed by failure stays uncertain and never retries across restart/resolution")
                    marker = root / "crash-marker"
                    program = ("import ctypes,os,time,pathlib;parent=os.getppid();"
                               "assert ctypes.CDLL(None).prctl(1,9)==0;"
                               "assert os.getppid()==parent;"
                               f"pathlib.Path({str(marker)!r}).write_text(str(os.getpid()));time.sleep(2)")
                    database = root / "crash.sqlite"
                    profile = fixture.profile("crash", [{"type": "upload", "profile": str(upload)},
                                                        {"type": "command", "program": sys.executable,
                                                         "args": ["-I", "-c", program], "timeout_ms": 5000}])
                    fixture.command("add", database, "crash", profile, "--at", utc(.2), "--missed", "once", "--grace-ms", 5000)
                    child = fixture.start(database)
                    fixture.wait(lambda: marker.exists() or child.poll() is not None)
                    if not marker.exists():
                        raise RuntimeError("Crash fixture Task stopped early: " + child.communicate(timeout=1)[1].decode(errors="replace") + json.dumps(fixture.command("list", database)))
                    require(len(requests) == 2, "Crash fixture upload was not confirmed before interruption")
                    child.kill()
                    stdout, stderr = child.communicate(timeout=5)
                    require(stdout == b"", "Crash fixture leaked unattended action payload")
                    state = fixture.command("list", database)
                    require(state["uncertain_run"] and state["schedules"][0]["next_ordinal"] == 1, "Abrupt exit lost durable run admission")
                    fixture.command("serve", database, success=False)
                    require(len(requests) == 2, "Abrupt-exit upload was retried")
                    fixture.command("resolve", database, state["uncertain_run"]["id"], "complete")
                    child = fixture.start(database)
                    fixture.stop(child)
                    require(len(requests) == 2, "Resolved crash replayed the consumed occurrence")
                    report["checks"].append("actual SIGKILL after confirmed private upload preserves intent and consumed ordinal; restart never retries")
                    raw = root / "guarded.bin"
                    raw.write_bytes(b"original")
                    marker = root / "raw-guard-marker"
                    release = root / "raw-release"
                    os.mkfifo(release, 0o600)
                    release_fd = os.open(release, os.O_RDWR | os.O_NONBLOCK)
                    try:
                        code = (f"import pathlib;pathlib.Path({str(marker)!r}).write_text('ready');"
                                f"open({str(release)!r},'rb').read(1)")
                        profile = root / "raw-guard.json"
                        profile.write_text(json.dumps({"version": 1, "source": {"type": "raw_file", "path": str(raw)},
                                                       "actions": [{"type": "command", "program": sys.executable,
                                                                    "args": ["-I", "-c", code], "timeout_ms": 5000},
                                                                   {"type": "upload", "profile": str(upload)}]}))
                        database = root / "raw-guard.sqlite"
                        fixture.command("add", database, "raw-guard", profile, "--at", utc(.2), "--missed", "once", "--grace-ms", 5000)
                        child = fixture.start(database)
                        fixture.wait(marker.exists)
                        raw.rename(root / "prior-raw.bin")
                        raw.write_bytes(b"injected")
                        os.write(release_fd, b"x")
                        stdout, stderr = child.communicate(timeout=8)
                        require(child.returncode != 0 and not stdout and len(requests) == 2,
                                "Raw payload replaced after preparation reached upload or leaked receipts")
                        require(fixture.command("list", database)["uncertain_run"], "Raw identity failure lost run intent")
                        report["checks"].append("actual CLI raw replacement after prepared command handshake is rejected at upload open with no connection")
                    finally:
                        os.close(release_fd)
                finally:
                    server.shutdown()
                    worker.join(timeout=5)

            if args.native_x11:
                native_x11(fixture, report)

            if args.idle_seconds:
                require(1 <= args.idle_seconds <= 600, "Idle fixture bound is 1–600 seconds")
                database = root / "idle.sqlite"
                fixture.command("add", database, "idle", fixture.profile("idle"), "--at", utc(86400))
                child = fixture.start(database)
                time.sleep(1)
                before = snapshot(child.pid)
                began = time.monotonic()
                time.sleep(args.idle_seconds)
                after = snapshot(child.pid)
                elapsed = time.monotonic() - began
                require(all(before[k] == after[k] for k in ("pid", "starttime", "exe_device", "exe_inode")), "Idle process identity changed")
                report["idle"] = {"elapsed_seconds": elapsed, "before": before, "after": after,
                                  "cpu_percent_one_core": (after["cpu_ticks"] - before["cpu_ticks"]) / os.sysconf("SC_CLK_TCK") / elapsed * 100,
                                  "note": "Development Linux observation; no hardware sleep/wake or reference-machine acceptance"}
                fixture.stop(child)
        finally:
            fixture.close()
    Path(args.report).write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": len(report["checks"]), "report": args.report}))


if __name__ == "__main__":
    main()
