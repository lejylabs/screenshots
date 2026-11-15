#!/usr/bin/env python3
"""Verify bounded TIFF input with independent fixtures, FFmpeg and offline CLI calls.

Requires Python 3, FFmpeg and a built screenshots executable. Uses only synthetic
images and an inert QR payload; no capture, clipboard, accounts or network calls.
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
            + png_chunk(b"IDAT", zlib.compress(raw)) + png_chunk(b"IEND", b""))


def rgba_bytes(rows, samples=4):
    return bytes(channel for row in rows for pixel in row
                 for channel in (*pixel[:3], pixel[3] if samples == 4 else 255))


def tiff_bytes(rows, endian, samples):
    """Write classic chunky TIFF directly; no image/TIFF library supplies fixtures."""
    width, height, rows_per_strip = len(rows[0]), len(rows), 2
    strips = [bytes(channel for row in rows[start:start + rows_per_strip]
                    for pixel in row for channel in pixel[:samples])
              for start in range(0, height, rows_per_strip)]
    tags = [(256, 4, [width]), (257, 4, [height]), (258, 3, [8] * samples),
            (259, 3, [1]), (262, 3, [2]), (273, 4, [0] * len(strips)),
            (274, 3, [1]), (277, 3, [samples]), (278, 4, [rows_per_strip]),
            (279, 4, [len(strip) for strip in strips]), (282, 5, [72, 1]),
            (283, 5, [72, 1]), (284, 3, [1]), (296, 3, [2]), (317, 3, [1])]
    if samples == 4:
        tags.append((338, 3, [2]))
    tags.append((339, 3, [1] * samples))
    tags.sort()
    external = bytearray()
    locations = {}
    directory_end = 8 + 2 + 12 * len(tags) + 4
    for tag, kind, values in tags:
        size = (2 if kind == 3 else 4) * len(values)
        if size > 4:
            locations[tag] = directory_end + len(external)
            external.extend(bytes(size))
    raster_start = directory_end + len(external)
    offsets = []
    offset = raster_start
    for strip in strips:
        offsets.append(offset)
        offset += len(strip)
    entries = bytearray()
    for tag, kind, values in tags:
        if tag == 273:
            values = offsets
        packed = struct.pack(endian + ("H" if kind == 3 else "I") * len(values), *values)
        count = len(values) // 2 if kind == 5 else len(values)
        entries.extend(struct.pack(endian + "HHI", tag, kind, count))
        if len(packed) <= 4:
            entries.extend(packed.ljust(4, b"\x00"))
        else:
            location = locations[tag]
            entries.extend(struct.pack(endian + "I", location))
            relative = location - directory_end
            external[relative:relative + len(packed)] = packed
    marker = b"II" if endian == "<" else b"MM"
    return (marker + struct.pack(endian + "HIH", 42, 8, len(tags)) + entries
            + struct.pack(endian + "I", 0) + external + b"".join(strips))


def malformed_fixtures(valid):
    endian, directory = "<", 8
    count = struct.unpack_from("<H", valid, directory)[0]
    entries = {struct.unpack_from("<H", valid, 10 + i * 12)[0]: 10 + i * 12
               for i in range(count)}
    next_ifd = 10 + 12 * count

    def change(offset, code, value):
        data = bytearray(valid)
        struct.pack_into(endian + code, data, offset, value)
        return bytes(data)

    def scalar(tag, value):
        return change(entries[tag] + 8, "H" if tag not in (256, 257, 278) else "I", value)

    strip_offsets = struct.unpack_from("<I", valid, entries[273] + 8)[0]
    strip_counts = struct.unpack_from("<I", valid, entries[279] + 8)[0]
    bits = struct.unpack_from("<I", valid, entries[258] + 8)[0]
    return {
        "header-truncated": valid[:7], "bigtiff": change(2, "H", 43),
        "ifd-truncated": valid[:next_ifd + 3], "raster-truncated": valid[:-1],
        "tag-array-truncated": valid[:bits + 7],
        "extra-page": change(next_ifd, "I", 8),
        "duplicate-tag": change(entries[257], "H", 256),
        "unordered-tag": change(entries[256], "H", 257),
        "unknown-tag": change(entries[339], "H", 65000),
        "wrong-type": change(entries[256] + 2, "H", 2),
        "wrong-count": change(entries[258] + 4, "I", 3),
        "compression": scalar(259, 5), "grayscale": scalar(262, 1),
        "orientation": scalar(274, 3), "planar": scalar(284, 2),
        "predictor": scalar(317, 2), "associated-alpha": scalar(338, 1),
        "zero-width": scalar(256, 0), "zero-rows-per-strip": scalar(278, 0),
        "sixteen-bit": change(bits, "H", 16),
        "strip-outside-file": change(strip_offsets, "I", len(valid) + 1),
        "strip-overlaps-ifd": change(strip_offsets, "I", 8),
        "strip-duplicate": change(strip_offsets + 4, "I",
                                  struct.unpack_from("<I", valid, strip_offsets)[0]),
        "strip-wrong-size": change(strip_counts, "I", 1),
        "metadata-overlap": change(entries[258] + 8, "I", strip_offsets),
    }


def invoke(binary, arguments, env):
    result = subprocess.run([binary, *map(str, arguments)], capture_output=True,
                            env=env, timeout=20)
    require(len(result.stdout) <= 65536 and len(result.stderr) <= 65536,
            "CLI output exceeded fixture limits")
    return result


def successful(binary, arguments, env, label):
    result = invoke(binary, arguments, env)
    require(result.returncode == 0 and not result.stderr, f"{label} failed")
    return json.loads(result.stdout)


def decode(ffmpeg, path, env):
    result = subprocess.run([ffmpeg, "-hide_banner", "-loglevel", "error", "-nostdin",
                             "-threads", "1", "-i", str(path), "-frames:v", "1",
                             "-f", "rawvideo", "-pix_fmt", "rgba", "-threads", "1", "-"],
                            capture_output=True, env=env, timeout=20)
    require(result.returncode == 0 and not result.stderr, "Independent image decode failed")
    return result.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=str(REPO / "target/release/screenshots"))
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    binary, ffmpeg = executable(args.binary, "screenshots"), executable("ffmpeg", "FFmpeg")
    started = time.monotonic()
    rows = [[((x * 71 + y * 19) & 255, (x * 29 + y * 61) & 255,
              (x * 47 + y * 37 + 13) & 255, (0, 128, 255)[(x + y) % 3])
             for x in range(3)] for y in range(5)]
    with tempfile.TemporaryDirectory(prefix="screenshots-tiff-input-") as temporary:
        root, files, originals = Path(temporary), set(), {}
        env = dict(os.environ)
        env.update({name: str(root / name.lower()) for name in STATE_DIRS})
        for name in ("DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS"):
            env.pop(name, None)

        def source(name, data):
            path = root / name
            path.write_bytes(data)
            files.add(name)
            originals[path] = sha256(path)
            return path

        def output(name):
            files.add(name)
            return root / name

        cases = []
        for endian, label in (("<", "little"), (">", "big")):
            for samples in (3, 4):
                name = f"{label}-{'rgb' if samples == 3 else 'rgba'}"
                fixture = source(name + ".dat", tiff_bytes(rows, endian, samples))
                expected = rgba_bytes(rows, samples)
                require(decode(ffmpeg, fixture, env) == expected,
                        "Independent TIFF fixture has incorrect pixels")
                result = output(name + ".png")
                summary = successful(binary, ("image", "convert", fixture, result), env,
                                     "Synthetic TIFF conversion")
                require(summary == {"format": "png", "width": 3, "height": 5},
                        "TIFF conversion summary mismatch")
                require(decode(ffmpeg, result, env) == expected, "TIFF input RGBA mismatch")
                details = successful(binary, ("image", "info", fixture), env, "TIFF information")
                require(details == {"width": 3, "height": 5, "decoded_rgba_bytes": 60,
                                    "source_file_bytes": fixture.stat().st_size},
                        "TIFF information mismatch")
                pixel = successful(binary, ("image", "pixel", fixture, 0, 0), env, "TIFF pixel")
                require(pixel.get("rgba") == list(expected[:4]), "TIFF sampled pixel mismatch")
                cases.append(name)

        png = source("source.png", png_bytes(rows))
        exported = output("own-export.tiff")
        successful(binary, ("image", "convert", png, exported), env, "Own TIFF export")
        originals[exported] = sha256(exported)
        roundtrip = output("own-roundtrip.png")
        successful(binary, ("image", "convert", exported, roundtrip), env, "Own TIFF round trip")
        require(decode(ffmpeg, exported, env) == rgba_bytes(rows)
                and decode(ffmpeg, roundtrip, env) == rgba_bytes(rows),
                "Own TIFF export lost RGBA or hidden RGB")
        before = sha256(roundtrip)
        refused = invoke(binary, ("image", "convert", exported, roundtrip), env)
        require(refused.returncode != 0 and not refused.stdout and sha256(roundtrip) == before,
                "Existing PNG output was not preserved")

        malformed = malformed_fixtures(tiff_bytes(rows, "<", 4))
        rejected_output = root / "rejected.png"
        for name, data in malformed.items():
            invalid = source("invalid-" + name + ".tiff", data)
            refused = invoke(binary, ("image", "convert", invalid, rejected_output), env)
            require(refused.returncode != 0 and not refused.stdout and not rejected_output.exists(),
                    f"Malformed TIFF was accepted: {name}")

        payload = "https://example.invalid/inert-tiff-qr"
        qr_png, qr_tiff = output("qr.png"), output("qr.tiff")
        successful(binary, ("qr", "generate", payload, qr_png), env, "QR generation")
        originals[qr_png] = sha256(qr_png)
        successful(binary, ("image", "convert", qr_png, qr_tiff), env, "QR TIFF export")
        originals[qr_tiff] = sha256(qr_tiff)
        scanned = successful(binary, ("qr", "scan", qr_tiff), env, "TIFF QR scan")
        codes = scanned.get("codes", [])
        require(len(codes) == 1 and codes[0].get("text") == payload, "TIFF QR payload changed")
        require(all(sha256(path) == digest for path, digest in originals.items()),
                "TIFF input or source file changed")
        require({path.name for path in root.iterdir()} == files,
                "Unexpected user state or staging file")
    report = {"passed": True, "binary_sha256": sha256(Path(binary)), "cases": cases,
              "multi_strip_partial_final_strip": True, "hidden_rgb_and_alpha_exact": True,
              "signature_based_input": True, "own_export_roundtrip": True,
              "info_and_pixel": True, "malformed_refused": len(malformed),
              "qr_tiff_roundtrip": True, "no_overwrite": True,
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
        print(f"TIFF input fixture failed: {error}", file=sys.stderr)
        raise SystemExit(1)
