#!/usr/bin/env python3
"""Build a native portable archive without installing or publishing it."""

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import zipfile


ROOT = Path(__file__).resolve().parents[1]


def command(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True, encoding="utf-8").strip()


def write_text(path, text):
    path.write_text(text, encoding="utf-8", newline="\n")


def package(binary, output, name, target, epoch, info, licenses):
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
        shutil.copyfile(binary, executable)
        executable.chmod(0o755)
        with binary.open("rb") as content:
            info = dict(info, binary_sha256=hashlib.file_digest(content, "sha256").hexdigest())
        write_text(folder / "BUILD-INFO.json", json.dumps(info, indent=2) + "\n")
        for source in ["LICENSE", "THIRD_PARTY_NOTICES.md", "README.md", "Cargo.lock"]:
            shutil.copyfile(ROOT / source, folder / source)
        shutil.copyfile(ROOT / "docs/packaging.md", folder / "PACKAGING.md")
        if "linux" in target:
            shutil.copyfile(ROOT / "packaging/screenshots.desktop", folder / "screenshots.desktop")
        license_dir = folder / "licenses"
        license_dir.mkdir()
        write_text(license_dir / "dependencies.json", json.dumps([
            {key: value for key, value in item.items() if key != "license_files"}
            for item in licenses
        ], indent=2) + "\n")
        for item in licenses:
            for source in item.get("license_files", []):
                # Package roots are Cargo metadata, never user image or config paths.
                source = Path(source)
                relative = license_dir / (item["name"] + "-" + item["version"]) / source.name
                relative.parent.mkdir(exist_ok=True)
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
                        entry.external_attr = (0o755 if path == executable else 0o644) << 16
                        entry.compress_type = zipfile.ZIP_DEFLATED
                        stream.writestr(entry, path.read_bytes())
        else:
            with archive.open("wb") as raw, gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=epoch) as zipped:
                with tarfile.open(fileobj=zipped, mode="w") as stream:
                    for path in sorted(folder.rglob("*")):
                        if path.is_file():
                            entry = stream.gettarinfo(path, path.relative_to(staging).as_posix())
                            entry.uid = entry.gid = 0
                            entry.uname = entry.gname = ""
                            entry.mtime = epoch
                            entry.mode = 0o755 if path == executable else 0o644
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
        except BaseException:
            destination.unlink()
            raise
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, help="Native Rust target triple")
    parser.add_argument("--binary", type=Path, help="Package this already-built binary instead of building")
    parser.add_argument("--output", type=Path, default=ROOT / "target/packages")
    args = parser.parse_args()
    host = next(line.split(": ", 1)[1] for line in command("rustc", "-vV").splitlines() if line.startswith("host: "))
    if args.target != host:
        parser.error("Build and package on the native target; cross-built archives need a separate runtime check")
    if args.binary is None:
        subprocess.run(["cargo", "build", "--locked", "--release", "-p", "screenshots"], cwd=ROOT, check=True)
    binary = args.binary or ROOT / "target/release" / ("screenshots.exe" if "windows" in host else "screenshots")
    metadata = json.loads(command("cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", host))
    version = next(item["version"] for item in metadata["packages"] if item["name"] == "screenshots")
    licenses = []
    for item in sorted(metadata["packages"], key=lambda item: (item["name"], item["version"])):
        source = Path(item["manifest_path"]).parent
        files = [path for path in source.iterdir() if path.is_file() and path.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE"))]
        if item.get("license_file"):
            files.append(source / item["license_file"])
        licenses.append({"name": item["name"], "version": item["version"], "license": item["license"], "source": item["source"], "license_files": sorted(set(map(str, files)))})
    commit = command("git", "rev-parse", "HEAD")
    epoch = int(os.environ.get("SOURCE_DATE_EPOCH", command("git", "show", "-s", "--format=%ct", "HEAD")))
    info = {"version": version, "target": host, "commit": commit, "dirty": bool(command("git", "status", "--porcelain")), "rustc": command("rustc", "-vV"), "source_date_epoch": epoch, "binary_input": "explicit prebuilt binary" if args.binary else "cargo build --locked --release", "signing": "unsigned; native runtime acceptance is separate"}
    print(package(binary, args.output, f"screenshots-{version}-{host}", host, epoch, info, licenses))


if __name__ == "__main__":
    main()
