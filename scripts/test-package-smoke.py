#!/usr/bin/env python3
"""Verify and run a packaged binary without opening a window or user data."""

import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import sys
import tarfile
import tempfile
import zipfile


def smoke(archive):
    checksum = archive.with_name(archive.name + ".sha256").read_text()
    with archive.open("rb") as content:
        digest = hashlib.file_digest(content, "sha256").hexdigest()
    if checksum != digest + "  " + archive.name + "\n":
        raise ValueError("Archive checksum does not match")
    with tempfile.TemporaryDirectory(prefix="screenshots-package-smoke-") as temporary:
        root = Path(temporary)
        extracted = root / "extract"
        extracted.mkdir()
        if archive.suffix == ".zip":
            with zipfile.ZipFile(archive) as stream:
                members = [(item.filename, stream.read(item), item.external_attr >> 16) for item in stream.infolist()]
        else:
            with tarfile.open(archive) as stream:
                if any(not item.isfile() for item in stream.getmembers()):
                    raise ValueError("Package contains a non-regular file")
                members = [(item.name, stream.extractfile(item).read(), item.mode) for item in stream.getmembers()]
        for name, data, mode in members:
            path = PurePosixPath(name)
            if path.is_absolute() or ".." in path.parts or len(path.parts) < 2 or "\\" in name or ":" in name:
                raise ValueError("Invalid archive member")
            destination = extracted.joinpath(*path.parts)
            destination.parent.mkdir(parents=True, exist_ok=True)
            with destination.open("xb") as content:
                content.write(data)
            destination.chmod(mode & 0o777)
        folders = list(extracted.iterdir())
        if len(folders) != 1 or not folders[0].is_dir():
            raise ValueError("Package must contain one application folder")
        folder = folders[0]
        binary = folder / ("screenshots.exe" if os.name == "nt" else "screenshots")
        info = json.loads((folder / "BUILD-INFO.json").read_text())
        with binary.open("rb") as content:
            if hashlib.file_digest(content, "sha256").hexdigest() != info["binary_sha256"]:
                raise ValueError("Packaged binary hash does not match build info")
        environment = dict(os.environ)
        for variable in ["DISPLAY", "WAYLAND_DISPLAY", "RUST_LOG"]:
            environment.pop(variable, None)
        for variable in ["HOME", "USERPROFILE", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "APPDATA", "LOCALAPPDATA"]:
            isolated = root / variable.lower()
            isolated.mkdir()
            environment[variable] = str(isolated)
        for argument in ["--version", "--help"]:
            result = subprocess.run([str(binary), argument], env=environment, cwd=root, capture_output=True, text=True, timeout=15, check=True)
            if "screenshots" not in result.stdout.lower():
                raise ValueError("Headless command did not identify screenshots")
        for variable in ["HOME", "USERPROFILE", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "APPDATA", "LOCALAPPDATA"]:
            if any(Path(environment[variable]).iterdir()):
                raise ValueError("Headless help/version unexpectedly wrote user state")
        print(json.dumps({"archive": archive.name, "binary_sha256": info["binary_sha256"], "headless_help_and_version": "passed", "user_state": "unchanged"}))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("Usage: test-package-smoke.py ARCHIVE")
    smoke(Path(sys.argv[1]).resolve())
