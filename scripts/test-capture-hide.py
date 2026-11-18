#!/usr/bin/env python3
"""Run the owned-window acknowledgment fixture on an authenticated private Xvfb.

Requires the already-built app-core test binary. Never captures desktop pixels,
reads clipboard data, contacts a user display, or changes power/session state.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import tempfile
import time


def require(value, message):
    if not value:
        raise RuntimeError(message)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--core-test-binary", type=Path, required=True)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    require(os.name == "posix" and os.uname().sysname == "Linux", "Private fixture requires Linux")
    require("WAYLAND_SOCKET" not in os.environ, "Refuses an inherited Wayland descriptor")
    tools = {name: shutil.which(name) for name in ("Xvfb", "xauth", "xdpyinfo")}
    require(all(tools.values()), "Xvfb, xauth and xdpyinfo must be installed")
    binary = args.core_test_binary.resolve(strict=True)
    require(binary.is_file() and os.access(binary, os.X_OK), "Expected an existing app-core test binary")
    with binary.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    identity = binary.stat()
    with tempfile.TemporaryDirectory(prefix="screenshots-capture-hide-") as temporary:
        root = Path(temporary)
        require(root.stat().st_mode & 0o077 == 0, "Fixture directory is not private")
        display = next((f":{i}" for i in range(320, 380)
                        if not Path(f"/tmp/.X{i}-lock").exists()
                        and not Path(f"/tmp/.X11-unix/X{i}").exists()), None)
        require(display, "No private display slot is free")
        env = dict(os.environ)
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DBUS_SESSION_BUS_ADDRESS", "AT_SPI_BUS_ADDRESS"):
            env.pop(key, None)
        env.update({key: str(root / key) for key in ("HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME")})
        auth = root / "authority"
        env.update(DISPLAY=display, XAUTHORITY=str(auth), XDG_SESSION_TYPE="x11",
                   SCREENSHOTS_PRIVATE_HIDE_FIXTURE="owned-native-window")
        subprocess.run([tools["xauth"], "-f", str(auth), "add", display, ".", secrets.token_hex(16)],
                       env=env, stdin=subprocess.DEVNULL, capture_output=True, check=True, timeout=5)
        with (root / "server.log").open("wb") as log:
            server = subprocess.Popen([tools["Xvfb"], display, "-screen", "0", "64x64x24", "-auth", str(auth),
                                       "-nolisten", "tcp", "-noreset"], env=env, stdin=subprocess.DEVNULL,
                                      stdout=log, stderr=subprocess.STDOUT)
        try:
            until = time.monotonic() + 5
            while True:
                require(server.poll() is None, "Owned Xvfb stopped")
                result = subprocess.run([tools["xdpyinfo"], "-display", display], env=env,
                                        stdin=subprocess.DEVNULL, capture_output=True, timeout=1)
                if result.returncode == 0:
                    break
                require(time.monotonic() < until, "Owned Xvfb startup deadline")
                time.sleep(0.025)
            require(binary.stat() == identity, "Selected test binary changed")
            result = subprocess.run([str(binary), "--exact",
                                     "capture_visibility::tests::private_owned_x11_hidden_fixture_rejects_foreign_and_destroyed_windows",
                                     "--ignored", "--nocapture"], env=env, stdin=subprocess.DEVNULL,
                                    capture_output=True, timeout=10)
            require(result.returncode == 0 and b"1 passed" in result.stdout,
                    "Owned native fixture failed: " + result.stderr.decode(errors="replace"))
        finally:
            server.terminate()
            try:
                server.wait(timeout=3)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=3)
    report = {"scope": "private X-server state, owned window only; no pixels, compositor or physical presentation claim",
              "binary_sha256": digest, "passed": 1}
    contents = json.dumps(report, indent=2) + "\n"
    if args.report:
        args.report.write_text(contents)
    print(contents, end="")


if __name__ == "__main__":
    main()
