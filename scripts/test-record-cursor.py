#!/usr/bin/env python3
"""Verify recorded cursor inclusion on a private synthetic 96x64 Xvfb screen.

Requires Python 3, libX11, xauth, xdpyinfo, FFmpeg/ffprobe, Xvfb and a built
screenshots binary. No downloads, package installation or user display access.
"""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import tempfile
import time

REPO = Path(__file__).resolve().parents[1]


def executable(value, label):
    path = shutil.which(value) if value else None
    if not path:
        raise SystemExit(f"{label} is unavailable; provide its path explicitly")
    return str(Path(path).resolve())


def digest(path):
    value = hashlib.sha256()
    with open(path, "rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


class XColor(ctypes.Structure):
    _fields_ = [("pixel", ctypes.c_ulong), ("red", ctypes.c_ushort),
                ("green", ctypes.c_ushort), ("blue", ctypes.c_ushort),
                ("flags", ctypes.c_char), ("pad", ctypes.c_char)]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--xvfb", default=shutil.which("Xvfb"))
    parser.add_argument("--binary", default=str(REPO / "target/release/screenshots"))
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    xvfb = executable(args.xvfb, "Xvfb")
    binary = executable(args.binary, "screenshots binary")
    xauth = executable(shutil.which("xauth"), "xauth")
    xdpyinfo = executable(shutil.which("xdpyinfo"), "xdpyinfo")
    ffmpeg = executable(shutil.which("ffmpeg"), "FFmpeg")
    executable(shutil.which("ffprobe"), "ffprobe")
    xlib = ctypes.CDLL("libX11.so.6")
    signatures = [
        ("XOpenDisplay", [ctypes.c_char_p], ctypes.c_void_p),
        ("XDefaultScreen", [ctypes.c_void_p], ctypes.c_int),
        ("XRootWindow", [ctypes.c_void_p, ctypes.c_int], ctypes.c_ulong),
        ("XCreateGC", [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_void_p], ctypes.c_void_p),
        ("XCreateBitmapFromData", [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_char_p, ctypes.c_uint, ctypes.c_uint], ctypes.c_ulong),
        ("XCreatePixmapCursor", [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.POINTER(XColor), ctypes.POINTER(XColor), ctypes.c_uint, ctypes.c_uint], ctypes.c_ulong),
        ("XDefineCursor", [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong], ctypes.c_int),
        ("XWarpPointer", [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_int, ctypes.c_int, ctypes.c_uint, ctypes.c_uint, ctypes.c_int, ctypes.c_int], ctypes.c_int),
        ("XSetForeground", [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_ulong], ctypes.c_int),
        ("XFillRectangle", [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_uint, ctypes.c_uint], ctypes.c_int),
        ("XSync", [ctypes.c_void_p, ctypes.c_int], ctypes.c_int),
        ("XFreeCursor", [ctypes.c_void_p, ctypes.c_ulong], ctypes.c_int),
        ("XFreePixmap", [ctypes.c_void_p, ctypes.c_ulong], ctypes.c_int),
        ("XFreeGC", [ctypes.c_void_p, ctypes.c_void_p], ctypes.c_int),
        ("XCloseDisplay", [ctypes.c_void_p], ctypes.c_int),
    ]
    for name, arguments, result in signatures:
        function = getattr(xlib, name)
        function.argtypes = arguments
        function.restype = result

    with tempfile.TemporaryDirectory(prefix="screenshots-record-cursor-") as temporary:
        root = Path(temporary)
        display = next((f":{n}" for n in range(90, 150)
                        if not Path(f"/tmp/.X{n}-lock").exists()
                        and not Path(f"/tmp/.X11-unix/X{n}").exists()), None)
        if display is None:
            raise SystemExit("No unused private display from :90 through :149")
        auth = root / "authority"
        env = {key: value for key, value in os.environ.items()
               if key not in ["WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS"]}
        env.update(DISPLAY=display, XAUTHORITY=str(auth), XDG_SESSION_TYPE="x11",
                   MESA_SHADER_CACHE_DISABLE="true")
        state = ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME",
                 "XDG_DATA_HOME", "XDG_CACHE_HOME"]
        env.update({name: str(root / name.lower()) for name in state})
        subprocess.run([xauth, "-f", str(auth), "add", display, ".", secrets.token_hex(16)],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        connection = None
        gc = source = mask = cursor = None
        with (root / "xvfb.log").open("wb") as log:
            server = subprocess.Popen([xvfb, display, "-screen", "0", "96x64x24",
                                       "-auth", str(auth), "-nolisten", "tcp", "-noreset"],
                                      env=env, stdout=log, stderr=subprocess.STDOUT)
        try:
            for _ in range(100):
                if server.poll() is not None:
                    raise RuntimeError("Private Xvfb exited before becoming ready")
                ready = subprocess.run([xdpyinfo, "-display", display], env=env,
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                       timeout=2)
                if ready.returncode == 0:
                    break
                time.sleep(0.05)
            else:
                raise RuntimeError("Private Xvfb did not become ready")
            old_auth = os.environ.get("XAUTHORITY")
            os.environ["XAUTHORITY"] = str(auth)
            try:
                connection = xlib.XOpenDisplay(display.encode())
            finally:
                if old_auth is None:
                    os.environ.pop("XAUTHORITY", None)
                else:
                    os.environ["XAUTHORITY"] = old_auth
            if not connection:
                raise RuntimeError("Cannot connect to private Xvfb")
            window = xlib.XRootWindow(connection, xlib.XDefaultScreen(connection))
            gc = xlib.XCreateGC(connection, window, 0, None)
            if not gc:
                raise RuntimeError("Cannot create fixture graphics context")
            xlib.XSetForeground(connection, gc, 0x22aa66)
            xlib.XFillRectangle(connection, window, gc, 0, 0, 96, 64)
            bits = b"\xff" * 8
            source = xlib.XCreateBitmapFromData(connection, window, bits, 8, 8)
            mask = xlib.XCreateBitmapFromData(connection, window, bits, 8, 8)
            black = XColor(0, 0, 0, 0, b"\x07", b"\0")
            cursor = xlib.XCreatePixmapCursor(connection, source, mask,
                                             ctypes.byref(black), ctypes.byref(black), 0, 0)
            if not source or not mask or not cursor:
                raise RuntimeError("Cannot create fixture cursor")
            xlib.XDefineCursor(connection, window, cursor)
            xlib.XWarpPointer(connection, 0, window, 0, 0, 0, 0, 40, 24)
            xlib.XSync(connection, False)
            results = {"screen": [96, 64], "cursor_patch": [40, 24, 8, 8],
                       "binary_sha256": digest(binary), "cases": {},
                       "scope": "isolated synthetic Xvfb; not hardware acceptance"}
            frames = {}
            for mode in ["default", "hide", "show"]:
                output = root / f"{mode}.mkv"
                command = [binary, "record", "x11", str(output), "--seconds", "1",
                           "--fps", "2", "--width", "96", "--height", "64"]
                if mode != "default":
                    command += ["--cursor", mode]
                run = subprocess.run(command, env=env, stdout=subprocess.PIPE,
                                     stderr=subprocess.PIPE, timeout=15)
                if run.returncode or run.stderr:
                    raise RuntimeError(f"Recording {mode} failed: {run.stderr.decode(errors='replace')}")
                results["cases"][mode] = json.loads(run.stdout)
                frames[mode] = subprocess.check_output(
                    [ffmpeg, "-v", "error", "-i", str(output), "-frames:v", "1",
                     "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"], env=env, timeout=10)
                if len(frames[mode]) != 96 * 64 * 4:
                    raise RuntimeError("Unexpected decoded frame size")
            if frames["default"] != frames["hide"]:
                raise RuntimeError("Default frame differs from explicit hide")
            changed = [(pixel % 96, pixel // 96) for pixel in range(96 * 64)
                       if frames["show"][pixel * 4:pixel * 4 + 4]
                       != frames["hide"][pixel * 4:pixel * 4 + 4]]
            if len(changed) != 64 or not all(40 <= x < 48 and 24 <= y < 32 for x, y in changed):
                raise RuntimeError(f"Cursor changes escaped the expected 8x8 patch: {changed}")
            results["changed_pixels"] = len(changed)
            results["change_bounds"] = [min(x for x, _ in changed), min(y for _, y in changed),
                                         max(x for x, _ in changed), max(y for _, y in changed)]
            if any(Path(env[name]).exists() for name in state):
                raise RuntimeError("Recording created unexpected user-state paths")
            results["user_state_created"] = False
        finally:
            if connection:
                if cursor:
                    xlib.XFreeCursor(connection, cursor)
                if source:
                    xlib.XFreePixmap(connection, source)
                if mask:
                    xlib.XFreePixmap(connection, mask)
                if gc:
                    xlib.XFreeGC(connection, gc)
                xlib.XCloseDisplay(connection)
            server.terminate()
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
    rendered = json.dumps(results, indent=2) + "\n"
    if args.report:
        args.report.write_text(rendered)
    print(rendered, end="")


if __name__ == "__main__":
    main()
