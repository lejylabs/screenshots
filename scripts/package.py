#!/usr/bin/env python3
"""Build a native portable archive without installing or publishing it."""

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
import zipfile


ROOT = Path(__file__).resolve().parents[1]


def command(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True, encoding="utf-8").strip()


def write_text(path, text):
    path.write_text(text, encoding="utf-8", newline="\n")


def license_files(item):
    source = Path(item["manifest_path"]).parent
    files = [path for path in source.iterdir() if path.is_file() and path.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "PATENTS"))]
    if item.get("license_file"):
        files.append(source / item["license_file"])
    if item["name"] in {"accesskit_unix", "accesskit_atspi_common"}:
        # Their published crates omit the workspace licenses restored in vendor/.
        for name in ["LICENSE-MIT", "LICENSE-APACHE"]:
            path = source / name
            if not path.is_file() or path.is_symlink():
                raise ValueError("Vendored AccessKit license text is missing")
            files.append(path)
    if item["name"] == "libwebp-sys":
        # The statically compiled C library's notices live below the Rust wrapper.
        for name in ["COPYING", "PATENTS"]:
            path = source / "vendor" / name
            if not path.is_file() or path.is_symlink():
                raise ValueError("Bundled libwebp license or patent notice is missing")
            files.append(path)
    return sorted(set(map(str, files)))


def bounded_license(path):
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_NOFOLLOW", 0))
    with os.fdopen(descriptor, "rb") as source:
        metadata, named = os.fstat(source.fileno()), path.lstat()
        if (not stat.S_ISREG(metadata.st_mode) or not stat.S_ISREG(named.st_mode)
                or (metadata.st_dev, metadata.st_ino) != (named.st_dev, named.st_ino)
                or not 1 <= metadata.st_size <= 1024 * 1024):
            raise ValueError("Native dependency notice must be a regular file of at most 1 MiB")
        content = source.read(metadata.st_size)
        if len(content) != metadata.st_size:
            raise ValueError("Native dependency notice was truncated")
        return content


def native_licenses(target):
    if "windows" in target:
        expected = "x64-windows-static-md"
    elif target == "aarch64-apple-darwin":
        expected = "arm64-osx"
    elif target == "x86_64-apple-darwin":
        expected = "x64-osx"
    else:
        return []
    installed = os.environ.get("VCPKG_INSTALLED_ROOT")
    if not installed or not Path(installed).is_absolute() or os.environ.get("VCPKGRS_TRIPLET") != expected:
        raise ValueError("Native packaging requires the pinned static vcpkg installation and target triplet")
    baseline = json.loads((ROOT / "packaging/windows/vcpkg.json").read_text(encoding="utf-8"))["builtin-baseline"]
    result = []
    for name, license in [("curl", "curl"), ("libssh2", "BSD-3-Clause"), ("openssl", "Apache-2.0"), ("zlib", "Zlib")]:
        path = Path(installed) / expected / "share" / name / "copyright"
        bounded_license(path)
        result.append({"name": "native-" + name, "version": "vcpkg-" + baseline,
                       "license": license, "source": "vcpkg baseline " + baseline,
                       "license_files": [str(path)], "bounded_native_notice": True})
    return result


def runtime_bundle(source, destination, target):
    """Copy only the frozen manifest's bounded regular members; preserve executable modes."""
    identifiers = {"x86_64-unknown-linux-gnu": "linux-x64", "aarch64-unknown-linux-gnu": "linux-arm64",
                   "x86_64-pc-windows-msvc": "win-x64", "aarch64-apple-darwin": "osx-arm64", "x86_64-apple-darwin": "osx-x64"}
    if source.is_symlink() or not source.is_dir():
        raise ValueError("The packaged SXCU runtime directory is required")
    def regular(path, limit):
        descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_NOFOLLOW", 0))
        stream = os.fdopen(descriptor, "rb")
        metadata, named = os.fstat(stream.fileno()), path.lstat()
        if (not stat.S_ISREG(metadata.st_mode) or not stat.S_ISREG(named.st_mode)
                or (metadata.st_dev, metadata.st_ino) != (named.st_dev, named.st_ino) or metadata.st_size > limit):
            stream.close()
            raise ValueError("Runtime member is not a bounded regular file")
        return stream, metadata.st_size
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError("Duplicate runtime manifest field")
            result[key] = value
        return result
    manifest_path = source / "RUNTIME-MANIFEST.json"
    with regular(manifest_path, 128 * 1024)[0] as stream:
        raw = stream.read(128 * 1024 + 1)
    if len(raw) > 128 * 1024:
        raise ValueError("Runtime manifest exceeds 128 KiB")
    manifest = json.loads(raw, object_pairs_hook=pairs)
    if (not isinstance(manifest, dict) or set(manifest) != {"version", "protocol", "rid", "sdk", "runtime", "entrypoint", "files"}
            or type(manifest["version"]) is not int or manifest["version"] != 1
            or type(manifest["protocol"]) is not int or manifest["protocol"] != 1
            or manifest["sdk"] != "10.0.404" or manifest["runtime"] != "10.0.15"
            or manifest["entrypoint"] != ("screenshots-sxcu.exe" if "windows" in target else "screenshots-sxcu")
            or manifest["rid"] != identifiers.get(target)
            or not isinstance(manifest["files"], list) or not 1 <= len(manifest["files"]) <= 512):
        raise ValueError("Runtime manifest does not match the native package")
    expected, executables, total = set(), set(), 0
    members = []
    for member in manifest["files"]:
        if not isinstance(member, dict) or set(member) != {"path", "bytes", "sha256", "executable"}:
            raise ValueError("Invalid runtime member declaration")
        name = member["path"]
        if not isinstance(name, str):
            raise ValueError("Runtime member path must be a string")
        path = PurePosixPath(name)
        if (not isinstance(name, str) or path.is_absolute() or path.as_posix() != name or len(name) > 4096
                or not 1 <= len(path.parts) <= 23
                or any(part in ("", ".", "..") or "\\" in part or ":" in part or any(ord(c) < 32 for c in part) for part in path.parts)
                or name in expected or name == "RUNTIME-MANIFEST.json"
                or type(member["bytes"]) is not int or not 0 <= member["bytes"] <= 64 * 1024 * 1024
                or type(member["executable"]) is not bool or not isinstance(member["sha256"], str)
                or not re.fullmatch(r"[0-9a-f]{64}", member["sha256"])):
            raise ValueError("Invalid or duplicate runtime member")
        expected.add(name)
        total += member["bytes"]
        members.append((path, member))
        if member["executable"]:
            executables.add(destination / path)
    if total > 256 * 1024 * 1024 or not isinstance(manifest["entrypoint"], str) or destination / manifest["entrypoint"] not in executables:
        raise ValueError("Runtime byte budget or executable entrypoint is invalid")
    directories = {parent.as_posix() for relative, _ in members for parent in relative.parents if parent.as_posix() != "."}
    actual = set()
    allowed_files = expected | {"RUNTIME-MANIFEST.json"}
    def scan(folder):
        # Iterate entries without materializing an arbitrarily wide directory.
        with os.scandir(folder) as entries:
            for entry in entries:
                path = Path(entry.path)
                relative = path.relative_to(source).as_posix()
                metadata = entry.stat(follow_symlinks=False)
                if (stat.S_ISLNK(metadata.st_mode)
                        or getattr(metadata,"st_file_attributes",0) & getattr(stat,"FILE_ATTRIBUTE_REPARSE_POINT",0x400)):
                    raise ValueError("Runtime contains a symlink or junction")
                if entry.is_dir(follow_symlinks=False):
                    if relative not in directories:
                        raise ValueError("Runtime contains an undeclared directory")
                    scan(path)
                elif entry.is_file(follow_symlinks=False):
                    if relative not in allowed_files or len(actual) >= len(allowed_files):
                        raise ValueError("Runtime contains an undeclared or excessive member")
                    actual.add(relative)
                else:
                    raise ValueError("Runtime contains a special file")
    scan(source)
    if actual != expected | {"RUNTIME-MANIFEST.json"}:
        raise ValueError("Runtime contains undeclared or missing files")
    destination.mkdir(parents=True)
    for relative, member in members:
        output = destination / relative
        output.parent.mkdir(parents=True, exist_ok=True)
        digest = hashlib.sha256()
        with regular(source / relative, 64 * 1024 * 1024)[0] as stream, output.open("xb") as copied:
            remaining = member["bytes"]
            while remaining:
                chunk = stream.read(min(remaining, 65536))
                if not chunk:
                    raise ValueError("Runtime member was truncated")
                copied.write(chunk)
                digest.update(chunk)
                remaining -= len(chunk)
            if stream.read(1) or digest.hexdigest() != member["sha256"]:
                raise ValueError("Runtime member changed or failed verification")
        output.chmod(0o755 if output in executables else 0o644)
    (destination / "RUNTIME-MANIFEST.json").write_bytes(raw)
    return executables, {key: manifest[key] for key in ("protocol", "rid", "sdk", "runtime")} | {"manifest_sha256": hashlib.sha256(raw).hexdigest()}



def linux_abi(binary):
    """Record the executable's linked glibc requirement rather than guessing its build host."""
    with tempfile.TemporaryFile() as output:
        subprocess.run(["readelf", "--version-info", "--wide", str(binary)],
                       stdout=output, stderr=subprocess.PIPE, check=True, timeout=30)
        if output.tell() > 8 * 1024 * 1024:
            raise ValueError("ELF version metadata exceeds 8 MiB")
        output.seek(0)
        versions = re.findall(rb"\bGLIBC_(\d+)\.(\d+)(?:\.(\d+))?\b", output.read())
    if not versions:
        raise ValueError("Native Linux executable has no declared glibc ABI")
    minimum = max(tuple(int(value or b"0") for value in version) for version in versions)
    return {"glibc": ".".join(map(str, minimum[:3 if minimum[2] else 2]))}


def package(binary, output, name, target, epoch, info, licenses, runtime=None, *, verify_native_abi=False):
    """Stage complete files, then publish without replacing an existing archive."""
    if not binary.is_file() or binary.is_symlink():
        raise ValueError("Package input must be a regular binary, not a symlink")
    output.mkdir(parents=True, exist_ok=True)
    windows = "windows" in target
    filename = name + (".zip" if windows else ".tar.gz")
    destination = output / filename
    checksum = output / (filename + ".sha256")
    if destination.exists() or checksum.exists():
        raise FileExistsError("Package or checksum already exists")
    with tempfile.TemporaryDirectory(prefix=".package-", dir=output) as temporary:
        staging = Path(temporary)
        folder = staging / name
        folder.mkdir()
        executable = folder / ("screenshots.exe" if windows else "screenshots")
        descriptor = os.open(binary, os.O_RDONLY | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_NOFOLLOW", 0))
        digest = hashlib.sha256()
        with os.fdopen(descriptor, "rb") as source, executable.open("xb") as copied:
            admitted = os.fstat(source.fileno())
            if not stat.S_ISREG(admitted.st_mode) or not 0 < admitted.st_size <= 512 * 1024 * 1024:
                raise ValueError("Package executable must be a regular file of at most 512 MiB")
            remaining = admitted.st_size
            while remaining:
                chunk = source.read(min(remaining, 65536))
                if not chunk:
                    raise ValueError("Package executable was truncated during copying")
                copied.write(chunk)
                digest.update(chunk)
                remaining -= len(chunk)
            current = os.fstat(source.fileno())
            if source.read(1) or (admitted.st_size,admitted.st_mtime_ns,admitted.st_ctime_ns) != (current.st_size,current.st_mtime_ns,current.st_ctime_ns):
                raise ValueError("Package executable changed during copying")
        executable.chmod(0o755)
        if verify_native_abi and "linux" in target:
            # Inspect the frozen staged executable, not a concurrently rebuilt input.
            info = dict(info, linux_abi=linux_abi(executable))
        executables = {executable}
        if runtime is not None:
            entries, runtime_info = runtime_bundle(runtime, folder / "lib/screenshots-sxcu", target)
            executables.update(entries)
            info = dict(info, sharex_runtime=runtime_info)
        info = dict(info, binary_sha256=digest.hexdigest())
        write_text(folder / "BUILD-INFO.json", json.dumps(info, indent=2) + "\n")
        for source in ["LICENSE", "THIRD_PARTY_NOTICES.md", "README.md", "Cargo.lock"]:
            shutil.copyfile(ROOT / source, folder / source)
        write_text(folder / "PACKAGING.md", (ROOT / "docs/packaging.md").read_text(encoding="utf-8").replace("](desktop-integration.md)", "](DESKTOP-INTEGRATION.md)").replace("](native-signing.md)", "](NATIVE-SIGNING.md)"))
        shutil.copyfile(ROOT / "docs/native-signing.md", folder / "NATIVE-SIGNING.md")
        shutil.copyfile(ROOT / "scripts/install-user.py", folder / "install-user.py")
        shutil.copyfile(ROOT / "docs/desktop-integration.md", folder / "DESKTOP-INTEGRATION.md")
        if "linux" in target:
            shutil.copyfile(ROOT / "packaging/screenshots.desktop", folder / "screenshots.desktop")
        license_dir = folder / "licenses"
        license_dir.mkdir()
        write_text(license_dir / "dependencies.json", json.dumps([
            {key: value for key, value in item.items() if key not in ("license_files", "bounded_native_notice")}
            for item in licenses
        ], indent=2) + "\n")
        for item in licenses:
            for source in item.get("license_files", []):
                # Package roots are Cargo metadata, never user image or config paths.
                source = Path(source)
                relative = license_dir / (item["name"] + "-" + item["version"]) / source.name
                relative.parent.mkdir(exist_ok=True)
                if item.get("bounded_native_notice"):
                    relative.write_bytes(bounded_license(source))
                else:
                    shutil.copyfile(source, relative)
        archive = staging / filename
        if windows:
            # ZIP timestamps cannot represent dates before 1980.
            import time
            date = time.gmtime(max(epoch, 315532800))[:6]
            with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as stream:
                for path in sorted(folder.rglob("*")):
                    if path.is_file():
                        entry = zipfile.ZipInfo(path.relative_to(staging).as_posix(), date)
                        entry.external_attr = (0o755 if path in executables else 0o644) << 16
                        entry.compress_type = zipfile.ZIP_DEFLATED
                        with path.open("rb") as source, stream.open(entry, "w") as archived:
                            shutil.copyfileobj(source, archived, 65536)
        else:
            with archive.open("wb") as raw, gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=epoch) as zipped:
                with tarfile.open(fileobj=zipped, mode="w") as stream:
                    for path in sorted(folder.rglob("*")):
                        if path.is_file():
                            entry = stream.gettarinfo(path, path.relative_to(staging).as_posix())
                            entry.uid = entry.gid = 0
                            entry.uname = entry.gname = ""
                            entry.mtime = epoch
                            entry.mode = 0o755 if path in executables else 0o644
                            with path.open("rb") as content:
                                stream.addfile(entry, content)
        with archive.open("rb") as content:
            digest = hashlib.file_digest(content, "sha256").hexdigest()
        staged_checksum = staging / checksum.name
        write_text(staged_checksum, digest + "  " + filename + "\n")
        # Hard links provide atomic no-replace publication on the destination filesystem.
        os.link(archive, destination)
        try:
            os.link(staged_checksum, checksum)
        except BaseException as error:
            # A shared-name unlink can delete a concurrent replacement. Keep
            # any already linked archive for review instead of destructive rollback.
            error.add_note('Archive publication may be incomplete; the output directory was preserved. Use a new directory before retrying.')
            raise
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, help="Native Rust target triple")
    parser.add_argument("--binary", type=Path, help="Package this already-built binary instead of building")
    parser.add_argument("--sharex-runtime", type=Path, help="Pinned native helper bundle; defaults beside the executable")
    parser.add_argument("--output", type=Path, default=ROOT / "target/packages")
    args = parser.parse_args()
    host = next(line.split(": ", 1)[1] for line in command("rustc", "-vV").splitlines() if line.startswith("host: "))
    if args.target != host:
        parser.error("Build and package on the native target; cross-built archives need a separate runtime check")
    if args.binary is None:
        subprocess.run(["cargo", "build", "--locked", "--release", "-p", "screenshots"], cwd=ROOT, check=True)
    binary = args.binary or ROOT / "target/release" / ("screenshots.exe" if "windows" in host else "screenshots")
    runtime = args.sharex_runtime or binary.parent / "lib/screenshots-sxcu"
    if runtime.is_symlink() or not runtime.is_dir():
        parser.error("The native private SXCU runtime is required; build scripts/build-sharex-helper.py first")
    metadata = json.loads(command("cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", host))
    version = next(item["version"] for item in metadata["packages"] if item["name"] == "screenshots")
    licenses = []
    for item in sorted(metadata["packages"], key=lambda item: (item["name"], item["version"])):
        licenses.append({"name": item["name"], "version": item["version"], "license": item["license"], "source": item["source"], "license_files": license_files(item)})
    licenses.extend(native_licenses(host))
    commit = command("git", "rev-parse", "HEAD")
    epoch = int(os.environ.get("SOURCE_DATE_EPOCH", command("git", "show", "-s", "--format=%ct", "HEAD")))
    info = {"version": version, "target": host, "commit": commit, "dirty": bool(command("git", "status", "--porcelain")), "rustc": command("rustc", "-vV"), "source_date_epoch": epoch, "binary_input": "explicit prebuilt binary" if args.binary else "cargo build --locked --release", "signing": "unsigned; native runtime acceptance is separate"}
    print(package(binary, args.output, f"screenshots-{version}-{host}", host, epoch, info, licenses, runtime, verify_native_abi=True))


if __name__ == "__main__":
    main()
