#!/usr/bin/env python3
"""Build the pinned private SXCU runtime into ignored target storage only."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import stat
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
SDK, RUNTIME = "10.0.404", "10.0.15"
RIDS = {"win-x64", "linux-x64", "linux-arm64", "osx-arm64", "osx-x64"}


def native_rid():
    system = {"Windows": "win", "Linux": "linux", "Darwin": "osx"}.get(platform.system())
    architecture = {"AMD64": "x64", "x86_64": "x64", "aarch64": "arm64", "arm64": "arm64"}.get(platform.machine())
    value = f"{system}-{architecture}"
    if value not in RIDS:
        raise RuntimeError("Unsupported native SXCU build target")
    return value


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def environment():
    env = dict(os.environ)
    # Build tools may run on CI SDKs; none of their mutable state belongs in a user profile.
    env.update(DOTNET_CLI_HOME=str(ROOT / "target/tools/dotnet-cli-home"),
               DOTNET_CLI_TELEMETRY_OPTOUT="1", DOTNET_SKIP_FIRST_TIME_EXPERIENCE="1",
               DOTNET_GENERATE_ASPNET_CERTIFICATE="false", DOTNET_ADD_GLOBAL_TOOLS_TO_PATH="false",
               DOTNET_CLI_WORKLOAD_UPDATE_NOTIFY_DISABLE="true", DOTNET_NOLOGO="1",
               NUGET_PACKAGES=str(ROOT / "target/tools/nuget"), MSBUILDDISABLENODEREUSE="1", DOTNET_CLI_USE_MSBUILD_SERVER="0")
    for key in list(env):
        if key.upper().startswith(("COMPLUS_", "CORECLR_", "COREHOST_", "COR_")) or key.upper() in ("DOTNET_STARTUP_HOOKS", "DOTNET_ADDITIONAL_DEPS", "DOTNET_SHARED_STORE"):
            env.pop(key)
    return env


def build(args):
    rid = args.rid or native_rid()
    if rid not in RIDS:
        raise RuntimeError("Unsupported SXCU runtime identifier")
    target = ROOT / "target"
    dotnet = args.dotnet or target / "tools/dotnet-10.0.404" / ("dotnet.exe" if os.name == "nt" else "dotnet")
    if not Path(dotnet).is_file():
        found = shutil.which("dotnet")
        if not found:
            raise RuntimeError("Install the pinned SDK into target/tools, or use a CI-provided SDK")
        dotnet = Path(found)
    dotnet = Path(dotnet).resolve()
    env = environment()
    project = ROOT / "tools/sharex-helper/ShareXHelper.csproj"
    version = subprocess.check_output([str(dotnet), "--version"], cwd=project.parent, env=env, text=True, timeout=30).strip()
    if version != SDK:
        raise RuntimeError(f"SXCU build requires the exact SDK {SDK}")
    obj = target / "sharex-helper/obj" / rid
    properties = [f"-p:BaseOutputPath={target / 'sharex-helper/build' / rid}{os.sep}", "-p:UseSharedCompilation=false", f"-p:BaseIntermediateOutputPath={obj}{os.sep}", f"-p:MSBuildProjectExtensionsPath={obj}{os.sep}"]
    restore = [str(dotnet), "restore", str(project), "--configfile", str(project.parent / "NuGet.Config"), "--disable-build-servers", *properties]
    restore.append("--force-evaluate" if args.update_lock else "--locked-mode")
    subprocess.run(restore, cwd=project.parent, env=env, check=True, timeout=300)
    output = (args.output or target / "sharex-helper/publish" / rid).resolve()
    if not output.is_relative_to(target.resolve()) or not (output == (target / "sharex-helper/publish" / rid).resolve() or output.name == "screenshots-sxcu" and output.parent.name == "lib"):
        raise RuntimeError("Helper build output must remain under ignored target")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".sharex-publish-", dir=output.parent) as temporary:
        staged = Path(temporary) / "runtime"
        command = [str(dotnet), "publish", str(project), "--no-restore", "--configuration", "Release", "--runtime", rid,
                   "--self-contained", "true", "--output", str(staged), "--disable-build-servers", "-m:2", *properties]
        subprocess.run(command, cwd=project.parent, env=env, check=True, timeout=300)
        licenses = staged / "licenses"
        licenses.mkdir()
        packages = Path(env["NUGET_PACKAGES"])
        for source, destination in [
            (packages / f"microsoft.netcore.app.runtime.{rid}/{RUNTIME}/LICENSE.TXT", "dotnet-LICENSE.TXT"),
            (packages / f"microsoft.netcore.app.runtime.{rid}/{RUNTIME}/THIRD-PARTY-NOTICES.TXT", "dotnet-THIRD-PARTY-NOTICES.TXT"),
            (packages / "newtonsoft.json/13.0.4/LICENSE.md", "newtonsoft-LICENSE.md"),
            (ROOT / "LICENSE", "screenshots-LICENSE")]:
            if not source.is_file() or source.is_symlink() or not 1 <= source.stat().st_size <= 1024 * 1024:
                raise RuntimeError("A required runtime license is missing or invalid")
            shutil.copyfile(source, licenses / destination)
        files = []
        for path in sorted(staged.rglob("*")):
            if path.is_symlink():
                raise RuntimeError("Runtime bundle must not contain symlinks")
            if not path.is_file():
                continue
            size = path.stat().st_size
            if size > 64 * 1024 * 1024:
                raise RuntimeError("Runtime member exceeds 64 MiB")
            files.append({"path": path.relative_to(staged).as_posix(), "bytes": size, "sha256": digest(path),
                          "executable": bool(path.stat().st_mode & stat.S_IXUSR) if os.name != "nt" else path.suffix.lower() == ".exe"})
        if not 1 <= len(files) <= 512 or sum(item["bytes"] for item in files) > 256 * 1024 * 1024:
            raise RuntimeError("Runtime bundle exceeds its file/byte bound")
        manifest = {"version": 1, "protocol": 1, "rid": rid, "sdk": SDK, "runtime": RUNTIME,
                    "entrypoint": "screenshots-sxcu.exe" if rid.startswith("win-") else "screenshots-sxcu", "files": files}
        (staged / "RUNTIME-MANIFEST.json").write_text(json.dumps(manifest, separators=(",", ":")) + "\n", encoding="utf-8")
        if output.exists():
            shutil.rmtree(output)
        shutil.copytree(staged, output)
    if rid == native_rid():
        result = subprocess.run([str(output / manifest["entrypoint"]), "--self-test"], cwd=output, capture_output=True, text=True, timeout=5)
        if result.returncode or not json.loads(result.stdout).get("passed"):
            raise RuntimeError("Published native helper self-test failed")
    report = {"rid": rid, "sdk": SDK, "runtime": RUNTIME, "files": len(files), "bytes": sum(item["bytes"] for item in files),
              "manifest_sha256": digest(output / "RUNTIME-MANIFEST.json"), "native_self_test": rid == native_rid()}
    print(json.dumps(report))
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dotnet", type=Path)
    parser.add_argument("--rid", choices=sorted(RIDS))
    parser.add_argument("--output", type=Path)
    parser.add_argument("--update-lock", action="store_true", help="Explicit maintainer action, never used in frozen CI")
    args = parser.parse_args()
    build(args)


if __name__ == "__main__":
    main()
