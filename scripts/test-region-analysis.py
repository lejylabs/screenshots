#!/usr/bin/env python3
"""Verify region analysis on synthetic pixels in a private Xvfb server.

Requires Python 3, Pillow, libX11, xauth, xdpyinfo, Xvfb, a built screenshots
binary, and Tesseract with the selected language. Never uses the user's display.
"""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import statistics
import subprocess
import tempfile
import time

from PIL import Image, ImageDraw, ImageFont

REPO = Path(__file__).resolve().parents[1]


def executable(value, name):
    resolved = shutil.which(value) if value else None
    if not resolved:
        raise SystemExit(f"{name} is unavailable; provide its path explicitly")
    return str(Path(resolved).resolve())


def file_digest(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as source:
        for chunk in iter(lambda: source.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--xvfb", default=shutil.which("Xvfb"))
    parser.add_argument("--binary", default=str(REPO / "target/release/screenshots"))
    parser.add_argument("--font", default="/usr/share/fonts/noto/NotoSansMono-Regular.ttf")
    parser.add_argument("--language", default="deu")
    parser.add_argument("--samples", type=int, choices=range(1, 6), default=5)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    xvfb = executable(args.xvfb, "Xvfb")
    binary = executable(args.binary, "screenshots binary")
    xauth = executable(shutil.which("xauth"), "xauth")
    xdpyinfo = executable(shutil.which("xdpyinfo"), "xdpyinfo")
    if not Path(args.font).is_file():
        raise SystemExit("Font is unavailable; provide --font /path/to/font.ttf")
    executable(shutil.which("tesseract"), "Tesseract")

    xlib = ctypes.CDLL("libX11.so.6")
    xlib.XOpenDisplay.argtypes = [ctypes.c_char_p]
    xlib.XOpenDisplay.restype = ctypes.c_void_p
    xlib.XDefaultScreen.argtypes = [ctypes.c_void_p]
    xlib.XDefaultScreen.restype = ctypes.c_int
    xlib.XRootWindow.argtypes = [ctypes.c_void_p, ctypes.c_int]
    xlib.XRootWindow.restype = ctypes.c_ulong
    xlib.XCreateGC.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_void_p]
    xlib.XCreateGC.restype = ctypes.c_void_p
    xlib.XSetForeground.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_ulong]
    xlib.XFillRectangle.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_void_p,
                                   ctypes.c_int, ctypes.c_int, ctypes.c_uint, ctypes.c_uint]
    xlib.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
    xlib.XFreeGC.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
    xlib.XCloseDisplay.argtypes = [ctypes.c_void_p]

    with tempfile.TemporaryDirectory(prefix="screenshots-region-analysis-") as temporary:
        root = Path(temporary)
        display = next((f":{n}" for n in range(90, 150)
                        if not Path(f"/tmp/.X{n}-lock").exists()
                        and not Path(f"/tmp/.X11-unix/X{n}").exists()), None)
        if display is None:
            raise SystemExit("No unused fixture X display from :90 through :149")
        auth = root / "authority"
        env = {key: value for key, value in os.environ.items()
               if key not in ["WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DBUS_SESSION_BUS_ADDRESS",
                              "DBUS_STARTER_ADDRESS", "AT_SPI_BUS_ADDRESS"]}
        runtime = root / "runtime"
        runtime.mkdir(mode=0o700)
        env["XDG_RUNTIME_DIR"] = str(runtime)
        env.update(DISPLAY=display, XAUTHORITY=str(auth), XDG_SESSION_TYPE="x11",
                   MESA_SHADER_CACHE_DISABLE="true")
        state = ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME",
                 "XDG_DATA_HOME", "XDG_CACHE_HOME"]
        env.update({name: str(root / name.lower()) for name in state})
        subprocess.run([xauth, "-f", str(auth), "add", display, ".", secrets.token_hex(16)],
                       check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        connection = None
        gc = None
        with (root / "xvfb.log").open("wb") as log:
            server = subprocess.Popen([xvfb, display, "-screen", "0", "800x600x24",
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
            previous_auth = os.environ.get("XAUTHORITY")
            os.environ["XAUTHORITY"] = str(auth)
            try:
                connection = xlib.XOpenDisplay(display.encode())
            finally:
                if previous_auth is None:
                    os.environ.pop("XAUTHORITY", None)
                else:
                    os.environ["XAUTHORITY"] = previous_auth
            if not connection:
                raise RuntimeError("Could not open private Xvfb display")
            screen = xlib.XDefaultScreen(connection)
            window = xlib.XRootWindow(connection, screen)
            gc = xlib.XCreateGC(connection, window, 0, None)
            if not gc:
                raise RuntimeError("Could not create fixture graphics context")

            def fill(color, x, y, width, height):
                xlib.XSetForeground(connection, gc, color)
                xlib.XFillRectangle(connection, window, gc, x, y, width, height)

            def paint(image, left, top):
                for y in range(image.height):
                    x = 0
                    while x < image.width:
                        color = image.getpixel((x, y))
                        end = x + 1
                        while end < image.width and image.getpixel((end, y)) == color:
                            end += 1
                        if color != (255, 255, 255):
                            fill((color[0] << 16) | (color[1] << 8) | color[2],
                                 left + x, top + y, end - x, 1)
                        x = end

            fill(0xffffff, 0, 0, 800, 600)
            payload = "isolated-native-region https://example.invalid/never-opened"
            qr_path = root / "qr.png"
            subprocess.run([binary, "qr", "generate", payload, str(qr_path)], env=env,
                           check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
            with Image.open(qr_path) as source:
                qr = source.convert("RGB")
            paint(qr, 30, 30)
            text = Image.new("RGB", (700, 150), "white")
            ImageDraw.Draw(text).text((20, 20), "HELLO 123",
                                     font=ImageFont.truetype(args.font, 66), fill="black")
            paint(text, 30, 330)
            xlib.XSync(connection, False)
            listed = subprocess.run([binary, "list", "displays"], env=env, check=True,
                                    stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
            displays = json.loads(listed.stdout)
            if len(displays) != 1:
                raise RuntimeError(f"Private display inventory was unexpected: {displays}")
            display_id = str(displays[0]["id"])
            results = {"display_info": displays, "samples": args.samples,
                       "binary_sha256": file_digest(binary),
                       "scope": "isolated synthetic Xvfb; not reference-hardware acceptance"}
            commands = {
                "qr": [binary, "qr-region", display_id, "30", "30", str(qr.width), str(qr.height)],
                "ocr": [binary, "ocr-region", display_id, "30", "330", "700", "150",
                        "--language", args.language],
            }
            for name, command in commands.items():
                samples = []
                for _ in range(args.samples):
                    started = time.monotonic()
                    run = subprocess.run(command, env=env, stdout=subprocess.PIPE,
                                         stderr=subprocess.PIPE, timeout=35)
                    samples.append(time.monotonic() - started)
                    if run.returncode or run.stderr:
                        raise RuntimeError(f"{name} failed: {run.stderr.decode(errors='replace')}")
                    result = json.loads(run.stdout)
                    matched = (result["codes"][0]["text"] == payload if name == "qr"
                               else result["text"] == "HELLO 123\n")
                    if not matched:
                        raise RuntimeError(f"Unexpected {name} result: {result}")
                results[name] = {"result": result, "seconds": samples,
                                 "median_seconds": statistics.median(samples)}
            if any(Path(env[name]).exists() for name in state):
                raise RuntimeError("Region fixture created unexpected user-state paths")
            results["user_state_created"] = False
        finally:
            if gc:
                xlib.XFreeGC(connection, gc)
            if connection:
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
