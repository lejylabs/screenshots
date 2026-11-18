#!/usr/bin/env python3
"""Finite native policy gate; owns an authenticated TCP-disabled Xvfb and every pixel.

Requires an already built executable, Xvfb, xauth, xdpyinfo, FFmpeg and installed
libX11/libXcomposite. No user server, input, clipboard or service is accessed.
"""
import argparse
import contextlib
import importlib.util
import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import sys
import tempfile
import time


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def executable(value):
    result = shutil.which(str(value))
    require(result, f"Missing executable: {value}")
    return str(Path(result).resolve())


class VisualInfo(c.Structure):
    _fields_ = [("visual", c.c_void_p), ("visualid", c.c_ulong), ("screen", c.c_int),
                ("depth", c.c_int), ("class_", c.c_int), ("red_mask", c.c_ulong),
                ("green_mask", c.c_ulong), ("blue_mask", c.c_ulong),
                ("colormap_size", c.c_int), ("bits_per_rgb", c.c_int)]


class Attributes(c.Structure):
    _fields_ = [("background_pixmap", c.c_ulong), ("background_pixel", c.c_ulong),
                ("border_pixmap", c.c_ulong), ("border_pixel", c.c_ulong),
                ("bit_gravity", c.c_int), ("win_gravity", c.c_int),
                ("backing_store", c.c_int), ("backing_planes", c.c_ulong),
                ("backing_pixel", c.c_ulong), ("save_under", c.c_int),
                ("event_mask", c.c_long), ("do_not_propagate_mask", c.c_long),
                ("override_redirect", c.c_int), ("colormap", c.c_ulong), ("cursor", c.c_ulong)]


def native():
    x = c.CDLL("libX11.so.6")
    ptr, ul, ui, integer = c.c_void_p, c.c_ulong, c.c_uint, c.c_int
    signatures = [
        ("XOpenDisplay", ptr, c.c_char_p), ("XCloseDisplay", integer, ptr),
        ("XDefaultScreen", integer, ptr), ("XRootWindow", ul, ptr, integer),
        ("XCreateSimpleWindow", ul, ptr, ul, integer, integer, ui, ui, ui, ul, ul),
        ("XCreateWindow", ul, ptr, ul, integer, integer, ui, ui, ui, integer, ui, ptr, ul, c.POINTER(Attributes)),
        ("XCreateColormap", ul, ptr, ul, ptr, integer), ("XFreeColormap", integer, ptr, ul),
        ("XMatchVisualInfo", integer, ptr, integer, integer, integer, c.POINTER(VisualInfo)),
        ("XMapWindow", integer, ptr, ul), ("XDestroyWindow", integer, ptr, ul),
        ("XResizeWindow", integer, ptr, ul, ui, ui),
        ("XClearWindow", integer, ptr, ul), ("XSync", integer, ptr, integer),
        ("XCreateGC", ptr, ptr, ul, ul, ptr), ("XFreeGC", integer, ptr, ptr),
        ("XSetForeground", integer, ptr, ptr, ul),
        ("XFillRectangle", integer, ptr, ul, ptr, integer, integer, ui, ui),
        ("XStoreName", integer, ptr, ul, c.c_char_p),
        ("XInternAtom", ul, ptr, c.c_char_p, integer),
        ("XChangeProperty", integer, ptr, ul, ul, ul, integer, integer, ptr, integer),
    ]
    for name, result, *args in signatures:
        fn = getattr(x, name)
        fn.restype, fn.argtypes = result, args
    composite = c.CDLL("libXcomposite.so.1")
    for name in ("XCompositeRedirectWindow", "XCompositeUnredirectWindow"):
        fn = getattr(composite, name)
        fn.restype, fn.argtypes = None, [ptr, ul, integer]
    return x, composite


def supervised(args):
    # Native Xlib waits run only in a child; the outer owner never waits inside Xlib.
    spec = importlib.util.spec_from_file_location(
        "window_policy_process_owner", Path(__file__).with_name("test-unix-native-services.py"))
    owner = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(owner)
    family = owner.OwnedChildren()
    with owner.Interrupts() as interrupts:
        try:
            family.runtime = Path(tempfile.mkdtemp(prefix="screenshots-window-policy-private-"))
            family.runtime.chmod(0o700)
            command = [sys.executable, str(Path(__file__).resolve()), "--owned-worker",
                       "--owned-worker-root", str(family.runtime), "--binary", args.binary,
                       "--core-tests", args.core_tests, "--xvfb", args.xvfb]
            if args.report:
                command += ["--report", str(args.report.resolve())]
            environment = dict(os.environ, SCREENSHOTS_WINDOW_POLICY_OUTER_PID=str(os.getpid()))
            owner.run(command, 90, family, interrupts, env=environment)
        finally:
            # Includes separately grouped/adopted CLI helpers and Xvfb after worker death.
            family.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--xvfb", required=True)
    parser.add_argument("--core-tests", required=True, help="Built optimized app_core test executable; no Cargo is run by this fixture")
    parser.add_argument("--report", type=Path)
    parser.add_argument("--owned-worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--owned-worker-root", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if not args.owned_worker:
        require(args.owned_worker_root is None, "Worker root requires the private outer supervisor")
        supervised(args)
        return
    require(args.owned_worker_root and os.getppid() == int(os.environ.get("SCREENSHOTS_WINDOW_POLICY_OUTER_PID", "0")),
            "Native worker requires its live private outer owner")
    binary, xvfb = executable(args.binary), executable(args.xvfb)
    core_tests = executable(args.core_tests)
    xauth, xdpyinfo, ffmpeg = map(executable, ("xauth", "xdpyinfo", "ffmpeg"))
    identity = lambda: (lambda st: (st.st_dev, st.st_ino, st.st_size, st.st_mtime_ns))(Path(binary).stat())
    admitted_binary = identity()
    binary_hash = hashlib.file_digest(open(binary, "rb"), "sha256").hexdigest()
    x, composite = native()
    calls = pixels_checked = 0
    started = time.monotonic()
    with contextlib.nullcontext(args.owned_worker_root) as temporary:
        directory = Path(temporary)
        require(directory.stat().st_mode & 0o077 == 0, "Fixture directory must be private")
        display = next((f":{i}" for i in range(420, 480)
                        if not Path(f"/tmp/.X{i}-lock").exists()
                        and not Path(f"/tmp/.X11-unix/X{i}").exists()), None)
        require(display, "No unused private display")
        authority = directory / "authority"
        # Subprocesses receive only redirected state and this private server's credentials.
        env = dict(os.environ)
        for name in ("WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DBUS_SESSION_BUS_ADDRESS", "AT_SPI_BUS_ADDRESS"):
            env.pop(name, None)
        for name in ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_RUNTIME_DIR"):
            env[name] = str(directory / name)
        env.update(DISPLAY=display, XAUTHORITY=str(authority), XDG_SESSION_TYPE="x11",
                   MESA_SHADER_CACHE_DISABLE="true")
        subprocess.run([xauth, "-f", str(authority), "add", display, ".", secrets.token_hex(16)],
                       env=env, capture_output=True, timeout=5, check=True)
        authority.chmod(0o600)
        log = (directory / "server.log").open("wb")
        server = subprocess.Popen([xvfb, display, "-screen", "0", "240x180x24", "-auth", str(authority),
                                   "-nolisten", "tcp", "-noreset", "+extension", "Composite", "+extension", "RENDER"],
                                  env=env, stdout=log, stderr=subprocess.STDOUT)
        connection = None
        gc = None
        cmap = 0
        owned = []
        try:
            for _ in range(100):
                require(server.poll() is None, "Private Xvfb stopped")
                if subprocess.run([xdpyinfo, "-display", display], env=env, capture_output=True, timeout=2).returncode == 0:
                    break
                time.sleep(0.05)
            else:
                raise RuntimeError("Private Xvfb startup deadline")
            old_auth = os.environ.get("XAUTHORITY")
            os.environ["XAUTHORITY"] = str(authority)
            try:
                connection = x.XOpenDisplay(display.encode())
            finally:
                if old_auth is None:
                    os.environ.pop("XAUTHORITY", None)
                else:
                    os.environ["XAUTHORITY"] = old_auth
            require(connection, "Cannot open owned private Xvfb")
            screen = x.XDefaultScreen(connection)
            root = x.XRootWindow(connection, screen)
            atom = lambda name: x.XInternAtom(connection, name.encode(), 0)

            def property32(target, name, kind, values):
                data = (c.c_ulong * len(values))(*values)
                x.XChangeProperty(connection, target, atom(name), kind, 32, 0, data, len(values))

            def register(window, name):
                property32(window, "_NET_WM_PID", 6, [os.getpid()])
                property32(window, "_NET_WM_STATE", 4, [])
                property32(window, "WM_STATE", atom("WM_STATE"), [1, 0])
                title = name.encode()
                x.XChangeProperty(connection, window, atom("_NET_WM_NAME"), atom("UTF8_STRING"), 8, 0, title, len(title))
                x.XStoreName(connection, window, title)
            for name in ("_NET_WM_STATE_HIDDEN", "_NET_WM_STATE_MAXIMIZED_VERT", "_NET_WM_STATE_MAXIMIZED_HORZ"):
                atom(name)
            # A small owned reparenting WM model, with exact declared decoration extents.
            frame = x.XCreateSimpleWindow(connection, root, 10, 10, 104, 88, 0, 0, 0x2266AA)
            client = x.XCreateSimpleWindow(connection, frame, 4, 20, 96, 64, 0, 0, 0x123456)
            owned.append(frame)
            register(client, "private decorated leaf client")
            property32(client, "_NET_FRAME_EXTENTS", 6, [4, 4, 20, 4])
            composite.XCompositeRedirectWindow(connection, frame, 0)
            x.XMapWindow(connection, client)
            x.XMapWindow(connection, frame)
            x.XClearWindow(connection, frame)
            x.XClearWindow(connection, client)
            # Independent ARGB leaf, painted with known premultiplied native values.
            visual = VisualInfo()
            require(x.XMatchVisualInfo(connection, screen, 32, 4, c.byref(visual)), "Private server has no ARGB visual")
            require((visual.red_mask, visual.green_mask, visual.blue_mask) == (0xFF0000, 0xFF00, 0xFF),
                    "Fixture requires standard ARGB channel packing")
            cmap = x.XCreateColormap(connection, root, visual.visual, 0)
            attrs = Attributes(background_pixel=0, border_pixel=0, colormap=cmap)
            argb = x.XCreateWindow(connection, root, 150, 30, 2, 2, 0, 32, 1, visual.visual,
                                   (1 << 1) | (1 << 3) | (1 << 13), c.byref(attrs))
            owned.append(argb)
            register(argb, "private ARGB leaf")
            property32(argb, "_NET_FRAME_EXTENTS", 6, [0, 0, 0, 0])
            composite.XCompositeRedirectWindow(connection, argb, 0)
            x.XMapWindow(connection, argb)
            gc = x.XCreateGC(connection, argb, 0, None)
            for index, pixel in enumerate((0, 0x80402010, 0xFF123456, 0x01010000)):
                x.XSetForeground(connection, gc, pixel)
                x.XFillRectangle(connection, argb, gc, index % 2, index // 2, 1, 1)
            # An opaque sibling covers the frame. Root-screen sampling would return green.
            occluder = x.XCreateSimpleWindow(connection, root, 10, 10, 104, 88, 0, 0, 0x00EE11)
            owned.append(occluder)
            x.XMapWindow(connection, occluder)
            property32(root, "_NET_CLIENT_LIST_STACKING", 33, [client, argb])
            property32(root, "_NET_CLIENT_LIST", 33, [client, argb])
            property32(root, "_NET_ACTIVE_WINDOW", 33, [client])
            x.XSync(connection, 0)

            def invoke(arguments, success=True, extra_env=None):
                nonlocal calls
                calls += 1
                require(identity() == admitted_binary, "Executable changed during fixture")
                result = subprocess.run([binary, *map(str, arguments)], env=env | (extra_env or {}),
                                        capture_output=True, timeout=15)
                require(identity() == admitted_binary, "Executable changed during fixture")
                require((result.returncode == 0) == success,
                        f"Unexpected owned CLI result ({arguments!r}, exit={result.returncode}): "
                        f"stderr={result.stderr.decode(errors='replace')} stdout={result.stdout.decode(errors='replace')}")
                if not success:
                    require(not result.stdout, "Failed capture published a success result")
                return result

            def capture(name, window, options, expected=None, extra_env=None):
                nonlocal pixels_checked
                output = directory / f"{name}.png"
                invoke(["capture-task", "window", window, output, "--capture-options", json.dumps(options)],
                       expected is not None, extra_env)
                if expected is None:
                    require(not output.exists(), "Rejected policy wrote an image")
                else:
                    raw = subprocess.check_output([ffmpeg, "-v", "error", "-i", str(output), "-frames:v", "1",
                                                   "-pix_fmt", "rgba", "-f", "rawvideo", "pipe:1"], env=env, timeout=10)
                    require(raw == expected, f"Independent RGBA oracle failed: {name}")
                    pixels_checked += len(raw) // 4

            listed = json.loads(invoke(["list", "windows"]).stdout)
            require({str(row["id"]) for row in listed} == {str(client), str(argb)}, "Window enumeration omitted owned EWMH clients")
            frame_pixels = bytearray(bytes((34, 102, 170, 255)) * (104 * 88))
            for yy in range(20, 84):
                for xx in range(4, 100):
                    offset = (yy * 104 + xx) * 4
                    frame_pixels[offset:offset+4] = bytes((18, 52, 86, 255))
            capture("frame", client, {"window_frame": "include"}, frame_pixels)
            capture("frame-preserve", client, {"window_frame": "include", "window_shadow": "exclude", "window_transparency": "preserve"}, frame_pixels)
            native_test_env = env | {"SCREENSHOTS_PRIVATE_WINDOW_POLICY": "owned-composite-frame-argb",
                                     "SCREENSHOTS_PRIVATE_WINDOW_POLICY_CLIENT": str(client),
                                     "SCREENSHOTS_PRIVATE_WINDOW_POLICY_PID": str(os.getpid())}
            native_core_case = "capture_native::platform::window_policy::tests::private_x11_policy_rejects_changes_after_readback_and_cancels_decoding"
            with open(core_tests, "rb") as native_binary:
                core_tests_hash = hashlib.file_digest(native_binary, "sha256").hexdigest()
            listed_core = subprocess.run([core_tests, native_core_case, "--ignored", "--exact", "--list"],
                                        env=native_test_env, capture_output=True, timeout=10, check=True)
            require(listed_core.stdout.decode().splitlines().count(native_core_case + ": test") == 1,
                    "Exact core native case was not discovered exactly once")
            core_run = subprocess.run([core_tests, native_core_case,
                                       "--ignored", "--exact", "--nocapture", "--test-threads=1"],
                                      env=native_test_env, capture_output=True, timeout=30, check=True)
            require(b"test result: ok. 1 passed; 0 failed; 0 ignored;" in core_run.stdout,
                    "Core native case did not execute and pass exactly once")
            capture("client", client, {"window_frame": "client"}, bytes((18, 52, 86, 255)) * (96 * 64))
            rgba = bytes((0, 0, 0, 0, 128, 64, 32, 128, 18, 52, 86, 255, 255, 0, 0, 1))
            capture("argb", argb, {"window_transparency": "preserve"}, rgba)
            bg = (12, 34, 56)
            flattened = bytearray()
            for offset in range(0, len(rgba), 4):
                alpha = rgba[offset+3]
                flattened.extend((rgba[offset+i] * alpha + bg[i] * (255-alpha) + 127) // 255 for i in range(3))
                flattened.append(255)
            capture("argb-opaque", argb, {"window_frame": "include", "window_transparency": "opaque", "background": bg}, flattened)
            capture("shadow", client, {"window_frame": "include", "window_shadow": "include"})
            property32(client, "_NET_FRAME_EXTENTS", 6, [5, 4, 20, 4]); x.XSync(connection, 0)
            capture("invalid-extents", client, {"window_frame": "include"})
            property32(client, "_NET_FRAME_EXTENTS", 6, [4, 4, 20, 4]); x.XSync(connection, 0)
            composite.XCompositeRedirectWindow(connection, client, 0); x.XSync(connection, 0)
            capture("redirected-client", client, {"window_frame": "include"})
            composite.XCompositeUnredirectWindow(connection, client, 0); x.XSync(connection, 0)
            child = x.XCreateSimpleWindow(connection, argb, 0, 0, 1, 1, 0, 0, 0)
            x.XMapWindow(connection, child); x.XSync(connection, 0)
            capture("child-hierarchy", argb, {"window_transparency": "preserve"})
            x.XDestroyWindow(connection, child); x.XSync(connection, 0)
            capture("wayland", argb, {"window_transparency": "preserve"}, extra_env={"WAYLAND_DISPLAY": "owned-denied-fixture"})
            # Different-depth ARGB windows can keep implicit server redirection.
            # Use the root-depth opaque frame to require a genuinely absent backing.
            composite.XCompositeUnredirectWindow(connection, frame, 0); x.XSync(connection, 0)
            capture("no-backing", client, {"window_frame": "include"})
            state_paths = [str(path.relative_to(directory)) for name in
                           ("XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME")
                           if Path(env[name]).exists() for path in Path(env[name]).rglob("*")]
            require(not state_paths, f"Owned capture initialized unrelated state: {state_paths}")
        finally:
            if connection:
                if gc:
                    x.XFreeGC(connection, gc)
                for window in reversed(owned):
                    x.XDestroyWindow(connection, window)
                if cmap:
                    x.XFreeColormap(connection, cmap)
                x.XSync(connection, 0)
                x.XCloseDisplay(connection)
            server.terminate()
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill(); server.wait(timeout=5)
            log.close()
    report = {"status": "passed", "binary_sha256": binary_hash, "core_tests_sha256": core_tests_hash,
              "core_case": native_core_case, "core_cases_executed": 1, "outer_deadline_seconds": 90,
              "whole_family_supervisor": True, "cli_invocations": calls,
              "exact_rgba_pixels": pixels_checked, "post_readback_identity_geometry_and_row_cancel_gate": "passed", "elapsed_seconds": time.monotonic() - started,
              "limits": "tiny owned Xvfb fixture; not native desktop/resource acceptance"}
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
