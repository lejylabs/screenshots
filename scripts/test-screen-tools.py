#!/usr/bin/env python3
"""Verify ruler CLI and, optionally, screen colors on an authenticated private Xvfb.

No connection to the user's display/clipboard; no capture files or uploads.
"""
import argparse
import ctypes as c
import hashlib
import json
import math
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]


def require(value, message):
    if not value:
        raise RuntimeError(message)


def executable(value):
    path = shutil.which(str(value))
    require(path, "Required executable unavailable: " + str(value))
    return str(Path(path).resolve())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release-fast/screenshots")
    parser.add_argument("--xvfb")
    parser.add_argument("--core-test-binary", type=Path)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    binary = executable(args.binary)
    with Path(binary).open("rb") as source:
        binary_digest = hashlib.file_digest(source, "sha256").hexdigest()
    def identity():
        stat = Path(binary).stat()
        return stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns
    binary_identity = identity()
    calls = 0
    with tempfile.TemporaryDirectory(prefix="screenshots-screen-tools-") as temporary:
        root = Path(temporary)
        env = dict(os.environ)
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DBUS_SESSION_BUS_ADDRESS", "AT_SPI_BUS_ADDRESS"):
            env.pop(key, None)
        env.update({key: str(root / key) for key in ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME")})

        def invoke(values, success=True):
            nonlocal calls
            require(identity() == binary_identity, "Selected binary changed during verification")
            result = subprocess.run([binary, *map(str, values)], env=env, stdin=subprocess.DEVNULL,
                                    capture_output=True, timeout=25)
            calls += 1
            if success:
                require(result.returncode == 0, "Command failed: " + result.stderr.decode(errors="replace"))
                return json.loads(result.stdout)
            require(result.returncode != 0 and not result.stdout, "Invalid command produced successful result")

        # Independent numeric expectations, including full i32 span and anisotropic physical DPI.
        result = invoke(["ruler", 10, 20, 13, 24])
        require(result["distance_pixels"] == 5 and result["rectangle_area_square_pixels"] == 12, "3-4-5 ruler mismatch")
        require("physical" not in result, "Ruler invented physical DPI")
        result = invoke(["ruler", -100, -200, -4, -8, "--dpi", 96, 192])
        require(result["physical"]["dx_inches"] == result["physical"]["dy_inches"] == 1, "Anisotropic DPI mismatch")
        require(math.isclose(result["physical"]["distance_mm"], math.sqrt(2) * 25.4), "Physical length mismatch")
        result = invoke(["ruler", -(2**31), -(2**31), 2**31 - 1, 2**31 - 1])
        require(result["rectangle_area_square_pixels"] == (2**32 - 1)**2, "Full-span area lost exact integer precision")
        require(math.isfinite(result["distance_pixels"]), "Full-span length overflowed")
        result = invoke(["ruler", 3, 4, 3, 4])
        require(result["distance_pixels"] == 0 and result["clockwise_angle_degrees"] is None, "Zero-length angle invented")
        result = invoke(["ruler", 3, 4, 0, 0])
        require(result["dx_pixels"] == -3 and result["dy_pixels"] == -4, "Directed length mismatch")
        result = invoke(["ruler", 0, 0, 0, 8])
        require(result["clockwise_angle_degrees"] == 90, "Downward ruler angle is not clockwise90 degrees")
        for values in (["ruler", 0, 0, 1], ["ruler", 2**31, 0, 0, 0],
                       ["screen-color", "fixture", -1, 0], ["screen-color", "../invalid", 0, 0]):
            invoke(values, False)
        for dpi in ("NaN", "inf", "0", "-1", "0.0000001", "1000001"):
            invoke(["ruler", 0, 0, 1, 1, "--dpi", dpi, "96"], False)
        ruler_cases = calls
        colors = 0
        direct_vendor_cases = 0
        dpi_stages = []
        if args.xvfb:
            require(os.name == "posix" and os.uname().sysname == "Linux", "Private Xvfb test requires Linux")
            require("WAYLAND_SOCKET" not in os.environ, "Private X11 fixture refuses an inherited Wayland descriptor")
            xvfb, xauth, xdpyinfo, xrandr = map(executable, (args.xvfb, "xauth", "xdpyinfo", "xrandr"))
            require(args.core_test_binary, "Private native sampling requires the matching app-core test binary")
            core_test = executable(args.core_test_binary)
            display = next((f":{i}" for i in range(320, 380)
                            if not Path(f"/tmp/.X{i}-lock").exists() and not Path(f"/tmp/.X11-unix/X{i}").exists()), None)
            require(display, "No free private X11 display")
            auth = root / "authority"
            env.update(DISPLAY=display, XAUTHORITY=str(auth), XDG_SESSION_TYPE="x11", MESA_SHADER_CACHE_DISABLE="true")
            subprocess.run([xauth, "-f", str(auth), "add", display, ".", secrets.token_hex(16)],
                           env=env, capture_output=True, check=True, timeout=5)
            with (root / "xvfb.log").open("wb") as log:
                server = subprocess.Popen([xvfb, display, "-screen", "0", "96x64x24", "-auth", str(auth),
                                           "-nolisten", "tcp", "-noreset"], env=env, stdout=log, stderr=subprocess.STDOUT)
            connection = gc = None
            try:
                deadline = time.monotonic() + 5
                while True:
                    require(server.poll() is None, "Private Xvfb stopped")
                    if subprocess.run([xdpyinfo, "-display", display], env=env, capture_output=True, timeout=1).returncode == 0:
                        break
                    require(time.monotonic() < deadline, "Private Xvfb startup deadline")
                    time.sleep(.025)
                x = c.CDLL("libX11.so.6")
                for name, parameters, returns in [
                    ("XOpenDisplay", [c.c_char_p], c.c_void_p), ("XCloseDisplay", [c.c_void_p], c.c_int),
                    ("XDefaultScreen", [c.c_void_p], c.c_int), ("XRootWindow", [c.c_void_p, c.c_int], c.c_ulong),
                    ("XCreateGC", [c.c_void_p, c.c_ulong, c.c_ulong, c.c_void_p], c.c_void_p),
                    ("XFreeGC", [c.c_void_p, c.c_void_p], c.c_int),
                    ("XSetForeground", [c.c_void_p, c.c_void_p, c.c_ulong], c.c_int),
                    ("XDrawPoint", [c.c_void_p, c.c_ulong, c.c_void_p, c.c_int, c.c_int], c.c_int),
                    ("XSync", [c.c_void_p, c.c_int], c.c_int),
                    ("XInternAtom", [c.c_void_p, c.c_char_p, c.c_int], c.c_ulong),
                    ("XChangeProperty", [c.c_void_p, c.c_ulong, c.c_ulong, c.c_ulong, c.c_int, c.c_int, c.c_void_p, c.c_int], c.c_int),
                ]:
                    function = getattr(x, name)
                    function.argtypes, function.restype = parameters, returns
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
                window = x.XRootWindow(connection, x.XDefaultScreen(connection))
                gc = x.XCreateGC(connection, window, 0, None)
                require(gc, "Cannot create own drawing context")
                points = [(0, 0, [18, 52, 86, 255]), (95, 63, [255, 128, 1, 255]),
                          (17, 33, [3, 200, 71, 255]), (94, 63, [11, 22, 33, 255])]
                extra_points = [(40, 30, [240, 22, 11, 255]), (70, 30, [5, 200, 33, 255])]
                for px, py, rgba in points + extra_points:
                    x.XSetForeground(connection, gc, rgba[0] << 16 | rgba[1] << 8 | rgba[2])
                    x.XDrawPoint(connection, window, gc, px, py)
                x.XSync(connection, 0)
                displays = invoke(["list", "displays"])
                require(len(displays) == 1 and (displays[0]["width"], displays[0]["height"]) == (96, 64), "Wrong private display geometry")
                identifier = displays[0]["id"]
                for px, py, rgba in points:
                    for suffix in ([], ["--global"]):
                        result = invoke(["screen-color", identifier, px, py, *suffix])
                        require(result["rgba"] == rgba, "Private native sample did not match independently drawn bytes")
                        require(result["hex_rgba"] == "#" + bytes(rgba).hex().upper(), "Color hex differs from sampled bytes")
                        require(result["encoding"] == "capture_backend_rgba8" and result["cursor"] == "excluded", "Color sample invented encoding or cursor policy")
                        colors += 1
                for values in (["screen-color", identifier, 96, 0], ["screen-color", identifier, 0, 64],
                               ["screen-color", identifier, -1, 0, "--global"], ["screen-color", "missing", 0, 0]):
                    invoke(values, False)
                resource = x.XInternAtom(connection, b"RESOURCE_MANAGER", 0)
                string = x.XInternAtom(connection, b"STRING", 0)
                def set_dpi(value):
                    contents = f"Xft.dpi:\t{value}\n".encode()
                    payload = c.create_string_buffer(contents)
                    x.XChangeProperty(connection, window, resource, string, 8, 0, payload, len(contents))
                    x.XSync(connection, 0)
                for value in (48, 192):
                    set_dpi(value)
                    metadata = invoke(["list", "displays"])
                    require(len(metadata) == 1 and (metadata[0]["width"], metadata[0]["height"]) == (96, 64), "Xft DPI changed physical display dimensions")
                    require(metadata[0]["scale"] == value / 96, "Xft UI scale metadata was lost")
                    for px, py, rgba in points:
                        for suffix in ([], ["--global"]):
                            result = invoke(["screen-color", identifier, px, py, *suffix])
                            require(result["rgba"] == rgba, "Scaled X11 sample did not use native raster coordinates")
                            colors += 1
                    invoke(["screen-color", identifier, 96, 0], False)
                    dpi_stages.append({"dpi": value, "native_monitor": [96, 64]})
                # A real right-half raster exists on the SAME root. Only the left48x64
                # monitor owns the selected RandR output. With Xft48 the old logical
                # bounds admitted x70 and leaked the independently drawn adjacent pixel.
                for values in (("--setmonitor", "private-left", "48/12x64/16+0+0", "screen"),
                               ("--setmonitor", "private-right", "48/12x64/16+48+0", "none")):
                    subprocess.run([xrandr, *values], env=env, capture_output=True, check=True, timeout=5)
                for value in (48, 192):
                    set_dpi(value)
                    metadata = invoke(["list", "displays"])
                    require(len(metadata) == 1 and (metadata[0]["width"], metadata[0]["height"]) == (48, 64), "Selected split monitor metadata is not native48x64")
                    for suffix in ([], ["--global"]):
                        result = invoke(["screen-color", identifier, 40, 30, *suffix])
                        require(result["rgba"] == [240, 22, 11, 255], "Selected monitor far-half sample changed with Xft DPI")
                        colors += 1
                        invoke(["screen-color", identifier, 70, 30, *suffix], False)
                    native_env = dict(env, SCREENSHOTS_PRIVATE_X11_BOUNDS="owned-48x64-raster", SCREENSHOTS_PRIVATE_X11_DPI=str(value))
                    proof = subprocess.run([core_test, "capture_native::platform::tests::private_x11_physical_region_bounds_do_not_read_the_adjacent_raster", "--ignored", "--exact", "--test-threads=1"],
                                           env=native_env, stdin=subprocess.DEVNULL, capture_output=True, timeout=10)
                    require(proof.returncode == 0 and b"1 passed" in proof.stdout,
                            "Direct native Xcap bounds proof failed: " + proof.stdout.decode(errors="replace") + proof.stderr.decode(errors="replace"))
                    direct_vendor_cases += 1
                    dpi_stages.append({"dpi": value, "native_monitor": [48, 64], "adjacent_root_pixel_refused": True})
                require(not any(path.suffix in (".png", ".db", ".sqlite", ".sqlite3") for path in root.rglob("*")), "Read-only tools created capture/history files")
            finally:
                if gc:
                    x.XFreeGC(connection, gc)
                if connection:
                    x.XCloseDisplay(connection)
                if server.poll() is None:
                    server.terminate()
                try:
                    server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait(timeout=5)
        require(identity() == binary_identity, "Selected binary changed during verification")
        report = {"scope": "owned CLI arithmetic and optional authenticated private Xvfb; no hardware/profile acceptance",
                  "binary_sha256": binary_digest, "ruler_and_invalid_argument_cases": ruler_cases,
                  "private_native_color_results": colors, "direct_native_vendor_bounds_cases": direct_vendor_cases,
                  "xft_dpi_stages": dpi_stages, "cli_invocations": calls}
    rendered = json.dumps(report, indent=2) + "\n"
    if args.report:
        args.report.write_text(rendered, encoding="utf-8", newline="\n")
    print(rendered, end="")


if __name__ == "__main__":
    main()
