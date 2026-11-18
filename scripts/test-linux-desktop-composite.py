#!/usr/bin/env python3
"""Owned Xvfb/RandR desktop CLI/Task proof; never connects to the user's display.

This proves native single-output pixels and rejects an outputless second monitor.
It deliberately does not count fake monitor arrays as native multi-output proof.
"""
import argparse
import ctypes as c
import hashlib
import json
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


def color(x, y):
    return bytes(((17*x + 3*y) % 256, (5*x + 11*y) % 256, (7*x + 19*y) % 256, 255))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release-fast/screenshots")
    parser.add_argument("--core-test-binary", type=Path, required=True)
    parser.add_argument("--xvfb", default="Xvfb")
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    require(os.name == "posix" and os.uname().sysname == "Linux", "Linux required")
    require("WAYLAND_SOCKET" not in os.environ, "Refusing an inherited Wayland descriptor")
    names = {name: executable(name) for name in ("xauth", "xdpyinfo", "xrandr", "ffmpeg")}
    binary, core, xvfb = map(executable, (args.binary, args.core_test_binary, args.xvfb))
    def identity(path):
        stat = Path(path).stat()
        return stat.st_dev, stat.st_ino, stat.st_size, stat.st_mtime_ns
    identities = {path: identity(path) for path in (binary, core)}
    digests = {}
    for path in identities:
        with open(path, "rb") as source:
            digests[path] = hashlib.file_digest(source, "sha256").hexdigest()
    calls = 0
    with tempfile.TemporaryDirectory(prefix="screenshots-desktop-") as temporary:
        root = Path(temporary)
        env = dict(os.environ)
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS", "AT_SPI_BUS_ADDRESS"):
            env.pop(key, None)
        for key in ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"):
            env[key] = str(root / key)
        display = next((f":{n}" for n in range(380, 440) if not Path(f"/tmp/.X{n}-lock").exists()
                        and not Path(f"/tmp/.X11-unix/X{n}").exists()), None)
        require(display, "No free private X11 display")
        authority = root / "authority"
        env.update(DISPLAY=display, XAUTHORITY=str(authority), XDG_SESSION_TYPE="x11")
        subprocess.run([names["xauth"], "-f", str(authority), "add", display, ".", secrets.token_hex(16)],
                       env=env, capture_output=True, check=True, timeout=5)
        with (root / "xvfb.log").open("wb") as log:
            server = subprocess.Popen([xvfb, display, "-screen", "0", "96x64x24", "-auth", str(authority),
                                       "-nolisten", "tcp", "-noreset"], env=env, stdout=log, stderr=subprocess.STDOUT)
        connection = gc = None
        try:
            deadline = time.monotonic() + 5
            while True:
                require(server.poll() is None, "Private Xvfb stopped")
                if subprocess.run([names["xdpyinfo"], "-display", display], env=env, capture_output=True, timeout=1).returncode == 0:
                    break
                require(time.monotonic() < deadline, "Private Xvfb startup deadline")
                time.sleep(.025)
            x = c.CDLL("libX11.so.6")
            for name, parameters, returns in [
                ("XOpenDisplay", [c.c_char_p], c.c_void_p), ("XCloseDisplay", [c.c_void_p], c.c_int),
                ("XDefaultScreen", [c.c_void_p], c.c_int), ("XRootWindow", [c.c_void_p, c.c_int], c.c_ulong),
                ("XCreateGC", [c.c_void_p, c.c_ulong, c.c_ulong, c.c_void_p], c.c_void_p),
                ("XFreeGC", [c.c_void_p, c.c_void_p], c.c_int), ("XSetForeground", [c.c_void_p, c.c_void_p, c.c_ulong], c.c_int),
                ("XDrawPoint", [c.c_void_p, c.c_ulong, c.c_void_p, c.c_int, c.c_int], c.c_int),
                ("XSync", [c.c_void_p, c.c_int], c.c_int), ("XInternAtom", [c.c_void_p, c.c_char_p, c.c_int], c.c_ulong),
                ("XChangeProperty", [c.c_void_p, c.c_ulong, c.c_ulong, c.c_ulong, c.c_int, c.c_int, c.c_void_p, c.c_int], c.c_int),
            ]:
                function = getattr(x, name)
                function.argtypes, function.restype = parameters, returns
            previous_auth = os.environ.get("XAUTHORITY")
            os.environ["XAUTHORITY"] = str(authority)
            try:
                connection = x.XOpenDisplay(display.encode())
            finally:
                if previous_auth is None:
                    os.environ.pop("XAUTHORITY", None)
                else:
                    os.environ["XAUTHORITY"] = previous_auth
            require(connection, "Cannot open own authenticated Xvfb")
            window = x.XRootWindow(connection, x.XDefaultScreen(connection))
            gc = x.XCreateGC(connection, window, 0, None)
            require(gc, "Cannot create own drawing context")
            for y in range(64):
                for px in range(96):
                    red, green, blue, _ = color(px, y)
                    x.XSetForeground(connection, gc, red << 16 | green << 8 | blue)
                    x.XDrawPoint(connection, window, gc, px, y)
            x.XSync(connection, 0)
            def command(values, success=True, overrides=None):
                nonlocal calls
                require(all(identity(path) == stamp for path, stamp in identities.items()), "Selected binary changed")
                result = subprocess.run([binary, *map(str, values)], env=dict(env, **(overrides or {})),
                                        stdin=subprocess.DEVNULL, capture_output=True, timeout=25)
                calls += 1
                if success:
                    require(result.returncode == 0, result.stderr.decode(errors="replace"))
                    return [json.loads(line) for line in result.stdout.splitlines()]
                require(result.returncode != 0 and not result.stdout, "Failed capture emitted a success result")
            def decode(path):
                return subprocess.run([names["ffmpeg"], "-v", "error", "-i", str(path), "-frames:v", "1",
                                       "-f", "rawvideo", "-pix_fmt", "rgba", "-"],
                                      env=env, capture_output=True, check=True, timeout=5).stdout
            expected = b"".join(color(px, y) for y in range(64) for px in range(96))
            stages = []
            for dpi in (48, 96, 192):
                contents = f"Xft.dpi:\t{dpi}\n".encode()
                x.XChangeProperty(connection, window, x.XInternAtom(connection, b"RESOURCE_MANAGER", 0),
                                  x.XInternAtom(connection, b"STRING", 0), 8, 0, c.create_string_buffer(contents), len(contents))
                x.XSync(connection, 0)
                metadata = command(["list", "displays"])[0]
                require(len(metadata) == 1 and (metadata[0]["width"], metadata[0]["height"]) == (96, 64), "Native inventory changed")
                target = root / f"desktop-{dpi}.png"
                started = time.monotonic()
                require(command(["capture", "desktop", target])[0] == {"width": 96, "height": 64}, "Wrong desktop extent")
                capture_ms = (time.monotonic() - started) * 1000
                require(decode(target) == expected, "Native root pixels disagree with independently drawn raster")
                stages.append({"xft_dpi": dpi, "native_sources": 1, "exact_rgba_bytes": len(expected), "cli_capture_encode_publish_ms": capture_ms})
            target = root / "task.png"
            (root / "recipe.json").write_text(json.dumps({"version": 1, "steps": [{"op": "redact", "x": 0, "y": 0, "width": 1, "height": 1}]}))
            profile = root / "task.json"
            profile.write_text(json.dumps({"version": 1, "source": {"type": "desktop"}, "recipe": "recipe.json",
                                           "actions": [{"type": "save", "path": target.name}]}))
            command(["task", "validate", profile])
            require(not target.exists(), "Task validation captured pixels")
            events = command(["task", "run", profile])
            require(events[-1]["type"] == "complete", "Desktop Task did not complete")
            require(decode(target) == bytes([0, 0, 0, 255]) + expected[4:], "Task did not save finalized recipe pixels")
            absent = root / "wayland.png"
            command(["capture", "desktop", absent], False, {"XDG_SESSION_TYPE": "wayland", "WAYLAND_DISPLAY": "owned-nonexistent-wayland"})
            require(not absent.exists(), "Wayland refusal published pixels")
            proof = subprocess.run([core, "desktop_composite::linux::tests::private_x11_restored_monitor_change_invalidates_desktop",
                                    "--ignored", "--exact", "--test-threads=1"],
                                   env=dict(env, SCREENSHOTS_PRIVATE_DESKTOP="owned-96x64"), stdin=subprocess.DEVNULL,
                                   capture_output=True, timeout=15)
            require(proof.returncode == 0 and b"1 passed" in proof.stdout, "Native restored-topology witness failed: " + (proof.stdout + proof.stderr).decode(errors="replace"))
            # A real adjacent root raster exists, but Xvfb exposes only one native output.
            for values in (("--setmonitor", "private-left", "48/12x64/16+0+0", "screen"),
                           ("--setmonitor", "private-right", "48/12x64/16+48+0", "none")):
                subprocess.run([names["xrandr"], *values], env=env, capture_output=True, check=True, timeout=5)
            split = root / "split.png"
            command(["capture", "desktop", split], False)
            require(not split.exists(), "Incomplete/outputless desktop inventory published a partial capture")
            require(not any(path.suffix in (".db", ".sqlite", ".sqlite3") for path in root.rglob("*")), "Capture created history")
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
        report = {"scope": "owned Xvfb/RandR single-output CLI/Task; no native multi-output or hardware acceptance",
                  "binary_sha256": digests[binary], "core_test_sha256": digests[core], "cli_invocations": calls,
                  "stages": stages, "restored_native_topology": "rejected", "outputless_split": "rejected",
                  "wayland_physical_coordinate_guess": "refused", "native_multi_output": "not_run"}
    rendered = json.dumps(report, indent=2) + "\n"
    if args.report:
        args.report.write_text(rendered)
    print(rendered, end="")


if __name__ == "__main__":
    main()
