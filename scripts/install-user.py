#!/usr/bin/env python3
"""Install, upgrade, roll back or remove a verified user-owned Screenshots package.

Online updates require an explicitly pinned Ed25519 public key and a detached
signature. No server, signing identity or periodic update task is assumed.
"""

import argparse
import base64
import contextlib
import functools
import hashlib
import gzip
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import ssl
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
import urllib.parse
import urllib.request
import uuid
import zipfile

OWNER = "screenshots-user-install-v1"
MAX_ARCHIVE = 512 * 1024 * 1024
MAX_EXPANDED = 1024 * 1024 * 1024
MAX_FILES = 10000
MAX_MANIFEST = 4 * 1024 * 1024
MAX_TRANSACTION = MAX_MANIFEST + 32768
MAX_VERSIONS = 128
STATE = ".screenshots-install.json"
MANIFEST = ".screenshots-version.json"
TRANSACTION = ".screenshots-transaction.json"
HELD_LOCKS = threading.local()


def require(value, message):
    if not value:
        raise ValueError(message)


@contextlib.contextmanager
def installation_lock(root_value, create_parent=False):
    """One OS lock serializes publication, trust changes, rollback and removal."""
    requested = Path(os.path.abspath(os.fspath(root_value)))
    if create_parent:
        requested.parent.mkdir(parents=True, exist_ok=True)
    parent = requested.parent.resolve(strict=True)
    root = parent / requested.name
    normalized = str(root).casefold() if os.name == "nt" or sys.platform == "darwin" else str(root)
    identity = hashlib.sha256(os.fsencode(normalized)).hexdigest()[:32]
    held = getattr(HELD_LOCKS, "roots", set()) if getattr(HELD_LOCKS, "pid", None) == os.getpid() else set()
    if normalized in held:
        yield
        return
    path = parent / (".screenshots-install-lock-" + identity)
    descriptor = os.open(path, os.O_RDWR | os.O_CREAT | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_NOFOLLOW", 0), 0o600)
    try:
        metadata = os.fstat(descriptor)
        named = path.lstat()
        require(stat.S_ISREG(metadata.st_mode) and metadata.st_nlink == 1 and metadata.st_size <= 8192,
                "Installation lock is not a bounded private regular file")
        require(stat.S_ISREG(named.st_mode) and (named.st_dev, named.st_ino) == (metadata.st_dev, metadata.st_ino),
                "Installation lock path changed or is a symlink")
        if os.name != "nt":
            require(metadata.st_uid == os.getuid(), "Installation lock has a different owner")
        deadline = time.monotonic() + 10
        while True:
            try:
                if os.name == "nt":
                    import msvcrt
                    os.lseek(descriptor, 0, os.SEEK_SET)
                    msvcrt.locking(descriptor, msvcrt.LK_NBLCK, 1)
                else:
                    import fcntl
                    fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                require(time.monotonic() < deadline, "Another installation operation holds the lock")
                time.sleep(0.025)
            except OSError as error:
                if os.name != "nt" or error.errno not in (13, 36):
                    raise
                require(time.monotonic() < deadline, "Another installation operation holds the lock")
                time.sleep(0.025)
        marker = OWNER.encode() + b"\n" + os.fsencode(normalized) + b"\n"
        require(len(marker) <= 8192, "Installation root exceeds the lock identity limit")
        os.lseek(descriptor, 0, os.SEEK_SET)
        existing = os.read(descriptor, 8193)
        require(not existing or existing == marker, "Installation lock contains unrelated data")
        if not existing:
            os.lseek(descriptor, 0, os.SEEK_SET)
            os.write(descriptor, marker)
            os.fsync(descriptor)
        HELD_LOCKS.roots = held | {normalized}
        HELD_LOCKS.pid = os.getpid()
        try:
            yield
        finally:
            HELD_LOCKS.roots = held
            if os.name == "nt":
                import msvcrt
                os.lseek(descriptor, 0, os.SEEK_SET)
                msvcrt.locking(descriptor, msvcrt.LK_UNLCK, 1)
            else:
                import fcntl
                fcntl.flock(descriptor, fcntl.LOCK_UN)
    finally:
        os.close(descriptor)
    # Keep the tiny lock file: unlinking it lets a waiter lock an obsolete inode.


def locked(root_argument=0, create_parent=False):
    def decorate(function):
        @functools.wraps(function)
        def invoke(*args, **kwargs):
            with installation_lock(args[root_argument], create_parent):
                return function(*args, **kwargs)
        return invoke
    return decorate


def pairs(values):
    result = {}
    for key, value in values:
        require(key not in result, "Duplicate JSON field")
        result[key] = value
    return result


def json_bytes(value):
    return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()


def read_json(path, limit=MAX_MANIFEST):
    return json.loads(regular_bytes(path, limit), object_pairs_hook=pairs)


def regular_bytes(path, limit):
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_NOFOLLOW", 0))
    with os.fdopen(descriptor, "rb") as source:
        metadata = os.fstat(source.fileno())
        require(stat.S_ISREG(metadata.st_mode) and metadata.st_size <= limit,
                "Input metadata is unavailable or excessive")
        data = source.read(metadata.st_size)
        require(len(data) == metadata.st_size, "Input was truncated")
        return data


def sha(path):
    path = Path(path)
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_NOFOLLOW", 0))
    with os.fdopen(descriptor, "rb") as source:
        metadata = os.fstat(source.fileno())
        require(stat.S_ISREG(metadata.st_mode) and metadata.st_size <= MAX_EXPANDED,
                "Hash input must be a bounded regular file")
        remaining, digest = metadata.st_size, hashlib.sha256()
        while remaining:
            chunk = source.read(min(65536, remaining))
            require(chunk, "Hash input was truncated")
            remaining -= len(chunk)
            digest.update(chunk)
        return digest.hexdigest()


def sync_directory(path):
    if os.name != "nt":
        descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)


def atomic_file(path, payload, expected=None):
    # Unpublished bytes must not make a first install or recovered root unowned.
    parent = path.parent.parent if path.name in (STATE, TRANSACTION, "update-public-key.pem") else path.parent
    temporary = parent / (".screenshots-write-" + str(uuid.uuid4()))
    try:
        with temporary.open("xb") as output:
            temporary.chmod(0o600)
            output.write(payload)
            output.flush()
            os.fsync(output.fileno())
        if expected is None:
            require(not path.exists() and not path.is_symlink(), "Managed metadata appeared before publication")
            os.link(temporary, path)
        else:
            require(read_json(path, MAX_TRANSACTION if path.name == TRANSACTION else 16384) == expected,
                    "Managed metadata changed; existing installation preserved")
            os.replace(temporary, path)
        sync_directory(path.parent)
    finally:
        temporary.unlink(missing_ok=True)


def atomic_json(path, value, expected=None):
    payload = json_bytes(value)
    limit = MAX_TRANSACTION if path.name == TRANSACTION else MAX_MANIFEST
    require(len(payload) <= limit, "Managed manifest exceeds its byte limit")
    atomic_file(path, payload, expected)


def version(value):
    require(isinstance(value, str) and re.fullmatch(r"(?:0|[1-9][0-9]{0,8})(?:\.(?:0|[1-9][0-9]{0,8})){2}", value),
            "Package version must be a bounded numeric major.minor.patch")
    return tuple(map(int, value.split(".")))


def target(value):
    require(isinstance(value, str) and len(value) <= 96 and re.fullmatch(r"[a-z0-9_]+(?:-[a-z0-9_]+){2,4}", value),
            "Invalid package target")
    return value


def local_target():
    import platform
    machine = platform.machine().lower()
    architecture = {"amd64": "x86_64", "x86_64": "x86_64", "aarch64": "aarch64", "arm64": "aarch64"}.get(machine)
    require(architecture is not None, "Unsupported install architecture")
    if sys.platform == "win32":
        return architecture + "-pc-windows-msvc"
    if sys.platform == "darwin":
        return architecture + "-apple-darwin"
    require(sys.platform.startswith("linux"), "Unsupported install platform")
    return architecture + "-unknown-linux-gnu"


def root_path(value, create=False):
    path = Path(value).absolute()
    require(not path.is_symlink(), "Install root must not be a symlink")
    if create and not path.exists():
        path.mkdir(parents=True, mode=0o700)
    require(path.is_dir(), "Install root must be a directory")
    return path


def installation_absent(value):
    path = Path(value).absolute()
    require(not path.is_symlink(), "Install root must not be a symlink")
    if not path.exists():
        return True
    require(path.is_dir(), "Install root must be a directory")
    return not any(path.iterdir())


def state(root, optional=False):
    path = root / STATE
    if optional and not path.exists():
        require(not path.is_symlink() and not any(root.iterdir()), "Existing nonempty directory is not an owned installation")
        return None
    value = read_json(path, 16384)
    return validate_state(value)


def validate_state(value):
    require(type(value) is dict, "Invalid install metadata object")
    require(set(value) == {"owner", "current", "previous", "target", "key_sha256"} and value["owner"] == OWNER,
            "Install metadata is not owned by Screenshots")
    target(value["target"])
    for key in ["current", "previous"]:
        require(value[key] is None or re.fullmatch(r"[0-9.]+-[a-z0-9_-]+", value[key]), "Invalid managed version path")
        if value[key] is not None:
            suffix = "-" + value["target"]
            require(value[key].endswith(suffix), "Managed version target mismatch")
            version(value[key][:-len(suffix)])
    require(value["current"] is not None, "Installation has no current version")
    require(value["key_sha256"] is None or re.fullmatch(r"[0-9a-f]{64}", value["key_sha256"]), "Invalid pinned update key hash")
    return value


def transaction(root):
    path = root / TRANSACTION
    if not path.exists() and not path.is_symlink():
        return None
    value = read_json(path, MAX_TRANSACTION)
    require(type(value) is dict and value.get("owner") == OWNER and value.get("kind") in ("install", "pin", "uninstall"),
            "Recovery intent is not owned by Screenshots")
    common = {"owner", "kind", "prior", "new"}
    extra = {"stage", "manifest_sha256"} if value["kind"] == "install" else {"public_key"} if value["kind"] == "pin" else {"remaining", "active", "launcher"}
    require(set(value) == common | extra,
            "Recovery intent has unknown or missing fields")
    if value["prior"] is not None:
        validate_state(value["prior"])
    validate_state(value["new"])
    if value["kind"] == "install":
        require(re.fullmatch(r"\.screenshots-stage-[0-9a-f-]{36}", value["stage"])
                and re.fullmatch(r"[0-9a-f]{64}", value["manifest_sha256"]), "Invalid install recovery intent")
    elif value["kind"] == "pin":
        require(value["prior"] is not None and value["prior"]["key_sha256"] is None,
                "Invalid trust-pin recovery intent")
        require(all(value["new"][key] == value["prior"][key] for key in ("owner", "current", "previous", "target")),
                "Trust-pin intent cannot change installed versions")
        public = base64.b64decode(value["public_key"], validate=True)
        require(len(public) <= 4096 and hashlib.sha256(public).hexdigest() == value["new"]["key_sha256"],
                "Pinned-key intent does not match its public key")
    else:
        require(value["prior"] == value["new"] and type(value["remaining"]) is list and len(value["remaining"]) <= MAX_VERSIONS,
                "Invalid uninstall recovery intent")
        require(all(isinstance(identifier, str) and re.fullmatch(r"[0-9.]+-[a-z0-9_-]+", identifier) for identifier in value["remaining"])
                and len(set(value["remaining"])) == len(value["remaining"]), "Invalid uninstall version list")
        require(value["active"] is None or (type(value["active"]) is dict and set(value["active"]) == {"identifier", "manifest"}
                and value["remaining"] and value["active"]["identifier"] == value["remaining"][0]), "Invalid active uninstall version")
        require(type(value["launcher"]) is dict and set(value["launcher"]) == {"name", "sha256", "size"}
                and value["launcher"]["name"] == executable_name(value["prior"]["target"])
                and re.fullmatch(r"[0-9a-f]{64}", value["launcher"]["sha256"])
                and type(value["launcher"]["size"]) is int and 0 <= value["launcher"]["size"] <= MAX_ARCHIVE,
                "Invalid uninstall launcher intent")
    return value


def finish_transaction(root):
    (root / TRANSACTION).unlink()
    sync_directory(root)


def replace_state(root, value):
    path = root / STATE
    expected = read_json(path, 16384) if path.exists() or path.is_symlink() else None
    atomic_json(path, value, expected)


def safe_name(name):
    require(isinstance(name, str) and len(name.encode()) <= 4096 and "\\" not in name and "\0" not in name,
            "Invalid archive path")
    path = PurePosixPath(name)
    require(not path.is_absolute() and 2 <= len(path.parts) <= 24 and all(part not in ("", ".", "..") for part in path.parts)
            and not any(":" in part for part in path.parts), "Archive path escapes package root")
    require(all(part == part.rstrip(" .") and part.split(".")[0].upper() not in
                {"CON", "PRN", "AUX", "NUL", *[f"COM{i}" for i in range(1, 10)], *[f"LPT{i}" for i in range(1, 10)]}
                for part in path.parts), "Archive contains a reserved native path")
    return path


def extract(package, destination):
    require(package.is_file() and not package.is_symlink() and package.stat().st_size <= MAX_ARCHIVE,
            "Package must be a regular archive of at most 512 MiB")
    seen, roots, total, path_bytes = set(), set(), 0, 0
    files = []

    def copy(name, size, source, mode):
        nonlocal total, path_bytes
        path = safe_name(name)
        require(name not in seen and len(seen) < MAX_FILES and 0 <= size <= MAX_ARCHIVE, "Duplicate or excessive archive entry")
        seen.add(name)
        path_bytes += len(name.encode())
        require(path_bytes <= 1024 * 1024, "Package path metadata exceeds its limit")
        roots.add(path.parts[0])
        total += size
        require(len(roots) == 1 and total <= MAX_EXPANDED, "Package exceeds expanded limit or has multiple roots")
        relative = Path(*path.parts[1:])
        require(relative.name not in (STATE, MANIFEST), "Package collides with installer metadata")
        output = destination / relative
        output.parent.mkdir(parents=True, exist_ok=True)
        remaining, digest = size, hashlib.sha256()
        with output.open("xb") as stream:
            while remaining:
                chunk = source.read(min(65536, remaining))
                require(chunk, "Truncated archive member")
                remaining -= len(chunk)
                digest.update(chunk)
                stream.write(chunk)
            require(not source.read(1), "Archive member exceeded its declared size")
            stream.flush()
            os.fsync(stream.fileno())
        output.chmod(0o755 if mode & 0o111 else 0o644)
        files.append({"path": relative.as_posix(), "sha256": digest.hexdigest(), "size": size})

    if package.suffix.lower() == ".zip":
        preflight_zip(package)
        with zipfile.ZipFile(package) as archive:
            require(len(archive.infolist()) <= MAX_FILES, "Too many archive entries")
            for item in archive.infolist():
                mode = item.external_attr >> 16
                require(not item.is_dir() and (not stat.S_IFMT(mode) or stat.S_ISREG(mode)), "Package contains a nonregular entry")
                with archive.open(item) as stream:
                    copy(item.filename, item.file_size, stream, mode)
    else:
        preflight_tar(package)
        with tarfile.open(package, "r|gz") as archive:
            for item in archive:
                require(item.isfile() and not item.issym() and not item.islnk(), "Package contains a nonregular entry")
                with archive.extractfile(item) as stream:
                    copy(item.name, item.size, stream, item.mode)
    require(files, "Package contains no files")
    info = read_json(destination / "BUILD-INFO.json", 16384)
    require({"version", "target", "binary_sha256"} <= set(info), "Package build information is incomplete")
    version(info["version"])
    target(info["target"])
    require(info["target"] == local_target(), "Package target does not match this machine")
    executable = "screenshots.exe" if "windows" in info["target"] else "screenshots"
    require((destination / executable).is_file() and sha(destination / executable) == info["binary_sha256"], "Package executable hash does not match build information")
    for required in ["LICENSE", "README.md", "Cargo.lock", "THIRD_PARTY_NOTICES.md"]:
        require((destination / required).is_file(), "Package is missing required notices or metadata")
    return info, files, executable


def preflight_zip(package):
    with package.open("rb") as source:
        length = source.seek(0, os.SEEK_END)
        source.seek(max(0, length - 65557))
        tail = source.read(65557)
    position = tail.rfind(b"PK\x05\x06")
    require(position >= 0 and position + 22 <= len(tail), "ZIP end directory is missing")
    _, disk, directory_disk, disk_count, count, size, offset, comment = struct.unpack_from("<4s4H2IH", tail, position)
    require(position + 22 + comment == len(tail) and disk == directory_disk == 0 and disk_count == count
            and 0 < count <= MAX_FILES and size <= 16 * 1024 * 1024 and offset + size <= length
            and count != 65535 and size != 0xffffffff and offset != 0xffffffff,
            "ZIP directory exceeds its bounded single-volume contract")


def preflight_tar(package):
    total, entries = 0, 0
    with gzip.open(package, "rb") as source:
        while True:
            block = source.read(512)
            require(len(block) == 512, "Truncated tar header")
            if block == bytes(512):
                return
            item = tarfile.TarInfo.frombuf(block, "utf-8", "strict")
            entries += 1
            require(entries <= MAX_FILES * 2 and item.size >= 0, "Excessive tar metadata")
            if item.type in (tarfile.XHDTYPE, tarfile.XGLTYPE):
                require(item.size <= 65536, "Tar extended metadata exceeds 64 KiB")
            elif item.type in (tarfile.GNUTYPE_LONGNAME, tarfile.GNUTYPE_LONGLINK):
                require(item.size <= 4096, "Tar long-name metadata exceeds 4096 bytes")
            else:
                require(item.isfile() and item.size <= MAX_ARCHIVE, "Tar contains an unsupported or excessive member")
            total += 512 + (item.size + 511) // 512 * 512
            require(total <= MAX_EXPANDED + 16 * 1024 * 1024, "Tar exceeds its expanded-byte budget")
            remaining = (item.size + 511) // 512 * 512
            while remaining:
                chunk = source.read(min(65536, remaining))
                require(chunk, "Truncated tar member")
                remaining -= len(chunk)


def verify_version(root, identifier):
    require((root / "versions").is_dir() and not (root / "versions").is_symlink(), "Version directory must not be a symlink")
    folder = root / "versions" / identifier
    return verify_folder(folder)


def verify_folder(folder):
    require(folder.is_dir() and not folder.is_symlink(), "Managed version is unavailable")
    manifest = read_json(folder / MANIFEST)
    require(set(manifest) == {"owner", "version", "target", "executable", "files", "archive_sha256"} and manifest["owner"] == OWNER,
            "Version metadata is not owned by Screenshots")
    require(len(manifest["files"]) <= MAX_FILES, "Excessive installed manifest")
    expected = {MANIFEST}
    expected_directories = set()
    for item in manifest["files"]:
        require(set(item) == {"path", "sha256", "size"}, "Invalid installed member metadata")
        relative = safe_name("root/" + item["path"])
        parent = folder
        for component in relative.parts[1:-1]:
            parent /= component
            require(parent.is_dir() and not parent.is_symlink(), "Installed parent path changed; preserve the installation")
        file = folder / Path(*relative.parts[1:])
        require(file.is_file() and not file.is_symlink() and file.stat().st_size == item["size"] and sha(file) == item["sha256"],
                "Installed file changed; preserve it and inspect the installation")
        expected.add(item["path"])
        expected_directories.update(parent.as_posix() for parent in Path(item["path"]).parents if parent != Path("."))
    actual, actual_directories = set(), set()
    for file in folder.rglob("*"):
        metadata = file.lstat()
        require(stat.S_ISREG(metadata.st_mode) or stat.S_ISDIR(metadata.st_mode),
                "Unexpected special file or symlink inside installed version")
        if stat.S_ISREG(metadata.st_mode):
            actual.add(file.relative_to(folder).as_posix())
        else:
            actual_directories.add(file.relative_to(folder).as_posix())
    require(actual == expected and actual_directories == expected_directories,
            "Installed version contains unowned files or directories; preserved")
    require(manifest["executable"] in expected, "Invalid installed executable")
    return manifest


def executable_name(target_name):
    return "screenshots.exe" if "windows" in target_name else "screenshots"


def version_names(root):
    versions = root / "versions"
    require(versions.is_dir() and not versions.is_symlink(), "Version directory is unavailable")
    names = []
    for path in versions.iterdir():
        require(len(names) < MAX_VERSIONS, "Retained version count exceeds its limit")
        require(re.fullmatch(r"[0-9.]+-[a-z0-9_-]+", path.name), "Unknown entry in managed version directory")
        names.append(path.name)
    return sorted(names)


def publish_entry(root, folder, executable, previous):
    entry = root / executable
    if entry.exists() or entry.is_symlink():
        require(previous is not None and entry.is_file() and not entry.is_symlink(), "Launcher conflicts with existing file")
        old = verify_version(root, previous)
        require(sha(entry) == sha(root / "versions" / previous / old["executable"]), "Installed launcher changed; preserved")
    temporary = root.parent / (".screenshots-launch-" + str(uuid.uuid4()))
    try:
        # Prefer one filesystem link; copying remains streamed for filesystems without links.
        try:
            os.link(folder / executable, temporary)
        except OSError:
            with (folder / executable).open("rb") as source, temporary.open("xb") as output:
                shutil.copyfileobj(source, output, 65536)
                output.flush()
                os.fsync(output.fileno())
            temporary.chmod(0o755)
        if previous is None:
            os.link(temporary, entry)
        else:
            os.replace(temporary, entry)
        sync_directory(root)
    finally:
        temporary.unlink(missing_ok=True)


@locked(1, create_parent=True)
def install(package, root_value, verified_digest=None):
    package = Path(package).absolute()
    require(package.is_file() and not package.is_symlink() and package.stat().st_size <= MAX_ARCHIVE,
            "Package must be a bounded regular archive")
    if verified_digest is None:
        checksum = package.with_name(package.name + ".sha256")
        require(checksum.is_file() and not checksum.is_symlink() and checksum.stat().st_size <= 1024,
                "Local package checksum sidecar is required")
        text = regular_bytes(checksum, 1024).decode().strip()
        require(re.fullmatch(r"[0-9a-f]{64}  " + re.escape(package.name), text), "Invalid local package checksum")
        verified_digest = text[:64]
    with tempfile.TemporaryDirectory(prefix="screenshots-package-") as temporary:
        snapshot = Path(temporary) / ("package.zip" if package.suffix.lower() == ".zip" else "package.tar.gz")
        descriptor = os.open(package, os.O_RDONLY | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_NOFOLLOW", 0))
        with os.fdopen(descriptor, "rb") as source, snapshot.open("xb") as output:
            metadata = os.fstat(source.fileno())
            require(stat.S_ISREG(metadata.st_mode) and metadata.st_size <= MAX_ARCHIVE, "Package changed before opening")
            remaining, digest = metadata.st_size, hashlib.sha256()
            while remaining:
                chunk = source.read(min(65536, remaining))
                require(chunk, "Package was truncated during snapshot")
                remaining -= len(chunk)
                digest.update(chunk)
                output.write(chunk)
            output.flush()
            os.fsync(output.fileno())
        require(digest.hexdigest() == verified_digest, "Package checksum mismatch")
        return install_snapshot(snapshot, root_value, verified_digest)


@locked(1, create_parent=True)
def install_snapshot(package, root_value, digest):
    root = root_path(root_value, True)
    require(root.stat().st_dev == root.parent.stat().st_dev,
            "Install root must share its parent filesystem for recoverable publication")
    require(not (root / TRANSACTION).exists() and not (root / TRANSACTION).is_symlink(), "An interrupted operation requires explicit recovery")
    prior = state(root, True)
    if prior:
        verify_version(root, prior["current"])
    staging = root.parent / (".screenshots-stage-" + str(uuid.uuid4()))
    staging.mkdir(mode=0o700)
    try:
        info, files, executable = extract(package, staging)
        identifier = info["version"] + "-" + info["target"]
        if prior and prior["current"] == identifier:
            existing = verify_version(root, identifier)
            require(existing["archive_sha256"] == digest, "Same installed version has different contents")
            return {"status": "unchanged", "version": info["version"], "target": info["target"]}
        if prior:
            require(len(version_names(root)) < MAX_VERSIONS, "Retained version limit reached; remove an older owned installation explicitly")
            current = verify_version(root, prior["current"])
            require(info["target"] == prior["target"] and version(info["version"]) > version(current["version"]),
                    "Upgrade must use a newer version of the same target; use rollback for the previous version")
        manifest = {"owner": OWNER, "version": info["version"], "target": info["target"], "executable": executable,
                    "files": files, "archive_sha256": digest}
        atomic_json(staging / MANIFEST, manifest)
        versions = root / "versions"
        require(not versions.is_symlink(), "Version directory must not be a symlink")
        folder = versions / identifier
        require(not folder.exists() and not folder.is_symlink(), "Version destination already exists; inspect it")
        value = {"owner": OWNER, "current": identifier, "previous": prior["current"] if prior else None,
                 "target": info["target"], "key_sha256": prior["key_sha256"] if prior else None}
        intent = {"owner": OWNER, "kind": "install", "prior": prior, "new": value,
                  "stage": staging.name, "manifest_sha256": sha(staging / MANIFEST)}
        for directory in sorted((path for path in staging.rglob("*") if path.is_dir()),
                                key=lambda path: len(path.parts), reverse=True):
            sync_directory(directory)
        sync_directory(staging)
        sync_directory(root.parent)
        atomic_json(root / TRANSACTION, intent)
        versions.mkdir(mode=0o700, exist_ok=True)
        require(not versions.is_symlink(), "Version directory must not be a symlink")
        sync_directory(root)
        os.rename(staging, folder)
        sync_directory(versions)
        sync_directory(root.parent)
        atomic_json(root / STATE, value, prior)
        publish_entry(root, folder, executable, prior["current"] if prior else None)
        finish_transaction(root)
        return {"status": "installed" if prior is None else "upgraded", "version": info["version"], "target": info["target"], "previous_retained": prior is not None}
    finally:
        if staging.exists() and not (root / TRANSACTION).exists():
            shutil.rmtree(staging)
        # An owned intent retains staging/published versions for explicit recovery.


@locked()
def status(root_value):
    if installation_absent(root_value):
        return {"status": "not_installed"}
    root = root_path(root_value)
    pending = transaction(root)
    if pending:
        return {"status": "recovery_required", "operation": pending["kind"],
                "version": pending["new"]["current"].split("-")[0], "state_published": (root / STATE).exists()}
    value = state(root)
    current = verify_version(root, value["current"])
    entry = root / current["executable"]
    require(entry.is_file() and not entry.is_symlink() and sha(entry) == sha(root / "versions" / value["current"] / current["executable"]), "Launcher and current-version metadata differ; inspect interrupted upgrade")
    interrupted = sorted(path.name for path in root.iterdir() if path.name.startswith((".screenshots-stage-", ".screenshots-write-", ".screenshots-launch-")))
    return {"status": "installed", "version": current["version"], "target": current["target"], "previous_retained": value["previous"] is not None,
            "update_key_pinned": value["key_sha256"] is not None, "unfinished": interrupted}


@locked()
def rollback(root_value):
    root = root_path(root_value)
    require(transaction(root) is None, "An interrupted operation requires explicit recovery")
    value = state(root)
    require(value["previous"] is not None, "No previous version is retained")
    previous = verify_version(root, value["previous"])
    verify_version(root, value["current"])
    publish_entry(root, root / "versions" / value["previous"], previous["executable"], value["current"])
    changed = dict(value, current=value["previous"], previous=value["current"])
    atomic_json(root / STATE, changed, value)
    return {"status": "rolled_back", "version": previous["version"], "target": previous["target"]}


@locked()
def recover(root_value, choice):
    """Reconcile the stable executable after interruption, preserving both versions."""
    if installation_absent(root_value):
        require(choice == "current", "No previous installation is available")
        return {"status": "uninstalled", "user_data": "preserved"}
    root = root_path(root_value)
    pending = transaction(root)
    if pending:
        return recover_transaction(root, pending, choice)
    value = state(root)
    selected = value[choice]
    require(selected is not None, "Requested recovery version is unavailable")
    chosen = verify_version(root, selected)
    entry = root / chosen["executable"]
    old_identifier = None
    if entry.exists() or entry.is_symlink():
        require(entry.is_file() and not entry.is_symlink(), "Recovery launcher is not a regular file")
        digest = sha(entry)
        for identifier in [value["current"], value["previous"]]:
            if identifier:
                old = verify_version(root, identifier)
                if digest == sha(root / "versions" / identifier / old["executable"]):
                    old_identifier = identifier
                    break
        require(old_identifier is not None, "Recovery found an unowned launcher; preserved")
    publish_entry(root, root / "versions" / selected, chosen["executable"], old_identifier)
    if choice == "previous":
        atomic_json(root / STATE, dict(value, current=value["previous"], previous=value["current"]), value)
    return {"status": "recovered", "version": chosen["version"], "target": chosen["target"]}


def recover_transaction(root, pending, choice):
    current_state = read_json(root / STATE, 16384) if (root / STATE).exists() else None
    if pending["kind"] == "uninstall":
        require(current_state is None or current_state == pending["prior"], "Installation state differs from uninstall intent; preserved")
        require(choice == "current", "An interrupted uninstall can only finish; deleted owned files cannot be restored")
        return finish_uninstall(root, pending)
    accepted_states = [pending["prior"], pending["new"]]
    if pending["kind"] == "install" and pending["prior"] is not None:
        accepted_states.append(dict(pending["prior"], previous=pending["new"]["current"]))
    require(current_state in accepted_states, "Installation state differs from recovery intent; preserved")
    if pending["kind"] == "pin":
        public = base64.b64decode(pending["public_key"], validate=True)
        key = root / "update-public-key.pem"
        if key.exists() or key.is_symlink():
            require(regular_bytes(key, 4096) == public, "Pinned key differs from recovery intent; preserved")
        if choice == "current":
            if not key.exists():
                atomic_file(key, public)
            replace_state(root, pending["new"])
        else:
            if key.exists():
                key.unlink()
                sync_directory(root)
            replace_state(root, pending["prior"])
        finish_transaction(root)
        return {"status": "recovered", "operation": "pin", "update_key_pinned": choice == "current"}

    selected_state = pending["new"] if choice == "current" else pending["prior"]
    require(selected_state is not None, "First installation has no previous version; recover current")
    if choice == "previous":
        selected_state = dict(selected_state, previous=pending["new"]["current"])
    versions = root / "versions"
    versions.mkdir(mode=0o700, exist_ok=True)
    require(versions.is_dir() and not versions.is_symlink(), "Recovery version directory is unavailable")
    sync_directory(root)
    published = versions / pending["new"]["current"]
    staging = root.parent / pending["stage"]
    old_staging = root / pending["stage"]
    require(not ((staging.exists() or staging.is_symlink()) and (old_staging.exists() or old_staging.is_symlink())),
            "Multiple staged versions exist; preserved")
    if old_staging.exists() or old_staging.is_symlink():
        staging = old_staging
    require(not (published.exists() and staging.exists()), "Both staged and published version exist; preserved")
    if not published.exists():
        verify_folder(staging)
        require(sha(staging / MANIFEST) == pending["manifest_sha256"], "Staged version differs from recovery intent")
        os.rename(staging, published)
        sync_directory(versions)
        sync_directory(staging.parent)
    manifest = verify_version(root, pending["new"]["current"])
    require(sha(published / MANIFEST) == pending["manifest_sha256"]
            and manifest["version"] + "-" + manifest["target"] == pending["new"]["current"],
            "Published version differs from recovery intent")
    selected = verify_version(root, selected_state["current"])
    entry = root / selected["executable"]
    previous_identifier = None
    if entry.exists() or entry.is_symlink():
        require(entry.is_file() and not entry.is_symlink(), "Recovery launcher is not a regular file")
        digest = sha(entry)
        candidates = {pending["new"]["current"]}
        if pending["prior"]:
            candidates.update(value for value in (pending["prior"]["current"], pending["prior"]["previous"]) if value)
        for identifier in candidates:
            candidate = verify_version(root, identifier)
            if sha(root / "versions" / identifier / candidate["executable"]) == digest:
                previous_identifier = identifier
                break
        require(previous_identifier is not None, "Recovery found an unowned launcher; preserved")
    replace_state(root, selected_state)
    publish_entry(root, root / "versions" / selected_state["current"], selected["executable"], previous_identifier)
    finish_transaction(root)
    return {"status": "recovered", "version": selected["version"], "target": selected["target"]}


@locked()
def uninstall(root_value):
    if installation_absent(root_value):
        return {"status": "uninstalled", "user_data": "preserved"}
    root = root_path(root_value)
    pending = transaction(root)
    if pending:
        require(pending["kind"] == "uninstall", "A different interrupted operation requires explicit recovery")
        return finish_uninstall(root, pending)
    value = state(root)
    status(root)
    identifiers = version_names(root)
    for identifier in identifiers:
        verify_version(root, identifier)
    allowed = {STATE, "versions", executable_name(value["target"])}
    if value["key_sha256"]:
        require(sha(root / "update-public-key.pem") == value["key_sha256"], "Pinned key changed; installation preserved")
        allowed.add("update-public-key.pem")
    require({path.name for path in root.iterdir()} == allowed, "Install root contains unowned or unfinished files; preserved")
    entry = root / executable_name(value["target"])
    pending = {"owner": OWNER, "kind": "uninstall", "prior": value, "new": value,
               "remaining": identifiers, "active": None,
               "launcher": {"name": entry.name, "size": entry.stat().st_size, "sha256": sha(entry)}}
    atomic_json(root / TRANSACTION, pending)
    return finish_uninstall(root, pending)


def removal_members(manifest):
    require(type(manifest) is dict and set(manifest) == {"owner", "version", "target", "executable", "files", "archive_sha256"}
            and manifest["owner"] == OWNER and type(manifest["files"]) is list and len(manifest["files"]) <= MAX_FILES,
            "Uninstall manifest is not owned or bounded")
    files, directories = {}, set()
    for item in manifest["files"]:
        require(type(item) is dict and set(item) == {"path", "sha256", "size"}, "Invalid uninstall member")
        safe_name("root/" + item["path"])
        require(item["path"] not in files and type(item["size"]) is int and 0 <= item["size"] <= MAX_ARCHIVE
                and isinstance(item["sha256"], str) and re.fullmatch(r"[0-9a-f]{64}", item["sha256"]), "Invalid uninstall member admission")
        files[item["path"]] = item
        directories.update(parent.as_posix() for parent in Path(item["path"]).parents if parent != Path("."))
    require(manifest["executable"] in files, "Uninstall executable is not owned")
    return files, directories


def verify_removal_tree(folder, manifest):
    files, directories = removal_members(manifest)
    if not folder.exists() and not folder.is_symlink():
        return files, directories
    require(folder.is_dir() and not folder.is_symlink(), "Uninstall version path changed")
    for path in folder.rglob("*"):
        relative = path.relative_to(folder).as_posix()
        metadata = path.lstat()
        if stat.S_ISDIR(metadata.st_mode):
            require(relative in directories, "Uninstall found an unowned directory; preserved")
        else:
            require(stat.S_ISREG(metadata.st_mode), "Uninstall found a special file or symlink; preserved")
            if relative == MANIFEST:
                require(read_json(path) == manifest, "Version metadata changed during uninstall; preserved")
            else:
                require(relative in files and metadata.st_size == files[relative]["size"] and sha(path) == files[relative]["sha256"],
                        "Uninstall found an unowned or modified file; preserved")
    return files, directories


def delete_owned_file(path, item):
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        return
    require(stat.S_ISREG(metadata.st_mode) and metadata.st_size == item["size"] and sha(path) == item["sha256"],
            "Uninstall member changed before removal; preserved")
    current = path.lstat()
    require((metadata.st_dev, metadata.st_ino, metadata.st_size, metadata.st_mtime_ns, metadata.st_ctime_ns)
            == (current.st_dev, current.st_ino, current.st_size, current.st_mtime_ns, current.st_ctime_ns),
            "Uninstall member changed during verification; preserved")
    path.unlink()


def finish_uninstall(root, pending):
    value = pending["prior"]
    state_file = root / STATE
    if state_file.exists() or state_file.is_symlink():
        require(read_json(state_file, 16384) == value, "Install state differs from uninstall intent; preserved")
    allowed = {STATE, TRANSACTION, "versions", pending["launcher"]["name"]}
    if value["key_sha256"]:
        allowed.add("update-public-key.pem")
    require({path.name for path in root.iterdir()} <= allowed, "Uninstall found unowned root entries; preserved")
    versions = root / "versions"
    if pending["remaining"]:
        require(versions.is_dir() and not versions.is_symlink(), "Uninstall version directory changed")
    if versions.exists() or versions.is_symlink():
        require(versions.is_dir() and not versions.is_symlink(), "Uninstall version directory changed; preserved")
        require(set(version_names(root)) <= set(pending["remaining"]), "Uninstall found an unowned retained version; preserved")
    while pending["remaining"]:
        identifier = pending["remaining"][0]
        folder = versions / identifier
        if pending["active"] is None:
            manifest = verify_version(root, identifier)
            changed = dict(pending, active={"identifier": identifier, "manifest": manifest})
            atomic_json(root / TRANSACTION, changed, pending)
            pending = changed
        manifest = pending["active"]["manifest"]
        files, directories = verify_removal_tree(folder, manifest)
        for name, item in files.items():
            delete_owned_file(folder / name, item)
        for name in sorted(directories, key=lambda value: len(Path(value).parts), reverse=True):
            directory = folder / name
            if directory.exists() or directory.is_symlink():
                require(directory.is_dir() and not directory.is_symlink(), "Uninstall directory changed; preserved")
                directory.rmdir()
        manifest_file = folder / MANIFEST
        if manifest_file.exists() or manifest_file.is_symlink():
            require(read_json(manifest_file) == manifest, "Uninstall version metadata changed; preserved")
            manifest_file.unlink()
        if folder.exists():
            folder.rmdir()
            sync_directory(versions)
        changed = dict(pending, remaining=pending["remaining"][1:], active=None)
        atomic_json(root / TRANSACTION, changed, pending)
        pending = changed
    if versions.exists():
        versions.rmdir()
    delete_owned_file(root / pending["launcher"]["name"], pending["launcher"])
    key = root / "update-public-key.pem"
    if value["key_sha256"] and (key.exists() or key.is_symlink()):
        require(sha(key) == value["key_sha256"], "Pinned key changed during uninstall; preserved")
        key.unlink()
    if state_file.exists() or state_file.is_symlink():
        require(read_json(state_file, 16384) == value, "Install state changed during uninstall; preserved")
        state_file.unlink()
    finish_transaction(root)
    root.rmdir()
    sync_directory(root.parent)
    return {"status": "uninstalled", "user_data": "preserved"}


def openssl(arguments):
    result = subprocess.run(["openssl", *map(str, arguments)], stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10)
    require(result.returncode == 0, "Ed25519 verification helper failed")


@locked()
def pin_key(root_value, public_key):
    root = root_path(root_value)
    value = state(root)
    source = Path(public_key)
    require(source.is_file() and not source.is_symlink() and 1 <= source.stat().st_size <= 4096, "Public key must be a bounded regular PEM file")
    public = regular_bytes(source, 4096)
    match = re.fullmatch(rb"\s*-----BEGIN PUBLIC KEY-----\s*([A-Za-z0-9+/=\r\n]+)\s*-----END PUBLIC KEY-----\s*", public)
    require(match is not None, "Update trust key must be a public-key PEM")
    der = base64.b64decode(re.sub(rb"\s", b"", match[1]), validate=True)
    require(len(der) == 44 and der[:12] == bytes.fromhex("302a300506032b6570032100"), "Update trust key must be Ed25519")
    pending = transaction(root)
    if pending:
        require(pending["kind"] == "pin" and pending["public_key"] == base64.b64encode(public).decode(),
                "A different interrupted operation requires explicit recovery")
        recover_transaction(root, pending, "current")
        return {"status": "update_key_pinned", "public_key_sha256": hashlib.sha256(public).hexdigest(), "recovered": True}
    require(value["key_sha256"] is None, "An update key is already pinned; changing trust requires an explicit new installation")
    destination = root / "update-public-key.pem"
    require(not destination.exists() and not destination.is_symlink(), "Public key destination contains an unowned file")
    changed = dict(value, key_sha256=hashlib.sha256(public).hexdigest())
    pending = {"owner": OWNER, "kind": "pin", "prior": value, "new": changed,
               "public_key": base64.b64encode(public).decode()}
    atomic_json(root / TRANSACTION, pending)
    recover_transaction(root, pending, "current")
    return {"status": "update_key_pinned", "public_key_sha256": changed["key_sha256"]}


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, newurl):
        return None


def url(value):
    require(isinstance(value, str) and len(value) <= 2048 and not any(ord(ch) < 32 for ch in value), "Invalid update URL")
    parsed = urllib.parse.urlsplit(value)
    require(parsed.scheme == "https" and parsed.hostname and not parsed.username and not parsed.password and not parsed.fragment,
            "Updates require HTTPS without credentials or fragments")
    return value


def download(address, destination, limit, expected_size=None):
    address = url(address)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect(), urllib.request.HTTPSHandler(context=ssl.create_default_context()))
    deadline = time.monotonic() + 60
    with opener.open(urllib.request.Request(address, headers={"User-Agent": "screenshots-updater/1"}), timeout=15) as source, destination.open("xb") as output:
        require(source.status == 200, "Update server did not return a complete resource")
        declared = source.headers.get("Content-Length")
        if declared is not None:
            require(declared.isdecimal() and int(declared) <= limit and (expected_size is None or int(declared) == expected_size), "Update size admission failed")
        total = 0
        while True:
            require(time.monotonic() < deadline, "Update download exceeded its deadline")
            chunk = source.read(min(65536, limit + 1 - total))
            if not chunk:
                break
            total += len(chunk)
            require(total <= limit, "Update resource exceeded its byte limit")
            output.write(chunk)
        require(expected_size is None or total == expected_size, "Update resource was truncated")
        output.flush()
        os.fsync(output.fileno())


@locked()
def apply_signed(root_value, manifest_path, signature_path, artifact):
    root = root_path(root_value)
    require(transaction(root) is None, "An interrupted operation requires explicit recovery")
    installed = state(root)
    manifest = authenticated_manifest(root, installed, manifest_path, signature_path)
    validate_update_manifest(root, installed, manifest)
    artifact = Path(artifact)
    require(artifact.is_file() and not artifact.is_symlink() and artifact.stat().st_size == manifest["size"] and sha(artifact) == manifest["sha256"], "Artifact does not match authenticated manifest")
    # Freeze the authenticated input before parsing; later source edits cannot change it.
    with tempfile.TemporaryDirectory(prefix="screenshots-update-inspect-") as temporary:
        temporary = Path(temporary)
        snapshot = temporary / ("artifact.zip" if artifact.suffix.lower() == ".zip" else "artifact.tar.gz")
        descriptor = os.open(artifact, os.O_RDONLY | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_NOFOLLOW", 0))
        with os.fdopen(descriptor, "rb") as source, snapshot.open("xb") as output:
            opened = os.fstat(source.fileno())
            require(stat.S_ISREG(opened.st_mode) and opened.st_size == manifest["size"], "Artifact changed before opening")
            remaining = opened.st_size
            while remaining:
                chunk = source.read(min(65536, remaining))
                require(chunk, "Artifact was truncated during snapshot")
                remaining -= len(chunk)
                output.write(chunk)
            output.flush()
            os.fsync(output.fileno())
        require(sha(snapshot) == manifest["sha256"], "Artifact changed before authenticated parsing")
        inspected = temporary / "expanded"
        inspected.mkdir()
        info, _, _ = extract(snapshot, inspected)
        require(info["version"] == manifest["version"] and info["target"] == manifest["target"], "Artifact build information differs from signed manifest")
        return install_snapshot(snapshot, root, manifest["sha256"])


def authenticated_manifest(root, installed, manifest_path, signature_path):
    require(installed["key_sha256"] is not None, "No update public key is pinned")
    key = root / "update-public-key.pem"
    key_bytes = regular_bytes(key, 4096)
    require(hashlib.sha256(key_bytes).hexdigest() == installed["key_sha256"], "Pinned update public key changed")
    data = regular_bytes(manifest_path, 16384)
    signature = regular_bytes(signature_path, 64)
    require(len(signature) == 64, "Ed25519 signature must be exactly 64 bytes")
    with tempfile.TemporaryDirectory(prefix="screenshots-signature-") as temporary:
        temporary = Path(temporary)
        pinned, content, detached = temporary / "key.pem", temporary / "manifest.json", temporary / "manifest.sig"
        pinned.write_bytes(key_bytes)
        content.write_bytes(data)
        detached.write_bytes(signature)
        openssl(["pkeyutl", "-verify", "-pubin", "-inkey", pinned, "-rawin", "-in", content, "-sigfile", detached])
    return json.loads(data, object_pairs_hook=pairs)


def validate_update_manifest(root, installed, manifest):
    require(set(manifest) == {"version", "target", "url", "sha256", "size"}, "Unknown or missing signed update fields")
    version(manifest["version"])
    require(manifest["target"] == installed["target"], "Signed update target does not match installation")
    current = verify_version(root, installed["current"])
    require(version(manifest["version"]) > version(current["version"]), "Signed update is not newer than installed version")
    url(manifest["url"])
    require(isinstance(manifest["sha256"], str) and re.fullmatch(r"[0-9a-f]{64}", manifest["sha256"]), "Invalid signed artifact digest")
    require(type(manifest["size"]) is int and 1 <= manifest["size"] <= MAX_ARCHIVE, "Invalid signed artifact size")


@locked()
def update(root_value, manifest_url):
    root = root_path(root_value)
    require(transaction(root) is None, "An interrupted operation requires explicit recovery")
    installed = state(root)
    require(installed["key_sha256"] is not None, "No update public key is pinned")
    require(not urllib.parse.urlsplit(url(manifest_url)).query, "Update manifest URL must not contain a query")
    with tempfile.TemporaryDirectory(prefix="screenshots-update-") as temporary:
        temporary = Path(temporary)
        manifest, signature = temporary / "manifest.json", temporary / "manifest.sig"
        download(manifest_url, manifest, 16384)
        download(url(manifest_url) + ".sig", signature, 64, 64)
        # Authenticate before trusting the artifact URL or allocating the artifact download.
        data = authenticated_manifest(root, installed, manifest, signature)
        validate_update_manifest(root, installed, data)
        artifact = temporary / ("release.zip" if str(data["url"]).lower().endswith(".zip") else "release.tar.gz")
        download(data["url"], artifact, MAX_ARCHIVE, data["size"])
        return apply_signed(root, manifest, signature, artifact)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="action", required=True)
    command = commands.add_parser("install"); command.add_argument("package"); command.add_argument("root")
    for name in ["status", "rollback", "uninstall"]:
        command = commands.add_parser(name); command.add_argument("root")
    command = commands.add_parser("pin-key"); command.add_argument("root"); command.add_argument("public_key")
    command = commands.add_parser("recover"); command.add_argument("root"); command.add_argument("choice", choices=["current", "previous"])
    command = commands.add_parser("update"); command.add_argument("root"); command.add_argument("manifest_url")
    command = commands.add_parser("apply-signed"); command.add_argument("root"); command.add_argument("manifest"); command.add_argument("signature"); command.add_argument("artifact")
    args = parser.parse_args()
    try:
        if args.action == "install": result = install(args.package, args.root)
        elif args.action == "status": result = status(args.root)
        elif args.action == "rollback": result = rollback(args.root)
        elif args.action == "uninstall": result = uninstall(args.root)
        elif args.action == "pin-key": result = pin_key(args.root, args.public_key)
        elif args.action == "recover": result = recover(args.root, args.choice)
        elif args.action == "update": result = update(args.root, args.manifest_url)
        else: result = apply_signed(args.root, args.manifest, args.signature, args.artifact)
        print(json.dumps(result))
    except Exception:
        # Paths, signed URLs and server response payloads are not diagnostic text.
        print("Installation/update completion was not confirmed; completed changes may remain. Inspect status before retrying.", file=sys.stderr)
        raise SystemExit(1)


if __name__ == "__main__":
    main()
