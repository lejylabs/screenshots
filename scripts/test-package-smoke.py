#!/usr/bin/env python3
"""Verify and run a packaged binary without opening a window or user data."""

import hashlib
import json
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("package_smoke_install", ROOT / "scripts/install-user.py")
INSTALL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(INSTALL)


def smoke(archive):
    checksum = INSTALL.regular_bytes(archive.with_name(archive.name + ".sha256"), 1024).decode("utf-8")
    digest = INSTALL.sha(archive)
    if checksum != digest + "  " + archive.name + "\n":
        raise ValueError("Archive checksum does not match")
    with tempfile.TemporaryDirectory(prefix="screenshots-package-smoke-") as temporary:
        root = Path(temporary)
        folder = root / "extract"
        folder.mkdir()
        info, _, executable = INSTALL.extract(archive, folder)
        binary = folder / executable
        environment = dict(os.environ)
        for variable in ["DISPLAY", "WAYLAND_DISPLAY", "RUST_LOG"]:
            environment.pop(variable, None)
        for variable in ["HOME", "USERPROFILE", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "APPDATA", "LOCALAPPDATA"]:
            isolated = root / variable.lower()
            isolated.mkdir()
            environment[variable] = str(isolated)
        for argument in ["--version", "--help"]:
            result = subprocess.run([str(binary), argument], env=environment, cwd=root, capture_output=True, text=True, encoding="utf-8", timeout=15, check=True)
            if "screenshots" not in result.stdout.lower():
                raise ValueError("Headless command did not identify screenshots")
        runtime_checked = False
        if "sharex_runtime" in info:
            runtime = folder / "lib/screenshots-sxcu"
            manifest_bytes = INSTALL.regular_bytes(runtime / "RUNTIME-MANIFEST.json", 128 * 1024)
            if hashlib.sha256(manifest_bytes).hexdigest() != info["sharex_runtime"]["manifest_sha256"]:
                raise ValueError("Packaged runtime manifest does not match build info")
            manifest = json.loads(manifest_bytes)
            entrypoint = manifest["entrypoint"]
            if entrypoint not in ("screenshots-sxcu", "screenshots-sxcu.exe"):
                raise ValueError("Unexpected packaged helper entrypoint")
            result = subprocess.run([str(runtime / entrypoint), "--self-test"], env=environment, cwd=root,
                                    capture_output=True, text=True, encoding="utf-8", timeout=15, check=True)
            summary = json.loads(result.stdout)
            if not summary.get("passed"):
                raise ValueError("Packaged private runtime self-test failed")
            profile = root / "smoke.sxcu"
            profile.write_text(json.dumps({"Version":"18.0.0","Name":"Package smoke","DestinationType":"ImageUploader","RequestMethod":"POST","RequestURL":"https://example.invalid/upload","Body":"MultipartFormData","FileFormName":"file","URL":"{json:url}"}), encoding="utf-8")
            subprocess.run([str(binary), "sharex", "inspect", str(profile)], env=environment, cwd=root,
                           capture_output=True, text=True, encoding="utf-8", timeout=15, check=True)
            runtime_checked = True
        for variable in ["HOME", "USERPROFILE", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "APPDATA", "LOCALAPPDATA"]:
            if any(Path(environment[variable]).iterdir()):
                raise ValueError("Headless help/version unexpectedly wrote user state")
        print(json.dumps({"archive": archive.name, "binary_sha256": info["binary_sha256"], "headless_help_and_version": "passed", "private_runtime": "passed" if runtime_checked else "not present in synthetic fixture", "user_state": "unchanged"}))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("Usage: test-package-smoke.py ARCHIVE")
    smoke(Path(sys.argv[1]).resolve())
