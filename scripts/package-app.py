#!/usr/bin/env python3
"""Build native application bundles/installers from a verified portable archive.

macOS: .app plus .pkg when pkgbuild is available. Windows: unsigned .msix via
the installed Windows SDK MakeAppx. Signing/notarization requires real identity.
"""

import argparse
import gzip
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import zlib

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("install_user", ROOT / "scripts/install-user.py")
INSTALL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(INSTALL)
ID = "me.lejy.screenshots"


def info_plist(version):
    INSTALL.version(version)
    return {"CFBundleIdentifier": ID, "CFBundleName": "Screenshots", "CFBundleDisplayName": "Screenshots",
            "CFBundleExecutable": "screenshots", "CFBundlePackageType": "APPL", "CFBundleShortVersionString": version,
            "CFBundleVersion": version, "NSHighResolutionCapable": True, "LSMinimumSystemVersion": "12.0",
            "CFBundleDocumentTypes": [{"CFBundleTypeName": "Image", "CFBundleTypeRole": "Viewer", "LSHandlerRank": "Alternate",
                                        "LSItemContentTypes": ["public.png", "public.jpeg", "org.webmproject.webp", "com.microsoft.bmp", "public.tiff"]}]}


def app_bundle(portable, app, info):
    INSTALL.require(not app.exists() and not app.is_symlink(), "Application bundle destination exists")
    executable = portable / "screenshots"
    INSTALL.require(executable.is_file(), "Portable macOS executable is missing")
    binary = app / "Contents/MacOS"
    resources = app / "Contents/Resources"
    binary.mkdir(parents=True)
    resources.mkdir()
    shutil.copyfile(executable, binary / "screenshots")
    (binary / "screenshots").chmod(0o755)
    for path in portable.iterdir():
        if path.name == "screenshots":
            continue
        INSTALL.require(not path.is_symlink(), "Portable bundle contains a symlink")
        if path.is_dir():
            shutil.copytree(path, resources / path.name)
        else:
            shutil.copyfile(path, resources / path.name)
    with (app / "Contents/Info.plist").open("xb") as output:
        plistlib.dump(info_plist(info["version"]), output, sort_keys=True)
    (app / "Contents/PkgInfo").write_bytes(b"APPL????")


def png(path, side):
    # Small deterministic installer asset, without introducing an image dependency.
    raw = bytearray()
    for y in range(side):
        raw.append(0)
        for x in range(side):
            border = side // 6 <= x < 5 * side // 6 and side // 4 <= y < 3 * side // 4
            lens = (x - side // 2) ** 2 + (y - side // 2) ** 2 < (side // 7) ** 2
            raw.extend((235, 239, 245, 255) if border and not lens else (35, 45, 65, 255))
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xffffffff)
    path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", side, side, 8, 6, 0, 0, 0))
                     + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b""))


def appx_manifest(version, architecture):
    INSTALL.version(version)
    INSTALL.require(architecture in ("x64", "arm64"), "Unsupported MSIX architecture")
    return f'''<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
 xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
 xmlns:uap3="http://schemas.microsoft.com/appx/manifest/uap/windows10/3"
 xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities"
 IgnorableNamespaces="uap uap3 rescap">
 <Identity Name="me.lejy.screenshots" Publisher="CN=Screenshots" Version="{version}.0" ProcessorArchitecture="{architecture}"/>
 <Properties><DisplayName>Screenshots</DisplayName><PublisherDisplayName>Screenshots</PublisherDisplayName><Logo>Assets\\StoreLogo.png</Logo></Properties>
 <Resources><Resource Language="en-us"/></Resources>
 <Dependencies><TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.19041.0" MaxVersionTested="10.0.26100.0"/></Dependencies>
 <Applications><Application Id="Screenshots" Executable="screenshots.exe" EntryPoint="Windows.FullTrustApplication">
  <uap:VisualElements DisplayName="Screenshots" Description="Capture and annotate screenshots" BackgroundColor="transparent" Square150x150Logo="Assets\\Square150.png" Square44x44Logo="Assets\\Square44.png"/>
  <Extensions><uap:Extension Category="windows.fileTypeAssociation"><uap3:FileTypeAssociation Name="screenshotsimage" Parameters="--open &quot;%1&quot;">
   <uap:SupportedFileTypes><uap:FileType>.png</uap:FileType><uap:FileType>.jpg</uap:FileType><uap:FileType>.jpeg</uap:FileType><uap:FileType>.webp</uap:FileType><uap:FileType>.bmp</uap:FileType><uap:FileType>.tif</uap:FileType><uap:FileType>.tiff</uap:FileType></uap:SupportedFileTypes>
  </uap3:FileTypeAssociation></uap:Extension></Extensions>
 </Application></Applications>
 <Capabilities><rescap:Capability Name="runFullTrust"/></Capabilities>
</Package>
'''


def sdk_makeappx():
    found = shutil.which("makeappx.exe")
    if found:
        return found
    base = Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")) / "Windows Kits/10/bin"
    paths = sorted(base.glob("*/x64/makeappx.exe"), reverse=True)
    INSTALL.require(paths, "Windows SDK MakeAppx is unavailable")
    return str(paths[0])


def tar_gzip(directory, output, epoch):
    with output.open("wb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", filename="", mtime=epoch) as zipped:
        with tarfile.open(fileobj=zipped, mode="w") as stream:
            for file in sorted(directory.rglob("*")):
                if not file.is_file():
                    continue
                item = stream.gettarinfo(file, file.relative_to(directory).as_posix())
                item.uid = item.gid = 0
                item.uname = item.gname = ""
                item.mtime = epoch
                item.mode = 0o755 if file.name == "screenshots" and file.parent.name in ("MacOS", "bin") else 0o644
                with file.open("rb") as content:
                    stream.addfile(item, content)


def deb_package(portable, output, info):
    architecture = {"x86_64": "amd64", "aarch64": "arm64"}.get(info["target"].split("-")[0])
    INSTALL.require(architecture is not None, "Unsupported Debian architecture")
    epoch = int(info.get("source_date_epoch", 0))
    with tempfile.TemporaryDirectory(prefix=".deb-", dir=output.parent) as temporary:
        temporary = Path(temporary)
        data, control = temporary / "data", temporary / "control"
        (data / "usr/bin").mkdir(parents=True)
        docs = data / "usr/share/doc/screenshots"
        docs.mkdir(parents=True)
        (data / "usr/share/applications").mkdir(parents=True)
        shutil.copyfile(portable / "screenshots", data / "usr/bin/screenshots")
        (data / "usr/bin/screenshots").chmod(0o755)
        for source in portable.iterdir():
            if source.name == "screenshots":
                continue
            if source.is_dir():
                shutil.copytree(source, docs / source.name)
            else:
                shutil.copyfile(source, docs / source.name)
        (data / "usr/share/applications/me.lejy.screenshots.desktop").write_text(
            "[Desktop Entry]\nType=Application\nVersion=1.0\nName=Screenshots\nExec=/usr/bin/screenshots --open %f\nTerminal=false\nCategories=Graphics;Utility;\nMimeType=image/png;image/jpeg;image/webp;image/bmp;image/tiff;\n")
        control.mkdir()
        (control / "control").write_text(
            f"Package: screenshots\nVersion: {info['version']}\nArchitecture: {architecture}\n"
            "Maintainer: Screenshots contributors <noreply@github.com>\nSection: graphics\nPriority: optional\n"
            "Depends: libc6, libgcc-s1, libxkbcommon0, libxkbcommon-x11-0, libxcb1, libdbus-1-3, libgbm1, libfontconfig1, libfreetype6\n"
            "Homepage: https://github.com/lejylabs/screenshots\nDescription: Native screenshot capture and sharing application\n")
        (temporary / "debian-binary").write_bytes(b"2.0\n")
        tar_gzip(control, temporary / "control.tar.gz", epoch)
        tar_gzip(data, temporary / "data.tar.gz", epoch)
        with output.open("xb") as archive:
            archive.write(b"!<arch>\n")
            for name in ["debian-binary", "control.tar.gz", "data.tar.gz"]:
                source = temporary / name
                size = source.stat().st_size
                header = f"{name + '/':<16}{epoch:<12}{0:<6}{0:<6}{'100644':<8}{size:<10}`\n".encode("ascii")
                INSTALL.require(len(header) == 60, "Debian ar header exceeds its field widths")
                archive.write(header)
                with source.open("rb") as content:
                    shutil.copyfileobj(content, archive, 65536)
                if size % 2:
                    archive.write(b"\n")


def publish_file(source, destination):
    checksum = destination.with_name(destination.name + ".sha256")
    INSTALL.require(not destination.exists() and not destination.is_symlink() and not checksum.exists() and not checksum.is_symlink(), "Package destination exists")
    os.link(source, destination)
    try:
        with checksum.open("x") as output:
            output.write(INSTALL.sha(destination) + "  " + destination.name + "\n")
    except BaseException:
        destination.unlink()
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    INSTALL.require(args.package.is_file() and not args.package.is_symlink() and args.package.stat().st_size <= INSTALL.MAX_ARCHIVE,
                    "Portable package must be a bounded regular archive")
    digest = INSTALL.sha(args.package)
    sidecar = args.package.with_name(args.package.name + ".sha256")
    INSTALL.require(INSTALL.regular_bytes(sidecar, 1024).decode().strip() == digest + "  " + args.package.name, "Portable archive checksum mismatch")
    with tempfile.TemporaryDirectory(prefix=".native-package-", dir=args.output) as temporary:
        temporary = Path(temporary)
        snapshot = temporary / ("input.zip" if args.package.suffix.lower() == ".zip" else "input.tar.gz")
        descriptor = os.open(args.package, os.O_RDONLY | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_NOFOLLOW", 0))
        with os.fdopen(descriptor, "rb") as source, snapshot.open("xb") as output:
            size = os.fstat(source.fileno()).st_size
            INSTALL.require(size <= INSTALL.MAX_ARCHIVE, "Portable package changed before opening")
            remaining = size
            while remaining:
                data = source.read(min(65536, remaining))
                INSTALL.require(data, "Portable package was truncated")
                remaining -= len(data)
                output.write(data)
        INSTALL.require(INSTALL.sha(snapshot) == digest, "Portable package changed before authenticated parsing")
        portable = temporary / "portable"
        portable.mkdir()
        info, _, _ = INSTALL.extract(snapshot, portable)
        name = f"screenshots-{info['version']}-{info['target']}"
        if sys.platform == "darwin":
            app = temporary / "Screenshots.app"
            app_bundle(portable, app, info)
            archive = temporary / (name + ".app.tar.gz")
            epoch = int(info.get("source_date_epoch", 0))
            with archive.open("wb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", filename="", mtime=epoch) as zipped:
                with tarfile.open(fileobj=zipped, mode="w") as stream:
                    for file in sorted(app.rglob("*")):
                        if not file.is_file():
                            continue
                        item = stream.gettarinfo(file, (Path("Screenshots.app") / file.relative_to(app)).as_posix())
                        item.uid = item.gid = 0
                        item.uname = item.gname = ""
                        item.mtime = epoch
                        item.mode = 0o755 if file.name == "screenshots" and file.parent.name == "MacOS" else 0o644
                        with file.open("rb") as content:
                            stream.addfile(item, content)
            publish_file(archive, args.output / archive.name)
            if shutil.which("pkgbuild"):
                package = temporary / (name + ".pkg")
                subprocess.run(["pkgbuild", "--component", str(app), "--install-location", "/Applications", "--identifier", ID,
                                "--version", info["version"], str(package)], check=True, timeout=120,
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                publish_file(package, args.output / package.name)
            print(json.dumps({"status": "packaged", "kind": "macOS app/pkg", "signing": "unsigned; no notarization claimed"}))
        elif os.name == "nt":
            assets = portable / "Assets"
            assets.mkdir()
            for filename, side in [("StoreLogo.png", 50), ("Square150.png", 150), ("Square44.png", 44)]:
                png(assets / filename, side)
            (portable / "AppxManifest.xml").write_text(appx_manifest(info["version"], "arm64" if info["target"].startswith("aarch64") else "x64"))
            package = temporary / (name + ".msix")
            subprocess.run([sdk_makeappx(), "pack", "/d", str(portable), "/p", str(package), "/o"], check=True, timeout=120,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            publish_file(package, args.output / package.name)
            print(json.dumps({"status": "packaged", "kind": "MSIX", "signing": "unsigned; production installation requires trusted signature"}))
        elif sys.platform.startswith("linux"):
            package = temporary / (name + ".deb")
            deb_package(portable, package, info)
            publish_file(package, args.output / package.name)
            print(json.dumps({"status": "packaged", "kind": "Debian", "signing": "unsigned development package"}))
        else:
            raise ValueError("Native application packaging is unavailable on this platform")


if __name__ == "__main__":
    main()
