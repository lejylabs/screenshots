#!/usr/bin/env python3
"""Explicit clipboard tests on an authenticated private Xvfb selection only."""
import argparse
import ctypes as c
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import secrets
import select
import selectors
import socket
import shutil
import signal
import struct
import subprocess
import tempfile
import threading
import time
import zlib


def require(value, message):
    if not value:
        raise RuntimeError(message)


def event_flood(binary, env, owner, output):
    # Reuse the finite private-worker observer; it only inspects this CLI family.
    spec = importlib.util.spec_from_file_location(
        "clipboard_private_worker_observer", Path(__file__).with_name("test-capture-worker-performance.py"))
    observer = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(observer)
    child = subprocess.Popen([binary, "clipboard", "image", str(output)], env=env,
                             stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    tracked = {}
    poller = select.poll()
    maximum = {}
    limits = []
    samples = 0
    started = time.monotonic()
    released = False
    try:
        while child.poll() is None:
            require(time.monotonic() - started < 8, "Private event-flood operation exceeded its deadline")
            parents = [child.pid, *tracked]
            for parent in parents:
                if parent in tracked:
                    try:
                        if observer.identity(parent) != tracked[parent][0]:
                            continue
                    except (FileNotFoundError, ProcessLookupError):
                        continue
                for pid in observer.children(parent):
                    if pid in tracked:
                        continue
                    try:
                        # Check lineage and identity around pidfd admission; never
                        # signal a process through a potentially reused numeric PID.
                        status = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
                        if int(status[1]) != parent:
                            continue
                        original = observer.identity(pid)
                        if original[0] != int(status[19]):
                            continue
                        require(len(tracked) < 32, "Private helper-family observation exceeds32 processes")
                        descriptor = os.pidfd_open(pid)
                        try:
                            if observer.identity(pid) != original:
                                continue
                            tracked[pid] = (original, descriptor)
                            poller.register(descriptor, select.POLLIN)
                            descriptor = None
                        finally:
                            if descriptor is not None:
                                os.close(descriptor)
                    except (FileNotFoundError, ProcessLookupError):
                        continue
            group = {}
            for pid in [child.pid, *tracked]:
                memory = observer.process_memory(pid)
                if memory and (pid == child.pid or memory[0] == tracked[pid][0]):
                    for name, value in memory[1].items():
                        group[name] = group.get(name, 0) + value
            if group:
                samples += 1
                for name, value in group.items():
                    maximum[name] = max(maximum.get(name, 0), value)
            if owner.flood_started.is_set() and tracked and not released:
                for pid in tracked:
                    try:
                        line = next(line for line in Path(f"/proc/{pid}/limits").read_text().splitlines()
                                    if line.startswith("Max address space"))
                        limits.append({"soft_bytes": int(line.split()[3]), "hard_bytes": int(line.split()[4])})
                    except (FileNotFoundError, ProcessLookupError, ValueError, StopIteration):
                        pass
                owner.flood_release.set()
                released = True
            time.sleep(.002)  # Finite fixture observer, never a production idle sampler.
        stdout, stderr = child.communicate(timeout=2)
        require(child.returncode != 0 and not stdout, "Event flood produced success or output")
        require(b"private-flood-value" not in stderr, "Event flood leaked clipboard content")
        require(released and owner.flood_sent == 5000, "Private genuine event flood was not fully emitted")
        require(not output.exists(), "Event flood published an image")
        exited = {fd for fd, _ in poller.poll(0)}
        cleanup_started = time.monotonic()
        while len(exited) < len(tracked) and time.monotonic() - cleanup_started < 1:
            exited.update(fd for fd, _ in poller.poll(10))
        require(len(exited) == len(tracked), "Event flood retained an observed owned helper")
        return {"genuine_xfixes_notifications_before_targets_reply": owner.flood_sent,
                "elapsed_ms": (time.monotonic() - started) * 1000,
                "observed_helper_count": len(tracked), "all_observed_helper_pidfds_exited": True,
                "memory_samples": samples, "maximum_observed_near_simultaneous_sum_kib": maximum,
                "observed_helper_address_space_limits": limits, "hardware": observer.hardware(),
                "caveat": "Finite2ms observer may miss peaks; sequential smaps sums include shared RSS multiple times. These are observations, not upper-bound/private-footprint or reference-hardware acceptance claims."}
    finally:
        owner.flood_release.set()
        if child.poll() is None:
            child.kill()
            child.communicate(timeout=5)
        for _, descriptor in tracked.values():
            try:
                signal.pidfd_send_signal(descriptor, signal.SIGKILL)
            except ProcessLookupError:
                pass
            finally:
                os.close(descriptor)


class SelectionRequest(c.Structure):
    _fields_ = [("kind", c.c_int), ("serial", c.c_ulong), ("send", c.c_int), ("display", c.c_void_p),
                ("owner", c.c_ulong), ("requestor", c.c_ulong), ("selection", c.c_ulong),
                ("target", c.c_ulong), ("property", c.c_ulong), ("time", c.c_ulong)]


class Selection(c.Structure):
    _fields_ = [("kind", c.c_int), ("serial", c.c_ulong), ("send", c.c_int), ("display", c.c_void_p),
                ("requestor", c.c_ulong), ("selection", c.c_ulong), ("target", c.c_ulong),
                ("property", c.c_ulong), ("time", c.c_ulong)]


class Property(c.Structure):
    _fields_ = [("kind", c.c_int), ("serial", c.c_ulong), ("send", c.c_int), ("display", c.c_void_p),
                ("window", c.c_ulong), ("atom", c.c_ulong), ("time", c.c_ulong), ("state", c.c_int)]


class Event(c.Union):
    _fields_ = [("kind", c.c_int), ("request", SelectionRequest), ("selection", Selection),
                ("property", Property), ("pad", c.c_long * 24)]


def image():
    state = 17
    pixels = bytearray()
    for _ in range(512 * 128):
        for _ in range(4):
            state = (state * 1664525 + 1013904223) & 0xffffffff
            pixels.append((state >> 24) & 255)
    raw = b"".join(b"\0" + pixels[y * 2048:(y + 1) * 2048] for y in range(128))
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xffffffff)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 512, 128, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")), bytes(pixels)


class Owner:
    def __init__(self, display, authority):
        self.x = x = c.CDLL("libX11.so.6")
        x.XInitThreads.restype = c.c_int
        require(x.XInitThreads(), "Cannot initialize private Xlib thread safety")
        x.XOpenDisplay.argtypes, x.XOpenDisplay.restype = [c.c_char_p], c.c_void_p
        x.XDefaultRootWindow.argtypes, x.XDefaultRootWindow.restype = [c.c_void_p], c.c_ulong
        x.XCreateSimpleWindow.argtypes = [c.c_void_p, c.c_ulong, c.c_int, c.c_int, c.c_uint, c.c_uint, c.c_uint, c.c_ulong, c.c_ulong]
        x.XCreateSimpleWindow.restype = c.c_ulong
        x.XInternAtom.argtypes, x.XInternAtom.restype = [c.c_void_p, c.c_char_p, c.c_int], c.c_ulong
        x.XSelectInput.argtypes = [c.c_void_p, c.c_ulong, c.c_long]
        x.XSetSelectionOwner.argtypes = [c.c_void_p, c.c_ulong, c.c_ulong, c.c_ulong]
        x.XGrabServer.argtypes, x.XUngrabServer.argtypes = [c.c_void_p], [c.c_void_p]
        x.XGetSelectionOwner.argtypes, x.XGetSelectionOwner.restype = [c.c_void_p, c.c_ulong], c.c_ulong
        x.XChangeProperty.argtypes = [c.c_void_p, c.c_ulong, c.c_ulong, c.c_ulong, c.c_int, c.c_int, c.c_void_p, c.c_int]
        x.XSendEvent.argtypes = [c.c_void_p, c.c_ulong, c.c_int, c.c_long, c.POINTER(Event)]
        x.XPending.argtypes, x.XNextEvent.argtypes = [c.c_void_p], [c.c_void_p, c.POINTER(Event)]
        x.XConnectionNumber.argtypes, x.XConnectionNumber.restype = [c.c_void_p], c.c_int
        x.XFlush.argtypes, x.XCloseDisplay.argtypes = [c.c_void_p], [c.c_void_p]
        self.error_handler = c.CFUNCTYPE(c.c_int, c.c_void_p, c.c_void_p)(lambda *_: 0)
        x.XSetErrorHandler.argtypes = [c.c_void_p]
        x.XSetErrorHandler(self.error_handler)
        previous = os.environ.get("XAUTHORITY")
        os.environ["XAUTHORITY"] = str(authority)
        try:
            self.connection = x.XOpenDisplay(display.encode())
        finally:
            if previous is None:
                os.environ.pop("XAUTHORITY", None)
            else:
                os.environ["XAUTHORITY"] = previous
        require(self.connection, "Cannot connect private selection owner")
        self.window = x.XCreateSimpleWindow(self.connection, x.XDefaultRootWindow(self.connection), 0, 0, 1, 1, 0, 0, 0)
        self.alternate_window = x.XCreateSimpleWindow(self.connection, x.XDefaultRootWindow(self.connection), 0, 0, 1, 1, 0, 0, 0)
        self.requests = 0
        self.flood_started = threading.Event()
        self.flood_release = threading.Event()
        self.flood_sent = 0
        self.atoms = {name: x.XInternAtom(self.connection, name.encode(), 0) for name in
                      ("CLIPBOARD", "TARGETS", "UTF8_STRING", "text/uri-list", "image/png", "INCR")}
        x.XSetSelectionOwner(self.connection, self.atoms["CLIPBOARD"], self.window, 0)
        x.XFlush(self.connection)
        self.configuration = {}
        self.lock = threading.Lock()
        self.stop = threading.Event()
        self.pending = {}
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()

    def set(self, payloads, mode="direct", hint=None, chunk=32768, generation_change=None, flood=0):
        with self.lock:
            self.configuration = {"payloads": {self.atoms[k]: v for k, v in payloads.items()}, "mode": mode,
                                  "hint": hint, "chunk": chunk, "generation_change": generation_change, "flood": flood}
            self.pending.clear()
            self.flood_started.clear()
            self.flood_release.clear()
            self.flood_sent = 0

    def property(self, window, atom, kind, data, bits=8):
        if bits == 32:
            value = (c.c_ulong * len(data))(*data)
            count = len(data)
        else:
            value = (c.c_ubyte * len(data)).from_buffer_copy(data)
            count = len(data)
        self.x.XChangeProperty(self.connection, window, atom, kind, bits, 0, value, count)
        self.x.XFlush(self.connection)

    def serve(self):
        selector = selectors.DefaultSelector()
        selector.register(self.x.XConnectionNumber(self.connection), selectors.EVENT_READ)
        try:
            while not self.stop.is_set():
                if not self.x.XPending(self.connection):
                    selector.select(.05)  # Fixture-only active private owner lifetime.
                    continue
                event = Event()
                self.x.XNextEvent(self.connection, c.byref(event))
                with self.lock:
                    config = dict(self.configuration)
                    if event.kind == 30:
                        self.requests += 1
                        request = event.request
                        prop = request.property or request.target
                        payloads = config.get("payloads", {})
                        reply_property = prop
                        if request.target == self.atoms["TARGETS"]:
                            if config.get("flood"):
                                self.flood_started.set()
                                require(self.flood_release.wait(timeout=2), "Private flood observer did not release its gate")
                                # This server belongs exclusively to the fixture. Hold
                                # other requests until all genuine notifications precede
                                # the requestor's ordered reply and our TARGETS reply.
                                self.x.XGrabServer(self.connection)
                                try:
                                    for _ in range(config["flood"]):
                                        self.x.XSetSelectionOwner(self.connection, self.atoms["CLIPBOARD"], self.window, 0)
                                    self.flood_sent = config["flood"]
                                finally:
                                    self.x.XUngrabServer(self.connection)
                                    self.x.XFlush(self.connection)
                            if config.get("generation_change") in ("same", "aba"):
                                if config["generation_change"] == "aba":
                                    self.x.XSetSelectionOwner(self.connection, self.atoms["CLIPBOARD"], self.alternate_window, 0)
                                self.x.XSetSelectionOwner(self.connection, self.atoms["CLIPBOARD"], self.window, 0)
                                self.x.XFlush(self.connection)
                            self.property(request.requestor, prop, 4, [self.atoms["TARGETS"], *payloads], 32)
                        elif request.target not in payloads or config.get("mode") == "stall":
                            if config.get("mode") == "stall":
                                continue
                            reply_property = 0
                        elif config.get("mode") == "incr":
                            data = payloads[request.target]
                            hint = config.get("hint")
                            self.x.XSelectInput(self.connection, request.requestor, 1 << 22)
                            self.pending[(request.requestor, prop)] = [data, 0, config["chunk"], request.target]
                            self.property(request.requestor, prop, self.atoms["INCR"], [len(data) if hint is None else hint], 32)
                        else:
                            self.property(request.requestor, prop, request.target, payloads[request.target])
                        reply = Event()
                        reply.selection = Selection(31, 0, 1, self.connection, request.requestor, request.selection,
                                                    request.target, reply_property, request.time)
                        self.x.XSendEvent(self.connection, request.requestor, 0, 0, c.byref(reply))
                        self.x.XFlush(self.connection)
                    elif event.kind == 28 and event.property.state == 1:
                        key = (event.property.window, event.property.atom)
                        if key in self.pending:
                            data, offset, chunk, target = self.pending[key]
                            if offset > 0 and config.get("generation_change") == "incr_same":
                                self.x.XSetSelectionOwner(self.connection, self.atoms["CLIPBOARD"], self.window, 0)
                                self.x.XFlush(self.connection)
                                self.configuration["generation_change"] = None
                            part = data[offset:offset + chunk]
                            self.property(key[0], key[1], target, part)
                            self.pending[key][1] += len(part)
                            if not part:
                                del self.pending[key]
        finally:
            selector.close()

    def close(self):
        self.stop.set()
        self.thread.join(timeout=2)
        self.x.XCloseDisplay(self.connection)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--xvfb", default=shutil.which("Xvfb"))
    parser.add_argument("--report", required=True)
    args = parser.parse_args()
    binary = str(Path(args.binary).resolve(strict=True))
    require(args.xvfb, "Private Xvfb executable is required")
    report = {"binary_sha256": hashlib.file_digest(open(binary, "rb"), "sha256").hexdigest(), "checks": []}
    with tempfile.TemporaryDirectory(prefix="screenshots-private-clipboard-") as temporary:
        root = Path(temporary)
        display = next((f":{i}" for i in range(190, 250) if not Path(f"/tmp/.X{i}-lock").exists()
                        and not Path(f"/tmp/.X11-unix/X{i}").exists()), None)
        require(display, "No unused private display")
        authority = root / "authority"
        env = dict(os.environ, DISPLAY=display, XAUTHORITY=str(authority), XDG_SESSION_TYPE="x11")
        for name in ("WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DBUS_SESSION_BUS_ADDRESS"):
            env.pop(name, None)
        for name in ("XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"):
            env[name] = str(root / name)
        subprocess.run(["xauth", "-f", str(authority), "add", display, ".", secrets.token_hex(16)], check=True, capture_output=True, timeout=5)
        server = subprocess.Popen([args.xvfb, display, "-screen", "0", "128x96x24", "-auth", str(authority),
                                   "-nolisten", "tcp", "-noreset"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        owner = None
        try:
            for _ in range(100):
                require(server.poll() is None, "Private Xvfb exited")
                if subprocess.run(["xdpyinfo", "-display", display], env=env, capture_output=True, timeout=2).returncode == 0:
                    break
                time.sleep(.025)
            else:
                raise RuntimeError("Private Xvfb was not ready")
            owner = Owner(display, authority)
            def run(*values, success=True, environment=None, inherited=()):
                result = subprocess.run([binary, "clipboard", *map(str, values)], env=environment or env,
                                        stdin=subprocess.DEVNULL, capture_output=True, timeout=12, pass_fds=inherited)
                require((result.returncode == 0) == success, "Unexpected clipboard result: " + result.stderr.decode(errors="replace"))
                require(not result.stderr if success else not result.stdout, "Clipboard leaked an unexpected output channel")
                return json.loads(result.stdout) if success else result
            owner.set({"UTF8_STRING": b"private synthetic text", "text/uri-list": b"file:///definitely-missing/%FF.png\r\n"})
            value = run("inspect")
            require("UTF8_STRING" in value["formats"] and "text" not in value, "Metadata inspection fetched content")
            require(run("text")["text"] == "private synthetic text", "Text changed")
            files = run("files")["files"]
            require(len(files) == 1 and 255 in files[0]["native"] and files[0]["encoding"] == "unix_bytes", "Native path bytes changed")
            require(owner.x.XGetSelectionOwner(owner.connection, owner.atoms["CLIPBOARD"]) == owner.window, "Read changed clipboard owner")
            report["checks"].append("metadata/text/native file references, no referenced-file access or ownership change")
            data, pixels = image()
            require(len(data) > 65536, "Direct property fixture did not span chunks")
            owner.set({"image/png": data})
            require(run("image-info")["width"] == 512, "Direct segmented image info changed")
            output = root / "image.png"
            run("image", output)
            actual = subprocess.run(["ffmpeg", "-v", "error", "-i", str(output), "-f", "rawvideo", "-pix_fmt", "rgba", "-"], check=True, capture_output=True, timeout=5).stdout
            require(actual == pixels, "PNG alpha/hidden RGB changed")
            prior = output.read_bytes()
            run("image", output, success=False)
            require(output.read_bytes() == prior, "Clipboard output replaced an existing file")
            report["checks"].append("paged direct image offer, exact PNG pixels/alpha, explicit output/no overwrite")
            owner.set({"image/png": data}, mode="incr")
            require(run("image-info")["height"] == 128, "INCR image changed")
            owner.set({"UTF8_STRING": b"x" * 65536}, mode="incr")
            require(len(run("text")["text"]) == 65536, "Boundary INCR text changed")
            report["checks"].append("bounded INCR image and exact text limit")
            owner.set({"UTF8_STRING": b"x" * 65537})
            run("text", success=False)
            owner.set({"UTF8_STRING": b"small"}, mode="incr", hint=65537)
            run("text", success=False)
            owner.set({"UTF8_STRING": b"x" * 65537}, mode="incr", hint=0)
            run("text", success=False)
            report["checks"].append("oversized direct offer, INCR hint and cumulative overflow rejected")
            owner.set({"UTF8_STRING": b"not returned"}, mode="stall")
            started = time.monotonic()
            run("text", success=False)
            require(3 <= time.monotonic() - started < 8, "Native read deadline was not bounded")
            report["checks"].append("stalled owner deadline, no clipboard values in errors")
            owner.set({"UTF8_STRING": b"unrelated X11 value"})
            wayland = dict(env, XDG_SESSION_TYPE="wayland", WAYLAND_DISPLAY="private-nonexistent-wayland")
            result = run("text", success=False, environment=wayland)
            require(b"unrelated X11 value" not in result.stderr, "Wayland CLI read ambient X11 content")
            report["checks"].append("Wayland CLI refuses ambient X11 fallback")
            left, right = socket.socketpair()
            try:
                descriptor = dict(env, WAYLAND_SOCKET=str(left.fileno()))
                descriptor.pop("WAYLAND_DISPLAY", None)
                descriptor.pop("XDG_SESSION_TYPE", None)
                before = owner.requests
                run("text", success=False, environment=descriptor, inherited=(left.fileno(),))
                require(owner.requests == before, "Descriptor-only Wayland context requested unrelated X11 clipboard data")
            finally:
                left.close()
                right.close()
            report["checks"].append("descriptor-only private Wayland socket freezes/refuses context before unrelated X11 request")
            for mutation in ("same", "aba"):
                owner.set({"UTF8_STRING": b"must not be accepted across generation"}, generation_change=mutation)
                run("text", success=False)
            owner.set({"UTF8_STRING": b"x" * 65536}, mode="incr", generation_change="incr_same")
            run("text", success=False)
            report["checks"].append("XFixes same-owner republish/A-B-A across TARGETS and same-owner INCR republish rejected")
            owner.set({"image/png": data, "UTF8_STRING": b"private-flood-value"}, flood=5000)
            report["event_flood_observation"] = event_flood(binary, env, owner, root / "flood-output.png")
            report["checks"].append("5000 genuine XFixes notifications before TARGETS reply fail without output; observed owned helper pidfds all exit")
        finally:
            if owner:
                owner.close()
            server.terminate()
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=5)
    Path(args.report).write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": len(report["checks"]), "report": args.report}))


if __name__ == "__main__":
    main()
