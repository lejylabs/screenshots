#!/usr/bin/env python3
"""Verify offline GIF assembly using synthetic PNGs, GIF parsing and FFmpeg.

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


def parse_gif(data):
    require(data[:6] in (b"GIF87a", b"GIF89a") and len(data) >= 13, "Invalid GIF header")
    width, height, packed = struct.unpack_from("<HHB", data, 6)
    offset = 13 + (3 * (1 << ((packed & 7) + 1)) if packed & 128 else 0)
    controls, images, applications, trailer = [], 0, [], False

    def blocks(position):
        result = bytearray()
        while True:
            require(position < len(data), "Truncated GIF subblocks")
            length = data[position]
            position += 1
            if length == 0:
                return position, bytes(result)
            require(position + length <= len(data), "Truncated GIF subblock")
            result.extend(data[position:position + length])
            position += length

    while offset < len(data):
        kind = data[offset]
        offset += 1
        if kind == 0x3B:
            require(offset == len(data), "Trailing bytes after GIF trailer")
            trailer = True
            break
        if kind == 0x21:
            require(offset < len(data), "Truncated GIF extension")
            label = data[offset]
            offset += 1
            if label == 0xF9:
                require(offset + 6 <= len(data) and data[offset] == 4 and data[offset + 5] == 0,
                        "Invalid graphic control extension")
                flags, delay = struct.unpack_from("<BH", data, offset + 1)
                controls.append((flags >> 2 & 7, delay))
                offset += 6
            else:
                offset, payload = blocks(offset)
                if label == 0xFF:
                    applications.append(payload)
        elif kind == 0x2C:
            require(offset + 9 <= len(data), "Truncated GIF image descriptor")
            x, y, frame_width, frame_height, flags = struct.unpack_from("<HHHHB", data, offset)
            require((x, y, frame_width, frame_height) == (0, 0, width, height),
                    "GIF frame does not fill its canvas")
            offset += 9
            if flags & 128:
                offset += 3 * (1 << ((flags & 7) + 1))
            require(offset < len(data), "Missing GIF LZW minimum code size")
            offset += 1
            offset, _ = blocks(offset)
            images += 1
        else:
            raise RuntimeError("Unknown GIF block")
    require(trailer, "Missing GIF trailer")
    require(images == 3 and len(controls) == 3, "GIF frame/control count mismatch")
    require(controls == [(2, 12)] * 3, "GIF delay/disposal contract mismatch")
    require(not applications, "GIF unexpectedly contains an application/loop extension")
    return width, height


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=str(REPO / "target/release/screenshots"))
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    binary, ffmpeg = executable(args.binary, "screenshots"), executable("ffmpeg", "FFmpeg")
    started = time.monotonic()
    red = [[(255, 0, 0, 255)] * 3 for _ in range(2)]
    transparent = [[(17, 39, 85, 0)] * 3 for _ in range(2)]
    transparent[0][1] = (0, 0, 255, 255)
    green = [[(0, 255, 0, 255)] * 3 for _ in range(2)]
    expected = b"".join(rgba_bytes(rows) for rows in (red, transparent, green))
    with tempfile.TemporaryDirectory(prefix="screenshots-gif-assembly-") as temporary:
        root = Path(temporary)
        env = dict(os.environ)
        env.update({name: str(root / name.lower()) for name in STATE_DIRS})
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS"):
            env.pop(name, None)
        inputs = [root / f"frame-{index}.png" for index in range(3)]
        for path, rows in zip(inputs, (red, transparent, green)):
            path.write_bytes(png_bytes(rows))
        originals = [sha256(path) for path in inputs]
        output = root / "animation.gif"
        result = subprocess.run([binary, "gif", str(output), "--delay-ms", "120", *map(str, inputs)],
                                capture_output=True, env=env, timeout=20)
        require(result.returncode == 0 and not result.stderr, "GIF assembly failed")
        summary = json.loads(result.stdout)
        require(summary == {"format": "gif", "frames": 3, "delay_ms": 120},
                "GIF assembly summary mismatch")
        require(parse_gif(output.read_bytes()) == (3, 2), "GIF dimensions changed")
        decoded = subprocess.run([ffmpeg, "-hide_banner", "-loglevel", "error", "-nostdin",
                                  "-ignore_loop", "1", "-i", str(output), "-fps_mode", "passthrough",
                                  "-frames:v", "3", "-f", "rawvideo", "-pix_fmt", "rgba", "-"],
                                 capture_output=True, env=env, timeout=20)
        require(decoded.returncode == 0 and not decoded.stderr, "Independent GIF decode failed")
        require(len(decoded.stdout) == len(expected), "Decoded GIF frame count or dimensions changed")
        for index in range(0, len(expected), 4):
            actual_pixel = decoded.stdout[index:index + 4]
            expected_pixel = expected[index:index + 4]
            require(actual_pixel[3] == expected_pixel[3], "GIF alpha/disposal did not clear prior frame")
            if expected_pixel[3] != 0:
                require(actual_pixel == expected_pixel, "GIF visible RGB pixel changed")
        before = sha256(output)
        refused = subprocess.run([binary, "gif", str(output), "--delay-ms", "120", *map(str, inputs)],
                                 capture_output=True, env=env, timeout=20)
        require(refused.returncode != 0 and not refused.stdout and sha256(output) == before,
                "GIF output overwrite was not refused")
        require([sha256(path) for path in inputs] == originals, "GIF source frames changed")
        require({p.name for p in root.iterdir()} == {"frame-0.png", "frame-1.png", "frame-2.png", "animation.gif"},
                "Unexpected user state or staging files")
    report = {"passed": True, "binary_sha256": sha256(Path(binary)), "frames": 3,
              "delay_ms": 120, "background_disposal": True, "no_loop_extension": True,
              "independent_visible_rgba_exact": True, "source_user_state_unchanged": True,
              "elapsed_seconds": round(time.monotonic() - started, 3)}
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"GIF fixture failed: {error}", file=sys.stderr)
        raise SystemExit(1)
