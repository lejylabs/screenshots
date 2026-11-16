#!/usr/bin/env python3
"""Run the actual focused GPUI Paste fixture on a private headless Sway server."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import stat
import struct
import subprocess
import tempfile
import time
import zlib


def require(value, message):
    if not value:
        raise RuntimeError(message)


def png():
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xffffffff)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 2, 1, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(bytes([0, 11, 23, 37, 0, 43, 59, 71, 128]))) + chunk(b"IEND", b""))


def stop(child):
    if child and child.poll() is None:
        child.terminate()
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--test-binary", required=True)
    parser.add_argument("--compositor", required=True)
    parser.add_argument("--library-dir", required=True)
    parser.add_argument("--report", required=True)
    args = parser.parse_args()
    binary = str(Path(args.test_binary).resolve(strict=True))
    compositor = str(Path(args.compositor).resolve(strict=True))
    report = {"test_binary_sha256": hashlib.file_digest(open(binary, "rb"), "sha256").hexdigest(), "checks": []}
    with tempfile.TemporaryDirectory(prefix="screenshots-private-wayland-") as temporary:
        root = Path(temporary)
        runtime = root / "runtime"
        runtime.mkdir(mode=0o700)
        config = root / "sway.conf"
        config.write_text("xwayland disable\noutput * mode 1024x768\nfocus_follows_mouse no\nseat seat0 fallback true\ndefault_border none\n")
        env = dict(os.environ, XDG_RUNTIME_DIR=str(runtime), WLR_BACKENDS="headless", WLR_HEADLESS_OUTPUTS="1",
                   WLR_LIBINPUT_NO_DEVICES="1", WLR_RENDERER="pixman", LD_LIBRARY_PATH=str(Path(args.library_dir).resolve()),
                   SCREENSHOTS_PRIVATE_WAYLAND_CLIPBOARD=str(root))
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "SWAYSOCK", "DBUS_SESSION_BUS_ADDRESS"):
            env.pop(name, None)
        for name in ("XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"):
            env[name] = str(root / name)
        server = owner = client = keyboard = None
        with (root / "compositor.log").open("wb") as compositor_log:
            try:
                server = subprocess.Popen([compositor, "-c", str(config), "--unsupported-gpu"], env=env,
                                          stdin=subprocess.DEVNULL, stdout=compositor_log, stderr=subprocess.STDOUT)
                until = time.monotonic() + 8
                while time.monotonic() < until:
                    require(server.poll() is None, "Private compositor exited")
                    sockets = [p for p in runtime.glob("wayland-*") if p.is_socket()]
                    if sockets:
                        env["WAYLAND_DISPLAY"] = sockets[0].name
                        break
                    time.sleep(.025)
                else:
                    raise RuntimeError("Private Wayland socket readiness timed out")
                # An input-free headless seat needs a private virtual keyboard for standard
                # keyboard-focus/selection events. It never connects to the user's socket.
                keyboard = subprocess.Popen([str(Path(compositor).with_name("wtype")), "-k", "Shift_L", "-s", "20000"],
                                            env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
                time.sleep(.1)
                require(keyboard.poll() is None, "Private virtual keyboard did not stay available")
                owner = subprocess.Popen(["wl-copy", "--foreground", "--type", "image/png"], env=env,
                                         stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
                owner.stdin.write(png())
                owner.stdin.close()
                time.sleep(.15)
                require(owner.poll() is None, "Private synthetic clipboard producer exited")
                client = subprocess.Popen([binary, "--exact", "private_wayland_clipboard_fixture::focused_wayland_paste_uses_native_offer_and_existing_editor",
                                          "--ignored", "--nocapture", "--test-threads=1"], env=env,
                                         stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                # Only this private compositor's explicit IPC path can receive focus commands.
                swaymsg = str(Path(compositor).with_name("swaymsg"))
                until = time.monotonic() + 12
                observed = []
                while time.monotonic() < until and client.poll() is None:
                    ipc = next(runtime.glob("sway-ipc.*.sock"), None)
                    if ipc:
                        tree = subprocess.run([swaymsg, "-s", str(ipc), "-t", "get_tree", "-r"], env=env,
                                              capture_output=True, timeout=2)
                        if tree.returncode == 0:
                            def walk(node):
                                yield node
                                for child in node.get("nodes", []) + node.get("floating_nodes", []):
                                    yield from walk(child)
                            views = [n for n in walk(json.loads(tree.stdout)) if n.get("app_id") or n.get("window")]
                            observed = [{"name": n.get("name"), "app_id": n.get("app_id"), "focused": n.get("focused")} for n in views]
                            for view in views:
                                if view.get("name") == "screenshots":
                                    subprocess.run([swaymsg, "-s", str(ipc), f"[con_id={view['id']}] focus"], env=env,
                                                   capture_output=True, timeout=2)
                    time.sleep(.05)
                stdout, stderr = client.communicate(timeout=30)
                require(client.returncode == 0, "Private native Paste fixture failed: " + stderr.decode(errors="replace")[-5000:]
                        + stdout.decode(errors="replace")[-3000:] + " private views=" + json.dumps(observed))
                report["checks"].append("actual focused GPUI Wayland offer read, existing Paste/editor, exact RGBA including hidden RGB/alpha, no image/history/clipboard output")
            finally:
                stop(client)
                stop(owner)
                stop(keyboard)
                stop(server)
    Path(args.report).write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": len(report["checks"]), "report": args.report}))


if __name__ == "__main__":
    main()
