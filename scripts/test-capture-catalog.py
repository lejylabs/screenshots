#!/usr/bin/env python3
"""Verify native cursor/defaults and monitor-bound tasks on an authenticated private Xvfb.

The user's display, pointer, hotkeys, clipboard and configuration are never used.
Requires the built CLI, libX11/libXtst, Xvfb, xauth, xdpyinfo and FFmpeg.
"""
import argparse
import ctypes as c
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile
import time

REPO = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("task_host_fixture", REPO / "scripts/test-task-host.py")
host_fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(host_fixture)
require, executable = host_fixture.require, host_fixture.executable
BG, FG, CURSOR_BG = (20, 40, 60, 255), (240, 20, 80, 255), (5, 200, 30, 255)
ROWS = (0b010, 0b111, 0b010)


class Color(c.Structure):
    _fields_ = [("pixel", c.c_ulong), ("red", c.c_ushort), ("green", c.c_ushort),
                ("blue", c.c_ushort), ("flags", c.c_byte), ("pad", c.c_byte)]


def native():
    x = c.CDLL("libX11.so.6")
    signatures = [
        ("XOpenDisplay", [c.c_char_p], c.c_void_p), ("XCloseDisplay", [c.c_void_p], c.c_int),
        ("XDefaultScreen", [c.c_void_p], c.c_int), ("XRootWindow", [c.c_void_p, c.c_int], c.c_ulong),
        ("XCreateSimpleWindow", [c.c_void_p, c.c_ulong, c.c_int, c.c_int, c.c_uint, c.c_uint, c.c_uint, c.c_ulong, c.c_ulong], c.c_ulong),
        ("XMapWindow", [c.c_void_p, c.c_ulong], c.c_int), ("XDestroyWindow", [c.c_void_p, c.c_ulong], c.c_int),
        ("XSetWindowBackground", [c.c_void_p, c.c_ulong, c.c_ulong], c.c_int),
        ("XClearWindow", [c.c_void_p, c.c_ulong], c.c_int),
        ("XStoreName", [c.c_void_p, c.c_ulong, c.c_char_p], c.c_int),
        ("XInternAtom", [c.c_void_p, c.c_char_p, c.c_int], c.c_ulong),
        ("XChangeProperty", [c.c_void_p, c.c_ulong, c.c_ulong, c.c_ulong, c.c_int, c.c_int, c.c_void_p, c.c_int], c.c_int),
        ("XCreateBitmapFromData", [c.c_void_p, c.c_ulong, c.c_char_p, c.c_uint, c.c_uint], c.c_ulong),
        ("XCreatePixmapCursor", [c.c_void_p, c.c_ulong, c.c_ulong, c.POINTER(Color), c.POINTER(Color), c.c_uint, c.c_uint], c.c_ulong),
        ("XDefineCursor", [c.c_void_p, c.c_ulong, c.c_ulong], c.c_int),
        ("XFreeCursor", [c.c_void_p, c.c_ulong], c.c_int), ("XFreePixmap", [c.c_void_p, c.c_ulong], c.c_int),
        ("XWarpPointer", [c.c_void_p, c.c_ulong, c.c_ulong, c.c_int, c.c_int, c.c_uint, c.c_uint, c.c_int, c.c_int], c.c_int),
        ("XSetInputFocus", [c.c_void_p, c.c_ulong, c.c_int, c.c_ulong], c.c_int),
        ("XStringToKeysym", [c.c_char_p], c.c_ulong),
        ("XKeysymToKeycode", [c.c_void_p, c.c_ulong], c.c_ubyte),
        ("XSync", [c.c_void_p, c.c_int], c.c_int),
    ]
    for name, arguments, result in signatures:
        function = getattr(x, name)
        function.argtypes, function.restype = arguments, result
    xtest = c.CDLL("libXtst.so.6")
    xtest.XTestFakeKeyEvent.argtypes = [c.c_void_p, c.c_uint, c.c_int, c.c_ulong]
    xtest.XTestFakeKeyEvent.restype = c.c_int
    return x, xtest


def expected(width, height, origin, cursor):
    result = bytearray(bytes(BG) * width * height)
    if cursor:
        for yy, row in enumerate(ROWS):
            for xx in range(3):
                x, y = 27 + xx - origin[0], 28 + yy - origin[1]
                if 0 <= x < width and 0 <= y < height:
                    start = (y * width + x) * 4
                    result[start:start + 4] = bytes(FG if row & (1 << xx) else CURSOR_BG)
    return result


def digest(path):
    value = hashlib.sha256()
    with open(path, "rb") as file:
        for chunk in iter(lambda: file.read(65536), b""):
            value.update(chunk)
    return value.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=str(REPO / "target/release-fast/screenshots"))
    parser.add_argument("--xvfb", required=True)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    binary, xvfb = executable(args.binary), executable(args.xvfb)
    identity = lambda: (lambda value: (value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns))(Path(binary).stat())
    binary_identity = identity()
    xauth, xdpyinfo, ffmpeg = executable("xauth"), executable("xdpyinfo"), executable("ffmpeg")
    x, xtest = native()
    started = time.monotonic()
    checked = calls = 0
    with tempfile.TemporaryDirectory(prefix="screenshots-catalog-private-") as temporary:
        root = Path(temporary)
        display = next((f":{i}" for i in range(160, 220) if not Path(f"/tmp/.X{i}-lock").exists()
                        and not Path(f"/tmp/.X11-unix/X{i}").exists()), None)
        require(display, "No unused private X11 display")
        env = dict(os.environ)
        for name in ("WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DBUS_SESSION_BUS_ADDRESS", "AT_SPI_BUS_ADDRESS"):
            env.pop(name, None)
        state_keys = ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME")
        env.update({name: str(root / name) for name in state_keys})
        auth = root / "authority"
        env.update(DISPLAY=display, XAUTHORITY=str(auth), XDG_SESSION_TYPE="x11", MESA_SHADER_CACHE_DISABLE="true")
        subprocess.run([xauth, "-f", str(auth), "add", display, ".", secrets.token_hex(16)], env=env,
                       capture_output=True, check=True, timeout=5)
        with (root / "xvfb.log").open("wb") as log:
            server = subprocess.Popen([xvfb, display, "-screen", "0", "128x96x24", "-auth", str(auth),
                                       "-nolisten", "tcp", "-noreset"], env=env, stdout=log, stderr=subprocess.STDOUT)
        connection = window = cursor = source = mask = host = endpoint_data = None
        try:
            for _ in range(100):
                require(server.poll() is None, "Private Xvfb stopped")
                if subprocess.run([xdpyinfo, "-display", display], env=env, capture_output=True, timeout=2).returncode == 0:
                    break
                time.sleep(0.05)
            else:
                raise RuntimeError("Private Xvfb startup deadline")
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
            screen = x.XDefaultScreen(connection)
            desktop = x.XRootWindow(connection, screen)
            bg = (BG[0] << 16) | (BG[1] << 8) | BG[2]
            x.XSetWindowBackground(connection, desktop, bg)
            x.XClearWindow(connection, desktop)
            window = x.XCreateSimpleWindow(connection, desktop, 13, 17, 96, 64, 0, 0, bg)
            require(window, "Cannot create own fixture window")
            atom = lambda name: x.XInternAtom(connection, name.encode(), 0)

            def property32(target, name, kind, values):
                array = (c.c_ulong * len(values))(*values)
                x.XChangeProperty(connection, target, atom(name), kind, 32, 0, array, len(values))
            property32(desktop, "_NET_CLIENT_LIST_STACKING", 33, [window])
            property32(desktop, "_NET_CLIENT_LIST", 33, [window])
            property32(desktop, "_NET_ACTIVE_WINDOW", 33, [window])
            property32(window, "_NET_WM_PID", 6, [os.getpid()])
            property32(window, "WM_STATE", atom("WM_STATE"), [1, 0])
            property32(window, "_NET_WM_STATE", 4, [])
            for name in ("_NET_WM_STATE_HIDDEN", "_NET_WM_STATE_MAXIMIZED_VERT", "_NET_WM_STATE_MAXIMIZED_HORZ"):
                atom(name)
            title = b"private cursor fixture"
            x.XChangeProperty(connection, window, atom("_NET_WM_NAME"), atom("UTF8_STRING"), 8, 0, title, len(title))
            x.XStoreName(connection, window, title)
            x.XMapWindow(connection, window)
            x.XSetInputFocus(connection, window, 1, 0)
            source = x.XCreateBitmapFromData(connection, desktop, bytes(ROWS), 3, 3)
            mask = x.XCreateBitmapFromData(connection, desktop, bytes([7, 7, 7]), 3, 3)
            color = lambda rgba: Color(0, rgba[0] * 257, rgba[1] * 257, rgba[2] * 257, 7, 0)
            fg, cbg = color(FG), color(CURSOR_BG)
            cursor = x.XCreatePixmapCursor(connection, source, mask, c.byref(fg), c.byref(cbg), 1, 1)
            require(cursor, "Cannot create own cursor")
            x.XDefineCursor(connection, desktop, cursor)
            x.XDefineCursor(connection, window, cursor)
            x.XWarpPointer(connection, 0, desktop, 0, 0, 0, 0, 28, 29)
            x.XSync(connection, 0)

            def invoke(arguments, success=True):
                nonlocal calls
                calls += 1
                require(identity() == binary_identity, "Executable changed during fixture; rerun against one build")
                result = subprocess.run([binary, *map(str, arguments)], env=env, capture_output=True, timeout=15)
                require(identity() == binary_identity, "Executable changed during fixture; rerun against one build")
                require((result.returncode == 0) == success,
                        f"Unexpected CLI status for {arguments[0]}: {result.stderr.decode(errors='replace')}")
                if success:
                    require(not result.stderr, "Successful CLI emitted unexpected diagnostics")
                else:
                    require(not result.stdout, "Failed CLI published a success result")
                return result

            def pixels(path, width, height, origin, include=True):
                nonlocal checked
                raw = subprocess.check_output([ffmpeg, "-v", "error", "-i", str(path), "-frames:v", "1", "-pix_fmt", "rgba", "-f", "rawvideo", "pipe:1"], env=env, timeout=10)
                require(raw == expected(width, height, origin, include), f"Independent pixel oracle failed for {path.name}")
                checked += width * height
            displays = json.loads(invoke(["list", "displays"]).stdout)
            require(len(displays) == 1, "Fixture must have exactly one display")
            display_id = displays[0]["id"]
            dpi_stages = []
            for dpi in (48, 96, 192):
                resource = f"Xft.dpi:\t{dpi}\n".encode()
                x.XChangeProperty(connection, desktop, atom("RESOURCE_MANAGER"), atom("STRING"),
                                  8, 0, resource, len(resource))
                x.XSync(connection, 0)
                metadata = json.loads(invoke(["list", "displays"]).stdout)
                require(len(metadata) == 1 and metadata[0]["id"] == display_id,
                        "Xft DPI changed native display identity")
                require((metadata[0]["width"], metadata[0]["height"]) == (128, 96)
                        and metadata[0]["scale"] == dpi / 96,
                        "Xft DPI did not retain independent root geometry and UI scale")
                stage_checked = checked
                stage_calls = calls
                for name, target, width, height, origin, include in [
                    ("display-exclude", ["display", display_id], 128, 96, (0, 0), False),
                    ("display-include", ["display", display_id], 128, 96, (0, 0), True),
                    ("region-exclude", ["region", display_id, 20, 20, 30, 25], 30, 25, (20, 20), False),
                    ("region-include", ["region", display_id, 20, 20, 30, 25], 30, 25, (20, 20), True),
                    ("hotspot-exclude", ["region", display_id, 28, 29, 1, 1], 1, 1, (28, 29), False),
                    ("hotspot-include", ["region", display_id, 28, 29, 1, 1], 1, 1, (28, 29), True),
                    ("clip-left", ["region", display_id, 28, 28, 2, 3], 2, 3, (28, 28), True),
                    ("clip-right", ["region", display_id, 27, 28, 2, 3], 2, 3, (27, 28), True),
                    ("clip-top", ["region", display_id, 27, 29, 3, 2], 3, 2, (27, 29), True),
                    ("clip-bottom", ["region", display_id, 27, 28, 3, 2], 3, 2, (27, 28), True),
                    ("edge-exclude", ["region", display_id, 127, 95, 1, 1], 1, 1, (127, 95), False),
                    ("edge-include", ["region", display_id, 127, 95, 1, 1], 1, 1, (127, 95), True),
                ]:
                    path = root / f"dpi-{dpi}-{name}.png"
                    options = {"cursor": "include" if include else "exclude"}
                    result = json.loads(invoke(["capture-task", *target, path, "--capture-options", json.dumps(options)]).stdout)
                    require((result["width"], result["height"]) == (width, height),
                            "Xft DPI changed capture dimensions or resampled the native raster")
                    pixels(path, width, height, origin, include)
                dpi_stages.append({"dpi": dpi, "ui_scale": dpi / 96, "native_raster": [128, 96],
                                   "capture_invocations": calls - stage_calls,
                                   "exact_rgba_pixels_checked": checked - stage_checked})
            # The existing catalog/hotkey checks retain their original 1x saved geometry.
            resource = b"Xft.dpi:\t96\n"
            x.XChangeProperty(connection, desktop, atom("RESOURCE_MANAGER"), atom("STRING"),
                              8, 0, resource, len(resource))
            x.XSync(connection, 0)
            for name, target, width, height, origin, include in [
                ("without", ["display", display_id], 128, 96, (0, 0), False),
                ("with", ["display", display_id], 128, 96, (0, 0), True),
                ("window", ["window", window], 96, 64, (13, 17), True),
                ("region", ["region", display_id, 20, 20, 30, 25], 30, 25, (20, 20), True),
                ("clipped", ["region", display_id, 28, 29, 1, 1], 1, 1, (28, 29), True),
            ]:
                path = root / f"{name}.png"
                options = {"cursor": "include" if include else "exclude"}
                if name == "window":
                    options.update(window_frame="client", window_shadow="exclude", window_transparency="opaque", background=[1, 2, 3])
                invoke(["capture-task", *target, path, "--capture-options", json.dumps(options)])
                pixels(path, width, height, origin, include)
            blocked = root / "parent-self-window.png"
            rejected = subprocess.Popen([binary, "capture-task", "window", str(window), str(blocked),
                                         "--delay-ms", "250", "--capture-options", '{"window_frame":"client"}'],
                                        env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            calls += 1
            try:
                property32(window, "_NET_WM_PID", 6, [rejected.pid])
                x.XSync(connection, 0)
                out, errors = rejected.communicate(timeout=10)
                require(rejected.returncode != 0 and not out and not blocked.exists(),
                        "Native worker did not preserve its originating parent's own-window guard")
            finally:
                if rejected.poll() is None:
                    rejected.kill()
                    rejected.communicate(timeout=3)
                property32(window, "_NET_WM_PID", 6, [os.getpid()])
                x.XSync(connection, 0)
            missing = root / "no-implicit-fallback.json"
            invoke(["capture-catalog", "bind", missing, "crop", display_id, 20, 20, 30, 25], success=False)
            require(not missing.exists(), "Physical binding must not silently fall back to connector")
            catalog, task = root / "catalog.json", root / "task.json"
            task.write_text(json.dumps({"version": 1, "source": {"type": "template", "catalog": str(catalog), "name": "crop"},
                                        "actions": [{"type": "save", "path": str(root / "task-{uuid}.png")}] }))
            invoke(["capture-catalog", "bind", catalog, "crop", display_id, 20, 20, 30, 25,
                    "--connector", "true", "--profile", task, "--hotkey", "Ctrl+Shift+F9",
                    "--defaults", json.dumps({"cursor": "include", "delay_ms": 20})])
            stored = json.loads(catalog.read_bytes())
            require(stored["templates"][0]["monitor"]["kind"] == "connector", "Explicit connector binding not retained")
            require("display_id" not in stored["templates"][0], "Session display ID must not be persisted as identity")
            source_hash = hashlib.sha256(catalog.read_bytes()).hexdigest()
            invoke(["capture-catalog", "validate", catalog])
            final = root / "template.png"
            invoke(["capture-catalog", "capture", catalog, "crop", final])
            pixels(final, 30, 25, (20, 20))
            invoke(["capture-catalog", "run-task", catalog, "crop"])
            outputs = list(root.glob("task-*.png"))
            require(len(outputs) == 1, "Assigned template task did not save exactly once")
            pixels(outputs[0], 30, 25, (20, 20))
            config, endpoint = root / "host.json", root / "endpoint.json"
            invoke(["capture-catalog", "host-config", catalog, config])
            invoke(["task-host", "validate", config])
            host = subprocess.Popen([binary, "task-host", "serve", str(config), str(endpoint)], env=env,
                                    stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            ready = host_fixture.ready(host)
            require(ready["hotkeys"] == 1 and ready["profiles"] == 1, "Template host did not register its mapping")
            endpoint_data = json.loads(endpoint.read_bytes())
            keys = [x.XKeysymToKeycode(connection, x.XStringToKeysym(key)) for key in (b"Control_L", b"Shift_L", b"F9")]
            require(all(keys), "Private native key mapping failed")
            previous_job = None
            for _ in range(3):
                for key in keys:
                    require(xtest.XTestFakeKeyEvent(connection, key, 1, 0), "Private hotkey press failed")
                for key in reversed(keys):
                    require(xtest.XTestFakeKeyEvent(connection, key, 0, 0), "Private hotkey release failed")
                x.XSync(connection, 0)
                deadline = time.monotonic() + 10
                while True:
                    status = host_fixture.request(endpoint_data, {"type": "status"})
                    if status.get("last") and status["last"]["job"] != previous_job and not status["busy"]:
                        require(status["last"]["success"], "Template hotkey task failed")
                        previous_job = status["last"]["job"]
                        break
                    require(time.monotonic() < deadline, "Template hotkey did not finish")
                    time.sleep(0.01)
            outputs = list(root.glob("task-*.png"))
            require(len(outputs) == 4, "Each physical template hotkey must save exactly once")
            for path in outputs:
                pixels(path, 30, 25, (20, 20))
            host_fixture.request(endpoint_data, {"type": "stop"})
            _, errors = host.communicate(timeout=5)
            require(host.returncode == 0 and not errors and not endpoint.exists(), "Template host teardown failed")
            host = None
            for key in ("geometry", "identity"):
                bad = json.loads(catalog.read_bytes())
                if key == "geometry":
                    bad["templates"][0]["geometry"]["width"] += 1
                else:
                    bad["templates"][0]["monitor"]["key"] = "x11:missing-monitor"
                path = root / f"bad-{key}.json"
                path.write_text(json.dumps(bad))
                destination = root / f"bad-{key}.png"
                invoke(["capture-catalog", "capture", path, "crop", destination], success=False)
                require(not destination.exists(), "Stale binding published pixels")
            require(hashlib.sha256(catalog.read_bytes()).hexdigest() == source_hash, "Capture modified its source catalog")
            require(all(not (root / name).exists() for name in state_keys), "Fixture created user-state paths")
            post = root / "post.png"
            invoke(["capture-task", "display", display_id, post])
            pixels(post, 128, 96, (0, 0), False)
            require(identity() == binary_identity, "Executable changed during fixture; rerun against one build")
            report = {"binary_sha256": digest(binary), "cli_invocations": calls,
                      "binary_identity_stable": True,
                      "exact_rgba_pixels_checked": checked, "cursor_hotspot_and_clipping": True,
                      "xft_dpi_cursor_stages": dpi_stages,
                      "xft_dpi_root_pixels_without_resampling": True,
                      "connector_fallback_explicit": True, "native_template_capture": True,
                      "assigned_task_capture": True, "physical_template_hotkey": True,
                      "rapid_hotkey_taps": 3, "source_display_unchanged": True,
                      "originating_parent_self_window_guard": True,
                      "stale_binding_refusals": True, "source_catalog_preserved": True,
                      "user_state_unchanged": True, "host_endpoint_cleaned": True,
                      "elapsed_seconds": time.monotonic() - started}
            if args.report:
                args.report.parent.mkdir(parents=True, exist_ok=True)
                args.report.write_text(json.dumps(report, indent=2) + "\n")
            print(json.dumps(report, indent=2))
        finally:
            if host:
                host.terminate()
                try:
                    host.communicate(timeout=3)
                except subprocess.TimeoutExpired:
                    host.kill()
                    host.communicate(timeout=3)
            if connection:
                if window:
                    x.XDestroyWindow(connection, window)
                if cursor:
                    x.XFreeCursor(connection, cursor)
                for pixmap in (source, mask):
                    if pixmap:
                        x.XFreePixmap(connection, pixmap)
                x.XSync(connection, 0)
                x.XCloseDisplay(connection)
            server.terminate()
            try:
                server.wait(timeout=3)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=3)


if __name__ == "__main__":
    main()
