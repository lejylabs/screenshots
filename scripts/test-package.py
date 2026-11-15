#!/usr/bin/env python3
"""Portable packaging regression tests; never installs into the user's system."""

import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys
import tarfile
import tempfile
import unittest
from unittest import mock
import zipfile

spec = importlib.util.spec_from_file_location("package", Path(__file__).with_name("package.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
smoke_spec = importlib.util.spec_from_file_location("package_smoke", Path(__file__).with_name("test-package-smoke.py"))
smoke_module = importlib.util.module_from_spec(smoke_spec)
smoke_spec.loader.exec_module(smoke_module)


class PackagingTests(unittest.TestCase):
    def test_bundled_webp_notices_are_required_and_packaged(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "Cargo.toml").write_text("", encoding="utf-8")
            vendor = root / "vendor"
            vendor.mkdir()
            item = {"name": "libwebp-sys", "manifest_path": str(root / "Cargo.toml")}
            with self.assertRaises(ValueError):
                module.license_files(item)
            for name in ["COPYING", "PATENTS"]:
                (vendor / name).write_text(name, encoding="utf-8")
            paths = module.license_files(item)
            self.assertEqual(set(map(lambda path: Path(path).name, paths)), {"COPYING", "PATENTS"})
            binary = root / "binary"
            binary.write_bytes(b"fixture")
            archive = module.package(binary, root / "packages", "test", "linux", 0, {}, [{"name":"libwebp-sys", "version":"0.14.4", "license_files":paths}])
            with tarfile.open(archive) as stream:
                for name in ["COPYING", "PATENTS"]:
                    self.assertEqual(stream.extractfile(f"test/licenses/libwebp-sys-0.14.4/{name}").read(), name.encode())

    def test_commands_decode_utf8_independently_of_the_platform_locale(self):
        expected = '{"description":"Zażółć 中文 😀"}'
        self.assertEqual(module.command(
            sys.executable, "-c", "import sys; sys.stdout.buffer.write(bytes.fromhex('" + expected.encode("utf-8").hex() + "'))"
        ), expected)
        with mock.patch.object(module.subprocess, "check_output", return_value='{"description":"Zażółć 中文 😀"}\n') as run:
            self.assertEqual(module.command("cargo", "metadata"), '{"description":"Zażółć 中文 😀"}')
        run.assert_called_once_with(
            ("cargo", "metadata"), cwd=module.ROOT, text=True, encoding="utf-8"
        )

    def test_generated_text_is_utf8_with_lf_newlines(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "text.json"
            module.write_text(path, '{"name":"Zażółć 中文 😀"}\n')
            self.assertEqual(path.read_bytes(), '{"name":"Zażółć 中文 😀"}\n'.encode("utf-8"))

    def test_archives_are_repeatable_and_never_replace_outputs(self):
        for target in ["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc", "aarch64-apple-darwin"]:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                binary = root / "binary"
                binary.write_bytes(b"synthetic executable fixture\x00")
                license_file = root / "LICENSE-test"
                license_file.write_text("Synthetic license fixture\n", encoding="utf-8", newline="\n")
                license_info = [{"name": "fixture", "version": "1", "license": "MIT", "license_files": [str(license_file)]}]
                first = module.package(binary, root / "one", "screenshots-test", target, 1700000000, {"fixture": True}, license_info)
                second = module.package(binary, root / "two", "screenshots-test", target, 1700000000, {"fixture": True}, license_info)
                self.assertEqual(first.read_bytes(), second.read_bytes())
                with self.assertRaises(FileExistsError):
                    module.package(binary, root / "one", "screenshots-test", target, 1700000000, {}, [])
                checksum = first.with_name(first.name + ".sha256").read_text(encoding="utf-8")
                self.assertEqual(checksum, hashlib.sha256(first.read_bytes()).hexdigest() + "  " + first.name + "\n")
                self.assertNotIn(b"\r", first.with_name(first.name + ".sha256").read_bytes())
                if first.suffix == ".zip":
                    with zipfile.ZipFile(first) as archive:
                        files = {name: archive.read(name) for name in archive.namelist()}
                else:
                    with tarfile.open(first) as archive:
                        files = {item.name: archive.extractfile(item).read() for item in archive.getmembers()}
                        self.assertTrue(all(item.uid == 0 and item.gid == 0 for item in archive.getmembers()))
                prefix = "screenshots-test/"
                executable = prefix + ("screenshots.exe" if "windows" in target else "screenshots")
                self.assertEqual(files[executable], binary.read_bytes())
                self.assertIn(prefix + "licenses/fixture-1/LICENSE-test", files)
                self.assertNotIn(str(root).encode(), files[prefix + "licenses/dependencies.json"])
                info = json.loads(files[prefix + "BUILD-INFO.json"])
                self.assertEqual(info["binary_sha256"], hashlib.sha256(binary.read_bytes()).hexdigest())
                for required in ["LICENSE", "THIRD_PARTY_NOTICES.md", "Cargo.lock", "PACKAGING.md"]:
                    self.assertIn(prefix + required, files)
                self.assertEqual(prefix + "screenshots.desktop" in files, "linux" in target)

    def test_missing_binary_creates_no_output(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with self.assertRaises(ValueError):
                module.package(root / "missing", root / "output", "test", "linux", 0, {}, [])
            self.assertFalse((root / "output").exists())

    @unittest.skipIf(os.name == "nt", "POSIX shell fixture")
    def test_extracted_artifact_smoke_and_bad_checksum(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "fixture"
            binary.write_text("#!/bin/sh\nprintf 'screenshots package fixture\\n'\n", encoding="utf-8", newline="\n")
            archive = module.package(binary, root / "packages", "screenshots-test", "x86_64-unknown-linux-gnu", 1700000000, {}, [])
            smoke_module.smoke(archive)
            archive.with_name(archive.name + ".sha256").write_text("invalid checksum\n", encoding="utf-8", newline="\n")
            with self.assertRaises(ValueError):
                smoke_module.smoke(archive)


if __name__ == "__main__":
    unittest.main()
