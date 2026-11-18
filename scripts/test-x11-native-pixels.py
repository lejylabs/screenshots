#!/usr/bin/env python3
"""Finite synthetic X11 packing fixture; never opens an inherited display.

Requires an already built optimized app_core test executable, Xvfb, xauth,
xdpyinfo and libX11. No Cargo, downloads, clipboard, input or user services.
The outer process reuses the existing pidfd/subreaper ownership implementation.
"""
import argparse
import ctypes as c
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import sys
import time

TEST = "capture_native::platform::tests::private_x11_truecolor_root_region_and_window_pixels"


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def executable(value):
    result = shutil.which(str(value))
    require(result, f"Missing executable: {value}")
    return str(Path(result).resolve())


def digest(path):
    with open(path, "rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


class VisualInfo(c.Structure):
    _fields_ = [("visual", c.c_void_p), ("visualid", c.c_ulong), ("screen", c.c_int),
                ("depth", c.c_int), ("class_", c.c_int), ("red_mask", c.c_ulong),
                ("green_mask", c.c_ulong), ("blue_mask", c.c_ulong),
                ("colormap_size", c.c_int), ("bits_per_rgb", c.c_int)]


def xlib():
    x = c.CDLL("libX11.so.6")
    ptr, ul, ui, integer = c.c_void_p, c.c_ulong, c.c_uint, c.c_int
    for name, result, arguments in [
        ("XOpenDisplay", ptr, [c.c_char_p]), ("XCloseDisplay", integer, [ptr]),
        ("XDefaultScreen", integer, [ptr]), ("XRootWindow", ul, [ptr, integer]),
        ("XDefaultDepth", integer, [ptr, integer]), ("XDefaultVisual", ptr, [ptr, integer]),
        ("XVisualIDFromVisual", ul, [ptr]),
        ("XMatchVisualInfo", integer, [ptr, integer, integer, integer, c.POINTER(VisualInfo)]),
        ("XCreateSimpleWindow", ul, [ptr, ul, integer, integer, ui, ui, ui, ul, ul]),
        ("XMapWindow", integer, [ptr, ul]), ("XDestroyWindow", integer, [ptr, ul]),
        ("XCreateGC", ptr, [ptr, ul, ul, ptr]), ("XFreeGC", integer, [ptr, ptr]),
        ("XSetForeground", integer, [ptr, ptr, ul]),
        ("XDrawPoint", integer, [ptr, ul, ptr, integer, integer]),
        ("XSync", integer, [ptr, integer]),
        ("XInternAtom", ul, [ptr, c.c_char_p, integer]),
        ("XChangeProperty", integer, [ptr, ul, ul, ul, integer, integer, ptr, integer]),
        ("XGetImage", ptr, [ptr, ul, integer, integer, ui, ui, ul, integer]),
        ("XGetPixel", ul, [ptr, integer, integer]), ("XDestroyImage", integer, [ptr]),
    ]:
        function = getattr(x, name)
        function.restype, function.argtypes = result, arguments
    return x


def supervised(args):
    spec = importlib.util.spec_from_file_location(
        "x11_pixels_process_owner", Path(__file__).with_name("test-unix-native-services.py"))
    owner = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(owner)
    test_binary, xvfb = executable(args.core_tests), executable(args.xvfb)
    admitted_core_hash, admitted_xvfb_hash = digest(test_binary), digest(xvfb)
    family = owner.OwnedChildren()
    with owner.Interrupts() as interrupts:
        try:
            root = family.create_runtime()
            # Keep the guard-recognizable private basename, without replacing owner semantics.
            renamed = root.with_name(root.name.replace("screenshots-native-", "screenshots-x11-pixels-private-", 1))
            root.rename(renamed)
            family.runtime = renamed
            for depth in (16, 24, 30):
                require(digest(test_binary) == admitted_core_hash and digest(xvfb) == admitted_xvfb_hash,
                        "Fixture executable changed between private depths")
                command = [sys.executable, str(Path(__file__).resolve()), "--owned-worker",
                           "--worker-root", str(renamed), "--depth", str(depth),
                           "--core-tests", test_binary, "--xvfb", xvfb]
                owner.run(command, 45, family, interrupts,
                          env=dict(os.environ, SCREENSHOTS_X11_PIXELS_OUTER_PID=str(os.getpid())))
                # Xvfb and every native worker must exit before the next depth starts.
                family.discover()
                require(not family.handles, "Private depth worker left live or unreaped descendants")
                require(digest(test_binary) == admitted_core_hash and digest(xvfb) == admitted_xvfb_hash,
                        "Fixture executable changed during private depth")
            results = {
                "scope": "owned synthetic Xvfb; not HDR/transfer/gamut/hardware acceptance",
                "depths": [16, 24, 30], "exact_root_and_window_pixels": True,
                "core_test_sha256": admitted_core_hash,
                "xvfb_sha256": admitted_xvfb_hash,
                "runner_sha256": digest(__file__),
            }
        finally:
            family.close()
    rendered = json.dumps(results, indent=2) + "\n"
    if args.report:
        args.report.write_text(rendered)
    print(rendered, end="")


def worker(args):
    require(os.getppid() == int(os.environ.get("SCREENSHOTS_X11_PIXELS_OUTER_PID", "0")),
            "Native worker requires its live private outer owner")
    directory = Path(args.worker_root)
    require(directory.is_dir() and directory.stat().st_mode & 0o077 == 0
            and directory.name.startswith("screenshots-x11-pixels-private-"), "Invalid private root")
    test_binary, xvfb, xauth, xdpyinfo = map(executable, (args.core_tests, args.xvfb, "xauth", "xdpyinfo"))
    binary_hash = digest(test_binary)
    display = next((f":{number}" for number in range(480, 540)
                    if not Path(f"/tmp/.X{number}-lock").exists()
                    and not Path(f"/tmp/.X11-unix/X{number}").exists()), None)
    require(display, "No unused private display")
    authority = directory / f"authority-{args.depth}"
    env = {key: value for key, value in os.environ.items()
           if key not in ("WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DBUS_SESSION_BUS_ADDRESS", "AT_SPI_BUS_ADDRESS")}
    for name in ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME",
                 "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_RUNTIME_DIR"):
        env[name] = str(directory / name)
        Path(env[name]).mkdir(exist_ok=True, mode=0o700)
    env.update(DISPLAY=display, XAUTHORITY=str(authority), XDG_SESSION_TYPE="x11",
               SCREENSHOTS_PRIVATE_X11_PIXELS="owned-truecolor",
               SCREENSHOTS_PRIVATE_X11_DEPTH=str(args.depth), MESA_SHADER_CACHE_DISABLE="true")
    subprocess.run([xauth, "-f", str(authority), "add", display, ".", secrets.token_hex(16)],
                   env=env, check=True, capture_output=True, timeout=5)
    authority.chmod(0o600)
    with (directory / f"xvfb-{args.depth}.log").open("wb") as log:
        server = subprocess.Popen([xvfb, display, "-screen", "0", f"96x64x{args.depth}",
                                   "-auth", str(authority), "-nolisten", "tcp", "-noreset"],
                                  env=env, stdout=log, stderr=subprocess.STDOUT)
    x = xlib()
    connection = gc = window = None
    try:
        startup_deadline = time.monotonic() + 8
        while True:
            require(server.poll() is None, f"Private Xvfb depth {args.depth} stopped")
            if subprocess.run([xdpyinfo, "-display", display], env=env,
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=1).returncode == 0:
                break
            require(time.monotonic() < startup_deadline, "Private Xvfb startup deadline")
            time.sleep(0.05)
        # This process opens only the just-created server, using its private cookie.
        os.environ["XAUTHORITY"] = str(authority)
        connection = x.XOpenDisplay(display.encode())
        require(connection, "Cannot open private Xvfb")
        screen = x.XDefaultScreen(connection)
        require(x.XDefaultDepth(connection, screen) == args.depth, "Private root depth differs")
        visual = VisualInfo()
        require(x.XMatchVisualInfo(connection, screen, args.depth, 4, c.byref(visual)), "Missing TrueColor visual")
        require(x.XVisualIDFromVisual(x.XDefaultVisual(connection, screen)) == visual.visualid,
                "Fixture TrueColor visual is not the root visual")
        masks, palette = {
            16: ((0xf800, 0x07e0, 0x001f), (0, 0xf800, 0x07e0, 0x001f, 0x7bef)),
            24: ((0xff0000, 0xff00, 0xff), (0, 0xff0000, 0xff00, 0xff, 0x7f8081)),
            30: ((0x3ff00000, 0xffc00, 0x3ff), (0, 0x3ff00000, 0xffc00, 0x3ff, 0x1ff80201)),
        }[args.depth]
        require((visual.red_mask, visual.green_mask, visual.blue_mask) == masks,
                "Private Xvfb masks differ from the literal independent fixture")
        root = x.XRootWindow(connection, screen)
        window = x.XCreateSimpleWindow(connection, root, 20, 20, 5, 3, 0, 0, 0)
        require(window, "Cannot create owned test window")
        x.XMapWindow(connection, window)
        gc = x.XCreateGC(connection, root, 0, None)
        require(gc, "Cannot create owned graphics context")
        atom = lambda name: x.XInternAtom(connection, name.encode(), 0)
        data = (c.c_ulong * 1)(window)
        x.XChangeProperty(connection, root, atom("_NET_CLIENT_LIST_STACKING"), 33, 32, 0, data, 1)
        for target, left, top in ((root, 3, 4), (window, 0, 0)):
            for y in range(3):
                for column in range(5):
                    x.XSetForeground(connection, gc, palette[(column + y) % 5])
                    x.XDrawPoint(connection, target, gc, left + column, top + y)
        x.XSync(connection, 0)

        def verify_source():
            for target, left, top in ((root, 3, 4), (window, 0, 0)):
                image = x.XGetImage(connection, target, left, top, 5, 3, c.c_ulong(-1).value, 2)
                require(image, "Cannot read independent private source")
                try:
                    require(all(x.XGetPixel(image, column, y) == palette[(column + y) % 5]
                                for y in range(3) for column in range(5)), "Private source pixels changed")
                finally:
                    x.XDestroyImage(image)

        verify_source()
        require(digest(test_binary) == binary_hash, "Core executable changed before fixture")
        result = subprocess.run([test_binary, TEST, "--ignored", "--exact", "--nocapture", "--test-threads=1"],
                                env=env, capture_output=True, timeout=15)
        require(result.returncode == 0 and b"1 passed" in result.stdout and b"0 failed" in result.stdout,
                f"Private depth {args.depth} exact core test failed: {result.stdout!r} {result.stderr!r}")
        require(digest(test_binary) == binary_hash, "Core executable changed during fixture")
        verify_source()
    finally:
        if connection:
            if gc:
                x.XFreeGC(connection, gc)
            if window:
                x.XDestroyWindow(connection, window)
            x.XCloseDisplay(connection)
        server.terminate()
        try:
            server.wait(timeout=3)
        except subprocess.TimeoutExpired:
            server.kill()
            server.wait(timeout=3)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--xvfb", required=True)
    parser.add_argument("--core-tests", required=True)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--owned-worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--worker-root", help=argparse.SUPPRESS)
    parser.add_argument("--depth", type=int, choices=(16, 24, 30), help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.owned_worker:
        require(args.worker_root and args.depth, "Missing private worker arguments")
        worker(args)
    else:
        require(args.worker_root is None and args.depth is None, "Private arguments require owned worker")
        supervised(args)


if __name__ == "__main__":
    main()
