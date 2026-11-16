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
    def test_vendored_accesskit_original_licenses_are_required_and_packaged(self):
        hashes = {
            "LICENSE-MIT": "23f18e03dc49df91622fe2a76176497404e46ced8a715d9d2b67a7446571cca3",
            "LICENSE-APACHE": "62c7a1e35f56406896d7aa7ca52d0cc0d272ac022b5d2796e7d6905db8a3636a",
        }
        licenses = []
        for name, version in [("accesskit_unix", "0.22.1"), ("accesskit_atspi_common", "0.19.1")]:
            item = {"name": name, "version": version, "license": "MIT OR Apache-2.0",
                    "manifest_path": str(module.ROOT / "vendor" / name / "Cargo.toml")}
            paths = module.license_files(item)
            self.assertEqual({Path(path).name for path in paths}, set(hashes))
            for path in paths:
                self.assertEqual(hashlib.sha256(Path(path).read_bytes()).hexdigest(), hashes[Path(path).name])
            licenses.append({key: item[key] for key in ("name", "version", "license")} | {"license_files": paths})
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            binary = root / "binary"
            binary.write_bytes(b"synthetic executable")
            for target in ["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"]:
                with self.subTest(target=target):
                    archive = module.package(binary, root / target, "accesskit-test", target, 0, {}, licenses)
                    opener = zipfile.ZipFile if "windows" in target else tarfile.open
                    with opener(archive) as stream:
                        for item in licenses:
                            for name, digest in hashes.items():
                                path = f"accesskit-test/licenses/{item['name']}-{item['version']}/{name}"
                                data = stream.read(path) if "windows" in target else stream.extractfile(path).read()
                                self.assertEqual(hashlib.sha256(data).hexdigest(), digest)
            for item in licenses:
                # A metadata entry alone must not silently ship without its notices.
                with self.subTest(missing=item["name"]):
                    absent = {"name": item["name"], "manifest_path": str(root / "Cargo.toml")}
                    with self.assertRaises(ValueError):
                        module.license_files(absent)

    def test_checksum_failure_never_deletes_published_or_replaced_archive(self):
        for replace in [False, True]:
            with self.subTest(replace=replace), tempfile.TemporaryDirectory() as temporary:
                root=Path(temporary);binary=root/'binary';binary.write_bytes(b'fixture');output=root/'out';link=os.link;created=[]
                def fail_checksum(source,destination):
                    if destination.name.endswith('.sha256'):
                        if replace:
                            created[0].unlink();created[0].write_bytes(b'foreign replacement')
                        raise OSError('injected checksum publish failure')
                    link(source,destination);created.append(destination)
                with mock.patch.object(module.os,'link',side_effect=fail_checksum):
                    with self.assertRaises(OSError):module.package(binary,output,'screenshots-test','x86_64-unknown-linux-gnu',1700000000,{},[])
                self.assertEqual(len(created),1);self.assertTrue(created[0].is_file())
                if replace:self.assertEqual(created[0].read_bytes(),b'foreign replacement')
    def test_linux_abi_uses_numeric_symbol_versions_and_requires_metadata(self):
        def readelf(arguments, **kwargs):
            kwargs["stdout"].write(b"Name: GLIBC_2.9 Name: GLIBC_2.39 Name: GLIBC_2.3.4")
        with mock.patch.object(module.subprocess,"run",side_effect=readelf):
            self.assertEqual(module.linux_abi(Path("owned-binary")),{"glibc":"2.39"})
        with mock.patch.object(module.subprocess,"run"):
            with self.assertRaises(ValueError): module.linux_abi(Path("missing-metadata"))

    def test_native_abi_is_measured_from_the_frozen_staged_binary(self):
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary); binary=root/"input"; binary.write_bytes(b"synthetic frozen executable")
            def probe(path):
                self.assertNotEqual(path,binary)
                self.assertEqual(path.read_bytes(),b"synthetic frozen executable")
                binary.write_bytes(b"later input replacement")
                return {"glibc":"2.39"}
            with mock.patch.object(module,"linux_abi",side_effect=probe):
                archive=module.package(binary,root/"output","abi-test","x86_64-unknown-linux-gnu",0,{},[],verify_native_abi=True)
            with tarfile.open(archive) as stream:
                info=json.loads(stream.extractfile("abi-test/BUILD-INFO.json").read())
                self.assertEqual(info["linux_abi"],{"glibc":"2.39"})
                self.assertEqual(stream.extractfile("abi-test/screenshots").read(),b"synthetic frozen executable")

    def test_runtime_members_are_verified_and_executable_modes_survive_the_archive(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            runtime = root / "runtime"
            runtime.mkdir()
            payloads = {"screenshots-sxcu": b"synthetic helper", "core.dll": b"synthetic library"}
            manifest = {"version":1,"protocol":1,"rid":"linux-x64","sdk":"10.0.404","runtime":"10.0.15","entrypoint":"screenshots-sxcu","files":[
                {"path":name,"bytes":len(data),"sha256":hashlib.sha256(data).hexdigest(),"executable":name=="screenshots-sxcu"} for name,data in payloads.items()]}
            for name,data in payloads.items(): (runtime / name).write_bytes(data)
            (runtime / "RUNTIME-MANIFEST.json").write_text(json.dumps(manifest))
            binary = root / "binary"
            binary.write_bytes(b"synthetic executable")
            archive = module.package(binary,root / "output","runtime-test","x86_64-unknown-linux-gnu",0,{},[],runtime)
            with tarfile.open(archive) as stream:
                prefix = "runtime-test/lib/screenshots-sxcu/"
                self.assertEqual(stream.getmember(prefix+"screenshots-sxcu").mode,0o755)
                self.assertEqual(stream.getmember(prefix+"core.dll").mode,0o644)
                self.assertEqual(stream.extractfile(prefix+"core.dll").read(),payloads["core.dll"])
                info = json.loads(stream.extractfile("runtime-test/BUILD-INFO.json").read())
                self.assertEqual(info["sharex_runtime"]["rid"],"linux-x64")
                self.assertEqual(info["sharex_runtime"]["manifest_sha256"],hashlib.sha256((runtime / "RUNTIME-MANIFEST.json").read_bytes()).hexdigest())
            manifest_path = runtime / "RUNTIME-MANIFEST.json"
            original_manifest = manifest_path.read_bytes()
            for field,value in [("version",True),("protocol",True),("sdk","unverified"),("runtime",False),("entrypoint","core.dll")]:
                changed=dict(manifest); changed[field]=value
                manifest_path.write_text(json.dumps(changed))
                with self.assertRaises(ValueError): module.runtime_bundle(runtime,root/"invalid-header", "x86_64-unknown-linux-gnu")
            manifest_path.write_bytes(original_manifest)
            (runtime / "undeclared-empty").mkdir()
            with self.assertRaises(ValueError): module.runtime_bundle(runtime,root/"unexpected-directory", "x86_64-unknown-linux-gnu")
            (runtime / "undeclared-empty").rmdir()
            (runtime / "undeclared-wide").write_bytes(b"unexpected")
            with os.scandir(runtime) as entries:
                unexpected=next(entry for entry in entries if entry.name=="undeclared-wide")
            class RejectBeforeSecondEntry:
                def __enter__(self): return self
                def __exit__(self,*args): pass
                def __iter__(self):
                    yield unexpected
                    raise AssertionError("undeclared directory contents were eagerly materialized")
            with mock.patch.object(module.os,"scandir",return_value=RejectBeforeSecondEntry()):
                with self.assertRaises(ValueError): module.runtime_bundle(runtime,root/"wide-rejected","x86_64-unknown-linux-gnu")
            (runtime / "undeclared-wide").unlink()
            (runtime / "core.dll").write_bytes(b"altered")
            with self.assertRaises(ValueError): module.runtime_bundle(runtime,root/"rejected","x86_64-unknown-linux-gnu")
            (runtime / "core.dll").write_bytes(payloads["core.dll"])
            (runtime / "unexpected").write_bytes(b"unowned")
            with self.assertRaises(ValueError): module.runtime_bundle(runtime,root/"unexpected-copy","x86_64-unknown-linux-gnu")

    def test_static_mac_native_notices_use_the_matching_target_triplet(self):
        for target, triplet in [("aarch64-apple-darwin", "arm64-osx"), ("x86_64-apple-darwin", "x64-osx")]:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                for name in ["curl", "libssh2", "openssl", "zlib"]:
                    folder = root / triplet / "share" / name
                    folder.mkdir(parents=True)
                    (folder / "copyright").write_bytes((name + " notice").encode())
                with mock.patch.dict(os.environ, {"VCPKG_INSTALLED_ROOT": str(root), "VCPKGRS_TRIPLET": triplet}):
                    self.assertEqual(len(module.native_licenses(target)), 4)
                    other = "x64-osx" if triplet == "arm64-osx" else "arm64-osx"
                    with mock.patch.dict(os.environ, {"VCPKGRS_TRIPLET": other}):
                        with self.assertRaises(ValueError): module.native_licenses(target)

    def test_static_windows_native_notices_are_required_bounded_and_packaged(self):
        target = "x86_64-pc-windows-msvc"
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.assertEqual(module.native_licenses("x86_64-unknown-linux-gnu"), [])
            with mock.patch.dict(os.environ, {"VCPKG_INSTALLED_ROOT": str(root), "VCPKGRS_TRIPLET": "x64-windows-static-md"}):
                with self.assertRaises(FileNotFoundError):
                    module.native_licenses(target)
                share = root / "x64-windows-static-md/share"
                for name in ["curl", "libssh2", "openssl", "zlib"]:
                    (share / name).mkdir(parents=True)
                    (share / name / "copyright").write_bytes((name + " native notice").encode())
                notices = module.native_licenses(target)
                self.assertEqual(len(notices), 4)
                copyright = share / "curl/copyright"
                copyright.write_bytes(b"x" * (1024 * 1024 + 1))
                with self.assertRaises(ValueError):
                    module.native_licenses(target)
                copyright.write_bytes(b"curl native notice")
                binary = root / "binary"
                binary.write_bytes(b"synthetic executable")
                rust_license = root / "LICENSE-fixture"
                rust_license.write_bytes(b"existing Rust notice")
                rust = {"name": "rust-fixture", "version": "1", "license_files": [str(rust_license)]}
                archive = module.package(binary, root / "packages", "native-test", target, 0, {}, [rust, *notices])
                with zipfile.ZipFile(archive) as stream:
                    dependencies = json.loads(stream.read("native-test/licenses/dependencies.json"))
                    self.assertEqual({item["name"] for item in dependencies},
                                     {"rust-fixture", "native-curl", "native-libssh2", "native-openssl", "native-zlib"})
                    for item in notices:
                        path = f"native-test/licenses/{item['name']}-{item['version']}/copyright"
                        self.assertEqual(stream.read(path), (item["name"][7:] + " native notice").encode())
                    self.assertEqual(stream.read("native-test/licenses/rust-fixture-1/LICENSE-fixture"), b"existing Rust notice")
                    self.assertNotIn(str(root).encode(), stream.read("native-test/licenses/dependencies.json"))
                copyright.unlink()
                if os.name != "nt":
                    copyright.symlink_to(rust_license)
                    with self.assertRaises((OSError, ValueError)):
                        module.native_licenses(target)
                    copyright.unlink()
                    os.mkfifo(copyright)
                    with self.assertRaises(ValueError):
                        module.native_licenses(target)

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
                self.assertIn(b"](DESKTOP-INTEGRATION.md)",files[prefix+"PACKAGING.md"])
                self.assertIn(prefix+"DESKTOP-INTEGRATION.md",files)
                self.assertIn(b"](NATIVE-SIGNING.md)",files[prefix+"PACKAGING.md"])
                self.assertIn(prefix+"NATIVE-SIGNING.md",files)
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
            archive = module.package(binary, root / "packages", "screenshots-test", smoke_module.INSTALL.local_target(), 1700000000, {"version":"0.1.0","target":smoke_module.INSTALL.local_target()}, [])
            smoke_module.smoke(archive)
            archive.with_name(archive.name + ".sha256").write_text("invalid checksum\n", encoding="utf-8", newline="\n")
            with self.assertRaises(ValueError):
                smoke_module.smoke(archive)


if __name__ == "__main__":
    unittest.main()
