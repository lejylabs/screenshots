#!/usr/bin/env python3
"""Verify BMP export pixels independently using synthetic PNG input and FFmpeg.

No user images, desktop capture, clipboard, uploads or account access are used.
Requires Python 3, FFmpeg and a built screenshots executable.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import zlib

REPO = Path(__file__).resolve().parents[1]
STATE_DIRS = ("HOME", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "XDG_CONFIG_HOME",
              "XDG_DATA_HOME", "XDG_CACHE_HOME")


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def executable(value, label):
    path = shutil.which(value)
    require(path is not None, f"{label} is unavailable")
    return str(Path(path).resolve())


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def png_chunk(kind, data):
    return (struct.pack(">I", len(data)) + kind + data
            + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF))


def png_bytes(rows):
    height, width = len(rows), len(rows[0])
    raw = b"".join(b"\x00" + bytes(channel for pixel in row for channel in pixel)
                   for row in rows)
    return (b"\x89PNG\r\n\x1a\n"
            + png_chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + png_chunk(b"IDAT", zlib.compress(raw))
            + png_chunk(b"IEND", b""))


def rgba_bytes(rows):
    return bytes(channel for row in rows for pixel in row for channel in pixel)


def rotate90_flip_horizontal(rows):
    height, width = len(rows), len(rows[0])
    rotated = [[rows[height - 1 - x][y] for x in range(height)] for y in range(width)]
    return [list(reversed(row)) for row in rotated]


def convert(binary, source, output, env, extra=()):
    result = subprocess.run([binary, "image", "convert", str(source), str(output), *extra],
                            capture_output=True, env=env, timeout=20)
    require(len(result.stdout) <= 65536 and len(result.stderr) <= 65536,
            "Conversion output exceeded test limits")
    return result


def check_conversion(binary, ffmpeg, source, output, env, rows, extra=()):
    result = convert(binary, source, output, env, extra)
    require(result.returncode == 0, "BMP conversion failed")
    require(not result.stderr, "BMP conversion emitted unexpected diagnostics")
    value = json.loads(result.stdout)
    require(value == {"format": "bmp", "width": len(rows[0]), "height": len(rows)},
            "BMP conversion returned wrong dimensions or format")
    raw = subprocess.run([ffmpeg, "-hide_banner", "-loglevel", "error", "-nostdin",
                          "-threads", "1", "-i", str(output), "-frames:v", "1",
                          "-f", "rawvideo", "-pix_fmt", "rgba", "-threads", "1", "-"],
                         capture_output=True, env=env, timeout=20)
    require(raw.returncode == 0 and not raw.stderr, "Independent BMP decoding failed")
    require(raw.stdout == rgba_bytes(rows), "BMP RGBA pixels differ from the source oracle")
    header = output.read_bytes()[:54]
    require(header[:2] == b"BM", "Output does not contain a BMP signature")
    return {"width": len(rows[0]), "height": len(rows),
            "encoded_bytes": output.stat().st_size,
            "rgba_sha256": hashlib.sha256(raw.stdout).hexdigest()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=str(REPO / "target/release/screenshots"))
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    binary = executable(args.binary, "screenshots binary")
    ffmpeg = executable("ffmpeg", "FFmpeg")
    started = time.monotonic()
    rows = [[((x * 71 + y * 19) & 255, (x * 29 + y * 61) & 255,
              (x * 47 + y * 37 + 13) & 255, (0, 128, 255)[(x + y) % 3])
             for x in range(3)] for y in range(5)]
    with tempfile.TemporaryDirectory(prefix="screenshots-bmp-test-") as temporary:
        root = Path(temporary)
        env = dict(os.environ)
        for name in STATE_DIRS:
            env[name] = str(root / name.lower())
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS"):
            env.pop(name, None)
        source = root / "synthetic.png"
        source.write_bytes(png_bytes(rows))
        original = sha256(source)
        plain = root / "plain.BMP"
        transformed = root / "transformed.bmp"
        cases = [check_conversion(binary, ffmpeg, source, plain, env, rows),
                 check_conversion(binary, ffmpeg, source, transformed, env,
                                  rotate90_flip_horizontal(rows),
                                  ("--rotate", "90", "--flip-horizontal"))]
        before = sha256(plain)
        refused = convert(binary, source, plain, env)
        require(refused.returncode != 0 and not refused.stdout,
                "Existing BMP output was not rejected")
        require(sha256(plain) == before, "Existing BMP output was modified")
        require(sha256(source) == original, "PNG source was modified")
        require({path.name for path in root.iterdir()} == {
            "synthetic.png", "plain.BMP", "transformed.bmp"},
            "Unexpected user state or temporary files remained")
    report = {"passed": True, "binary_sha256": sha256(Path(binary)),
              "cases": cases, "hidden_rgb_and_alpha_preserved": True,
              "rotation_flip_exact": True, "no_overwrite": True,
              "source_user_state_unchanged": True,
              "elapsed_seconds": round(time.monotonic() - started, 3)}
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"BMP fixture failed: {error}", file=sys.stderr)
        raise SystemExit(1)
