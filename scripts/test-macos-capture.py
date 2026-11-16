#!/usr/bin/env python3
"""Explicit, pre-authorized macOS owned-window capture/AX/video gate.

Run only in a disposable native desktop session: --owned-session is mandatory.
No TCC request, input injection, camera or microphone is used. Exit 77 means an
unpassed native authorization/platform gate, never a successful mock result.
--self-test uses generated PNG bytes and FFmpeg only, on any platform.
"""
import argparse
import json
import math
import os
from pathlib import Path
import platform
import selectors
import shutil
import signal
import struct
import subprocess
import tempfile
import time
import zlib

REPO = Path(__file__).resolve().parents[1]


def require(value, message):
    if not value:
        raise RuntimeError(message)


def program(name):
    result = shutil.which(str(name))
    require(result, f"Required executable unavailable: {name}")
    return str(Path(result).resolve())


def line(child):
    data = bytearray()
    deadline = time.monotonic() + 10
    with selectors.DefaultSelector() as selector:
        selector.register(child.stdout, selectors.EVENT_READ)
        while b"\n" not in data:
            require(time.monotonic() < deadline, "Owned fixture readiness deadline")
            for key, _ in selector.select(0.1):
                part = os.read(key.fd, 4096)
                require(part, "Owned fixture exited before readiness")
                data.extend(part)
                require(len(data) <= 4096, "Owned fixture metadata exceeded its bound")
    return json.loads(data.splitlines()[0])


def colour(row, x):
    bits = (row >> (((x // 10) % 3) * 3)) & 7
    return (255 if bits & 4 else 0, 255 if bits & 2 else 0, 255 if bits & 1 else 0, 255)


def check_rows(pixels, width, height, scale, first=5, palette=None):
    require(len(pixels) == width * height * 4, "Decoded image extent mismatch")
    # The fixture paints whole logical rows; inspect the entire unoccluded ROI.
    for y in range(height):
        row = pixels[y * width * 4:(y + 1) * width * 4]
        for x in range(width):
            expected = colour(first + math.floor((y + 0.5) / scale), 30 + math.floor((x + 0.5) / scale))
            if palette is not None:
                valid = tuple(row[x * 4:x * 4 + 4]) in palette[expected]
            else:
                valid = all(abs(row[x * 4 + channel] - expected[channel])
                            <= (0 if channel == 3 else 2) for channel in range(4))
            require(valid,
                    f"Owned sRGB/palette pixel mismatch at physical {x},{y}")


def gif_palette(path):
    data = path.read_bytes()
    require(len(data) >= 13 and data[:6] in (b"GIF87a", b"GIF89a") and data[10] & 0x80,
            "Owned GIF has no declared global palette")
    count = 1 << ((data[10] & 7) + 1)
    require(len(data) >= 13 + count * 3, "Truncated owned GIF palette")
    table = [tuple(data[at:at + 3]) for at in range(13, 13 + count * 3, 3)]
    levels = [sorted({pixel[channel] for pixel in table}) for channel in range(3)]
    expected = {}
    for red in (0, 255):
        for green in (0, 255):
            for blue in (0, 255):
                source = (red, green, blue, 255)
                allowed = []
                for channel in range(3):
                    axis = levels[channel]
                    index = min(range(len(axis)), key=lambda at: abs(axis[at] - source[channel]))
                    allowed.append(set(axis[max(index - 1, 0):index + 2]))
                # RGB8 error diffusion and the reserved transparency entry may
                # select one adjacent level. Primary-color bits remain distinct.
                expected[source] = {(*pixel, 255) for pixel in table
                                    if all(pixel[c] in allowed[c] for c in range(3))}
    return expected


def check_video_bytes(pixels, width, height, scale, frames, palette=None):
    size = width * height * 4
    require(len(pixels) == frames * size, "Encoded native video frame count/geometry mismatch")
    for index in range(frames):
        check_rows(pixels[index * size:(index + 1) * size], width, height, scale, palette=palette)


def check_video(ffmpeg, path, width, height, scale, frames):
    require(width * height * 4 * (frames + 1) <= 16 * 1024 * 1024,
            "Owned decoded video oracle exceeded its 16-MiB bound")
    result = subprocess.run([ffmpeg, "-v", "error", "-nostdin", "-protocol_whitelist", "file,pipe",
                             "-i", str(path), "-map", "0:v:0", "-an", "-sn", "-dn",
                             "-frames:v", str(frames + 1), "-fps_mode", "passthrough",
                             "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"],
                            capture_output=True, timeout=10, check=True)
    palette = gif_palette(path) if path.suffix == ".gif" else None
    check_video_bytes(result.stdout, width, height, scale, frames, palette)
    return result.stdout, palette


def decode(ffmpeg, path, width, height):
    result = subprocess.run([ffmpeg, "-v", "error", "-nostdin", "-i", str(path),
                             "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"],
                            capture_output=True, timeout=10, check=True)
    require(len(result.stdout) == width * height * 4, "Native image size differs from admitted geometry")
    return result.stdout


def self_test(ffmpeg, root):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xffffffff)
    width, height, scale = 64, 24, 2
    expected = bytes(channel for y in range(height) for x in range(width)
                     for channel in colour(5 + y // scale, 30 + x // scale))
    rows = b"".join(b"\0" + expected[y * width * 4:(y + 1) * width * 4] for y in range(height))
    path = root / "generated.png"
    path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
                     + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))
    pixels = decode(ffmpeg, path, width, height)
    check_rows(pixels, width, height, scale)
    changed = bytearray(pixels)
    changed[4] ^= 32
    try:
        check_rows(changed, width, height, scale)
    except RuntimeError:
        pass
    else:
        raise RuntimeError("Pixel oracle accepted a modified generated row")
    for extension, codec in (("mkv", "ffv1"), ("gif", "gif")):
        video = root / f"generated.{extension}"
        subprocess.run([ffmpeg, "-v", "error", "-nostdin", "-loop", "1", "-framerate", "5",
                        "-i", str(path), "-frames:v", "3", "-c:v", codec,
                        "-pix_fmt", "rgb8" if codec == "gif" else "bgra", str(video)],
                       check=True, capture_output=True, timeout=10)
        decoded, palette = check_video(ffmpeg, video, width, height, scale, 3)
        repeated = bytearray(decoded)
        at = len(expected) + 4
        repeated[at] = 0 if repeated[at] >= 128 else 255
        try:
            check_video_bytes(repeated, width, height, scale, 3, palette)
        except RuntimeError:
            pass
        else:
            raise RuntimeError("Video oracle accepted a modified middle frame")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default=str(REPO / "target/release-fast/screenshots"))
    parser.add_argument("--owned-session", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    report = {"native_acceptance": False, "requested_permission": False, "checks": []}

    def finish(status):
        data = json.dumps(report, sort_keys=True, indent=2) + "\n"
        if args.report:
            args.report.write_text(data)
        print(data, end="")
        return status

    if not args.self_test and (platform.system() != "Darwin" or not args.owned_session):
        report["unpassed"] = "Requires an explicitly owned native macOS desktop session"
        return finish(77)
    ffmpeg = program("ffmpeg")
    with tempfile.TemporaryDirectory(prefix="screenshots-owned-mac-") as directory:
        root = Path(directory)
        self_test(ffmpeg, root)
        report["checks"].append("independent PNG/FFV1/GIF oracles check every generated frame and reject a modified middle frame")
        if args.self_test:
            return finish(0)
        binary = program(args.binary)
        original = Path(binary).stat()
        identity = lambda: (lambda value: (value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns))(Path(binary).stat())
        expected_identity = (original.st_dev, original.st_ino, original.st_size, original.st_mtime_ns)
        fixture = root / "owned-mac-fixture"
        subprocess.run([program("xcrun"), "swiftc", "-O", str(REPO / "tools/MacCaptureFixture/main.swift"),
                        "-o", str(fixture)], check=True, capture_output=True, timeout=90)
        env = dict(os.environ, SCREENSHOTS_OWNED_MAC_FIXTURE="1")
        child = subprocess.Popen([str(fixture)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                 stderr=subprocess.PIPE, env=env, start_new_session=True)
        try:
            meta = line(child)
            if not meta.get("screen_recording"):
                report["unpassed"] = "Existing Screen Recording authorization unavailable; no request made"
                return finish(77)
            require(child.poll() is None and meta["pid"] == child.pid and not meta["requested_permission"],
                    "Owned fixture identity mismatch")
            scale = meta["scale"]
            require(scale in (1, 2), "Owned pixel oracle requires integral 1x or 2x fixture scale")
            x, y, width, height = meta["region"]
            require(width * height * 4 <= 16 * 1024 * 1024, "Owned ROI exceeded the fixture pixel bound")

            def invoke(arguments, success=True, timeout=25):
                require(identity() == expected_identity and child.poll() is None, "Owned source or binary changed")
                result = subprocess.run([binary, *map(str, arguments)], capture_output=True, timeout=timeout)
                require(identity() == expected_identity and child.poll() is None, "Owned source or binary changed")
                require((result.returncode == 0) == success,
                        f"Unexpected native status for {arguments[0]}: {result.stderr.decode(errors='replace')}")
                if not success:
                    require(not result.stdout, "Failed native command published success metadata")
                    return None
                return json.loads(result.stdout)

            options = {"cursor": "exclude", "window_shadow": "exclude", "window_frame": "include"}
            window_path = root / "window.png"
            invoke(["capture-task", "window", meta["window"], window_path, "--capture-options", json.dumps(options)])
            ww, wh = meta["window_size"]
            pixels = decode(ffmpeg, window_path, ww, wh)
            roi = b"".join(pixels[((y + yy) * ww + x) * 4:((y + yy) * ww + x + width) * 4]
                           for yy in range(height))
            check_rows(roi, width, height, scale)
            report["checks"].append("owned full window physical extent, top-left crop, exact opaque sRGB rows")
            ax, ay = meta["alpha_pixel"]
            alpha_pixel = pixels[(ay * ww + ax) * 4:(ay * ww + ax + 1) * 4]
            require(abs(alpha_pixel[0] - 255) <= 2 and alpha_pixel[1:3] == b"\0\0"
                    and abs(alpha_pixel[3] - 128) <= 2,
                    "Owned translucent native window pixels were not preserved")
            opaque_path = root / "opaque.png"
            opaque = dict(options, window_transparency="opaque", background=[7, 11, 13])
            invoke(["capture-task", "window", meta["window"], opaque_path,
                    "--capture-options", json.dumps(opaque)])
            flattened = decode(ffmpeg, opaque_path, ww, wh)
            for at in range(0, len(pixels), 4):
                alpha = pixels[at + 3]
                require(flattened[at + 3] == 255 and all(
                    flattened[at + channel] == (pixels[at + channel] * alpha + background * (255 - alpha) + 127) // 255
                    for channel, background in enumerate((7, 11, 13))),
                    "Explicit opaque native window background differs from admitted straight-alpha blend")
            report["checks"].append("native translucent window alpha and explicit opaque RGB background")
            forbidden = root / "client.png"
            invoke(["capture-task", "window", meta["window"], forbidden, "--capture-options",
                    '{"window_frame":"client"}'], success=False)
            require(not forbidden.exists(), "Unsupported native client policy published a file")
            report["checks"].append("unsupported client-only policy fails without output")

            dx, dy, dw, dh = meta["display_region"]
            region_path = root / "region.png"
            invoke(["capture-task", "region", meta["display"], dx, dy, dw, dh, region_path,
                    "--capture-options", '{"cursor":"exclude"}'])
            without_cursor = decode(ffmpeg, region_path, dw, dh)
            check_rows(without_cursor, dw, dh, scale)
            report["checks"].append("fixed native display physical ROI and row order")
            cursor_path = root / "cursor.png"
            invoke(["capture-task", "region", meta["display"], dx, dy, dw, dh, cursor_path,
                    "--capture-options", '{"cursor":"include"}'])
            with_cursor = decode(ffmpeg, cursor_path, dw, dh)
            report["checks"].append("display cursor-including source executes with the same admitted extent")
            cx, cy = meta["cursor_roi"]
            cw, ch = meta["cursor_size"]
            hx, hy = meta["cursor_hotspot"]
            if 0 <= cx - hx and cx - hx + cw <= dw and 0 <= cy - hy and cy - hy + ch <= dh:
                changed = [at // 4 for at in range(0, len(with_cursor), 4)
                           if with_cursor[at:at + 4] != without_cursor[at:at + 4]]
                require(changed and all(cx - hx <= at % dw < cx - hx + cw
                                        and cy - hy <= at // dw < cy - hy + ch for at in changed),
                        "Cursor inclusion changed pixels outside the owned cursor or showed no cursor")
                report["cursor_visual_acceptance"] = True
                report["checks"].append("unchanged-pointer cursor inclusion changes only its owned image footprint")
            else:
                report["cursor_visual_acceptance"] = "unpassed; unchanged pointer lies outside the owned ROI"

            if meta["accessibility"]:
                scroll_path = root / "scroll.png"
                summary = invoke(["scroll-capture", meta["window"], scroll_path, "--region", x, y, width, height,
                                  "--steps", 128, "--wheel", 1, "--settle-ms", 100,
                                  "--min-overlap", 64, "--tolerance", 2], timeout=125)
                require(summary["width"] == width and summary["height"] > height
                        and summary["complete"] and summary["vertical_advances"], "Owned AX scroll did not complete")
                require(summary["height"] == (meta["document_height"] - 10) * scale,
                        "Owned AX join skipped or duplicated document rows")
                check_rows(decode(ffmpeg, scroll_path, width, summary["height"]), width, summary["height"], scale)
                report["checks"].append("retained AX target advances owned content and completes exact joined rows")
                child.stdin.write(b"reset\noffset\n")
                child.stdin.flush()
                require(line(child)["offset_points"] == 0, "Owned scroll view did not reset")
            else:
                report["scroll_acceptance"] = "unpassed; existing Accessibility authorization unavailable"

            base = ["--seconds", 1, "--fps", 5, "--width", dw, "--height", dh, "--x", dx, "--y", dy,
                    "--display", meta["display"], "--cursor", "hide"]
            for name, audio in (("screen.mkv", []), ("screen.gif", []),
                                ("screen-audio.mkv", ["--audio-mode", "test", "--audio-channels", 1])):
                destination = root / name
                summary = invoke(["record", "avfoundation", destination, *base, *audio], timeout=20)
                require(summary["display_id"] == meta["display"] and summary["native_source_frames"] == 5,
                        "Fixed-display native recording source/count mismatch")
                if audio:
                    require(summary["audio"]["samples"] == 48000 and summary["audio"]["channels"] == 1,
                            "Combined synthetic audio sample count mismatch")
                require(destination.stat().st_size > 0, "Native recorder did not publish its verified output")
                check_video(ffmpeg, destination, dw, dh, scale, 5)
            report["checks"].append("every native FFV1/GIF frame matches owned physical ROI; fixed RGB8 palette accounted")
            report["checks"].append("camera-free fixed-display finite video and exact 48-kHz synthetic combined audio")
            report["audio_acceptance"] = "unpassed; this fixture does not access a microphone"
            report["shadow_acceptance"] = "unpassed; full shadow extent requires separate native policy fixture"
            report["native_acceptance"] = not any(key.endswith("acceptance") and value != True
                                                    for key, value in report.items() if key != "native_acceptance")
            return finish(0)
        finally:
            if child.poll() is None:
                try:
                    child.stdin.write(b"quit\n")
                    child.stdin.flush()
                    child.wait(timeout=5)
                except (OSError, subprocess.TimeoutExpired):
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait(timeout=5)
            child.stdin.close()
            child.stdout.close()
            child.stderr.close()


if __name__ == "__main__":
    raise SystemExit(main())
