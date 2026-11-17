#!/usr/bin/env python3
"""Actual optimized desktop lifecycle on owned private Sway and Xvfb only.

Reuses the startup runner's bounded process/IPC fixtures. Never accesses user
input, clipboard, screens, buses or credentials. Scope cleanup is inode-bound.
"""
import argparse
import contextlib
import ctypes as c
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import secrets
import signal
import socket
import stat
import struct
import sys
import tempfile
import time

spec = importlib.util.spec_from_file_location("wayland_startup", Path(__file__).with_name("test-wayland-startup.py"))
w = importlib.util.module_from_spec(spec)
spec.loader.exec_module(w)
require, Process = w.require, w.Process
MAGIC = b"screenshots-reopen-v1\n"
_original_wait = Process.wait


def diagnostic_wait(self, seconds):
    try:
        return _original_wait(self, seconds)
    except RuntimeError as error:
        raise RuntimeError(f"Owned PID {self.pid}: {error}; stderr: {self.text('stderr')[-2500:]}") from error


Process.wait = diagnostic_wait


def until(check, processes=(), seconds=15):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        for process in processes:
            process.pump(0)
            require(process.status() is None, "Owned process exited: " + process.text("stderr")[-2500:])
        value = check()
        if value:
            return value
        time.sleep(.04)
    raise RuntimeError("Private native observation timed out")


class Scope:
    def __init__(self, identity):
        self.path = Path(f"/tmp/screenshots-{os.geteuid()}-launch-v1") / ("owner-" + hashlib.sha256(identity.encode()).hexdigest())
        require(not self.path.exists(), "Private scope unexpectedly already exists")
        self.lock_identity = None
        self.sockets = {}

    def observe(self):
        info = self.path.lstat()
        require(stat.S_ISREG(info.st_mode) and info.st_uid == os.geteuid() and stat.S_IMODE(info.st_mode) == 0o600 and info.st_nlink == 1,
                "Unsafe owned scope lock")
        identity = (info.st_dev, info.st_ino)
        require(self.lock_identity in (None, identity), "Owned lock inode changed")
        self.lock_identity = identity
        data = self.path.read_bytes()
        require(len(data) == len(MAGIC) + 81 and data.startswith(MAGIC), "Unexpected owned scope record")
        body = data[len(MAGIC):len(MAGIC)+49]
        require(hashlib.sha256(body).digest() == data[-32:], "Owned scope record checksum failed")
        name = body[:33].decode("ascii")
        require(name[0] == "s" and all(char in "0123456789abcdef" for char in name[1:]), "Unexpected endpoint name")
        endpoint = self.path.parent / name
        device, inode = int.from_bytes(body[33:41], "little"), int.from_bytes(body[41:49], "little")
        self.sockets[endpoint] = (device, inode)
        if endpoint.exists():
            metadata = endpoint.lstat()
            require(stat.S_ISSOCK(metadata.st_mode) and metadata.st_uid == os.geteuid()
                    and stat.S_IMODE(metadata.st_mode) == 0o600 and metadata.st_nlink == 1
                    and (metadata.st_dev, metadata.st_ino) == (device, inode), "Unexpected owned endpoint inode")
        return str(endpoint)

    def cleanup(self):
        # No directory enumeration and no unknown-file deletion, even after failure.
        for path, identity in self.sockets.items():
            if path.exists():
                metadata = path.lstat()
                require(stat.S_ISSOCK(metadata.st_mode) and metadata.st_uid == os.geteuid()
                        and (metadata.st_dev, metadata.st_ino) == identity, "Owned endpoint changed before cleanup")
                path.unlink()
        if self.lock_identity and self.path.exists():
            metadata = self.path.lstat()
            require((metadata.st_dev, metadata.st_ino) == self.lock_identity and stat.S_ISREG(metadata.st_mode)
                    and metadata.st_uid == os.geteuid() and self.path.read_bytes().startswith(MAGIC), "Owned lock changed before cleanup")
            self.path.unlink()


class Desktop:
    def __init__(self, binary, root, library, runtime, cleanup, scope, display=None, wayland=None):
        self.binary, self.root, self.library, self.runtime = binary, root, library, runtime
        self.cleanup, self.scope, self.display, self.wayland = cleanup, scope, display, wayland
        self.counter = 0
        self.children = []

    def launch(self, arguments=(), descriptor=None):
        self.counter += 1
        root = self.root / f"app-{self.counter}"
        root.mkdir(mode=0o700)
        env = w.environment(root, self.library, self.runtime)
        env["DISPLAY"] = self.display or w.PRIVATE_DISPLAY
        passed, connection, stream = (), None, None
        if self.wayland:
            if descriptor is None:
                env["WAYLAND_DISPLAY"] = str(self.wayland)
            else:
                original = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                stream = socket.socket(fileno=fcntl.fcntl(original.fileno(), fcntl.F_DUPFD_CLOEXEC, descriptor))
                original.close()
                require(stream.fileno() == descriptor, "Private requested descriptor already in use")
                env["WAYLAND_SOCKET"] = str(descriptor)
                passed, connection = (descriptor,), (stream, self.wayland)
        else:
            env["XDG_SESSION_TYPE"] = "x11"
            env["XAUTHORITY"] = str(self.root / "authority")
        try:
            process = Process([self.binary, *arguments], env, root, passed, connection)
        finally:
            if stream:
                stream.close()
        process.fixture_root = root
        self.children.append(process)
        self.cleanup.callback(process.close)
        return process


def secondary(desktop, primary, views, descriptor=None):
    start = time.monotonic()
    child = desktop.launch(["--show"], descriptor)
    require(child.wait(7) == 0, "Secondary launch failed: " + child.text("stderr")[-2500:])
    require(primary.status() is None and not views(child.pid), "Secondary became another GUI owner")
    require(not any(child.fixture_root.rglob("*.log")) and not any(path.name == "logs" for path in child.fixture_root.rglob("*")),
            "Forwarded secondary initialized file logging")
    return dict(pid=child.pid, descriptor=descriptor, exit=0, milliseconds=(time.monotonic()-start)*1000,
                no_window=True, no_file_logger=True)


def process_sample(process, expected_binary):
    require(process.status() is None and w.process_identity(process.pid) == process.identity, "Idle PID lifetime changed")
    root = Path(f"/proc/{process.pid}")
    executable = (root / "exe").stat()
    require((root / "exe").resolve() == expected_binary, "Idle executable changed")
    fields = (root / "stat").read_text().rsplit(")", 1)[1].split()
    result = dict(monotonic=time.monotonic(), pid=process.pid, starttime=process.identity,
                  executable_device=executable.st_dev, executable_inode=executable.st_ino,
                  cpu_ticks=int(fields[11])+int(fields[12]), fd_count=len(list((root / "fd").iterdir())), threads={})
    for path in (root / "task").iterdir():
        try:
            fields = (path / "stat").read_text().rsplit(")", 1)[1].split()
            thread = dict(cpu_ticks=int(fields[11])+int(fields[12]), starttime=fields[19], name=(path / "comm").read_text().strip())
            for line in (path / "status").read_text().splitlines():
                key, _, value = line.partition(":")
                if key in ("voluntary_ctxt_switches", "nonvoluntary_ctxt_switches"):
                    thread[key] = int(value.strip())
            result["threads"][path.name] = thread
        except FileNotFoundError:
            continue  # Preserve surviving TIDs; an operation-scoped worker may retire.
    result["thread_count"] = len(result["threads"])
    for line in (root / "status").read_text().splitlines():
        key, _, value = line.partition(":")
        if key in ("VmRSS", "VmHWM", "VmSwap"):
            result[key+"_KiB"] = int(value.split()[0])
    for line in (root / "smaps_rollup").read_text().splitlines():
        key, _, value = line.partition(":")
        if key in ("Rss", "Pss", "Private_Clean", "Private_Dirty", "Private_Hugetlb", "Anonymous", "Swap", "SwapPss"):
            result[key+"_KiB"] = int(value.split()[0])
    result["private_KiB"] = sum(result.get(key+"_KiB", 0) for key in ("Private_Clean", "Private_Dirty", "Private_Hugetlb"))
    result["io"] = {key: int(value.strip()) for key, value in (line.split(":", 1) for line in (root / "io").read_text().splitlines())}
    return result


def idle_observation(args, desktop, views, close, scope, report):
    primary = desktop.launch()
    until(lambda: views(primary.pid), [primary])
    scope.observe()
    until_time = time.monotonic() + 15
    while time.monotonic() < until_time:
        primary.pump(.1)
        require(primary.status() is None, "Idle warmup process exited")
    row = dict(route="five-minute-visible-empty-control", status="running", current_only=True,
               baseline=None, binary_sha256=report["binary_sha256"], clock_ticks_per_second=os.sysconf("SC_CLK_TCK"),
               development_fixture="1024x768 headless Sway/pixman + software graphics; no capture or input", samples=[])
    report["checks"].append(row)
    start = time.monotonic()
    for second in range(0, args.idle_seconds+1, 60):
        while time.monotonic() < start+second:
            primary.pump(min(.1, max(0, start+second-time.monotonic())))
            require(primary.status() is None, "Idle process exited")
        row["samples"].append(process_sample(primary, args.binary))
        args.report.write_text(json.dumps(report, indent=2)+"\n")
        print(json.dumps(dict(idle_sample_seconds=second, pid=primary.pid)), flush=True)
    first, last = row["samples"][0], row["samples"][-1]
    elapsed = last["monotonic"]-first["monotonic"]
    row.update(elapsed_seconds=elapsed,
               cpu_percent_one_core=100*(last["cpu_ticks"]-first["cpu_ticks"])/row["clock_ticks_per_second"]/elapsed,
               after_idle_show=secondary(desktop, primary, views),
               memory_note="Snapshots are retained separately; VmHWM is not summed or treated as a validated peak",
               limits="Development-only current observation; no exact slice baseline, GPU accounting, physical presentation/input or reference-hardware acceptance")
    require(views(primary.pid), "Explicit Show after idle lost its existing window")
    close(primary.pid, views(primary.pid))
    require(primary.wait(10) == 0, "Idle primary close failed")
    row["status"] = "passed"


def exercise(desktop, views, close, hide, scope, report, route):
    primary = desktop.launch(descriptor=100 if route == "wayland-inherited" else None)
    presented = lambda pid: [node for node in views(pid) if node.get("mapped", True)]
    first = until(lambda: presented(primary.pid), [primary])
    endpoint = scope.observe()
    row = dict(route=route, primary=primary.pid, endpoint=endpoint, windows=first, secondaries=[])
    report["checks"].append(row)
    row["stage"] = "secondary-show"
    row["secondaries"].append(secondary(desktop, primary, views, 101 if route == "wayland-inherited" else None))
    hide(primary.pid, first)
    row["secondaries"].append(secondary(desktop, primary, views, 102 if route == "wayland-inherited" else None))
    row["after_show_request"] = until(lambda: presented(primary.pid), [primary])
    row["stage"] = "primary-close"
    close(primary.pid, row["after_show_request"])
    require(primary.wait(10) == 0 and not views(primary.pid), "Close did not quit the primary cleanly")
    require(not Path(endpoint).exists(), "Clean quit retained its endpoint")
    row["stage"] = "restart-crash"
    restarted = desktop.launch()
    until(lambda: presented(restarted.pid), [restarted])
    crash_endpoint = scope.observe()
    restarted.signal(signal.SIGKILL)
    require(restarted.wait(5) == -signal.SIGKILL, "Owned crash fixture did not die")
    require(Path(crash_endpoint).exists(), "Crash fixture did not leave a recovery endpoint")
    recovered = desktop.launch()
    recovered_views = until(lambda: presented(recovered.pid), [recovered])
    scope.observe()
    require(not Path(crash_endpoint).exists(), "Restart did not remove its exact stale endpoint")
    close(recovered.pid, recovered_views)
    require(recovered.wait(10) == 0, "Recovered owner close failed")
    row["stage"] = "race"
    race = [desktop.launch() for _ in range(4)]
    until(lambda: sum(child.status() is not None for child in race) == 3, seconds=12)
    owners = [child for child in race if child.status() is None]
    require(len(owners) == 1 and all(child.status() in (None, 0) for child in race), "Concurrent launch did not elect exactly one owner")
    winner = owners[0]
    winner_views = until(lambda: presented(winner.pid), [winner])
    scope.observe()
    require(all(not views(child.pid) for child in race if child is not winner), "Race created extra windows")
    row.update(clean_quit=True, restart=True, crash_recovery=True, race_pids=[child.pid for child in race], race_owner=winner.pid)
    # Separate editors must bypass the main Show-only protocol and preserve each file.
    row["stage"] = "separate-editors"
    images = []
    for number in range(2):
        image = desktop.root / f"synthetic-{number}.png"
        image.write_bytes(w.tiny_png())
        editor = desktop.launch(["--open", str(image)])
        windows = until(lambda: [node for node in views(editor.pid) if "Edit a copy" in (node.get("name") or "")], [winner, editor])
        require(w.digest(image) == hashlib.sha256(w.tiny_png()).hexdigest(), "Editor changed its source image")
        images.append((editor, windows))
    row["separate_editors"] = [dict(pid=editor.pid, windows=windows, source_unchanged=True) for editor, windows in images]
    for editor, windows in images:
        close(editor.pid, windows)
        require(editor.wait(10) == 0, "Separate editor close failed")
    row["stage"] = "probe"
    probe = desktop.launch(["--resource-probe=empty", "--probe-seconds=10"])
    probe_views = until(lambda: [node for node in views(probe.pid) if "resource probe" in (node.get("name") or "")], [probe, winner])
    close(probe.pid, probe_views)
    require(probe.wait(10) == 0 and winner.status() is None, "Resource probe did not bypass main ownership")
    row["probe_bypass"] = True
    close(winner.pid, winner_views)
    require(winner.wait(10) == 0, "Race winner close failed")
    row["stage"] = "passed"


def sway_case(args, root, cleanup, report):
    base = w.environment(root, args.library_dir)
    config = root / "sway.conf"
    config.write_text("xwayland disable\noutput * mode 1024x768\nfocus_follows_mouse no\nseat seat0 fallback true\ndefault_border none\n")
    server = Process([args.compositor, "-c", str(config), "--unsupported-gpu"],
                     dict(base, WLR_BACKENDS="headless", WLR_HEADLESS_OUTPUTS="1", WLR_LIBINPUT_NO_DEVICES="1", WLR_RENDERER="pixman"), root)
    cleanup.callback(server.close)
    runtime = Path(base["XDG_RUNTIME_DIR"])
    def ready():
        sockets = [path for path in runtime.glob("wayland-*") if stat.S_ISSOCK(path.lstat().st_mode)]
        ipcs = [path for path in runtime.glob("sway-ipc.*.sock") if stat.S_ISSOCK(path.lstat().st_mode)]
        return (w.private_path(sockets[0]), w.private_path(ipcs[0])) if len(sockets) == len(ipcs) == 1 else None
    wayland, ipc_path = until(ready, [server], 10)
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as peer:
        peer.connect(str(wayland))
        pid, uid, _gid = struct.unpack("3i", peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
    require(pid == server.pid and uid == os.geteuid(), "Private Wayland compositor identity mismatch")
    namespace = Path("/proc/self/ns/pid").stat()
    scope = Scope(f"wayland-peer:{uid}:{pid}:{namespace.st_dev}:{namespace.st_ino}:{server.identity}")
    cleanup.callback(scope.cleanup)
    def views(pid):
        nodes = w.own_views(args.swaymsg, ipc_path, base, root, pid)
        require(all(node.get("window") is None and node.get("shell") == "xdg_shell" for node in nodes), "Private app fell through to X11")
        return [dict(id=node["id"], name=node.get("name"), shell=node.get("shell"), app_id=node.get("app_id"), focused=node.get("focused"), scratchpad_state=node.get("scratchpad_state")) for node in nodes]
    def close(pid, _nodes):
        result = w.ipc(args.swaymsg, ipc_path, base, root, f"[pid={pid}] kill")
        require(result and all(row.get("success") for row in result), "Private PID close rejected")
    def hide(pid, nodes):
        result = w.ipc(args.swaymsg, ipc_path, base, root, f"[pid={pid}] move scratchpad")
        require(result and all(row.get("success") for row in result), "Private scratchpad move failed")
    report["wayland_scope"] = dict(server_pid=pid, server_starttime=server.identity, uid=uid, lock=str(scope.path),
                                    identity_routes_tested=(["named"] if args.idle_only else ["named", "inherited-100", "inherited-101", "inherited-102"]), focus_policy="recorded separately; IPC acceptance does not guarantee compositor focus")
    desktop = Desktop(args.binary, root, args.library_dir, runtime, cleanup, scope, wayland=wayland)
    if not args.idle_only:
        exercise(desktop, views, close, hide, scope, report, "wayland-named")
        # Same compositor, lock and scope, despite original inherited FD renumbering.
        exercise(desktop, views, close, hide, scope, report, "wayland-inherited")
    if args.idle_seconds:
        idle_observation(args, desktop, views, close, scope, report)



def x11_case(args, root, cleanup, report):
    root.mkdir(mode=0o700)
    base = w.environment(root, args.library_dir)
    display = f":{20000 + secrets.randbelow(10000)}"
    number = display[1:]
    require(not Path(f"/tmp/.X{number}-lock").exists() and not Path(f"/tmp/.X11-unix/X{number}").exists(), "Private random X11 display unavailable")
    authority = root / "authority"
    base.update(DISPLAY=display, XAUTHORITY=str(authority), XDG_SESSION_TYPE="x11")
    auth = Process(["/usr/bin/xauth", "-f", str(authority), "add", display, ".", secrets.token_hex(16)], base, root)
    try:
        require(auth.wait(3) == 0, "Private Xauthority creation failed")
    finally:
        auth.close()
    server = Process([str(args.xvfb), display, "-screen", "0", "1024x768x24", "-auth", str(authority), "-nolisten", "tcp", "-noreset"], base, root)
    cleanup.callback(server.close)
    def command(arguments, empty=False):
        child = Process(arguments, base, root, cap=w.IPC_CAP)
        try:
            code = child.wait(3)
            require(code == 0 or (empty and code == 1), "Private X11 inspection failed: " + child.text("stderr")[-1000:])
            return child.text("stdout") if code == 0 else ""
        finally:
            child.close()
    until(lambda: command(["/usr/bin/xdpyinfo", "-display", display], True), [server], 10)
    x = c.CDLL("libX11.so.6")
    x.XOpenDisplay.argtypes, x.XOpenDisplay.restype = [c.c_char_p], c.c_void_p
    x.XCloseDisplay.argtypes = [c.c_void_p]
    x.XInternAtom.argtypes, x.XInternAtom.restype = [c.c_void_p, c.c_char_p, c.c_int], c.c_ulong
    class Client(c.Structure):
        _fields_ = [("type", c.c_int), ("serial", c.c_ulong), ("send_event", c.c_int), ("display", c.c_void_p),
                    ("window", c.c_ulong), ("message_type", c.c_ulong), ("format", c.c_int), ("data", c.c_long * 5)]
    class Event(c.Union):
        _fields_ = [("client", Client), ("pad", c.c_long * 24)]
    x.XSendEvent.argtypes = [c.c_void_p, c.c_ulong, c.c_int, c.c_long, c.POINTER(Event)]
    x.XFlush.argtypes = [c.c_void_p]
    # Explicitly scoped display/auth; no implicit default-display connection.
    previous = os.environ.get("XAUTHORITY")
    os.environ["XAUTHORITY"] = str(authority)
    try:
        connection = x.XOpenDisplay(display.encode())
    finally:
        if previous is None:
            os.environ.pop("XAUTHORITY", None)
        else:
            os.environ["XAUTHORITY"] = previous
    require(connection, "Could not connect owned authenticated Xvfb")
    cleanup.callback(x.XCloseDisplay, connection)
    wm_config = root / "openbox.xml"
    wm_config.write_text('<openbox_config xmlns="http://openbox.org/3.4/rc"><theme><name>Clearlooks</name></theme></openbox_config>')
    wm_env = dict(base, LD_LIBRARY_PATH=str(args.window_manager.parent.parent / "lib") + ":" + base["LD_LIBRARY_PATH"],
                  XDG_DATA_DIRS=str(args.window_manager.parent.parent / "share"))
    wm = Process([str(args.window_manager), "--config-file", str(wm_config), "--sm-disable"], wm_env, root)
    cleanup.callback(wm.close)
    until(lambda: command(["/usr/bin/xprop", "-root", "_NET_SUPPORTING_WM_CHECK"]).strip().endswith("not found.") is False,
          [server, wm], 5)
    scope = Scope(f"x11-server::{number}")
    cleanup.callback(scope.cleanup)
    def views(pid):
        identifiers = command(["/usr/bin/xdotool", "search", "--pid", str(pid)], True).split()
        result = []
        for identifier in identifiers:
            info = command(["/usr/bin/xwininfo", "-id", identifier])
            name = command(["/usr/bin/xdotool", "getwindowname", identifier]).strip()
            result.append(dict(id=int(identifier), name=name, mapped="Map State: IsViewable" in info, shell="X11"))
        return result
    def close(pid, nodes):
        # Like Sway's [pid=] kill, close both editor and its existing control shell.
        owned_nodes = views(pid)
        owned = {node["id"] for node in owned_nodes}
        require(all(node["id"] in owned for node in nodes), "X11 close target lost its PID ownership")
        for node in owned_nodes:
            event = Event()
            event.client.type, event.client.display, event.client.window = 33, connection, node["id"]
            event.client.message_type = x.XInternAtom(connection, b"WM_PROTOCOLS", 0)
            event.client.format, event.client.data[0] = 32, x.XInternAtom(connection, b"WM_DELETE_WINDOW", 0)
            require(x.XSendEvent(connection, node["id"], 0, 0, c.byref(event)), "Private WM_DELETE_WINDOW failed")
        x.XFlush(connection)
    def hide(pid, nodes):
        for node in nodes:
            require(node["id"] in {row["id"] for row in views(pid)}, "X11 unmap target lost PID ownership")
            command(["/usr/bin/xdotool", "windowminimize", str(node["id"])])
    desktop = Desktop(args.binary, root, args.library_dir, Path(base["XDG_RUNTIME_DIR"]), cleanup, scope, display=display)
    report["x11_scope"] = dict(server_pid=server.pid, server_starttime=server.identity, lock=str(scope.path),
                                authenticated=True, tcp_disabled=True, window_manager=True, window_manager_pid=wm.pid, window_manager_sha256=w.digest(args.window_manager),
                                limit="Private software X11 protocol checks; physical focus/input/presentation untested")
    exercise(desktop, views, close, hide, scope, report, "x11")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("binary", "cargo-json", "compositor", "swaymsg", "library-dir", "report"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--window-manager", type=Path)
    parser.add_argument("--xvfb", type=Path)
    parser.add_argument("--idle-seconds", type=int, choices=(0, 300), default=0)
    parser.add_argument("--idle-only", action="store_true")
    args = parser.parse_args()
    require(sys.platform.startswith("linux"), "Private native fixture is Linux-only")
    require(not args.idle_only or args.idle_seconds == 300, "Idle-only requires its full 300-second observation")
    for name in ("binary", "compositor", "swaymsg", "library_dir"):
        setattr(args, name, getattr(args, name).resolve(strict=True))
    records = w.cargo_records(args.cargo_json)
    require(any(row.get("reason") == "compiler-artifact" and row.get("target", {}).get("name") == "screenshots"
                and "bin" in row.get("target", {}).get("kind", []) and not row.get("profile", {}).get("test")
                and row.get("executable") and Path(row["executable"]).resolve() == args.binary for row in records), "Binary absent from exact Cargo stream")
    args.report.parent.mkdir(parents=True, exist_ok=True)
    require(not args.report.exists(), "Refusing to overwrite evidence")
    archived_runner = args.report.with_suffix(".runner.py")
    require(not archived_runner.exists(), "Refusing to overwrite runner evidence")
    archived_runner.write_bytes(Path(__file__).read_bytes())
    report = dict(status="running", linux_kernel=os.uname().release,
                  cpu_model=next((line.split(":", 1)[1].strip() for line in Path("/proc/cpuinfo").read_text().splitlines() if line.startswith("model name")), "unknown"),
                  binary_sha256=w.digest(args.binary), cargo_json_sha256=w.digest(args.cargo_json),
                  runner_sha256=w.digest(__file__), archived_runner=str(archived_runner), startup_fixture_sha256=w.digest(Path(__file__).with_name("test-wayland-startup.py")), compositor_sha256=w.digest(args.compositor), checks=[],
                  privacy="Owned headless Sway/authenticated Xvfb; synthetic image files only; no user display/input/clipboard/bus/credentials")
    try:
        with tempfile.TemporaryDirectory(prefix="screenshots-private-unix-lifecycle-") as temporary:
            with contextlib.ExitStack() as cleanup:
                sway_case(args, Path(temporary), cleanup, report)
            if args.xvfb:
                args.xvfb = args.xvfb.resolve(strict=True)
                require(args.window_manager, "X11 runtime requires a private window manager")
                args.window_manager = args.window_manager.resolve(strict=True)
                with contextlib.ExitStack() as cleanup:
                    x11_case(args, Path(temporary) / "x11", cleanup, report)
        require(w.digest(args.binary) == report["binary_sha256"], "Test binary changed during verification")
        report["status"] = "passed"
    except Exception as error:
        report.update(status="failed", error=str(error)[:8000])
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(dict(status=report["status"], checks=len(report["checks"]), report=str(args.report))))
    return 0 if report["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
