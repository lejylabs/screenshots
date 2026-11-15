#!/usr/bin/env python3
"""Independently verify exact strip removal with synthetic PNGs and FFmpeg.

Requires Python 3, FFmpeg and a built screenshots binary. No screen, network,
credentials or persistent user configuration is accessed.
"""
import argparse
import binascii
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import tempfile
import zlib

REPO = Path(__file__).resolve().parents[1]


def file_hash(path):
    result = hashlib.sha256()
    with open(path, "rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def png(width, height, pixels):
    def chunk(kind, payload):
        return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", binascii.crc32(kind + payload))
    rows = b"".join(b"\0" + pixels[y * width * 4:(y + 1) * width * 4] for y in range(height))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


def pixel(x, y):
    return bytes((10 + x, 30 + y, x + 3 * y, [0, 128, 255][x]))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=str(REPO / "target/release/screenshots"))
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    binary = shutil.which(args.binary)
    ffmpeg = shutil.which("ffmpeg")
    if not binary or not ffmpeg:
        raise SystemExit("A built screenshots binary and FFmpeg are required; use --binary PATH")
    binary = str(Path(binary).resolve())
    with tempfile.TemporaryDirectory(prefix="screenshots-strip-") as temporary:
        root = Path(temporary)
        env = {key: value for key, value in os.environ.items()
               if key not in ["DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS"]}
        state = ["HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME",
                 "XDG_DATA_HOME", "XDG_CACHE_HOME"]
        env.update({key: str(root / key.lower()) for key in state})
        source = root / "source.png"
        source.write_bytes(png(3, 4, b"".join(pixel(x, y) for y in range(4) for x in range(3))))
        original = file_hash(source)
        reports = []

        def run(command, success=True):
            result = subprocess.run([binary, *command], env=env, stdout=subprocess.PIPE,
                                    stderr=subprocess.PIPE, timeout=15)
            if (result.returncode == 0) != success:
                raise RuntimeError(f"Unexpected CLI status: {result.stderr.decode(errors='replace')}")
            if success:
                if result.stderr:
                    raise RuntimeError("Successful command wrote stderr")
                return json.loads(result.stdout)
            if result.stdout:
                raise RuntimeError("Failed command wrote success output")
            return None

        def verify(path, expected):
            actual = subprocess.check_output([ffmpeg, "-v", "error", "-i", str(path),
                        "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"],
                        env=env, timeout=10)
            if actual != expected:
                raise RuntimeError("Independent decoder pixels differ from the strip oracle")
            return hashlib.sha256(actual).hexdigest()

        for axis, start, length, width, height, expected in [
            ("vertical", 1, 1, 2, 4, b"".join(pixel(x, y) for y in range(4) for x in [0, 2])),
            ("horizontal", 1, 2, 3, 2, b"".join(pixel(x, y) for y in [0, 3] for x in range(3))),
        ]:
            output = root / f"{axis}.png"
            summary = run(["image", "remove-strip", str(source), str(output), axis, str(start), str(length)])
            if summary["width"] != width or summary["height"] != height:
                raise RuntimeError("Strip output dimensions are incorrect")
            reports.append({"case": axis, "width": width, "height": height,
                            "rgba_sha256": verify(output, expected)})
        recipe = root / "recipe.json"
        recipe.write_text(json.dumps({"version": 1, "steps": [
            {"op": "redact", "x": 2, "y": 1, "width": 1, "height": 2},
            {"op": "remove_strip", "axis": "vertical", "start": 1, "length": 1},
        ]}))
        output = root / "recipe.png"
        run(["workflow", str(recipe), str(source), str(output)])
        expected = b"".join(bytes((0, 0, 0, 255)) if x == 2 and y in [1, 2] else pixel(x, y)
                            for y in range(4) for x in [0, 2])
        reports.append({"case": "redact_then_strip", "width": 2, "height": 4,
                        "rgba_sha256": verify(output, expected)})
        absent = root / "absent.png"
        run(["image", "remove-strip", str(source), str(absent), "vertical", "0", "3"], False)
        if absent.exists():
            raise RuntimeError("Whole-axis failure published a file")
        preserved = file_hash(output)
        run(["image", "remove-strip", str(source), str(output), "horizontal", "0", "1"], False)
        if file_hash(output) != preserved or file_hash(source) != original:
            raise RuntimeError("Source or existing destination was modified")
        if any(Path(env[key]).exists() for key in state):
            raise RuntimeError("Commands created persistent user state")
        result = {"scope": "synthetic local image verification only", "binary_sha256": file_hash(binary),
                  "cases": reports, "whole_axis_rejected": True, "no_overwrite": True,
                  "source_unchanged": True, "user_state_created": False}
    rendered = json.dumps(result, indent=2) + "\n"
    if args.report:
        args.report.write_text(rendered)
    print(rendered, end="")


if __name__ == "__main__":
    main()
