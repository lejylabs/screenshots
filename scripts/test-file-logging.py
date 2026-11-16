#!/usr/bin/env python3
"""Private headless/log-worker checks; never starts a GUI or reads user logs."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile


def identity(path):
    value = path.stat()
    return value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--test-binary", type=Path, required=True)
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    tests = args.test_binary.resolve(strict=True)
    admitted = {path: identity(path) for path in (binary, tests)}
    hashes = {path: digest(path) for path in (binary, tests)}
    with tempfile.TemporaryDirectory(prefix="screenshots-private-file-logging-") as temporary:
        root = Path(temporary)
        env = dict(os.environ)
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DBUS_SESSION_BUS_ADDRESS", "AT_SPI_BUS_ADDRESS"):
            env.pop(name, None)
        for name in ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "TMPDIR", "TEMP", "TMP"):
            folder = root / name
            folder.mkdir()
            env[name] = str(folder)
        env["RUST_LOG"] = "trace"
        original_tree = {path.relative_to(root) for path in root.rglob("*")}
        def run(path, values, success=True):
            assert identity(path) == admitted[path], "Executable changed during verification"
            result = subprocess.run([str(path), *values], env=env, stdin=subprocess.DEVNULL,
                                    capture_output=True, timeout=20)
            assert (result.returncode == 0) == success, result.stderr.decode(errors="replace")
            assert identity(path) == admitted[path], "Executable changed during verification"
            return result

        # Windows KnownFolder ignores these environment paths. Do not risk the
        # real profile to test a regression in the CLI's early-return ordering.
        headless_requests = 0
        if os.name != "nt":
            for arguments in (["--help"], ["--version"], ["ruler", "0", "0", "3", "4"]):
                run(binary, arguments)
            run(binary, ["unrecognized-private-logging-fixture-command"], False)
            headless_requests = 4
        assert {path.relative_to(root) for path in root.rglob("*")} == original_tree, "Headless CLI changed the private user-state directory tree"
        result = run(tests, ["file_logging::tests::", "--test-threads=1", "--nocapture"])
        output = result.stdout.decode(errors="replace")
        expected = 15 if os.name == "nt" else 16
        assert f"{expected} passed; 0 failed" in output, "Expected all actual logging fixtures"
        assert not list(root.rglob("events-*.log")), "Fixture leaked private log files"
    assert all(identity(path) == admitted[path] and digest(path) == hashes[path] for path in admitted)
    report = {
        "passed": True,
        "scope": "private headless CLI and real log-worker/file fixtures; no GUI or real profile access",
        "headless_requests": headless_requests,
        "headless_windows_skip": "KnownFolder needs an actually private profile" if os.name == "nt" else None,
        "logging_fixtures": expected,
        "binary_sha256": hashes[binary],
        "test_binary_sha256": hashes[tests],
        "real_user_data_access": False,
        "gui_or_hardware_acceptance": False,
        "reference_idle_acceptance": False,
    }
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8", newline="\n")
    print(json.dumps(report))


if __name__ == "__main__":
    main()
