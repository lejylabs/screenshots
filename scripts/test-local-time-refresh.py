#!/usr/bin/env python3
"""Run the exact production Unix conversion in an owned single-thread process."""
from pathlib import Path
import json
import os
import signal
import subprocess
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def run(command, **options):
    process = subprocess.Popen(command, cwd=ROOT, start_new_session=True, **options)
    try:
        code = process.wait(timeout=240)
    except BaseException:
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        raise
    if code:
        raise subprocess.CalledProcessError(code, command)


def main():
    if os.name != "posix":
        raise SystemExit("This fixture requires Unix; do not treat Windows as tested")
    packages = tomllib.loads((ROOT / "Cargo.lock").read_text())["package"]
    versions = {name: next(p["version"] for p in packages if p["name"] == name)
                for name in ("jiff", "libc")}
    with tempfile.TemporaryDirectory(prefix="screenshots-local-time-") as temporary:
        fixture = Path(temporary)
        (fixture / "src").mkdir()
        manifest = ('[package]\nname="owned-local-time-refresh"\nversion="0.0.0"\n'
                    'edition="2024"\n[workspace]\n[dependencies]\n'
                    f'jiff={{version="={versions["jiff"]}",default-features=false,features=["std","js"]}}\n'
                    f'libc="={versions["libc"]}"\n')
        (fixture / "Cargo.toml").write_text(manifest)
        (fixture / "Cargo.lock").write_bytes((ROOT / "Cargo.lock").read_bytes())
        production = ROOT / "vendor/gpui-pre/src/local_time.rs"
        source = ('#[path=' + json.dumps(str(production)) + ']\nmod local_time;\n'
                  + (ROOT / "scripts/fixtures/local_time_refresh.rs").read_text())
        (fixture / "src/main.rs").write_text(source)
        with (fixture / "metadata.json").open("wb") as metadata:
            run(["cargo", "metadata", "--offline", "--format-version", "1", "--manifest-path",
                 str(fixture / "Cargo.toml")], stdout=metadata)
        selected = tomllib.loads((fixture / "Cargo.lock").read_text())["package"]
        original = {(p["name"], p["version"], p.get("source")) for p in packages}
        assert all(p["name"] == "owned-local-time-refresh"
                   or (p["name"], p["version"], p.get("source")) in original for p in selected), \
            "Fixture resolution introduced a dependency outside the application lock"
        run(["cargo", "run", "--release", "--locked", "--offline", "-j", "2",
             "--manifest-path", str(fixture / "Cargo.toml")])


if __name__ == "__main__":
    main()
