#!/usr/bin/env python3
"""Exercise real scrolling capture on an isolated authenticated Xvfb window.

Requires Python 3, libX11, xauth, xdpyinfo, FFmpeg, Xvfb and the built CLI.
Only this script's private server receives input; no user window is touched.
"""
import argparse
import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import secrets
import signal
import shutil
import subprocess
import tempfile
import time

REPO = Path(__file__).resolve().parents[1]
WIDTH, HEIGHT, TRIM, DOCUMENT, ADVANCE = 96, 80, 4, 227, 37


def require(value, message):
    if not value:
        raise RuntimeError(message)


def executable(value):
    found = shutil.which(value) if value else None
    require(found is not None, f"Executable unavailable: {value}")
    return str(Path(found).resolve())


def color(x, row):
    value = ((x * 17 + row * 991) * 0x85EBCA6B) & 0xFFFFFFFF
    return value & 255, value >> 8 & 255, value >> 16 & 255, 255


class Button(c.Structure):
    _fields_ = [("kind", c.c_int), ("serial", c.c_ulong), ("send_event", c.c_int),
                ("display", c.c_void_p), ("window", c.c_ulong), ("root", c.c_ulong),
                ("subwindow", c.c_ulong), ("time", c.c_ulong), ("x", c.c_int),
                ("y", c.c_int), ("x_root", c.c_int), ("y_root", c.c_int),
                ("state", c.c_uint), ("button", c.c_uint), ("same_screen", c.c_int)]


class Event(c.Union):
    _fields_ = [("kind", c.c_int), ("button", Button), ("pad", c.c_long * 24)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=str(REPO / "target/release-fast/screenshots"))
    parser.add_argument("--xvfb", default=shutil.which("Xvfb"))
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    binary, xvfb = executable(args.binary), executable(args.xvfb)
    xauth, xdpyinfo, ffmpeg = map(executable, ("xauth", "xdpyinfo", "ffmpeg"))
    x = c.CDLL("libX11.so.6")
    ptr, ul, integer, ui = c.c_void_p, c.c_ulong, c.c_int, c.c_uint

    def bind(name, result, *arguments):
        fn = getattr(x, name)
        fn.restype, fn.argtypes = result, arguments
        return fn

    open_display = bind("XOpenDisplay", ptr, c.c_char_p)
    screen = bind("XDefaultScreen", integer, ptr)
    root_window = bind("XRootWindow", ul, ptr, integer)
    create_window = bind("XCreateSimpleWindow", ul, ptr, ul, integer, integer, ui, ui, ui, ul, ul)
    atom = bind("XInternAtom", ul, ptr, c.c_char_p, integer)
    property_change = bind("XChangeProperty", integer, ptr, ul, ul, ul, integer, integer, ptr, integer)
    store_name = bind("XStoreName", integer, ptr, ul, c.c_char_p)
    select = bind("XSelectInput", integer, ptr, ul, c.c_long)
    map_window = bind("XMapWindow", integer, ptr, ul)
    focus = bind("XSetInputFocus", integer, ptr, ul, integer, ul)
    create_gc = bind("XCreateGC", ptr, ptr, ul, ul, ptr)
    foreground = bind("XSetForeground", integer, ptr, ptr, ul)
    point = bind("XDrawPoint", integer, ptr, ul, ptr, integer, integer)
    sync = bind("XSync", integer, ptr, integer)
    pending = bind("XPending", integer, ptr)
    next_event = bind("XNextEvent", integer, ptr, c.POINTER(Event))
    free_gc = bind("XFreeGC", integer, ptr, ptr)
    destroy = bind("XDestroyWindow", integer, ptr, ul)
    close = bind("XCloseDisplay", integer, ptr)
    started = time.monotonic()

    with tempfile.TemporaryDirectory(prefix="screenshots-scroll-capture-") as temporary:
        root = Path(temporary)
        display_name = next((f":{i}" for i in range(90, 150)
                             if not Path(f"/tmp/.X{i}-lock").exists()
                             and not Path(f"/tmp/.X11-unix/X{i}").exists()), None)
        require(display_name, "No unused private X display")
        authority = root / "authority"
        env = dict(os.environ)
        for key in ("WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS"):
            env.pop(key, None)
        env.update(DISPLAY=display_name, XAUTHORITY=str(authority), XDG_SESSION_TYPE="x11")
        env["MESA_SHADER_CACHE_DISABLE"] = "true"
        state_keys = ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME")
        env.update({key: str(root / key) for key in state_keys})
        subprocess.run([xauth, "-f", str(authority), "add", display_name, ".", secrets.token_hex(16)],
                       check=True, capture_output=True, timeout=5)
        with (root / "xvfb.log").open("wb") as log:
            server = subprocess.Popen([xvfb, display_name, "-screen", "0", "160x128x24",
                                       "-auth", str(authority), "-nolisten", "tcp", "-noreset"],
                                      env=env, stdout=log, stderr=subprocess.STDOUT)
        connection = gc = window = child = None
        try:
            for _ in range(100):
                require(server.poll() is None, "Private Xvfb exited")
                result = subprocess.run([xdpyinfo, "-display", display_name], env=env,
                                        capture_output=True, timeout=2)
                if result.returncode == 0:
                    break
                time.sleep(0.05)
            else:
                raise RuntimeError("Private Xvfb did not become ready")
            previous_authority = os.environ.get("XAUTHORITY")
            os.environ["XAUTHORITY"] = str(authority)
            try:
                connection = open_display(display_name.encode())
            finally:
                if previous_authority is None:
                    os.environ.pop("XAUTHORITY", None)
                else:
                    os.environ["XAUTHORITY"] = previous_authority
            require(connection, "Cannot connect fixture to private display")
            root_id = root_window(connection, screen(connection))
            window = create_window(connection, root_id, 20, 20, WIDTH, HEIGHT, 0, 0, 0)

            def prop(target, name, kind, values):
                data = (ul * len(values))(*values)
                property_change(connection, target, atom(connection, name.encode(), 0),
                                atom(connection, kind.encode(), 0), 32, 0, data, len(values))

            prop(window, "_NET_WM_PID", "CARDINAL", [os.getpid()])
            prop(window, "_NET_WM_STATE", "ATOM", [])
            for name in ("_NET_WM_STATE_HIDDEN", "_NET_WM_STATE_MAXIMIZED_VERT", "_NET_WM_STATE_MAXIMIZED_HORZ"):
                atom(connection, name.encode(), 0)
            prop(root_id, "_NET_CLIENT_LIST_STACKING", "WINDOW", [window])
            prop(root_id, "_NET_ACTIVE_WINDOW", "WINDOW", [window])
            store_name(connection, window, b"Private scrolling verification fixture")
            select(connection, window, (1 << 2) | (1 << 3))
            map_window(connection, window)
            focus(connection, window, 2, 0)
            gc = create_gc(connection, window, 0, None)
            offset = 0

            def paint():
                for y in range(HEIGHT):
                    for column in range(WIDTH):
                        rgb = (30, 40, 50, 255) if y < TRIM or y >= HEIGHT - TRIM else color(column, offset + y - TRIM)
                        foreground(connection, gc, rgb[0] << 16 | rgb[1] << 8 | rgb[2])
                        point(connection, window, gc, column, y)
                sync(connection, 0)

            paint()
            destination = root / "document.png"
            command = [binary, "scroll-capture", str(window), str(destination), "--region", "0", "0", str(WIDTH), str(HEIGHT),
                       "--wheel", "1", "--steps", "12", "--settle-ms", "50", "--min-overlap", "16",
                       "--trim-top", str(TRIM), "--trim-bottom", str(TRIM), "--delay-ms", "100"]
            child = subprocess.Popen(command, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            wheels = 0
            deadline = time.monotonic() + 20
            while child.poll() is None:
                require(time.monotonic() < deadline, "Live scrolling fixture timed out")
                while pending(connection):
                    event = Event()
                    next_event(connection, c.byref(event))
                    if event.kind == 4 and event.button.window == window and event.button.button == 5:
                        wheels += 1
                        offset = min(offset + ADVANCE, DOCUMENT - (HEIGHT - 2 * TRIM))
                        paint()
                time.sleep(0.001)
            stdout, stderr = child.communicate(timeout=2)
            require(child.returncode == 0, f"Live capture failed: {stderr.decode(errors='replace')}")
            require(not stderr, "Live capture emitted unexpected diagnostics")
            result = json.loads(stdout)
            require(result["width"] == WIDTH and result["height"] == DOCUMENT
                    and result["stop_reason"] == "unchanged" and result["complete"] is True,
                    f"Unexpected scrolling result: {result}")
            require(result["vertical_advances"] == [37, 37, 37, 37, 7], "Scrolling seams changed")
            require(wheels == result["scroll_actions"] == 7 and result["captured_frames"] == 8,
                    "Native scroll actions or captures changed")
            decoded = subprocess.run([ffmpeg, "-v", "error", "-nostdin", "-i", str(destination),
                                      "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "-"],
                                     capture_output=True, env=env, timeout=10)
            expected = bytes(channel for row in range(DOCUMENT) for column in range(WIDTH) for channel in color(column, row))
            require(decoded.returncode == 0 and not decoded.stderr and decoded.stdout == expected,
                    "Independent final document pixels differ")
            before = hashlib.sha256(destination.read_bytes()).hexdigest()
            refusal = subprocess.run(command, env=env, capture_output=True, timeout=5)
            require(refusal.returncode != 0 and not refusal.stdout
                    and hashlib.sha256(destination.read_bytes()).hexdigest() == before,
                    "Existing scrolling output was not preserved")

            # A different focused window stops before any wheel or capture output.
            focus(connection, root_id, 2, 0)
            sync(connection, 0)
            blocked = list(command)
            blocked[3] = str(root / "unfocused.png")
            result_unfocused = subprocess.run(blocked, env=env, capture_output=True, timeout=5)
            require(result_unfocused.returncode != 0 and not result_unfocused.stdout
                    and not (root / "unfocused.png").exists(), "Lost-focus target was not refused")
            focus(connection, window, 2, 0)
            sync(connection, 0)

            # Cancellation during an explicitly active settle wait publishes no image.
            canceled = list(command)
            canceled[3] = str(root / "canceled.png")
            canceled[canceled.index("--settle-ms") + 1] = "2000"
            child = subprocess.Popen(canceled, env=env, stdin=subprocess.DEVNULL,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            sent_signal = False
            deadline = time.monotonic() + 10
            while child.poll() is None:
                require(time.monotonic() < deadline, "Scrolling cancellation did not finish")
                while pending(connection):
                    event = Event()
                    next_event(connection, c.byref(event))
                    if event.kind == 4 and event.button.button == 5 and not sent_signal:
                        child.send_signal(signal.SIGTERM)
                        sent_signal = True
                time.sleep(0.001)
            canceled_stdout, canceled_stderr = child.communicate(timeout=2)
            require(sent_signal and child.returncode != 0 and not canceled_stdout
                    and b"canceled" in canceled_stderr and not (root / "canceled.png").exists(),
                    "Canceled scrolling capture published output or missed its signal")

            # The same ID with changed process identity must never continue scrolling.
            changed = list(command)
            changed[3] = str(root / "changed-process.png")
            child = subprocess.Popen(changed, env=env, stdin=subprocess.DEVNULL,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            changed_pid = False
            deadline = time.monotonic() + 10
            while child.poll() is None:
                require(time.monotonic() < deadline, "Changed target process did not fail")
                while pending(connection):
                    event = Event()
                    next_event(connection, c.byref(event))
                    if event.kind == 4 and event.button.button == 5 and not changed_pid:
                        prop(window, "_NET_WM_PID", "CARDINAL", [1])
                        sync(connection, 0)
                        changed_pid = True
                time.sleep(0.001)
            changed_stdout, changed_stderr = child.communicate(timeout=2)
            require(changed_pid and child.returncode != 0 and not changed_stdout
                    and b"changed" in changed_stderr and not (root / "changed-process.png").exists(),
                    "Changed process identity was not rejected")
            prop(window, "_NET_WM_PID", "CARDINAL", [os.getpid()])
            sync(connection, 0)
            saved_after_error = root / "saved-stdout-error.png"
            failed_stdout = list(command)
            failed_stdout[3] = str(saved_after_error)
            with open("/dev/full", "wb") as full:
                child = subprocess.Popen(failed_stdout, env=env, stdin=subprocess.DEVNULL,
                                         stdout=full, stderr=subprocess.PIPE)
                deadline = time.monotonic() + 10
                while child.poll() is None:
                    require(time.monotonic() < deadline, "Saved-output status fixture timed out")
                    while pending(connection):
                        event = Event()
                        next_event(connection, c.byref(event))
                    time.sleep(0.001)
                _, failed_stderr = child.communicate(timeout=2)
            require(child.returncode != 0 and saved_after_error.is_file()
                    and b"was saved" in failed_stderr,
                    "Post-publication stdout error did not preserve truthful saved status")
            unexpected = {key: [str(p.relative_to(root)) for p in Path(env[key]).rglob("*")]
                          for key in state_keys if Path(env[key]).exists()}
            require(not unexpected, f"Unexpected isolated user state: {unexpected}")
            require(not any(p.name.endswith(".part") for p in root.iterdir()), "Unpublished pixels remain")
        finally:
            if child is not None and child.poll() is None:
                child.kill()
                child.wait(timeout=5)
            if connection:
                if gc:
                    free_gc(connection, gc)
                if window:
                    destroy(connection, window)
                close(connection)
            server.terminate()
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=5)
    report = {"passed": True, "binary_sha256": hashlib.sha256(Path(binary).read_bytes()).hexdigest(),
              "native_scrolling_actions": wheels, "captured_frames": result["captured_frames"],
              "document_dimensions": [WIDTH, DOCUMENT], "fixed_bands_removed": True,
              "independent_rgba_exact": True, "private_display_only": True,
              "focus_loss_refused": True, "active_signal_cancellation": True,
              "changed_process_refused": True,
              "saved_output_after_stdout_failure": True,
              "user_state_unchanged": True, "elapsed_seconds": round(time.monotonic() - started, 3)}
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
