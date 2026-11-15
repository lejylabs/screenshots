#!/usr/bin/env python3
"""Verify BMP input via PNG round trips, independent FFmpeg pixels and QR scanning.

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


def convert(binary, source, output, env, extra=()):
    result = subprocess.run([binary, "image", "convert", str(source), str(output), *extra],
                            capture_output=True, env=env, timeout=20)
    require(len(result.stdout) <= 65536 and len(result.stderr) <= 65536,
            "Conversion output exceeded test limits")
    return result


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
    with tempfile.TemporaryDirectory(prefix="screenshots-bmp-input-") as temporary:
        root = Path(temporary)
        env = dict(os.environ)
        for name in STATE_DIRS:
            env[name] = str(root / name.lower())
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS"):
            env.pop(name, None)
        source = root / "synthetic.png"
        source.write_bytes(png_bytes(rows))
        original = sha256(source)
        bmp = root / "source.BMP"
        encoded = convert(binary, source, bmp, env)
        require(encoded.returncode == 0 and not encoded.stderr, "BMP export failed")
        before = sha256(bmp)
        misleading = root / "bmp-disguised.dat"
        misleading.write_bytes(bmp.read_bytes())
        decoded = root / "decoded.png"
        result = convert(binary, misleading, decoded, env)
        require(result.returncode == 0 and not result.stderr, "BMP input conversion failed")
        require(json.loads(result.stdout) == {"format": "png", "width": 3, "height": 5},
                "BMP input summary mismatch")
        raw = subprocess.run([ffmpeg, "-hide_banner", "-loglevel", "error", "-nostdin",
                              "-threads", "1", "-i", str(decoded), "-frames:v", "1",
                              "-f", "rawvideo", "-pix_fmt", "rgba", "-threads", "1", "-"],
                             capture_output=True, env=env, timeout=20)
        require(raw.returncode == 0 and not raw.stderr and raw.stdout == rgba_bytes(rows),
                "Round-trip RGBA differs from independent source oracle")
        payload = "https://example.invalid/inert-bmp-qr"
        qr_png, qr_bmp = root / "qr.png", root / "qr.bmp"
        generated = subprocess.run([binary, "qr", "generate", payload, str(qr_png)],
                                   capture_output=True, env=env, timeout=20)
        require(generated.returncode == 0 and not generated.stderr, "QR fixture generation failed")
        qr_export = convert(binary, qr_png, qr_bmp, env)
        require(qr_export.returncode == 0 and not qr_export.stderr, "QR BMP export failed")
        qr_before = sha256(qr_bmp)
        scanned = subprocess.run([binary, "qr", "scan", str(qr_bmp)],
                                 capture_output=True, env=env, timeout=20)
        require(scanned.returncode == 0 and not scanned.stderr, "QR scan from BMP failed")
        codes = json.loads(scanned.stdout).get("codes", [])
        require(len(codes) == 1 and codes[0].get("text") == payload, "QR BMP payload changed")
        refused = convert(binary, bmp, decoded, env)
        require(refused.returncode != 0 and not refused.stdout, "Existing PNG output was not rejected")
        require(sha256(source) == original and sha256(bmp) == before
                and sha256(misleading) == before and sha256(qr_bmp) == qr_before,
                "An input file was modified")
        expected = {"synthetic.png", "source.BMP", "bmp-disguised.dat", "decoded.png", "qr.png", "qr.bmp"}
        require({p.name for p in root.iterdir()} == expected, "Unexpected user state or staging files")
    report = {"passed": True, "binary_sha256": sha256(Path(binary)),
              "width": 3, "height": 5,
              "hidden_rgb_and_alpha_exact": True, "signature_based_input": True,
              "qr_bmp_roundtrip": True, "no_overwrite": True,
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
        print(f"BMP input fixture failed: {error}", file=sys.stderr)
        raise SystemExit(1)
