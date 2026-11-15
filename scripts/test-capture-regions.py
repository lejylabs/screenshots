#!/usr/bin/env python3
"""Check multi-region capture on a private synthetic 96x64 Xvfb screen.

Requires Python 3, libX11, xauth, xdpyinfo, FFmpeg, Xvfb and a built screenshots
binary. No download, installation, or connection to the user's display occurs.
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
REGIONS = [(0, 0, 48, 40), (24, 16, 48, 32), (36, 24, 24, 24)]


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


def color(x, y):
    return ((x * 17 + y * 3) % 256, (x * 5 + y * 11) % 256,
            (x * 7 + y * 19) % 256, 255)


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
    help_result = subprocess.run([binary, "--help"], check=True, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, timeout=10)
    if b"capture-regions" not in help_result.stdout:
        raise SystemExit("The selected binary does not include capture-regions; rebuild it first")
    xlib = ctypes.CDLL("libX11.so.6")
    signatures = [
        ("XOpenDisplay", [ctypes.c_char_p], ctypes.c_void_p),
        ("XDefaultScreen", [ctypes.c_void_p], ctypes.c_int),
        ("XRootWindow", [ctypes.c_void_p, ctypes.c_int], ctypes.c_ulong),
        ("XCreateGC", [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_void_p], ctypes.c_void_p),
        ("XSetForeground", [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_ulong], ctypes.c_int),
        ("XDrawPoint", [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_void_p, ctypes.c_int, ctypes.c_int], ctypes.c_int),
        ("XGetImage", [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_int, ctypes.c_uint, ctypes.c_uint, ctypes.c_ulong, ctypes.c_int], ctypes.c_void_p),
        ("XGetPixel", [ctypes.c_void_p, ctypes.c_int, ctypes.c_int], ctypes.c_ulong),
        ("XDestroyImage", [ctypes.c_void_p], ctypes.c_int),
        ("XSync", [ctypes.c_void_p, ctypes.c_int], ctypes.c_int),
        ("XFreeGC", [ctypes.c_void_p, ctypes.c_void_p], ctypes.c_int),
        ("XCloseDisplay", [ctypes.c_void_p], ctypes.c_int),
    ]
    for name, arguments, result in signatures:
        function = getattr(xlib, name)
        function.argtypes = arguments
        function.restype = result

    with tempfile.TemporaryDirectory(prefix="screenshots-capture-regions-") as temporary:
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
        connection = gc = None
        with (root / "xvfb.log").open("wb") as log:
            server = subprocess.Popen([xvfb, display, "-screen", "0", "96x64x24", "-auth",
                                       str(auth), "-nolisten", "tcp", "-noreset"], env=env,
                                      stdout=log, stderr=subprocess.STDOUT)
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
            for y in range(64):
                for x in range(96):
                    red, green, blue, _ = color(x, y)
                    xlib.XSetForeground(connection, gc, red << 16 | green << 8 | blue)
                    xlib.XDrawPoint(connection, window, gc, x, y)
            xlib.XSync(connection, False)

            def screen_pixels():
                image = xlib.XGetImage(connection, window, 0, 0, 96, 64,
                                      ctypes.c_ulong(-1).value, 2)
                if not image:
                    raise RuntimeError("Cannot inspect private synthetic screen")
                try:
                    return [xlib.XGetPixel(image, x, y) for y in range(64) for x in range(96)]
                finally:
                    xlib.XDestroyImage(image)

            before = screen_pixels()
            listed = subprocess.run([binary, "list", "displays"], env=env, check=True,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
            displays = json.loads(listed.stdout)
            if len(displays) != 1:
                raise RuntimeError(f"Unexpected private display inventory: {displays}")
            display_id = str(displays[0]["id"])
            destination = root / "regions"
            destination.mkdir()
            command = [binary, "capture-regions", str(destination)]
            for region in REGIONS:
                command += [display_id, *(str(value) for value in region)]
            run = subprocess.run(command, env=env, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, timeout=15)
            if run.returncode or run.stderr:
                raise RuntimeError(f"Capture failed: {run.stderr.decode(errors='replace')}")
            result = json.loads(run.stdout)
            expected_names = [f"region-{index:02}.png" for index in range(1, 4)]
            if result["files"] != expected_names or result["captured_displays"] != 1 or result["completed"] != 3:
                raise RuntimeError(f"Unexpected capture result: {result}")
            if sorted(path.name for path in destination.iterdir()) != expected_names:
                raise RuntimeError("Unexpected region output or staging files")
            decoded = []
            for name, (left, top, width, height) in zip(expected_names, REGIONS):
                pixels = subprocess.check_output([ffmpeg, "-v", "error", "-i",
                         str(destination / name), "-frames:v", "1", "-f", "rawvideo",
                         "-pix_fmt", "rgba", "pipe:1"], env=env, timeout=10)
                expected = bytes(channel for y in range(top, top + height)
                                 for x in range(left, left + width) for channel in color(x, y))
                if pixels != expected:
                    raise RuntimeError(f"Decoded pixels do not match synthetic region {name}")
                decoded.append(pixels)
            overlaps = 0
            for first in range(3):
                for second in range(first + 1, 3):
                    ax, ay, aw, ah = REGIONS[first]
                    bx, by, bw, bh = REGIONS[second]
                    for y in range(max(ay, by), min(ay + ah, by + bh)):
                        for x in range(max(ax, bx), min(ax + aw, bx + bw)):
                            ai = ((y - ay) * aw + x - ax) * 4
                            bi = ((y - by) * bw + x - bx) * 4
                            if decoded[first][ai:ai + 4] != decoded[second][bi:bi + 4]:
                                raise RuntimeError("Overlapping regions contain different pixels")
                            overlaps += 1
            if screen_pixels() != before:
                raise RuntimeError("Capture changed the private synthetic source")
            if any(Path(env[name]).exists() for name in state):
                raise RuntimeError("Capture created unexpected user-state paths")
            results = {"scope": "isolated synthetic Xvfb; not hardware acceptance",
                       "binary_sha256": digest(binary), "regions": REGIONS,
                       "result": {key: value for key, value in result.items() if key != "directory"},
                       "exact_decoded_pixels": True, "equal_overlap_pixels": overlaps,
                       "source_unchanged": True, "user_state_created": False}
        finally:
            if connection:
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
