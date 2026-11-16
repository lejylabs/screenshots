#!/usr/bin/env python3
"""Validate native manifest/bundle/Debian packaging without installing anything."""

import importlib.util
import io
import os
from pathlib import Path
import plistlib
import tarfile
import tempfile
import unittest
from unittest import mock
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("package_app", ROOT / "scripts/package-app.py")
PACKAGE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PACKAGE)


class NativePackageTests(unittest.TestCase):
    def test_msix_manifest_file_is_utf8_lf_with_a_non_utf8_host_default(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            publisher = 'CN=Müller & 東京 "Publisher"'
            original = Path.write_text
            def ansi_write(path, data, encoding=None, errors=None, newline=None):
                return original(path, data, encoding=encoding or "cp1252", errors=errors, newline=newline)
            with mock.patch.object(Path, "write_text", ansi_write):
                PACKAGE.write_appx_manifest(root, "0.1.0", "x64", publisher)
            content = (root / "AppxManifest.xml").read_bytes()
            self.assertNotIn(b"\r", content)
            self.assertIn('Müller'.encode("utf-8"), content)
            self.assertEqual(list(ET.fromstring(content))[0].attrib["Publisher"], publisher)

    @unittest.skipIf(os.name == "nt", "POSIX bundle executable modes require a Unix host; MSIX byte/manifest fixtures run here")
    def test_mac_bundle_has_document_types_and_exact_binary(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            portable, app = root / "portable", root / "Screenshots.app"
            portable.mkdir()
            (portable / "screenshots").write_bytes(b"synthetic Mach-O")
            (portable / "LICENSE").write_bytes(b"license retained")
            runtime = portable / "lib/screenshots-sxcu"
            runtime.mkdir(parents=True)
            (runtime / "screenshots-sxcu").write_bytes(b"native helper")
            (runtime / "screenshots-sxcu").chmod(0o755)
            (portable / "BUILD-INFO.json").write_bytes(b"build info")
            PACKAGE.app_bundle(portable, app, {"version": "0.1.0"})
            self.assertEqual((app / "Contents/MacOS/screenshots").read_bytes(), b"synthetic Mach-O")
            self.assertEqual((app / "Contents/Resources/LICENSE").read_bytes(), b"license retained")
            self.assertEqual((app / "Contents/Resources/lib/screenshots-sxcu/screenshots-sxcu").read_bytes(),b"native helper")
            self.assertTrue((app / "Contents/Resources/lib/screenshots-sxcu/screenshots-sxcu").stat().st_mode & 0o111)
            info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
            self.assertEqual(info["CFBundleIdentifier"], "me.lejy.screenshots")
            self.assertEqual(info["CFBundleExecutable"], "screenshots")
            self.assertEqual(info["CFBundleDocumentTypes"][0]["CFBundleTypeRole"], "Viewer")
            self.assertIn("public.png", info["CFBundleDocumentTypes"][0]["LSItemContentTypes"])
            with self.assertRaises(ValueError): PACKAGE.app_bundle(portable, app, {"version": "0.1.0"})

    def test_msix_manifest_routes_file_activation_without_changing_default(self):
        xml = ET.fromstring(PACKAGE.appx_manifest("0.1.0", "x64"))
        ns = {"a": "http://schemas.microsoft.com/appx/manifest/foundation/windows10",
              "uap": "http://schemas.microsoft.com/appx/manifest/uap/windows10",
              "uap3": "http://schemas.microsoft.com/appx/manifest/uap/windows10/3"}
        application = xml.find("a:Applications/a:Application", ns)
        self.assertEqual(application.attrib["Executable"], "screenshots.exe")
        association = xml.find(".//uap3:FileTypeAssociation", ns)
        self.assertEqual(association.attrib["Parameters"], '--open "%1"')
        self.assertEqual(len(association.findall("uap:SupportedFileTypes/uap:FileType", ns)), 7)
        with self.assertRaises(ValueError): PACKAGE.appx_manifest("bad", "x64")
        with self.assertRaises(ValueError): PACKAGE.appx_manifest("0.1.0", "unsupported")

    @unittest.skipIf(os.name == "nt", "Debian executable-mode and symlink layout requires a Unix host")
    def test_debian_ar_and_tar_members_have_real_package_layout(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            portable = root / "portable"
            portable.mkdir()
            (portable / "screenshots").write_bytes(b"synthetic ELF")
            (portable / "LICENSE").write_bytes(b"license retained")
            runtime = portable / "lib/screenshots-sxcu"
            runtime.mkdir(parents=True)
            (runtime / "screenshots-sxcu").write_bytes(b"native helper")
            (runtime / "screenshots-sxcu").chmod(0o755)
            (portable / "BUILD-INFO.json").write_bytes(b"build info")
            package = root / "screenshots.deb"
            PACKAGE.deb_package(portable, package, {"version": "0.1.0", "target": "x86_64-unknown-linux-gnu", "source_date_epoch": 17, "linux_abi": {"glibc":"2.39"}})
            data = package.read_bytes()
            self.assertTrue(data.startswith(b"!<arch>\n"))
            entries, position = {}, 8
            while position < len(data):
                header = data[position:position + 60]
                self.assertEqual(header[58:], b"`\n")
                name, size = header[:16].decode().strip().rstrip("/"), int(header[48:58])
                entries[name] = data[position + 60:position + 60 + size]
                position += 60 + size + size % 2
            self.assertEqual(entries["debian-binary"], b"2.0\n")
            with tarfile.open(fileobj=io.BytesIO(entries["control.tar.gz"])) as archive:
                control=archive.extractfile("control").read().decode()
                self.assertIn("libc6 (>= 2.39)",control)
                self.assertIn("libicu72 | libicu74 | libicu76 | libicu78",control)
                self.assertIn("libssl3t64 | libssl3",control)

            with tarfile.open(fileobj=io.BytesIO(entries["control.tar.gz"]), mode="r:gz") as archive:
                self.assertIn(b"Architecture: amd64", archive.extractfile("control").read())
            with tarfile.open(fileobj=io.BytesIO(entries["data.tar.gz"]), mode="r:gz") as archive:
                self.assertEqual(archive.getmember("usr/bin/screenshots").linkname,"../lib/screenshots/screenshots")
                self.assertEqual(archive.extractfile("usr/lib/screenshots/screenshots").read(), b"synthetic ELF")
                self.assertEqual(archive.getmember("usr/lib/screenshots/lib/screenshots-sxcu/screenshots-sxcu").mode,0o755)
                self.assertEqual(archive.extractfile("usr/lib/screenshots/BUILD-INFO.json").read(),b"build info")
                self.assertEqual(archive.extractfile("usr/share/doc/screenshots/LICENSE").read(), b"license retained")
                self.assertIn(b"--open %f", archive.extractfile("usr/share/applications/me.lejy.screenshots.desktop").read())


if __name__ == "__main__":
    unittest.main()
