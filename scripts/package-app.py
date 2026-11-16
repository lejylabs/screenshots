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
import re
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import zlib
from xml.sax.saxutils import quoteattr

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("install_user", ROOT / "scripts/install-user.py")
INSTALL = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(INSTALL)
ID = "me.lejy.screenshots"
SIGN_SPEC = importlib.util.spec_from_file_location("native_signing", ROOT / "scripts/native-signing.py")
SIGN = importlib.util.module_from_spec(SIGN_SPEC)
SIGN_SPEC.loader.exec_module(SIGN)


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


def appx_manifest(version, architecture, publisher="CN=Screenshots"):
    INSTALL.version(version)
    INSTALL.require(architecture in ("x64", "arm64"), "Unsupported MSIX architecture")
    SIGN.text(publisher,1024)
    publisher = quoteattr(publisher)
    return f'''<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
 xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
 xmlns:uap3="http://schemas.microsoft.com/appx/manifest/uap/windows10/3"
 xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities"
 IgnorableNamespaces="uap uap3 rescap">
 <Identity Name="me.lejy.screenshots" Publisher={publisher} Version="{version}.0" ProcessorArchitecture="{architecture}"/>
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
                if file.is_symlink():
                    item = stream.gettarinfo(file, file.relative_to(directory).as_posix())
                    item.uid = item.gid = 0
                    item.uname = item.gname = ""
                    item.mtime = epoch
                    stream.addfile(item)
                    continue
                if not file.is_file():
                    continue
                item = stream.gettarinfo(file, file.relative_to(directory).as_posix())
                item.uid = item.gid = 0
                item.uname = item.gname = ""
                item.mtime = epoch
                item.mode = 0o755 if file.stat().st_mode & 0o111 else 0o644
                with file.open("rb") as content:
                    stream.addfile(item, content)


def deb_package(portable, output, info):
    architecture = {"x86_64": "amd64", "aarch64": "arm64"}.get(info["target"].split("-")[0])
    INSTALL.require(architecture is not None, "Unsupported Debian architecture")
    epoch = int(info.get("source_date_epoch", 0))
    abi = info.get("linux_abi", {}).get("glibc")
    INSTALL.require(isinstance(abi, str) and re.fullmatch(r"[0-9]+\.[0-9]+(?:\.[0-9]+)?", abi),
                    "Debian packaging requires the recorded native glibc ABI")
    with tempfile.TemporaryDirectory(prefix=".deb-", dir=output.parent) as temporary:
        temporary = Path(temporary)
        data, control = temporary / "data", temporary / "control"
        (data / "usr/bin").mkdir(parents=True)
        private = data / "usr/lib/screenshots"
        private.mkdir(parents=True)
        docs = data / "usr/share/doc/screenshots"
        docs.mkdir(parents=True)
        (data / "usr/share/applications").mkdir(parents=True)
        shutil.copyfile(portable / "screenshots", private / "screenshots")
        (private / "screenshots").chmod(0o755)
        (data / "usr/bin/screenshots").symlink_to("../lib/screenshots/screenshots")
        for source in portable.iterdir():
            if source.name == "screenshots":
                continue
            if source.name == "lib":
                shutil.copytree(source, private / source.name)
            elif source.name == "BUILD-INFO.json":
                shutil.copyfile(source, private / source.name)
            elif source.is_dir():
                shutil.copytree(source, docs / source.name)
            else:
                shutil.copyfile(source, docs / source.name)
        (data / "usr/share/applications/me.lejy.screenshots.desktop").write_text(
            "[Desktop Entry]\nType=Application\nVersion=1.0\nName=Screenshots\nExec=/usr/bin/screenshots --open %f\nTerminal=false\nCategories=Graphics;Utility;\nMimeType=image/png;image/jpeg;image/webp;image/bmp;image/tiff;\n")
        control.mkdir()
        (control / "control").write_text(
            f"Package: screenshots\nVersion: {info['version']}\nArchitecture: {architecture}\n"
            "Maintainer: Screenshots contributors <noreply@github.com>\nSection: graphics\nPriority: optional\n"
            f"Depends: libc6 (>= {abi}), libgcc-s1, libstdc++6, libcurl4t64 | libcurl4, libxkbcommon0, libxkbcommon-x11-0, libxcb1, libdbus-1-3, libgbm1, libfontconfig1, libfreetype6, ca-certificates, libssl3t64 | libssl3, zlib1g, libgssapi-krb5-2, libicu72 | libicu74 | libicu76 | libicu78\n"
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


def file_id(path):
    metadata=path.lstat()
    INSTALL.require(not path.is_symlink() and path.is_file(),"Published member changed type")
    return metadata.st_dev,metadata.st_ino


def remove_owned(path,identity):
    try:
        if file_id(path)==identity:path.unlink()
    except (FileNotFoundError,ValueError):pass


def publication_handle(path):
    if os.name!='nt':return path.open('rb')
    # Keep identity alive while permitting owned-link rollback and private staging cleanup.
    import ctypes
    import msvcrt
    kernel=ctypes.WinDLL('kernel32',use_last_error=True)
    create=kernel.CreateFileW;create.argtypes=[ctypes.c_wchar_p,ctypes.c_uint32,ctypes.c_uint32,ctypes.c_void_p,ctypes.c_uint32,ctypes.c_uint32,ctypes.c_void_p];create.restype=ctypes.c_void_p
    handle=create(str(path.resolve()),0x80000000,7,None,3,0x80,None)
    if handle==ctypes.c_void_p(-1).value:raise ctypes.WinError(ctypes.get_last_error())
    try:descriptor=msvcrt.open_osfhandle(handle,os.O_RDONLY|os.O_BINARY)
    except BaseException:
        kernel.CloseHandle.argtypes=[ctypes.c_void_p];kernel.CloseHandle(handle);raise
    return os.fdopen(descriptor,'rb')


class Publication:
    def __init__(self,artifact,checksum):
        self.artifact=artifact;self.checksum=checksum
        self.artifact_id=os.fstat(artifact.fileno()).st_dev,os.fstat(artifact.fileno()).st_ino
        self.checksum_id=os.fstat(checksum.fileno()).st_dev,os.fstat(checksum.fileno()).st_ino
    def close(self):
        self.artifact.close();self.checksum.close()
    def __del__(self):self.close()


def publish_file(source, destination):
    checksum=destination.with_name(destination.name+".sha256")
    admit_destinations([source],destination.parent)
    # Retained handles prevent inode/file-ID reuse while the group can roll back.
    artifact_handle=publication_handle(source);checksum_handle=None
    source_id=os.fstat(artifact_handle.fileno()).st_dev,os.fstat(artifact_handle.fileno()).st_ino
    try:
        with tempfile.TemporaryDirectory(prefix='.publish-checksum-',dir=source.parent) as temporary:
            staged=Path(temporary)/'checksum'
            with staged.open('x') as output:
                output.write(INSTALL.sha(source)+"  "+destination.name+"\n");output.flush();os.fsync(output.fileno())
            checksum_handle=publication_handle(staged)
            os.link(source,destination)
            try:os.link(staged,checksum)
            except BaseException:
                remove_owned(destination,source_id)
                raise
        return Publication(artifact_handle,checksum_handle)
    except BaseException:
        artifact_handle.close()
        if checksum_handle:checksum_handle.close()
        raise


def admit_destinations(sources,output):
    for source in sources:
        destination=output/source.name
        INSTALL.require(not destination.exists() and not destination.is_symlink()
                        and not destination.with_name(destination.name+".sha256").exists()
                        and not destination.with_name(destination.name+".sha256").is_symlink(),"Package destination exists")
def publish_all(sources,output):
    admit_destinations(sources,output)
    published=[]
    try:
        for source in sources:
            destination=output/source.name
            receipt=publish_file(source,destination);published.append((destination,receipt))
    except BaseException:
        for destination,receipt in reversed(published):
            remove_owned(destination.with_name(destination.name+".sha256"),receipt.checksum_id)
            remove_owned(destination,receipt.artifact_id)
        raise
    finally:
        for _,receipt in published:receipt.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--package", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--sign-profile",type=Path,help="Explicit public identity selectors; no keys/passwords")
    parser.add_argument("--notary-profile",help="Existing macOS notary keychain profile (requires signing)")
    args = parser.parse_args()
    platform_name="macos" if sys.platform=="darwin" else "windows" if os.name=="nt" else "linux"
    INSTALL.require(args.notary_profile is None or platform_name=="macos" and args.sign_profile is not None,
                    "Notarization requires an explicit macOS signing profile")
    signing=SIGN.profile(args.sign_profile,platform_name) if args.sign_profile else None
    if args.notary_profile is not None:SIGN.text(args.notary_profile,256)

    args.output.mkdir(parents=True, exist_ok=True)
    INSTALL.require(args.package.is_file() and not args.package.is_symlink() and args.package.stat().st_size <= INSTALL.MAX_ARCHIVE,
                    "Portable package must be a bounded regular archive")
    digest = INSTALL.sha(args.package)
    sidecar = args.package.with_name(args.package.name + ".sha256")
    INSTALL.require(INSTALL.regular_bytes(sidecar, 1024).decode().strip() == digest + "  " + args.package.name, "Portable archive checksum mismatch")
    with tempfile.TemporaryDirectory(prefix=".native-package-", dir=args.output) as temporary:
        temporary = Path(temporary).resolve()
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
        outputs=[]
        if sys.platform=='darwin':
            planned=[temporary/(name+'.app.tar.gz')]
            if signing or shutil.which('pkgbuild'):planned.append(temporary/(name+('.signed.pkg' if signing else '.pkg')))
        else:planned=[temporary/(name+('.msix' if os.name=='nt' else '.deb'))]
        admit_destinations(planned,args.output)
        if sys.platform == "darwin":
            if signing:SIGN.native_tool("pkgbuild")
            app = temporary / "Screenshots.app"
            app_bundle(portable, app, info)
            if signing:info=SIGN.mac_app(app,info,signing)
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
                        item.mode = 0o755 if file.stat().st_mode & 0o111 else 0o644
                        with file.open("rb") as content:
                            stream.addfile(item, content)
            outputs.append(archive)
            if signing or shutil.which("pkgbuild"):
                package = temporary / (name + ".pkg")
                build=["pkgbuild", "--component", str(app), "--install-location", "/Applications", "--identifier", ID,
                       "--version", info["version"], str(package)]
                if signing:SIGN.Tools().run(build)
                else:subprocess.run(build,check=True,timeout=120,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
                if signing:
                    signed=temporary/(name+".signed.pkg")
                    state=SIGN.mac_package(package,signed,signing,args.notary_profile)
                    package=signed
                outputs.append(package)
            publish_all(outputs,args.output)
            print(json.dumps({"status":"packaged","kind":"macOS app/pkg","signing":"native verified" if signing else "unsigned", "notarization":"accepted and stapled" if args.notary_profile else "not requested"}))
        elif os.name == "nt":
            if signing:info=SIGN.windows_payload(portable,info,signing)
            assets = portable / "Assets"
            assets.mkdir()
            for filename, side in [("StoreLogo.png", 50), ("Square150.png", 150), ("Square44.png", 44)]:
                png(assets / filename, side)
            (portable / "AppxManifest.xml").write_text(appx_manifest(info["version"], "arm64" if info["target"].startswith("aarch64") else "x64", signing["publisher"] if signing else "CN=Screenshots"))
            package = temporary / (name + ".msix")
            build=["makeappx.exe" if signing else sdk_makeappx(), "pack", "/d", str(portable), "/p", str(package), "/o"]
            if signing:SIGN.Tools().run(build)
            else:subprocess.run(build,check=True,timeout=120,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            if signing:SIGN.windows_package(package,signing)
            publish_all([package],args.output)
            print(json.dumps({"status":"packaged","kind":"MSIX","signing":"native verified" if signing else "unsigned; production installation requires trusted signature"}))
        elif sys.platform.startswith("linux"):
            package = temporary / (name + ".deb")
            deb_package(portable, package, info)
            publish_all([package],args.output)
            print(json.dumps({"status": "packaged", "kind": "Debian", "signing": "unsigned development package"}))
        else:
            raise ValueError("Native application packaging is unavailable on this platform")


if __name__ == "__main__":
    main()
