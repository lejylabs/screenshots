#!/usr/bin/env python3
"""Owned Wayland startup E2E; no user display, input, clipboard or credentials.

Requires the newly built desktop executable, its exact Cargo JSON stream, an
explicitly compiled private public-API harness/record, and private Sway binaries.
Nothing in this runner builds code. All readiness observation is finite.
"""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import resource
import selectors
import signal
import socket
import stat
import struct
import subprocess
import sys
import tempfile
import time
import zlib

OUTPUT_CAP = 1024 * 1024
IPC_CAP = 256 * 1024
OUTER_SECONDS = 180
PRIVATE_DISPLAY = "screenshots-private-inaccessible-display"


def require(value, message):
    if not value:
        raise RuntimeError(message)


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def private_path(path):
    path = Path(path)
    metadata = path.lstat()
    require(stat.S_ISSOCK(metadata.st_mode) and metadata.st_uid == os.geteuid(),
            "Expected an owned private socket")
    return path


def process_identity(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    return fields[19]


class Process:
    """Bounded pipe observation; keep exited leaders unreaped until group cleanup."""
    def __init__(self, command, env, cwd, pass_fds=(), connect=None, cap=OUTPUT_CAP):
        def before_exec():
            os.umask(0o077)
            resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
            if connect is not None:
                stream, path = connect
                # Connect in this exec child: Sway's SO_PEERCRED then names this
                # actual desktop PID, rather than the Python socket creator.
                stream.settimeout(2)
                stream.connect(str(path))
                stream.settimeout(None)
        self.child = subprocess.Popen(command, env=env, cwd=cwd, pass_fds=pass_fds,
                                      stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                      stderr=subprocess.PIPE, start_new_session=True,
                                      preexec_fn=before_exec)
        self.pid = self.child.pid
        self.identity = process_identity(self.pid)
        self.cap = cap
        self.output = {"stdout": bytearray(), "stderr": bytearray()}
        self.selector = selectors.DefaultSelector()
        self.closed = False
        for label in self.output:
            stream = getattr(self.child, label)
            os.set_blocking(stream.fileno(), False)
            self.selector.register(stream, selectors.EVENT_READ, label)

    def status(self):
        # WNOWAIT prevents PID/PGID reuse before cleanup, including when a
        # failed native process leaves a descendant in its owned session.
        value = os.waitid(os.P_PID, self.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT)
        if value is None:
            return None
        return value.si_status if value.si_code == os.CLD_EXITED else -value.si_status

    def pump(self, timeout=0):
        for key, _ in self.selector.select(timeout):
            try:
                chunk = os.read(key.fileobj.fileno(), 65536)
            except BlockingIOError:
                continue
            if not chunk:
                self.selector.unregister(key.fileobj)
                continue
            target = self.output[key.data]
            room = self.cap - len(target)
            target.extend(chunk[:room])
            require(len(chunk) <= room, f"Owned process exceeded {self.cap}-byte {key.data} cap")

    def wait(self, seconds):
        deadline = time.monotonic() + seconds
        while self.status() is None:
            require(time.monotonic() < deadline, "Owned process exceeded its deadline")
            self.pump(min(.05, max(0, deadline - time.monotonic())))
        for _ in range(8):
            self.pump(0)
        return self.status()

    def text(self, label):
        return bytes(self.output[label]).decode(errors="replace")

    def signal(self, number):
        require(process_identity(self.pid) == self.identity, "Owned process identity changed")
        try:
            os.killpg(self.pid, number)
        except ProcessLookupError:
            pass

    def close(self):
        if self.closed:
            return
        try:
            if self.status() is None:
                self.signal(signal.SIGTERM)
                until = time.monotonic() + 3
                while self.status() is None and time.monotonic() < until:
                    try:
                        self.pump(.05)
                    except RuntimeError:
                        break
            # The leader remains unreaped, so this PGID is still this exact
            # owned session even if descendants survived its natural exit.
            self.signal(signal.SIGKILL)
            self.child.wait(timeout=3)
        finally:
            self.selector.close()
            for label in self.output:
                getattr(self.child, label).close()
            self.closed = True


def environment(root, library_dir, runtime=None):
    paths = {name: root / name.lower() for name in (
        "HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME",
        "XDG_CONFIG_DIRS", "XDG_DATA_DIRS", "TMPDIR")}
    for path in paths.values():
        path.mkdir(mode=0o700, parents=True, exist_ok=True)
    runtime = runtime or root / "runtime"
    runtime.mkdir(mode=0o700, parents=True, exist_ok=True)
    authority = root / "empty-xauthority"
    authority.touch(mode=0o600)
    # Start from nothing: no user session buses, socket overrides, loaders,
    # configuration, credentials, proxy variables or profile restoration.
    return dict({name: str(path) for name, path in paths.items()},
                PATH="/usr/bin:/bin", LANG="C.UTF-8", USER="screenshots-private",
                LOGNAME="screenshots-private", XDG_RUNTIME_DIR=str(runtime),
                XAUTHORITY=str(authority),
                DBUS_SESSION_BUS_ADDRESS=f"unix:path={root}/no-session-bus",
                DBUS_SYSTEM_BUS_ADDRESS=f"unix:path={root}/no-system-bus",
                LD_LIBRARY_PATH=str(library_dir), LIBGL_ALWAYS_SOFTWARE="1",
                MESA_SHADER_CACHE_DISABLE="true")


def tiny_png():
    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body) & 0xffffffff)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 2, 1, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(bytes([0, 11, 23, 37, 255, 43, 59, 71, 128]))) + chunk(b"IEND", b""))


def tree_nodes(tree):
    yield tree
    for child in tree.get("nodes", []) + tree.get("floating_nodes", []):
        yield from tree_nodes(child)


def ipc(swaymsg, path, env, cwd, *arguments):
    private_path(path)
    command = Process([swaymsg, "-s", str(path), "-r", *arguments], env, cwd, cap=IPC_CAP)
    try:
        code = command.wait(2)
        require(code == 0, "Private Sway IPC command failed: " + command.text("stderr")[-2000:])
        return json.loads(command.text("stdout"))
    finally:
        command.close()


def own_views(swaymsg, path, env, cwd, pid):
    tree = ipc(swaymsg, path, env, cwd, "-t", "get_tree")
    return [node for node in tree_nodes(tree) if node.get("pid") == pid
            and (node.get("shell") == "xdg_shell" or node.get("window") is not None)]


def closed_descriptor():
    limit = resource.getrlimit(resource.RLIMIT_NOFILE)[0]
    require(0 <= limit < 2**31 - 1024, "Cannot establish a guaranteed closed private FD number")
    return limit + 512  # Above the inherited process's descriptor allocation limit.


def descriptor_environment(env, state):
    result = dict(env, DISPLAY=PRIVATE_DISPLAY)
    require("WAYLAND_DISPLAY" not in result and "WAYLAND_SOCKET" not in result, "Descriptor-only context required")
    if state == "malformed":
        result["WAYLAND_SOCKET"] = "not-a-descriptor"
    elif state == "closed":
        result["WAYLAND_SOCKET"] = str(closed_descriptor())
    return result


def desktop_case(binary, route, state, case_root, base_env, server, swaymsg, ipc_path, wayland_socket):
    case_root.mkdir(mode=0o700)
    env = descriptor_environment(environment(case_root, Path(base_env["LD_LIBRARY_PATH"]),
                                              Path(base_env["XDG_RUNTIME_DIR"])), state)
    image = case_root / "synthetic.png"
    image.write_bytes(tiny_png())
    arguments = {"normal": [], "open": ["--open", str(image)],
                 "probe": ["--resource-probe=empty", "--probe-seconds=10"]}[route]
    stream = None
    app = None
    try:
        passed = ()
        connection = None
        if state == "valid":
            stream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            env["WAYLAND_SOCKET"] = str(stream.fileno())
            passed = (stream.fileno(),)
            connection = (stream, wayland_socket)
        app = Process([binary, *arguments], env, case_root, passed, connection)
        if stream is not None:
            stream.close()
            stream = None
        if state != "valid":
            code = app.wait(10)
            require(code != 0 and "WAYLAND_SOCKET" in app.text("stderr"),
                    "Invalid inherited authority did not fail closed with its own error")
            require(not own_views(swaymsg, ipc_path, base_env, case_root, app.pid),
                    "Invalid descriptor created a private GUI window")
            return dict(route=route, descriptor=state, pid=app.pid, exit=code, fail_closed=True)
        deadline = time.monotonic() + 15
        observed = []
        expected_title = {"normal": "screenshots", "open": "Edit a copy", "probe": "screenshots resource probe"}[route]
        while time.monotonic() < deadline:
            server.pump(0)
            require(server.status() is None, "Private compositor exited")
            app.pump(0)
            require(app.status() is None, "Desktop exited before its private window: " + app.text("stderr")[-3000:])
            observed = own_views(swaymsg, ipc_path, base_env, case_root, app.pid)
            if any(expected_title in (node.get("name") or "") for node in observed):
                break
            time.sleep(.05)
        else:
            task_root = Path(f"/proc/{app.pid}/task")
            tasks = {path.name: (path / "wchan").read_text()[:100]
                     for path in list(task_root.iterdir())[:64]}
            tree = ipc(swaymsg, ipc_path, base_env, case_root, "-t", "get_tree")
            raise RuntimeError("Actual descriptor-only GUI window readiness timed out: "
                               + json.dumps(observed)[:2000] + "; desktop: "
                               + app.text("stderr")[-2000:] + "; compositor: "
                               + server.text("stderr")[-2000:] + "; tasks: "
                               + json.dumps(tasks) + "; tree: " + json.dumps(tree)[:4000])
        require(all(node.get("window") is None and node.get("shell") == "xdg_shell" for node in observed),
                "Expected actual Wayland surfaces while private XWayland is disabled")
        response = ipc(swaymsg, ipc_path, base_env, case_root, f"[pid={app.pid}] kill")
        require(isinstance(response, list) and response and all(item.get("success") for item in response),
                "Private PID-scoped close was rejected")
        code = app.wait(8)
        require(code == 0, "Private GUI teardown failed: " + app.text("stderr")[-3000:])
        require(not own_views(swaymsg, ipc_path, base_env, case_root, app.pid), "Private GUI window survived teardown")
        return dict(route=route, descriptor=state, pid=app.pid, exit=code,
                    wayland_windows=[dict(id=n["id"], name=n.get("name"), app_id=n.get("app_id"), shell=n.get("shell")) for n in observed],
                    private_pid_ipc_close=True)
    finally:
        if stream is not None:
            stream.close()
        if app is not None:
            app.close()


def desktop_headless_case(binary, root, library_dir):
    root.mkdir(mode=0o700)
    env = descriptor_environment(environment(root, library_dir), "valid")
    left, peer = socket.socketpair()
    app = None
    try:
        env["WAYLAND_SOCKET"] = str(left.fileno())
        app = Process([binary, "--resource-probe=headless", "--probe-seconds=10"], env, root, (left.fileno(),))
        left.close()
        peer.setblocking(False)
        until = time.monotonic() + 7
        while time.monotonic() < until:
            app.pump(0)
            require(app.status() is None, "Headless probe exited before unused FD retirement observation")
            try:
                value = peer.recv(1)
            except BlockingIOError:
                value = None
            if value == b"":
                break
            require(value is None, "Headless probe sent a Wayland protocol request on unused authority")
            time.sleep(.025)
        else:
            raise RuntimeError("Headless probe retained its unused inherited socket")
        require(app.wait(14) == 0, "Headless probe failed")
        return dict(route="desktop_headless_probe", unused_fd_retired_before_exit=True, exit=0)
    finally:
        left.close()
        peer.close()
        if app is not None:
            app.close()


def public_api_case(harness, route, state, root, library_dir):
    root.mkdir(mode=0o700)
    env = descriptor_environment(environment(root, library_dir), state)
    env.update(SCREENSHOTS_PRIVATE_WAYLAND_STARTUP=str(root),
               SCREENSHOTS_HEADLESS_ROUTE=route, SCREENSHOTS_HEADLESS_DESCRIPTOR=state)
    if route == "zed_headless":
        env["ZED_HEADLESS"] = "1"
    pair = None
    child = None
    try:
        passed = ()
        if state == "valid":
            pair = socket.socketpair()
            env["WAYLAND_SOCKET"] = str(pair[0].fileno())
            passed = (pair[0].fileno(),)
        child = Process([harness, "--exact", "public_headless_startup_retires_inherited_wayland_socket",
                         "--ignored", "--nocapture", "--test-threads=1"], env, root, passed)
        if pair:
            pair[0].close()
        require(child.wait(10) == 0, "Public headless API failed: " + child.text("stderr")[-3000:])
        lines = [line.split("SCREENSHOTS_HEADLESS_OK ", 1)[1] for line in child.text("stdout").splitlines()
                 if "SCREENSHOTS_HEADLESS_OK " in line]
        require(len(lines) == 1, "Missing exact public-API success marker")
        value = json.loads(lines[0])
        require(value == dict(route=route, descriptor=state, headless=True, unused_fd_retired=True),
                "Public API result did not match the owned fixture")
        return value
    finally:
        if pair:
            pair[0].close()
            pair[1].close()
        if child is not None:
            child.close()


def cargo_records(path):
    require(Path(path).stat().st_size <= 64 * 1024 * 1024, "Cargo JSON exceeds the private 64 MiB metadata ceiling")
    records = []
    with Path(path).open() as stream:
        for line in stream:
            if line.strip():
                records.append(json.loads(line))
    require(any(v.get("reason") == "build-finished" and v.get("success") for v in records), "Cargo stream lacks successful build completion")
    return records


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--cargo-json", required=True)
    parser.add_argument("--public-api-harness", required=True)
    parser.add_argument("--harness-record", required=True)
    parser.add_argument("--compositor", required=True)
    parser.add_argument("--swaymsg", required=True)
    parser.add_argument("--library-dir", required=True)
    parser.add_argument("--report", required=True, type=Path)
    args = parser.parse_args()
    require(sys.platform.startswith("linux"), "This private native runner is Linux-only")
    binary, harness, compositor, swaymsg = [str(Path(value).resolve(strict=True)) for value in (
        args.binary, args.public_api_harness, args.compositor, args.swaymsg)]
    library_dir = Path(args.library_dir).resolve(strict=True)
    records = cargo_records(args.cargo_json)
    require(any(v.get("reason") == "compiler-artifact" and v.get("target", {}).get("name") == "screenshots"
                and "bin" in v.get("target", {}).get("kind", []) and not v.get("profile", {}).get("test")
                and v.get("executable") and Path(v["executable"]).resolve() == Path(binary) for v in records),
            "Desktop image is not the exact non-test binary in the supplied Cargo JSON")
    linkage = json.loads(Path(args.harness_record).read_text())
    require(linkage.get("cargo_json_sha256") == digest(args.cargo_json)
            and linkage.get("harness_sha256") == digest(harness)
            and linkage.get("output") == harness
            and linkage.get("source_sha256") == digest(linkage["source"])
            and linkage.get("gpui_kit_rlib_sha256") == digest(linkage["gpui_kit_rlib"]),
            "Public harness linkage record does not match exact current artifacts")
    args.report.parent.mkdir(parents=True, exist_ok=True)
    require(not args.report.exists(), "Refusing to overwrite prior native verification evidence")
    report = dict(status="running", binary_sha256=digest(binary), harness_sha256=digest(harness),
                  cargo_json_sha256=digest(args.cargo_json), compositor_sha256=digest(compositor),
                  swaymsg_sha256=digest(swaymsg), checks=[], privacy="private Sway/headless only; no capture, clipboard, credentials or user input")
    def deadline(_signal, _frame):
        raise RuntimeError("Private Wayland runner outer deadline exceeded")
    previous = signal.signal(signal.SIGALRM, deadline)
    signal.setitimer(signal.ITIMER_REAL, OUTER_SECONDS)
    server = None
    try:
        with contextlib.ExitStack() as cleanup:
            temporary = cleanup.enter_context(tempfile.TemporaryDirectory(prefix="screenshots-private-wayland-startup-"))
            root = Path(temporary)
            base = environment(root, library_dir)
            config = root / "sway.conf"
            config.write_text("xwayland disable\noutput * mode 1024x768\nfocus_follows_mouse no\nseat seat0 fallback true\ndefault_border none\n")
            server_env = dict(base, WLR_BACKENDS="headless", WLR_HEADLESS_OUTPUTS="1",
                              WLR_LIBINPUT_NO_DEVICES="1", WLR_RENDERER="pixman")
            server = Process([compositor, "-c", str(config), "--unsupported-gpu"], server_env, root)
            cleanup.callback(server.close)  # Before private socket/config directories are removed.
            until = time.monotonic() + 10
            ipc_path = wayland_socket = None
            while time.monotonic() < until:
                server.pump(.025)
                require(server.status() is None, "Private compositor failed: " + server.text("stderr")[-3000:])
                runtime = Path(base["XDG_RUNTIME_DIR"])
                sockets = [p for p in runtime.glob("wayland-*") if stat.S_ISSOCK(p.lstat().st_mode)]
                ipcs = [p for p in runtime.glob("sway-ipc.*.sock") if stat.S_ISSOCK(p.lstat().st_mode)]
                if len(sockets) == 1 and len(ipcs) == 1:
                    wayland_socket, ipc_path = private_path(sockets[0]), private_path(ipcs[0])
                    break
            require(wayland_socket is not None and ipc_path is not None, "Private compositor readiness timed out")
            for route in ("normal", "open", "probe"):
                for state in ("valid", "malformed", "closed"):
                    report["checks"].append(desktop_case(binary, route, state, root / f"{route}-{state}",
                                                         base, server, swaymsg, ipc_path, wayland_socket))
                    server.pump(0)
            report["checks"].append(desktop_headless_case(binary, root / "desktop-headless", library_dir))
            for route in ("headless", "current_true", "zed_headless"):
                for state in ("valid", "malformed", "closed"):
                    case = root / f"api-{route}-{state}"
                    report["checks"].append(public_api_case(harness, route, state, case, library_dir))
                    server.pump(0)
                    require(server.status() is None, "Private compositor exited during headless checks")
            require(digest(binary) == report["binary_sha256"] and digest(harness) == report["harness_sha256"],
                    "A tested executable changed during verification")
            report["status"] = "passed"
    except Exception as error:
        report["status"] = "failed"
        report["error"] = str(error)[:5000]
    finally:
        try:
            if server is not None:
                server.close()
        except Exception as error:
            report["status"] = "failed"
            report["cleanup_error"] = str(error)[:1000]
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous)
        args.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(dict(status=report["status"], checks=len(report["checks"]), report=str(args.report))))
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
