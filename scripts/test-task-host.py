#!/usr/bin/env python3
"""Exercise native global task hotkeys and authenticated IPC on private Xvfb.

An optional --idle-seconds 300 records a five-minute optimized host observation.
Only generated images, this private server and local IPC receive input.
"""
import argparse
import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import secrets
import selectors
import shutil
import socket
import struct
import subprocess
import tempfile
import threading
import time
import zlib
from http.server import BaseHTTPRequestHandler, HTTPServer

REPO = Path(__file__).resolve().parents[1]


class Selection(c.Structure):
    _fields_ = [("kind", c.c_int), ("serial", c.c_ulong), ("send_event", c.c_int),
                ("display", c.c_void_p), ("requestor", c.c_ulong), ("selection", c.c_ulong),
                ("target", c.c_ulong), ("property", c.c_ulong), ("time", c.c_ulong)]


class Event(c.Union):
    _fields_ = [("kind", c.c_int), ("selection", Selection), ("pad", c.c_long * 24)]


def require(value, message):
    if not value:
        raise RuntimeError(message)


def executable(value):
    path = shutil.which(value) if value else None
    require(path, f"Executable unavailable: {value}")
    return str(Path(path).resolve())


def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)


def source_png():
    raw = b"".join(b"\0" + bytes(channel for xx in range(32)
                                   for channel in (17, 29, 71, (0, 128, 255)[xx % 3])) for _ in range(24))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 32, 24, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


def request(endpoint, command, token=None):
    data = json.dumps({"version": 1, "token": endpoint["token"] if token is None else token,
                       "command": command}).encode()
    with socket.create_connection(("127.0.0.1", endpoint["port"]), timeout=2) as stream:
        stream.settimeout(2)
        stream.sendall(struct.pack(">I", len(data)) + data)

        def exact(count):
            data = bytearray()
            while len(data) < count:
                part = stream.recv(count - len(data))
                require(part, "Truncated task-host response")
                data.extend(part)
            return data
        size, = struct.unpack(">I", exact(4))
        require(1 <= size <= 4096, "Oversized task-host response")
        return json.loads(exact(size))


def ready(child):
    selector = selectors.DefaultSelector()
    selector.register(child.stdout, selectors.EVENT_READ)
    data = bytearray()
    deadline = time.monotonic() + 10
    while b"\n" not in data:
        require(time.monotonic() < deadline and child.poll() is None, "Task host did not become ready")
        for key, _ in selector.select(0.1):
            part = os.read(key.fd, 4096)
            require(part, "Task host exited before readiness")
            data.extend(part)
            require(len(data) <= 4096, "Task-host readiness exceeded its bound")
    selector.close()
    return json.loads(data.splitlines()[0])


def finished(endpoint):
    deadline = time.monotonic() + 10
    while True:
        state = request(endpoint, {"type": "status"})
        if state["busy"] is False:
            require(state["last"]["state"] == "completed" and state["last"]["success"] is True,
                    f"Synthetic task did not complete successfully: {state['last']}")
            return state
        require(time.monotonic() < deadline, "Saved task did not complete")
        time.sleep(0.01)


def proc(pid):
    base = Path(f"/proc/{pid}")
    fields = (base / "stat").read_text().rsplit(")", 1)[1].split()
    executable = (base / "exe").stat()
    result = {"cpu_ticks": int(fields[11]) + int(fields[12]), "starttime_ticks": int(fields[19]),
              "comm": (base / "comm").read_text().strip(), "process_state": fields[0],
              "executable_device": executable.st_dev, "executable_inode": executable.st_ino}
    status = (base / "status").read_text()
    for line in status.splitlines():
        name, _, value = line.partition(":")
        if name in ("VmRSS", "VmHWM", "VmSwap", "Threads", "Pid"):
            result[name] = int(value.split()[0])
    switches = 0
    for task in (base / "task").iterdir():
        for line in (task / "status").read_text().splitlines():
            if line.startswith(("voluntary_ctxt_switches:", "nonvoluntary_ctxt_switches:")):
                switches += int(line.split()[-1])
    result["context_switches"] = switches
    for line in (base / "smaps_rollup").read_text().splitlines():
        if line.startswith(("Pss:", "Private_Clean:", "Private_Dirty:", "Swap:", "SwapPss:", "Rss:")):
            result[line.split(":")[0]] = int(line.split()[1])
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=str(REPO / "target/release-fast/screenshots"))
    parser.add_argument("--xvfb", default=shutil.which("Xvfb"))
    parser.add_argument("--idle-seconds", type=int, default=0)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    require(0 <= args.idle_seconds <= 600, "Idle observation must be 0–600 seconds")
    binary, xvfb = executable(args.binary), executable(args.xvfb)
    xauth, xdpyinfo, ffmpeg, xrandr = executable("xauth"), executable("xdpyinfo"), executable("ffmpeg"), executable("xrandr")
    x = c.CDLL("libX11.so.6")
    xtest = c.CDLL("libXtst.so.6")
    x.XOpenDisplay.argtypes, x.XOpenDisplay.restype = [c.c_char_p], c.c_void_p
    x.XStringToKeysym.argtypes, x.XStringToKeysym.restype = [c.c_char_p], c.c_ulong
    x.XKeysymToKeycode.argtypes, x.XKeysymToKeycode.restype = [c.c_void_p, c.c_ulong], c.c_ubyte
    x.XSync.argtypes, x.XCloseDisplay.argtypes = [c.c_void_p, c.c_int], [c.c_void_p]
    x.XDefaultScreen.argtypes, x.XDefaultScreen.restype = [c.c_void_p], c.c_int
    x.XRootWindow.argtypes, x.XRootWindow.restype = [c.c_void_p, c.c_int], c.c_ulong
    x.XCreateSimpleWindow.argtypes = [c.c_void_p, c.c_ulong, c.c_int, c.c_int, c.c_uint, c.c_uint,
                                    c.c_uint, c.c_ulong, c.c_ulong]
    x.XCreateSimpleWindow.restype = c.c_ulong
    x.XInternAtom.argtypes, x.XInternAtom.restype = [c.c_void_p, c.c_char_p, c.c_int], c.c_ulong
    x.XChangeProperty.argtypes = [c.c_void_p, c.c_ulong, c.c_ulong, c.c_ulong, c.c_int, c.c_int, c.c_void_p, c.c_int]
    x.XMapWindow.argtypes = [c.c_void_p, c.c_ulong]
    x.XSetInputFocus.argtypes = [c.c_void_p, c.c_ulong, c.c_int, c.c_ulong]
    x.XDestroyWindow.argtypes = [c.c_void_p, c.c_ulong]
    x.XSetWindowBackground.argtypes, x.XClearWindow.argtypes = [c.c_void_p, c.c_ulong, c.c_ulong], [c.c_void_p, c.c_ulong]
    x.XConvertSelection.argtypes = [c.c_void_p, c.c_ulong, c.c_ulong, c.c_ulong, c.c_ulong, c.c_ulong]
    x.XPending.argtypes, x.XNextEvent.argtypes = [c.c_void_p], [c.c_void_p, c.POINTER(Event)]
    x.XGetWindowProperty.argtypes = [c.c_void_p, c.c_ulong, c.c_ulong, c.c_long, c.c_long, c.c_int, c.c_ulong,
                                   c.POINTER(c.c_ulong), c.POINTER(c.c_int), c.POINTER(c.c_ulong), c.POINTER(c.c_ulong), c.POINTER(c.c_void_p)]
    x.XGetSelectionOwner.argtypes, x.XGetSelectionOwner.restype = [c.c_void_p, c.c_ulong], c.c_ulong
    x.XFree.argtypes = [c.c_void_p]
    xtest.XTestFakeKeyEvent.argtypes, xtest.XTestFakeKeyEvent.restype = [c.c_void_p, c.c_uint, c.c_int, c.c_ulong], c.c_int
    xtest.XTestFakeButtonEvent.argtypes, xtest.XTestFakeButtonEvent.restype = [c.c_void_p, c.c_uint, c.c_int, c.c_ulong], c.c_int
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="screenshots-task-host-") as temporary:
        root = Path(temporary)
        display = next((f":{i}" for i in range(90, 150)
                        if not Path(f"/tmp/.X{i}-lock").exists()
                        and not Path(f"/tmp/.X11-unix/X{i}").exists()), None)
        require(display, "No unused private display")
        auth = root / "authority"
        env = dict(os.environ)
        for name in ("WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS"):
            env.pop(name, None)
        env.update(DISPLAY=display, XAUTHORITY=str(auth), XDG_SESSION_TYPE="x11",
                   MESA_SHADER_CACHE_DISABLE="true")
        state_keys = ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME")
        env.update({name: str(root / name) for name in state_keys})
        subprocess.run([xauth, "-f", str(auth), "add", display, ".", secrets.token_hex(16)],
                       check=True, capture_output=True, timeout=5)
        with (root / "xvfb.log").open("wb") as log:
            server = subprocess.Popen([xvfb, display, "-screen", "0", "128x96x24", "-auth", str(auth),
                                       "-nolisten", "tcp", "-noreset"], env=env, stdout=log, stderr=subprocess.STDOUT)
        child = connection = window = upload_server = upload_thread = None
        try:
            for _ in range(100):
                require(server.poll() is None, "Private Xvfb exited")
                if subprocess.run([xdpyinfo, "-display", display], env=env,
                                  capture_output=True, timeout=2).returncode == 0:
                    break
                time.sleep(0.05)
            else:
                raise RuntimeError("Private Xvfb did not become ready")
            previous = os.environ.get("XAUTHORITY")
            os.environ["XAUTHORITY"] = str(auth)
            try:
                connection = x.XOpenDisplay(display.encode())
            finally:
                if previous is None:
                    os.environ.pop("XAUTHORITY", None)
                else:
                    os.environ["XAUTHORITY"] = previous
            require(connection, "Cannot connect key fixture to private display")
            subprocess.run([xrandr, "--display", display, "--output", "screen", "--primary"],
                           env=env, check=True, capture_output=True, timeout=5)
            root_window = x.XRootWindow(connection, x.XDefaultScreen(connection))
            window = x.XCreateSimpleWindow(connection, root_window, 20, 20, 32, 24, 0, 0, 0x111D47)

            def prop(target, name, kind, values):
                data = (c.c_ulong * len(values))(*values)
                x.XChangeProperty(connection, target, x.XInternAtom(connection, name.encode(), 0),
                                  x.XInternAtom(connection, kind.encode(), 0), 32, 0, data, len(values))

            prop(window, "_NET_WM_PID", "CARDINAL", [os.getpid()])
            prop(window, "_NET_WM_STATE", "ATOM", [])
            for atom in ("_NET_WM_STATE_HIDDEN", "_NET_WM_STATE_MAXIMIZED_VERT", "_NET_WM_STATE_MAXIMIZED_HORZ"):
                x.XInternAtom(connection, atom.encode(), 0)
            prop(root_window, "_NET_CLIENT_LIST_STACKING", "WINDOW", [window])
            prop(root_window, "_NET_ACTIVE_WINDOW", "WINDOW", [window])
            x.XMapWindow(connection, window)
            x.XSetInputFocus(connection, window, 2, 0)
            x.XSync(connection, 0)
            clipboard_atom = x.XInternAtom(connection, b"CLIPBOARD", 0)

            def clipboard_bytes(target):
                target_atom = x.XInternAtom(connection, target.encode(), 0)
                property_atom = x.XInternAtom(connection, b"_SCREENSHOTS_TASK_CLIPBOARD_TEST", 0)
                x.XConvertSelection(connection, clipboard_atom, target_atom, property_atom, window, 0)
                x.XSync(connection, 0)
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    while x.XPending(connection):
                        event = Event()
                        x.XNextEvent(connection, c.byref(event))
                        if event.kind != 31:
                            continue
                        require(event.selection.property == property_atom, "Native clipboard owner refused the requested target")
                        actual, count, after, format_bits, data = c.c_ulong(), c.c_ulong(), c.c_ulong(), c.c_int(), c.c_void_p()
                        status = x.XGetWindowProperty(connection, window, property_atom, 0, 262144, 1,
                                                      target_atom, c.byref(actual), c.byref(format_bits),
                                                      c.byref(count), c.byref(after), c.byref(data))
                        try:
                            require(status == 0 and actual.value == target_atom and format_bits.value == 8
                                    and after.value == 0 and count.value <= 1024 * 1024,
                                    "Clipboard test payload is invalid or unexpectedly large")
                            return c.string_at(data, count.value)
                        finally:
                            if data:
                                x.XFree(data)
                    time.sleep(0.001)
                raise RuntimeError("Persistent clipboard owner did not answer")

            def expected_pixels(rgb):
                return bytes(channel for y in range(24) for xx in range(32)
                             for channel in ((0, 0, 0, 255) if 1 <= xx < 6 and 1 <= y < 5 else (*rgb, 255)))

            def decode_png(data):
                result = subprocess.run([ffmpeg, "-v", "error", "-nostdin", "-i", "pipe:0",
                                         "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "-"],
                                        input=data, capture_output=True, env=env, timeout=10)
                require(result.returncode == 0 and not result.stderr, "Clipboard PNG could not be independently decoded")
                return result.stdout

            source = root / "source.png"
            source.write_bytes(source_png())
            source_hash = hashlib.sha256(source.read_bytes()).hexdigest()
            profile = root / "task.json"
            (root / "recipe.json").write_text(json.dumps({"version": 1, "steps": [{"op": "redact", "x": 1, "y": 1, "width": 5, "height": 4}]}))
            profile.write_text(json.dumps({"version": 1, "source": {"type": "active_window"},
                                           "recipe": "recipe.json",
                                           "actions": [{"type": "command", "program": "/bin/sh", "args": ["-c", "sleep 0.4"], "timeout_ms": 3000},
                                                       {"type": "save", "path": "capture-{uuid}.png"}, {"type": "copy_image"}]}))
            for name, kind in (("display", "active_display"), ("primary", "primary_display")):
                (root / f"{name}.json").write_text(json.dumps({"version": 1, "source": {"type": kind},
                                                               "actions": [{"type": "save", "path": f"{name}-{{uuid}}.png"}]}))
            uploads = []
            class Upload(BaseHTTPRequestHandler):
                def do_PUT(self):
                    size = int(self.headers["Content-Length"])
                    require(0 < size <= 1024 * 1024, "Unexpected upload fixture size")
                    uploads.append(self.rfile.read(size))
                    payload = b"https://example.invalid/persistent-host-link"
                    self.send_response(200)
                    self.send_header("Content-Length", str(len(payload)))
                    self.end_headers()
                    self.wfile.write(payload)
                def log_message(self, *_):
                    pass
            upload_server = HTTPServer(("127.0.0.1", 0), Upload)
            upload_thread = threading.Thread(target=upload_server.serve_forever, daemon=True)
            upload_thread.start()
            (root / "upload.json").write_text(json.dumps({"url": f"http://127.0.0.1:{upload_server.server_port}/upload", "method": "PUT", "allow_http": True, "response": {"type": "text"}}))
            (root / "link.json").write_text(json.dumps({"version": 1, "source": {"type": "active_window"}, "recipe": "recipe.json",
                                                        "actions": [{"type": "upload", "profile": "upload.json"}, {"type": "copy_link"}]}))
            (root / "alpha.json").write_text(json.dumps({"version": 1, "source": {"type": "file", "path": "source.png"},
                                                         "actions": [{"type": "copy_image"}]}))
            config = root / "host.json"
            config.write_text(json.dumps({"version": 1, "task_timeout_ms": 5000,
                                          "bindings": [{"id": "fixture", "task": "task.json", "hotkey": "Ctrl+Shift+F9"},
                                                       {"id": "display", "task": "display.json"},
                                                       {"id": "primary", "task": "primary.json"},
                                                       {"id": "link", "task": "link.json"},
                                                       {"id": "alpha", "task": "alpha.json"}]}))
            endpoint_path = root / "endpoint.json"
            child = subprocess.Popen([binary, "task-host", "serve", str(config), str(endpoint_path)],
                                     env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            require(ready(child) == {"ready": True, "profiles": 5, "hotkeys": 1}, "Unexpected host readiness")
            require(endpoint_path.stat().st_mode & 0o777 == 0o600, "Endpoint is not private")
            endpoint = json.loads(endpoint_path.read_bytes())
            require(endpoint["pid"] == child.pid, "Endpoint and measured process identities differ")
            codes = [x.XKeysymToKeycode(connection, x.XStringToKeysym(name))
                     for name in (b"Control_L", b"Shift_L", b"F9")]
            require(all(codes), "Private X11 keys are unavailable")

            def key(code, down):
                require(xtest.XTestFakeKeyEvent(connection, code, int(down), 0), "Cannot inject fixture key")
                x.XSync(connection, 0)

            require(xtest.XTestFakeButtonEvent(connection, 1, 1, 0), "Cannot press fixture-only mouse button")
            x.XSync(connection, 0)
            for code in codes:
                key(code, True)
            deadline = time.monotonic() + 3
            while not request(endpoint, {"type": "status"})["busy"]:
                require(time.monotonic() < deadline, "Native global hotkey did not trigger task")
                time.sleep(0.005)
            busy = request(endpoint, {"type": "trigger", "profile": "fixture"})
            require(xtest.XTestFakeButtonEvent(connection, 1, 0, 0), "Cannot release fixture-only mouse button")
            x.XSync(connection, 0)
            require(busy == {"ok": False, "error": "busy"}, "A second task was queued while busy")
            finished(endpoint)
            require(decode_png(clipboard_bytes("image/png")) == expected_pixels((17, 29, 71)),
                    "Completed task did not leave finalized pixels owned by host")
            time.sleep(0.75)  # Fixture-only key hold: verify autorepeat cannot re-admit after completion.
            require(len(list(root.glob("capture-*.png"))) == 1 and not request(endpoint, {"type": "status"})["busy"],
                    "Held-key repeats launched more than one task")
            for code in reversed(codes):
                key(code, False)
            time.sleep(0.03)
            x.XSetWindowBackground(connection, window, 0x175997)
            x.XClearWindow(connection, window)
            x.XSync(connection, 0)
            for code in codes:
                key(code, True)
            for code in reversed(codes):
                key(code, False)
            deadline = time.monotonic() + 3
            while len(list(root.glob("capture-*.png"))) < 2:
                require(time.monotonic() < deadline, f"Second physical key task did not finish: {request(endpoint, {'type':'status'})}")
                time.sleep(0.01)
            finished(endpoint)
            require(decode_png(clipboard_bytes("image/png")) == expected_pixels((23, 89, 151)),
                    "Second hotkey task did not replace the host-owned clipboard")
            require(request(endpoint, {"type": "trigger", "profile": "fixture"}, "invalid-token")
                    == {"ok": False, "error": "unauthorized"}, "Invalid bearer token was accepted")
            require(request(endpoint, {"type": "trigger", "profile": "fixture"})["state"] == "accepted",
                    "Authenticated IPC did not admit task")
            finished(endpoint)
            require(len(list(root.glob("capture-*.png"))) == 3, "Expected exactly two hotkey tasks and one IPC task")
            decoded_captures = []
            for output in root.glob("capture-*.png"):
                decoded = subprocess.run([ffmpeg, "-v", "error", "-nostdin", "-i", str(output),
                                          "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "-"],
                                         capture_output=True, env=env, timeout=10)
                require(decoded.returncode == 0 and not decoded.stderr,
                        "Independent task output pixels differ")
                decoded_captures.append(decoded.stdout)
            require(decoded_captures.count(expected_pixels((17, 29, 71))) == 1
                    and decoded_captures.count(expected_pixels((23, 89, 151))) == 2,
                    "Saved final pixels do not match their captured/edited sources")
            for name in ("display", "primary"):
                require(request(endpoint, {"type": "trigger", "profile": name})["state"] == "accepted",
                        f"Native {name} capture was not admitted")
                finished(endpoint)
                outputs = list(root.glob(f"{name}-*.png"))
                require(len(outputs) == 1, "Expected one native display output")
                decoded = subprocess.run([ffmpeg, "-v", "error", "-nostdin", "-i", str(outputs[0]),
                                          "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "-"],
                                         capture_output=True, env=env, timeout=10)
                expected = bytes(channel for y in range(96) for xx in range(128)
                                 for channel in ((23, 89, 151, 255) if 20 <= xx < 52 and 20 <= y < 44 else (0, 0, 0, 255)))
                require(decoded.returncode == 0 and not decoded.stderr and decoded.stdout == expected,
                        f"Independent {name} capture pixels differ")
            require(request(endpoint, {"type": "trigger", "profile": "link"})["state"] == "accepted",
                    "Upload/link copy task was not admitted")
            finished(endpoint)
            require(clipboard_bytes("UTF8_STRING") == b"https://example.invalid/persistent-host-link",
                    "Completed upload task did not leave its link owned by host")
            require(len(uploads) == 1 and decode_png(uploads[0]) == expected_pixels((23, 89, 151)),
                    "Link copy task did not upload finalized pixels exactly once")
            upload_server.shutdown()
            upload_server.server_close()
            upload_thread.join(timeout=5)
            upload_server = upload_thread = None
            require(request(endpoint, {"type": "trigger", "profile": "fixture"})["state"] == "accepted",
                    "A new image task was not admitted after persistent link ownership")
            finished(endpoint)
            require(decode_png(clipboard_bytes("image/png")) == expected_pixels((23, 89, 151)),
                    "Final image owner was not retained after job completion")
            require(len(list(root.glob("capture-*.png"))) == 4, "Unexpected queued task after clipboard completion")
            require(request(endpoint, {"type": "trigger", "profile": "alpha"})["state"] == "accepted",
                    "Alpha image copy was not admitted")
            finished(endpoint)
            retained_expected = bytes(channel for y in range(24) for xx in range(32)
                                      for channel in (17, 29, 71, (0, 128, 255)[xx % 3]))
            require(decode_png(clipboard_bytes("image/png")) == retained_expected,
                    "Host clipboard transfer did not preserve alpha/hidden RGB")
            require(hashlib.sha256(source.read_bytes()).hexdigest() == source_hash, "Source image changed")
            idle = None
            if args.idle_seconds:
                before = proc(child.pid)
                identity_keys = ("Pid", "starttime_ticks", "comm", "executable_device", "executable_inode")
                identity = {key: before[key] for key in identity_keys}
                samples = [before]
                idle_start = time.monotonic()
                until = idle_start + args.idle_seconds
                while time.monotonic() < until:
                    require(child.poll() is None, "Host exited during idle observation")
                    time.sleep(max(0, min(30, until - time.monotonic())))
                    sample = proc(child.pid)
                    require({key: sample[key] for key in identity_keys} == identity,
                            "Host identity changed during idle observation")
                    samples.append(sample)
                elapsed = time.monotonic() - idle_start
                after = proc(child.pid)
                require(child.poll() is None and {key: after[key] for key in identity_keys} == identity,
                        "Host exited or changed identity during idle observation")
                cpu = (after["cpu_ticks"] - before["cpu_ticks"]) / os.sysconf("SC_CLK_TCK") / elapsed * 100
                idle = {"seconds": elapsed, "before": before, "after": after,
                        "samples": samples, "identity_stable": True,
                        "VmHWM_monotonic": all(a["VmHWM"] <= b["VmHWM"] for a, b in zip(samples, samples[1:])),
                        "mean_cpu_percent_one_core": cpu,
                        "context_switch_delta": after["context_switches"] - before["context_switches"],
                        "private_kib": after.get("Private_Clean", 0) + after.get("Private_Dirty", 0),
                        "private_resident_plus_swap_kib": after.get("Private_Clean", 0) + after.get("Private_Dirty", 0) + after.get("SwapPss", 0),
                        "max_observed_rss_kib": max(sample["VmRSS"] for sample in samples),
                        "memory_counter_caveat": "VmHWM is reported but must not establish a peak-memory gate when nonmonotonic; private resident and swap are separate live-consumer costs",
                        "source": "Linux /proc snapshots; development hardware, no GPU/private-platform acceptance inferred"}
                retained_png = clipboard_bytes("image/png")
                require(decode_png(retained_png) == retained_expected,
                        "Host clipboard ownership did not survive idle/swapping")
                idle["post_idle_clipboard_exact"] = True
                idle["retained_native_png_bytes"] = len(retained_png)
                idle["after_explicit_paste_read"] = proc(child.pid)
            require(request(endpoint, {"type": "stop"})["state"] == "stopping", "IPC stop did not acknowledge")
            stdout, stderr = child.communicate(timeout=10)
            require(child.returncode == 0 and not stdout and not stderr, "Task host did not stop cleanly")
            require(not endpoint_path.exists(), "Owned endpoint was not removed")
            require(x.XGetSelectionOwner(connection, clipboard_atom) == 0, "Host stop did not release clipboard ownership")
            require(not any(Path(env[name]).exists() for name in state_keys), "Host wrote unrelated user state")
        finally:
            if child is not None and child.poll() is None:
                child.kill()
                child.wait(timeout=5)
            if connection:
                if window:
                    x.XDestroyWindow(connection, window)
                x.XCloseDisplay(connection)
            if upload_server:
                upload_server.shutdown()
                upload_server.server_close()
            if upload_thread:
                upload_thread.join(timeout=5)
            server.terminate()
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=5)
    report = {"passed": True, "binary_sha256": hashlib.sha256(Path(binary).read_bytes()).hexdigest(),
              "native_hotkey_tasks": 2, "authenticated_ipc_tasks": 6, "busy_no_queue": True,
              "native_active_window_captures": 5, "native_active_display_capture": True,
              "native_primary_display_capture": True,
              "held_key_no_repeat": True, "endpoint_private_and_removed": True,
              "pointer_button_does_not_block_hotkey": True,
              "independent_task_pixels_exact": True,
              "persistent_finalized_image_ownership": True, "persistent_uploaded_link_ownership": True,
              "job_idle_while_clipboard_owned": True,
              "alpha_hidden_rgb_clipboard_exact": True,
              "source_user_state_unchanged": True, "private_display_only": True,
              "idle": idle, "elapsed_seconds": round(time.monotonic() - started, 3)}
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
