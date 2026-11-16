#!/usr/bin/env python3
"""Isolated user installation/update verification; no user data or desktop writes."""

import hashlib
import gzip
import http.server
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import ssl
import struct
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("install_user", ROOT / "scripts/install-user.py")
INSTALL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(INSTALL)


def package(directory, version="0.1.0", special=None):
    name = "screenshots-" + version
    destination = directory / (name + ".tar.gz")
    binary = b"synthetic executable " + version.encode()
    files = {INSTALL.executable_name(INSTALL.local_target()): binary,
             "BUILD-INFO.json": json.dumps({"version": version, "target": INSTALL.local_target(),
                                             "binary_sha256": hashlib.sha256(binary).hexdigest()}).encode()}
    for item in ["LICENSE", "README.md", "Cargo.lock", "THIRD_PARTY_NOTICES.md"]:
        files[item] = (item + " fixture").encode()
    with tarfile.open(destination, "w:gz") as archive:
        for path, data in files.items():
            info = tarfile.TarInfo(name + "/" + path)
            info.size = len(data)
            info.mode = 0o755 if path == INSTALL.executable_name(INSTALL.local_target()) else 0o644
            archive.addfile(info, io.BytesIO(data))
        if special:
            archive.addfile(special)
    digest = hashlib.sha256(destination.read_bytes()).hexdigest()
    destination.with_name(destination.name + ".sha256").write_text(digest + "  " + destination.name + "\n")
    return destination


def partial_write_crash(predicate, code):
    return (
        "original_open=Path.open\n"
        "class InterruptedWrite:\n"
        " def __init__(self,stream): self.stream=stream\n"
        " def __enter__(self): self.stream.__enter__();return self\n"
        " def __exit__(self,*args): return self.stream.__exit__(*args)\n"
        " def __getattr__(self,name): return getattr(self.stream,name)\n"
        " def write(self,payload):\n"
        f"  if {predicate}:\n"
        "   self.stream.write(payload[:len(payload)//2]);self.stream.flush();os.fsync(self.stream.fileno())\n"
        f"   os._exit({code})\n"
        "  return self.stream.write(payload)\n"
        "def opening(path,*args,**kwargs): return InterruptedWrite(original_open(path,*args,**kwargs))\n"
        "Path.open=opening\n")


class InstallTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="screenshots-install-test-")
        self.directory = Path(self.temporary.name)
        self.root = self.directory / "owned install"

    def tearDown(self):
        self.temporary.cleanup()

    def test_install_status_idempotence_upgrade_rollback_and_uninstall(self):
        first = package(self.directory)
        second = package(self.directory, "0.2.0")
        self.assertEqual(INSTALL.install(first, self.root)["status"], "installed")
        self.assertEqual(INSTALL.status(self.root)["version"], "0.1.0")
        self.assertEqual(INSTALL.install(first, self.root)["status"], "unchanged")
        self.assertEqual(INSTALL.install(second, self.root)["status"], "upgraded")
        self.assertEqual(INSTALL.status(self.root)["version"], "0.2.0")
        self.assertTrue(INSTALL.status(self.root)["previous_retained"])
        self.assertEqual(INSTALL.rollback(self.root)["version"], "0.1.0")
        self.assertEqual(INSTALL.status(self.root)["version"], "0.1.0")
        outside = self.directory / "history.sqlite3"
        outside.write_bytes(b"user database preserved")
        self.assertEqual(INSTALL.uninstall(self.root)["status"], "uninstalled")
        self.assertFalse(self.root.exists())
        self.assertEqual(outside.read_bytes(), b"user database preserved")

    def test_modified_or_unowned_files_are_preserved(self):
        first = package(self.directory)
        INSTALL.install(first, self.root)
        current = INSTALL.state(self.root)["current"]
        readme = self.root / "versions" / current / "README.md"
        readme.write_bytes(b"user edit")
        with self.assertRaises(ValueError): INSTALL.uninstall(self.root)
        self.assertEqual(readme.read_bytes(), b"user edit")
        with self.assertRaises(ValueError): INSTALL.install(package(self.directory, "0.2.0"), self.root)
        other = self.directory / "foreign"
        other.mkdir()
        (other / "private.png").write_bytes(b"unrelated")
        with self.assertRaises(ValueError): INSTALL.install(first, other)
        self.assertEqual((other / "private.png").read_bytes(), b"unrelated")

    def test_hostile_archive_and_checksum_are_rejected(self):
        hostile = tarfile.TarInfo("../escape")
        hostile.size = 0
        bad = package(self.directory, special=hostile)
        with self.assertRaises(ValueError): INSTALL.install(bad, self.root)
        self.assertFalse((self.directory / "escape").exists())
        shutil.rmtree(self.root)
        bad = package(self.directory, "0.2.0")
        bad.with_name(bad.name + ".sha256").write_text("0" * 64 + "  " + bad.name)
        with self.assertRaises(ValueError): INSTALL.install(bad, self.root)
        self.assertFalse((self.root / INSTALL.STATE).exists())

    def test_symlink_member_and_root_are_rejected(self):
        member = tarfile.TarInfo("package/linked")
        member.type = tarfile.SYMTYPE
        member.linkname = "/unrelated"
        bad = package(self.directory, special=member)
        with self.assertRaises(ValueError): INSTALL.install(bad, self.root)
        if os.name != "nt":
            foreign = self.directory / "foreign"
            foreign.mkdir()
            link = self.directory / "linked-root"
            link.symlink_to(foreign, target_is_directory=True)
            with self.assertRaises(ValueError): INSTALL.install(package(self.directory, "0.2.0"), link)

    def test_archive_metadata_budgets_and_reserved_paths_are_rejected(self):
        for name in ["root/NUL", "root/COM1.txt", "root/trailing.", "root/path:stream", "root/../outside"]:
            with self.assertRaises(ValueError): INSTALL.safe_name(name)
        archive = self.directory / "metadata.tar.gz"
        item = tarfile.TarInfo("pax")
        item.type, item.size = tarfile.XHDTYPE, 65537
        with gzip.open(archive, "wb") as output:
            output.write(item.tobuf())
        with self.assertRaises(ValueError): INSTALL.preflight_tar(archive)
        archive = self.directory / "directory.zip"
        archive.write_bytes(struct.pack("<4s4H2IH", b"PK\x05\x06", 0, 0, 10001, 10001, 0, 0, 0))
        with self.assertRaises(ValueError): INSTALL.preflight_zip(archive)
        oversized = self.directory / "oversized.tar.gz"
        with oversized.open("xb") as source:
            source.truncate(INSTALL.MAX_ARCHIVE + 1)
        with self.assertRaises(ValueError): INSTALL.install(oversized, self.root)

    def test_interrupted_launcher_publication_is_recoverable(self):
        first = package(self.directory)
        second = package(self.directory, "0.2.0")
        INSTALL.install(first, self.root)
        with patch.object(INSTALL, "publish_entry", side_effect=OSError("synthetic disk failure")):
            with self.assertRaises(OSError): INSTALL.install(second, self.root)
        self.assertEqual(INSTALL.status(self.root)["status"], "recovery_required")
        self.assertEqual(INSTALL.recover(self.root, "previous")["version"], "0.1.0")
        self.assertEqual(INSTALL.status(self.root)["version"], "0.1.0")
        self.assertEqual(INSTALL.recover(self.root, "previous")["version"], "0.2.0")
        self.assertEqual(INSTALL.status(self.root)["version"], "0.2.0")

    def child(self, body, *arguments):
        source = ("import importlib.util,sys,os\nfrom pathlib import Path\n"
                  "s=importlib.util.spec_from_file_location('installer',sys.argv[1]);m=importlib.util.module_from_spec(s);s.loader.exec_module(m)\n" + body)
        return subprocess.run([sys.executable, "-c", source, str(ROOT / "scripts/install-user.py"), *map(str, arguments)],
                              capture_output=True, timeout=15)

    def test_interrupted_previous_recovery_can_resume_either_choice(self):
        first = package(self.directory)
        second = package(self.directory, "0.2.0")
        for choice, expected in [("previous", "0.1.0"), ("current", "0.2.0")]:
            with self.subTest(choice=choice):
                root = self.directory / ("recover-" + choice)
                INSTALL.install(first, root)
                with patch.object(INSTALL, "publish_entry", side_effect=OSError("publication interrupted")):
                    with self.assertRaises(OSError): INSTALL.install(second, root)
                result = self.child(
                    "m.publish_entry=lambda *args: os._exit(97)\n"
                    "m.recover(sys.argv[2],'previous')\n", root)
                self.assertEqual(result.returncode, 97, result.stderr)
                self.assertEqual(INSTALL.status(root)["status"], "recovery_required")
                self.assertEqual(INSTALL.recover(root, choice)["version"], expected)
                self.assertEqual(INSTALL.status(root)["version"], expected)

    def test_terminal_uninstall_interruption_is_idempotent_and_preserves_empty_root(self):
        INSTALL.install(package(self.directory), self.root)
        result = self.child(
            "finish=m.finish_transaction\n"
            "def crash(root):\n finish(root)\n os._exit(98)\n"
            "m.finish_transaction=crash\n"
            "m.uninstall(sys.argv[2])\n", self.root)
        self.assertEqual(result.returncode, 98, result.stderr)
        self.assertTrue(self.root.is_dir())
        self.assertEqual(list(self.root.iterdir()), [])
        self.assertEqual(INSTALL.status(self.root)["status"], "not_installed")
        self.assertEqual(INSTALL.recover(self.root, "current")["status"], "uninstalled")
        self.assertEqual(INSTALL.uninstall(self.root)["status"], "uninstalled")
        self.assertTrue(self.root.is_dir())
        foreign = self.root / "user-file"
        foreign.write_bytes(b"preserved")
        with self.assertRaises((ValueError, FileNotFoundError)): INSTALL.uninstall(self.root)
        self.assertEqual(foreign.read_bytes(), b"preserved")

    @unittest.skipIf(os.name == "nt", "Dangling symlink fixture requires Unix")
    def test_terminal_uninstall_conflict_preserves_recovery_intent(self):
        INSTALL.install(package(self.directory), self.root)
        result = self.child(
            "remove=m.delete_owned_file\n"
            "def crash(path,item):\n"
            " if path.parent==Path(sys.argv[2]): os._exit(99)\n"
            " remove(path,item)\n"
            "m.delete_owned_file=crash\n"
            "m.uninstall(sys.argv[2])\n", self.root)
        self.assertEqual(result.returncode, 99, result.stderr)
        versions = self.root / "versions"
        self.assertFalse(versions.exists())
        versions.symlink_to(self.directory / "absent-user-directory", target_is_directory=True)
        with self.assertRaises(ValueError): INSTALL.uninstall(self.root)
        self.assertTrue(versions.is_symlink())
        self.assertTrue((self.root / INSTALL.TRANSACTION).is_file())
        self.assertTrue((self.root / INSTALL.STATE).is_file())
        self.assertTrue((self.root / INSTALL.executable_name(INSTALL.local_target())).is_file())
        versions.unlink()
        self.assertEqual(INSTALL.uninstall(self.root)["status"], "uninstalled")

    def test_first_install_crash_after_version_publication_recovers_without_state(self):
        archive = package(self.directory)
        result = self.child(
            "original=m.atomic_json\n"
            "def crash(path,value,expected=None):\n"
            " if path.name==m.STATE: os._exit(91)\n"
            " return original(path,value,expected)\n"
            "m.atomic_json=crash\nm.install(sys.argv[2],sys.argv[3])\n", archive, self.root)
        self.assertEqual(result.returncode, 91, result.stderr)
        self.assertFalse((self.root / INSTALL.STATE).exists())
        self.assertTrue((self.root / INSTALL.TRANSACTION).is_file())
        self.assertEqual(INSTALL.status(self.root)["status"], "recovery_required")
        self.assertFalse(INSTALL.status(self.root)["state_published"])
        self.assertEqual(INSTALL.recover(self.root, "current")["version"], "0.1.0")
        self.assertEqual(INSTALL.status(self.root)["version"], "0.1.0")
        self.assertFalse((self.root / INSTALL.TRANSACTION).exists())
        self.assertEqual(INSTALL.uninstall(self.root)["status"], "uninstalled")

    def test_first_install_preparation_crashes_leave_root_retryable(self):
        archive = package(self.directory)
        crashes = [
            "original=m.extract\n"
            "def crash(package,destination):\n"
            " result=original(package,destination)\n"
            " os._exit(93)\n"
            "m.extract=crash\n",
            partial_write_crash("isinstance(payload,bytes) and b'\"kind\":\"install\"' in payload", 93),
        ]
        for index, body in enumerate(crashes):
            root = self.directory / f"preparation-{index}"
            result = self.child(body + "m.install(sys.argv[2],sys.argv[3])\n", archive, root)
            self.assertEqual(result.returncode, 93, result.stderr)
            self.assertEqual(list(root.iterdir()), [])
            self.assertIsNone(INSTALL.state(root, True))
            self.assertEqual(INSTALL.install(archive, root)["status"], "installed")
            self.assertEqual(INSTALL.uninstall(root)["status"], "uninstalled")

    def test_first_install_intent_precedes_version_directory_and_recovers(self):
        archive = package(self.directory)
        result = self.child(
            "original=Path.mkdir\n"
            "def crash(path,*args,**kwargs):\n"
            " if path.name=='versions': os._exit(94)\n"
            " return original(path,*args,**kwargs)\n"
            "Path.mkdir=crash\nm.install(sys.argv[2],sys.argv[3])\n", archive, self.root)
        self.assertEqual(result.returncode, 94, result.stderr)
        self.assertFalse((self.root / "versions").exists())
        self.assertEqual(INSTALL.status(self.root)["status"], "recovery_required")
        self.assertEqual(INSTALL.recover(self.root, "current")["version"], "0.1.0")
        self.assertEqual(INSTALL.uninstall(self.root)["status"], "uninstalled")

    def test_failed_intent_write_leaves_first_install_root_empty(self):
        archive = package(self.directory)
        original = INSTALL.atomic_json
        def fail_intent(path, value, expected=None):
            if path.name == INSTALL.TRANSACTION:
                raise OSError("synthetic intent publication failure")
            return original(path, value, expected)
        with patch.object(INSTALL, "atomic_json", fail_intent):
            with self.assertRaises(OSError):
                INSTALL.install(archive, self.root)
        self.assertEqual(list(self.root.iterdir()), [])
        self.assertEqual(list(self.directory.glob(".screenshots-stage-*")), [])
        self.assertEqual(INSTALL.install(archive, self.root)["status"], "installed")

    def test_public_key_pin_crash_can_resume_or_cancel_without_unowned_key(self):
        INSTALL.install(package(self.directory), self.root)
        public = self.directory / "public.pem"
        import base64
        encoded = base64.b64encode(bytes.fromhex("302a300506032b6570032100") + bytes([1]) * 32)
        public.write_bytes(b"-----BEGIN PUBLIC KEY-----\n" + encoded + b"\n-----END PUBLIC KEY-----\n")
        body = ("original=m.atomic_json\n"
                "def crash(path,value,expected=None):\n"
                " if path.name==m.STATE: os._exit(92)\n"
                " return original(path,value,expected)\n"
                "m.atomic_json=crash\nm.pin_key(sys.argv[2],sys.argv[3])\n")
        result = self.child(body, self.root, public)
        self.assertEqual(result.returncode, 92, result.stderr)
        self.assertTrue((self.root / "update-public-key.pem").exists())
        self.assertIsNone(INSTALL.state(self.root)["key_sha256"])
        self.assertEqual(INSTALL.status(self.root)["operation"], "pin")
        self.assertEqual(INSTALL.recover(self.root, "previous")["update_key_pinned"], False)
        self.assertFalse((self.root / "update-public-key.pem").exists())
        result = self.child(body, self.root, public)
        self.assertEqual(result.returncode, 92, result.stderr)
        self.assertTrue(INSTALL.pin_key(self.root, public)["recovered"])
        self.assertTrue(INSTALL.status(self.root)["update_key_pinned"])
        self.assertEqual(INSTALL.uninstall(self.root)["status"], "uninstalled")

    def test_partial_private_key_publication_never_exposes_incomplete_final_key(self):
        INSTALL.install(package(self.directory), self.root)
        public = self.directory / "public.pem"
        import base64
        public.write_bytes(b"-----BEGIN PUBLIC KEY-----\n" + base64.b64encode(
            bytes.fromhex("302a300506032b6570032100") + bytes([2]) * 32)
            + b"\n-----END PUBLIC KEY-----\n")
        body = partial_write_crash(
            "isinstance(payload,bytes) and payload.startswith(b'-----BEGIN PUBLIC KEY-----')", 95)
        body += "m.pin_key(sys.argv[2],sys.argv[3])\n"
        result = self.child(body, self.root, public)
        self.assertEqual(result.returncode, 95, result.stderr)
        self.assertFalse((self.root / "update-public-key.pem").exists())
        self.assertEqual(INSTALL.status(self.root)["operation"], "pin")
        self.assertTrue(INSTALL.recover(self.root, "current")["update_key_pinned"])
        self.assertEqual((self.root / "update-public-key.pem").read_bytes(), public.read_bytes())
        if os.name != "nt":
            self.assertEqual((self.root / "update-public-key.pem").stat().st_mode & 0o777, 0o600)
        self.assertEqual(INSTALL.uninstall(self.root)["status"], "uninstalled")

    def test_os_lock_serializes_concurrent_upgrades_and_rechecks_version(self):
        INSTALL.install(package(self.directory), self.root)
        archives = [package(self.directory, "0.2.0"), package(self.directory, "0.3.0")]
        processes = []
        try:
            with INSTALL.installation_lock(self.root):
                for index, archive in enumerate(archives):
                    ready = self.directory / f"ready-{index}"
                    code = ("import importlib.util,sys\nfrom pathlib import Path\n"
                            "s=importlib.util.spec_from_file_location('installer',sys.argv[1]);m=importlib.util.module_from_spec(s);s.loader.exec_module(m)\n"
                            "Path(sys.argv[4]).write_text('ready')\nm.install(sys.argv[2],sys.argv[3])\n")
                    process = subprocess.Popen([sys.executable, "-c", code, str(ROOT / "scripts/install-user.py"),
                                                str(archive), str(self.root), str(ready)],
                                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                    processes.append(process)
                deadline = time.monotonic() + 5
                while not all((self.directory / f"ready-{index}").exists() for index in range(2)):
                    self.assertLess(time.monotonic(), deadline)
                    time.sleep(0.01)
                self.assertTrue(all(process.poll() is None for process in processes))
            results = [process.communicate(timeout=15) for process in processes]
            self.assertEqual(processes[1].returncode, 0, results[1][1])
            self.assertIn(processes[0].returncode, (0, 1))
            self.assertEqual(INSTALL.status(self.root)["version"], "0.3.0")
            current = INSTALL.state(self.root)["current"]
            self.assertEqual(INSTALL.sha(self.root / INSTALL.executable_name(INSTALL.local_target())),
                             INSTALL.sha(self.root / "versions" / current / INSTALL.executable_name(INSTALL.local_target())))
        finally:
            for process in processes:
                if process.poll() is None:
                    process.kill()
                process.communicate(timeout=5)

    def test_uninstall_preserves_unexpected_empty_directory_and_special_files(self):
        INSTALL.install(package(self.directory), self.root)
        folder = self.root / "versions" / INSTALL.state(self.root)["current"]
        extra = folder / "user-empty-directory"
        extra.mkdir()
        with self.assertRaises(ValueError): INSTALL.uninstall(self.root)
        self.assertTrue(extra.is_dir())
        extra.rmdir()
        if os.name != "nt":
            fifo = folder / "user-fifo"
            os.mkfifo(fifo)
            with self.assertRaises(ValueError): INSTALL.uninstall(self.root)
            self.assertTrue(fifo.exists())
            fifo.unlink()
        self.assertEqual(INSTALL.uninstall(self.root)["status"], "uninstalled")

    def test_uninstall_crash_after_first_owned_deletion_can_resume(self):
        INSTALL.install(package(self.directory), self.root)
        outside = self.directory / "private-user-image.png"
        outside.write_bytes(b"never touched")
        result = self.child(
            "original=m.delete_owned_file\n"
            "def crash(path,item):\n"
            " original(path,item)\n os._exit(93)\n"
            "m.delete_owned_file=crash\nm.uninstall(sys.argv[2])\n", self.root)
        self.assertEqual(result.returncode, 93, result.stderr)
        self.assertEqual(INSTALL.status(self.root)["operation"], "uninstall")
        intent = INSTALL.transaction(self.root)
        self.assertIsNotNone(intent["active"])
        folder = self.root / "versions" / intent["active"]["identifier"]
        self.assertTrue(any(not (folder / item["path"]).exists() for item in intent["active"]["manifest"]["files"]))
        self.assertEqual(INSTALL.recover(self.root, "current")["status"], "uninstalled")
        self.assertFalse(self.root.exists())
        self.assertEqual(outside.read_bytes(), b"never touched")

    def test_interrupted_uninstall_preserves_new_foreign_entries_before_retry(self):
        INSTALL.install(package(self.directory), self.root)
        result = self.child(
            "original=m.delete_owned_file\n"
            "def crash(path,item):\n"
            " original(path,item)\n os._exit(94)\n"
            "m.delete_owned_file=crash\nm.uninstall(sys.argv[2])\n", self.root)
        self.assertEqual(result.returncode, 94, result.stderr)
        intent = INSTALL.transaction(self.root)
        folder = self.root / "versions" / intent["active"]["identifier"]
        added = folder / "user-added-empty-directory"
        added.mkdir()
        with self.assertRaises(ValueError): INSTALL.uninstall(self.root)
        self.assertTrue(added.is_dir())
        added.rmdir()
        if os.name != "nt":
            fifo = folder / "user-added-fifo"
            os.mkfifo(fifo)
            with self.assertRaises(ValueError): INSTALL.uninstall(self.root)
            self.assertTrue(fifo.exists())
            fifo.unlink()
        self.assertEqual(INSTALL.uninstall(self.root)["status"], "uninstalled")

    @unittest.skipUnless(shutil.which("openssl"), "requires installed OpenSSL for synthetic Ed25519 fixtures")
    def test_signed_update_requires_pinned_key_digest_target_and_new_version(self):
        first = package(self.directory)
        second = package(self.directory, "0.2.0")
        INSTALL.install(first, self.root)
        private = self.directory / "fixture-private.pem"
        public = self.directory / "fixture-public.pem"
        subprocess.run(["openssl", "genpkey", "-algorithm", "ED25519", "-out", private], check=True, capture_output=True)
        subprocess.run(["openssl", "pkey", "-in", private, "-pubout", "-out", public], check=True, capture_output=True)
        self.assertEqual(INSTALL.pin_key(self.root, public)["status"], "update_key_pinned")
        with self.assertRaises(ValueError): INSTALL.pin_key(self.root, public)
        manifest = self.directory / "release.json"
        signature = self.directory / "release.json.sig"
        data = {"version": "0.2.0", "target": INSTALL.local_target(), "url": "https://example.invalid/release.tar.gz",
                "sha256": INSTALL.sha(second), "size": second.stat().st_size}
        manifest.write_bytes(INSTALL.json_bytes(data))
        subprocess.run(["openssl", "pkeyutl", "-sign", "-inkey", private, "-rawin", "-in", manifest, "-out", signature], check=True, capture_output=True)
        original_signature = signature.read_bytes()
        signature.write_bytes(bytes(64))
        with self.assertRaises(ValueError): INSTALL.apply_signed(self.root, manifest, signature, second)
        self.assertEqual(INSTALL.status(self.root)["version"], "0.1.0")
        signature.write_bytes(original_signature)
        self.assertEqual(INSTALL.apply_signed(self.root, manifest, signature, second)["status"], "upgraded")
        with self.assertRaises(ValueError): INSTALL.apply_signed(self.root, manifest, signature, second)
        self.assertTrue(INSTALL.status(self.root)["update_key_pinned"])

    @unittest.skipUnless(shutil.which("openssl"), "requires installed OpenSSL for isolated HTTPS fixture")
    def test_online_https_update_authenticates_before_artifact_download(self):
        first, second = package(self.directory), package(self.directory, "0.2.0")
        INSTALL.install(first, self.root)
        private, public = self.directory / "signing-private.pem", self.directory / "signing-public.pem"
        subprocess.run(["openssl", "genpkey", "-algorithm", "ED25519", "-out", private], check=True, capture_output=True)
        subprocess.run(["openssl", "pkey", "-in", private, "-pubout", "-out", public], check=True, capture_output=True)
        INSTALL.pin_key(self.root, public)
        certificate, tls_key = self.directory / "localhost.pem", self.directory / "localhost-key.pem"
        subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", tls_key,
                        "-out", certificate, "-days", "1", "-subj", "/CN=127.0.0.1", "-addext", "subjectAltName=IP:127.0.0.1"],
                       check=True, capture_output=True)
        requests, responses = [], {}

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                requests.append(self.path)
                data = responses[self.path]
                self.send_response(200)
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def log_message(self, *_):
                pass

        server = http.server.HTTPServer(("127.0.0.1", 0), Handler)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(certificate, tls_key)
        server.socket = context.wrap_socket(server.socket, server_side=True)
        address = f"https://127.0.0.1:{server.server_port}"
        manifest, signature = self.directory / "online.json", self.directory / "online.sig"
        data = {"version": "0.2.0", "target": INSTALL.local_target(), "url": address + "/artifact.tar.gz",
                "sha256": INSTALL.sha(second), "size": second.stat().st_size}
        manifest.write_bytes(INSTALL.json_bytes(data))
        subprocess.run(["openssl", "pkeyutl", "-sign", "-inkey", private, "-rawin", "-in", manifest, "-out", signature],
                       check=True, capture_output=True)
        responses.update({"/release.json": manifest.read_bytes(), "/release.json.sig": bytes(64), "/artifact.tar.gz": second.read_bytes()})
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with patch.dict(os.environ, {"SSL_CERT_FILE": str(certificate)}):
                with self.assertRaises(ValueError): INSTALL.update(self.root, address + "/release.json")
                self.assertEqual(requests, ["/release.json", "/release.json.sig"])
                self.assertEqual(INSTALL.status(self.root)["version"], "0.1.0")
                requests.clear()
                responses["/release.json.sig"] = signature.read_bytes()
                self.assertEqual(INSTALL.update(self.root, address + "/release.json")["status"], "upgraded")
                self.assertEqual(requests, ["/release.json", "/release.json.sig", "/artifact.tar.gz"])
                self.assertEqual(INSTALL.status(self.root)["version"], "0.2.0")
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)
            self.assertFalse(thread.is_alive())


if __name__ == "__main__":
    unittest.main()
