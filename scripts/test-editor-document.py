#!/usr/bin/env python3
"""Synthetic executable editor E2E; independent PNG writing/FFmpeg RGBA decoding."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import signal
import struct
import subprocess
import tempfile
import time
import zlib

ROOT = Path(__file__).resolve().parents[1]


def png(path, width, height, pixel):
    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    rows = b"".join(b"\0" + bytes(c for x in range(width) for c in pixel(x, y)) for y in range(height))
    path.write_bytes(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows, 1)) + chunk(b"IEND", b""))


def decode(path):
    probe = subprocess.run(["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height", "-of", "json", str(path)], capture_output=True, check=True, timeout=20)
    stream = json.loads(probe.stdout)["streams"][0]
    width, height = stream["width"], stream["height"]
    image = subprocess.run(["ffmpeg", "-v", "error", "-i", str(path), "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"], capture_output=True, check=True, timeout=20).stdout
    assert len(image) == width * height * 4
    return width, height, image


def pixel(image, x, y):
    width, _, data = image
    offset = (y * width + x) * 4
    return list(data[offset:offset + 4])


def layer(identifier, x, y, width, height, content, **extra):
    return {"id": identifier, "bounds": dict(x=x, y=y, width=width, height=height), "content": content, **extra}


def fill(identifier, x, y, width, height, color):
    return layer(identifier, x, y, width, height, {"tool": "draw", "shape": {"shape": "rectangle"}, "style": dict(fill=color, stroke=None, width=0)})


def scene(width, height, layers=(), **extra):
    return {"version": 1, "document": {"version": 1, "canvas": [width, height], "layers": list(layers)}, **extra}


def text_style(fonts, **extra):
    value = dict(fonts=fonts, size=24, weight=500, italic=False, underline=False, strike=False, color=[255, 255, 255, 200])
    value.update(extra)
    return value


class Runner:
    def __init__(self, binary):
        self.binary = binary.resolve()
        self.fd = None
        self.kwargs = {}
        if os.name == "posix" and Path("/proc/self/fd").is_dir():
            # Pin the actual inode across concurrent Cargo publication, without
            # copying/debug-stripping or changing its ctime through a hardlink.
            self.fd = os.open(self.binary, os.O_RDONLY)
            self.executable = f"/proc/self/fd/{self.fd}"
            self.kwargs["pass_fds"] = (self.fd,)
        else:
            self.executable = str(self.binary)
        digest = hashlib.sha256()
        if self.fd is not None:
            offset = 0
            while data := os.pread(self.fd, 1024 * 1024, offset):
                digest.update(data)
                offset += len(data)
        else:
            with self.binary.open("rb") as file:
                while data := file.read(1024 * 1024):
                    digest.update(data)
        self.digest = digest.hexdigest()
        self.renders = 0

    def command(self, *args):
        return [self.executable, "editor-document", *(str(arg) for arg in args)]

    def call(self, *args, ok=True):
        result = subprocess.run(self.command(*args), capture_output=True, timeout=40, **self.kwargs)
        if ok:
            assert result.returncode == 0, result.stderr.decode(errors="replace")
            return json.loads(result.stdout)
        assert result.returncode != 0, result.stdout
        return result.stderr.decode(errors="replace")

    def render(self, source, profile, output):
        value = self.call("render", source, profile, output)
        assert value["saved"] is True
        self.renders += 1
        return value

    def close(self):
        if self.fd is not None:
            os.close(self.fd)


def write(path, value):
    path.write_text(json.dumps(value), encoding="utf-8")
    return path


def rotated_privacy(runner, directory):
    source = directory / "rotated-secret.png"
    sanitized = directory / "rotated-independent-reference.png"
    png(source, 16, 16, lambda x, y: [217, 13, 131, 255] if 4 <= x < 8 and 4 <= y < 8 else [0, 17, 213, 255])
    png(sanitized, 16, 16, lambda x, y: [0, 0, 0, 255] if 4 <= x < 8 and 4 <= y < 8 else [0, 17, 213, 255])
    for shear in (0.0, 0.3):
        sine, cosine = math.sin(math.pi / 6), math.cos(math.pi / 6)
        a, b, c, d = cosine * 3, sine * 3, (cosine * shear - sine) * 3, (sine * shear + cosine) * 3
        determinant = a * d - b * c
        inverse = lambda x, y: ((d * (x - 40) - c * (y - 8)) / determinant, (-b * (x - 40) + a * (y - 8)) / determinant)
        candidates = [(x, y, inverse(x + 0.5, y + 0.5)[0]) for y in range(64) for x in range(64)
                      if all(inverse(cx, cy)[0] < 4 for cx, cy in ((x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1)))
                      and 3.6 < inverse(x + 0.5, y + 0.5)[0] < 3.95 and 4.5 < inverse(x + 0.5, y + 0.5)[1] < 7.5]
        px, py, sx = candidates[0]
        base = [a, b, c, d, 40, 8]
        mask = layer(1, 4, 4, 4, 4, dict(tool="redaction"), transform=base)
        clone = layer(2, 0, 70, 20, 20, dict(tool="clone", source=[1, 0, 0, 1, px - 5, py - 75]))
        lens = layer(3, 64, 64, 28, 28, dict(tool="magnifier", source=[0.5, 0, 0, 0.5, px + 0.5 - 39.25, py + 0.5 - 39.25], interpolation="bilinear", style=dict(fill=None, stroke=[0, 255, 0, 255], width=1)))
        value = scene(96, 96, [mask, clone, lens], source_size=[16, 16])
        value["document"].update(base=base, interpolation="bilinear")
        profile = write(directory / f"rotated-{shear}.json", value)
        raw_output, reference_output = directory / f"rotated-{shear}.png", directory / f"reference-{shear}.png"
        runner.render(source, profile, raw_output)
        runner.render(sanitized, profile, reference_output)
        image, reference = decode(raw_output), decode(reference_output)
        assert image == reference, "rotated/sheared noninterference against independently sanitized source"
        safe = pixel(image, px, py)
        assert safe[0] == 0 and pixel(image, 5, 75) == safe and pixel(image, 78, 78) == safe
        value["document"]["layers"] = [clone, lens]
        profile = write(directory / f"unmasked-{shear}.json", value)
        output = directory / f"unmasked-{shear}.png"
        runner.render(source, profile, output)
        image = decode(output)
        # Independent source bilinear equation: outside mask geometry, but with
        # a nonzero contribution from secret texel x=4. This prevents a uniform
        # fixture/final-opaque-overlay-only implementation from passing.
        red = math.floor(217 * (sx - 3.5) + 0.5)
        assert red > 20
        assert pixel(image, px, py)[0] == red and pixel(image, 5, 75)[0] == red and pixel(image, 78, 78)[0] == red
    return True


def font_budget_fixture(path, subtables):
    # Source-derived aliased PairPos1 attack, built from the existing OFL font.
    # The small default kern table revisits 128 x 128 pairs per aliased subtable.
    def words(values):
        return b"".join(struct.pack(">H", value) for value in values)
    def checksum(data):
        data += b"\0" * (-len(data) % 4)
        return sum(struct.unpack(">" + "I" * (len(data) // 4), data)) & 0xffffffff
    scripts = words([1]) + b"DFLT" + words([8, 4, 0, 0, 0xffff, 1, 0])
    features = words([1]) + b"kern" + words([8, 0, 1, 0])
    gpos = words([1, 0, 10, 10 + len(scripts), 10 + len(scripts) + len(features)]) + scripts + features
    gpos += words([1, 4, 2, 0, subtables]) + words([6 + 2 * subtables] * subtables)
    gpos += words([1, 266, 0, 0, 128]) + words([526] * 128) + words([1, 128]) + words(range(128)) + words([128]) + words(range(128))
    font = bytearray((ROOT / "vendor/cosmic-text/fonts/FiraMono-Medium.ttf").read_bytes())
    font += b"\0" * (-len(font) % 4)
    offset = len(font)
    font += gpos
    font += b"\0" * (-len(font) % 4)
    head = None
    found = False
    for entry in range(struct.unpack_from(">H", font, 4)[0]):
        start = 12 + 16 * entry
        if font[start:start + 4] == b"GPOS":
            struct.pack_into(">III", font, start + 4, checksum(gpos), offset, len(gpos))
            found = True
        if font[start:start + 4] == b"head":
            head = struct.unpack_from(">I", font, start + 8)[0] + 8
    assert found and head is not None
    struct.pack_into(">I", font, head, 0)
    struct.pack_into(">I", font, head, (0xb1b0afba - checksum(bytes(font))) & 0xffffffff)
    assert checksum(bytes(font)) == 0xb1b0afba
    path.write_bytes(font)


def legacy_kern_fixture(path, count, apple=True, adjustment=-120):
    def words(values):
        return b"".join(struct.pack(">H", value & 0xffff) for value in values)
    def checksum(data):
        data = bytes(data) + b"\0" * (-len(data) % 4)
        return sum(struct.unpack(">" + "I" * (len(data) // 4), data)) & 0xffffffff
    # Existing Fira Mono fixture: A=4,V=115, independently asserted by Rust cmap.
    body = words([1, 6, 0, 0, 4, 115, adjustment])
    if apple:
        kern = struct.pack(">II", 0x10000, count) + (struct.pack(">IHH", 22, 0, 0) + body) * count
    else:
        assert count == 1
        kern = words([0, 1, 0, 20, 1]) + body
    font = bytearray((ROOT / "vendor/cosmic-text/fonts/FiraMono-Medium.ttf").read_bytes())
    font += b"\0" * (-len(font) % 4)
    offset = len(font)
    font += kern
    font += b"\0" * (-len(font) % 4)
    records = []
    head = None
    for index in range(struct.unpack_from(">H", font, 4)[0]):
        record = bytearray(font[12 + index * 16:28 + index * 16])
        if record[:4] == b"DSIG":
            record[:4] = b"kern"
            struct.pack_into(">III", record, 4, checksum(kern), offset, len(kern))
        elif record[:4] == b"GPOS":
            record[:4] = b"XPOS"  # activate actual legacy kern in the control
        elif record[:4] == b"head":
            head = struct.unpack_from(">I", record, 8)[0] + 8
        records.append(record)
    for index, record in enumerate(sorted(records, key=lambda r: r[:4])):
        font[12 + index * 16:28 + index * 16] = record
    assert head is not None
    struct.pack_into(">I", font, head, 0)
    struct.pack_into(">I", font, head, (0xb1b0afba - checksum(font)) & 0xffffffff)
    assert checksum(font) == 0xb1b0afba and len(font) < 2 * 1024 * 1024
    path.write_bytes(font)


def check_cancellation(runner, directory):
    if os.name != "posix":
        return {"native_signal_cases": "Unix-only; Windows native CI remains separate"}
    source = directory / "large.png"
    png(source, 3840, 2160, lambda x, y: [x % 256, y % 256, 19, 255])
    layers = [layer(index + 1, 0, 0, 3840, 2160, {"tool": "blur", "radius": 64}) for index in range(96)]
    profile = write(directory / "cancel.json", scene(3840, 2160, layers))
    output = directory / "canceled.png"
    child = subprocess.Popen(runner.command("render", source, profile, output), stdout=subprocess.PIPE, stderr=subprocess.PIPE, **runner.kwargs)
    try:
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            assert child.poll() is None, "heavy fixture ended before cancellation"
            stat = Path(f"/proc/{child.pid}/stat")
            if not Path("/proc/self/stat").exists():
                time.sleep(0.2)
                break
            if stat.exists():
                ticks = stat.read_text().rsplit(")", 1)[1].split()
                if (int(ticks[11]) + int(ticks[12])) / os.sysconf("SC_CLK_TCK") > 0.12:
                    break
            time.sleep(0.01)
        start = time.monotonic()
        child.send_signal(signal.SIGTERM)
        _, error = child.communicate(timeout=8)
        assert child.returncode != 0
        assert not output.exists()
        assert not list(directory.glob(".screenshots-*.part"))
        canceled_ms = (time.monotonic() - start) * 1000
    finally:
        if child.poll() is None:
            child.kill()
            child.communicate(timeout=5)
    # This cancels during heavy render work, not a proven publication phase.
    # Fill stdout before launching. The child publishes its explicit output,
    # drops cancellation cleanup, then blocks while writing its tiny JSON result.
    read_fd, write_fd = os.pipe()
    os.set_blocking(write_fd, False)
    try:
        while True:
            os.write(write_fd, b"x" * 4096)
    except BlockingIOError:
        pass
    os.set_blocking(write_fd, True)
    small = write(directory / "blocked.json", scene(8, 4))
    output = directory / "blocked.png"
    child = subprocess.Popen(runner.command("render", directory / "small.png", small, output), stdout=write_fd, stderr=subprocess.PIPE, **runner.kwargs)
    os.close(write_fd)
    try:
        deadline = time.monotonic() + 5
        while not output.exists():
            assert child.poll() is None
            assert time.monotonic() < deadline
            time.sleep(0.01)
        child.send_signal(signal.SIGTERM)
        child.communicate(timeout=3)
        assert child.returncode != 0
    finally:
        if child.poll() is None:
            child.kill()
            child.communicate(timeout=5)
        os.close(read_fd)
    return {"sigterm_cleanup_ms": round(canceled_ms, 3), "render_cancellation_no_output_or_staging": True, "publication_cancellation_exercised": False, "blocked_success_stdout_terminates": True}


def run(runner, directory):
    source = directory / "source.png"
    png(source, 1280, 720, lambda x, y: [217, 13, 131, 0 if x == 8 else 255] if 8 <= x < 16 and 8 <= y < 16 else ([19, 37, 61, 0] if x == 0 else [0, 0, 200, 255]))
    initial_hash = hashlib.sha256(source.read_bytes()).hexdigest()
    logo = directory / "emoji.png"
    png(logo, 2, 2, lambda x, y: [200, 20, 40, 128])
    shape = lambda kind, style: {"tool": "draw", "shape": kind, "style": style}
    layers = [
        layer(1, 8, 8, 8, 8, {"tool": "redaction"}),
        fill(2, 32, 8, 16, 16, [255, 0, 0, 255]),
        layer(3, 56, 8, 16, 16, shape({"shape": "ellipse"}, dict(fill=[0, 255, 0, 255], stroke=None, width=0))),
        layer(4, 28, 32, 40, 16, shape({"shape": "arrow", "start": [32, 40], "end": [64, 40]}, dict(fill=None, stroke=[255, 255, 0, 255], width=3))),
        layer(5, 80, 8, 32, 24, shape({"shape": "speech_bubble", "tail": [120, 40], "corner": 4}, dict(fill=[0, 255, 255, 255], stroke=None, width=0))),
        layer(6, 28, 60, 40, 16, shape({"shape": "path", "points": [[32, 64], [48, 70], [64, 64]]}, dict(fill=None, stroke=[255, 128, 0, 255], width=4))),
        layer(7, 8, 32, 16, 16, {"tool": "highlight", "color": [255, 255, 0, 128]}),
        layer(8, 0, 0, 256, 160, {"tool": "highlight", "color": [0, 0, 0, 50], "outside": True}),
        fill(9, 8, 64, 8, 16, [255, 255, 255, 255]),
        layer(10, 8, 64, 16, 16, {"tool": "blur", "radius": 2}),
        fill(11, 32, 96, 2, 16, [255, 0, 0, 255]),
        layer(12, 32, 96, 16, 16, {"tool": "pixelate", "block": 4}),
        layer(13, 8, 96, 8, 8, {"tool": "clone", "source": [1, 0, 0, 1, 0, -88]}),
        layer(14, 80, 64, 24, 24, {"tool": "magnifier", "source": [0.5, 0, 0, 0.5, -36, -28], "style": dict(fill=[255, 255, 255, 255], stroke=[0, 255, 0, 255], width=1)}),
        layer(15, 200, 8, 16, 16, {"tool": "raster", "asset": "emoji", "kind": "emoji"}),
        fill(16, 128, 96, 16, 16, [0, 255, 0, 255]),
        layer(17, 128, 96, 8, 16, {"tool": "erase", "mode": "transparent", "shape": {"shape": "rectangle"}}),
        layer(18, 136, 96, 8, 16, {"tool": "erase", "mode": "restore_base", "shape": {"shape": "rectangle"}}),
        layer(19, 128, 8, 128, 64, {"tool": "text", "text": {"spans": [{"text": "AV fi", "style": text_style(["mono"], italic=True, underline=True, strike=True, weight=700)}]}}),
    ]
    profile = write(directory / "all.json", scene(1280, 720, layers, rasters=[dict(id="emoji", path="emoji.png")], fonts=[dict(id="mono", path=str(ROOT / "vendor/cosmic-text/fonts/FiraMono-Medium.ttf"))]))
    output = directory / "all.png"
    runner.render(source, profile, output)
    image = decode(output)
    assert pixel(image, 12, 12) == [0, 0, 0, 255]
    assert pixel(image, 40, 16) == [255, 0, 0, 255]
    assert pixel(image, 64, 16) == [0, 255, 0, 255]
    assert pixel(image, 45, 40) == [255, 255, 0, 255]
    assert pixel(image, 112, 36) != [0, 0, 200, 255]
    assert pixel(image, 48, 69) == [255, 128, 0, 255]
    assert pixel(image, 12, 40) == [128, 128, 100, 255]
    assert pixel(image, 300, 200) == [0, 0, 161, 255]
    assert pixel(image, 16, 70) == [102, 102, 222, 255]
    assert pixel(image, 35, 100) == [127, 0, 100, 255]
    assert pixel(image, 12, 100) == [0, 0, 0, 255]
    assert pixel(image, 132, 100) == [0, 0, 0, 0]
    assert pixel(image, 140, 100) == [0, 0, 200, 255]
    assert pixel(image, 208, 16) == [100, 10, 120, 255]
    assert sum(pixel(image, x, y) != [0, 0, 200, 255] for y in range(8, 56) for x in range(128, 192)) > 40
    assert pixel(image, 0, 0) == [19, 37, 61, 0]
    small = directory / "small.png"
    png(small, 8, 4, lambda x, y: [0, 0, 200, 255])
    book = dict(version=1, tools=[dict(name="green", content=fill(1, 0, 0, 1, 1, [0, 255, 0, 255])["content"])], exports=[dict(name="padded", preset=dict(padding=[2, 2, 2, 2], background=[255, 255, 255, 255]))])
    edits = [dict(action="tool_preset", id=1, name="green"), dict(action="transform_layer", id=1, transform=[1, 0, 0, 1, 2, 0]), dict(action="duplicate", id=1, new_id=2, transform=[1, 0, 0, 1, 0, 1]), dict(action="remove", id=1), dict(action="undo"), dict(action="redo"), dict(action="undo"), dict(action="scale", width=16, height=8), dict(action="crop", area=dict(x=4, y=2, width=8, height=4))]
    profile = write(directory / "edits.json", scene(8, 4, [fill(1, 1, 1, 2, 1, [255, 0, 0, 255])], edits=edits, presets=book, export_preset="padded"))
    output = directory / "edits.png"
    runner.render(small, profile, output)
    image = decode(output)
    expected = bytes(c for y in range(8) for x in range(12) for c in ([255] * 4 if x < 2 or x >= 10 or y < 2 or y >= 6 else ([0, 255, 0, 255] if 4 <= x < 8 else [0, 0, 200, 255])))
    assert image == (12, 8, expected)
    # Both effect input modes are genuine, distinct sources; neither is a fallback.
    for mode, expected_color in [("previous", [127, 0, 100, 255]), ("protected_base", [0, 0, 200, 255])]:
        profile = write(directory / f"mode-{mode}.json", scene(8, 4, [fill(1, 0, 0, 2, 4, [255, 0, 0, 255]), layer(2, 0, 0, 8, 4, dict(tool="pixelate", block=4, input=mode))]))
        output = directory / f"mode-{mode}.png"
        runner.render(small, profile, output)
        assert pixel(decode(output), 3, 1) == expected_color
    # Inspection never loads supplied fonts/images or emits literal label content.
    inspect_scene = scene(1280, 720, [layer(1, 0, 0, 640, 128, {"tool": "text", "text": {"spans": [{"text": "SYNTHETIC_PRIVATE_LABEL", "style": text_style(["missing"])}]}})], fonts=[dict(id="missing", path="not-a-real-font.ttf")])
    profile = write(directory / "inspect.json", inspect_scene)
    inspection = runner.call("inspect", profile)
    assert inspection["fonts"] == 1 and "SYNTHETIC_PRIVATE_LABEL" not in json.dumps(inspection)
    profile = write(directory / "plain.json", scene(8, 4))
    error = runner.call("render", directory / "missing.png", profile, directory / "wrong.jpg", ok=False)
    assert "conflicts" in error
    sentinel = directory / "existing.png"
    sentinel.write_bytes(b"do not overwrite")
    assert "already exists" in runner.call("render", directory / "missing.png", profile, sentinel, ok=False)
    assert sentinel.read_bytes() == b"do not overwrite"
    for alias_kind in ("hardlink", "symlink"):
        alias = directory / f"alias-{alias_kind}.png"
        if alias_kind == "hardlink":
            os.link(small, alias)
        else:
            try:
                alias.symlink_to(small)
            except OSError:
                continue
        profile = write(directory / f"alias-{alias_kind}.json", scene(8, 4, [layer(1, 0, 0, 4, 4, dict(tool="raster", asset="bad", kind="image"))], rasters=[dict(id="bad", path=alias.name)]))
        output = directory / f"refused-{alias_kind}.png"
        assert "original source file" in runner.call("render", small, profile, output, ok=False)
        assert not output.exists()
    # All advertised export presets call real encoders and retain dimensions.
    for format_name, extension in [("png", "png"), ("jpeg", "jpg"), ("webp", "webp"), ("webp_lossy", "webp")]:
        value = scene(8, 4)
        value["document"]["export"] = dict(format=format_name, quality=75, background=[255] * 4)
        profile = write(directory / f"format-{format_name}.json", value)
        output = directory / f"format-{format_name}.{extension}"
        saved = runner.render(small, profile, output)
        assert saved["format"] == format_name
        assert decode(output)[:2] == (8, 4)
    assert hashlib.sha256(source.read_bytes()).hexdigest() == initial_hash
    rotated = rotated_privacy(runner, directory)
    invalid = scene(8, 4, [fill(1, 1, 1, 2, 1, [0, 255, 0, 255])], edits=[dict(action="remove", id=1), dict(action="reorder", id=1, index=0)], fonts=[dict(id="unused", path="missing.ttf")])
    profile = write(directory / "invalid-sequence.json", invalid)
    inspect_error = runner.call("inspect", profile, ok=False)
    render_error = runner.call("render", directory / "missing.png", profile, directory / "invalid-sequence.png", ok=False)
    assert inspect_error == render_error and "Could not open editor source" not in render_error
    for use_preset in (False, True):
        invalid = scene(1, 1, fonts=[dict(id="unused", path="missing.ttf")])
        padding = dict(padding=[8192] * 4)
        if use_preset:
            invalid["presets"] = dict(version=1, exports=[dict(name="huge", preset=padding)])
            invalid["export_preset"] = "huge"
            invalid["edits"] = [dict(action="resize_canvas", width=2, height=2, anchor=[0, 0])]
        else:
            invalid["document"]["export"] = padding
        profile = write(directory / f"huge-{use_preset}.json", invalid)
        inspect_error = runner.call("inspect", profile, ok=False)
        output = directory / f"huge-{use_preset}.png"
        render_error = runner.call("render", directory / "missing.png", profile, output, ok=False)
        assert inspect_error == render_error and "256 MiB" in render_error and not output.exists()
    for subtables in (1, 512):
        font = directory / f"budget-{subtables}.ttf"
        font_budget_fixture(font, subtables)
        profile = write(directory / f"font-budget-{subtables}.json", scene(1280, 720, [layer(1, 0, 0, 128, 64, dict(tool="text", text=dict(spans=[dict(text="AA", style=text_style(["budget"]))])))], fonts=[dict(id="budget", path=font.name)]))
        output = directory / f"font-budget-{subtables}.png"
        if subtables == 1:
            runner.render(source, profile, output)
            assert any(pixel(decode(output), x, y) != [0, 0, 200, 255] for y in range(0, 32) for x in range(16, 48))
        else:
            error = runner.call("render", source, profile, output, ok=False)
            assert "preprocessing exhausted" in error and not output.exists()
            assert not list(directory.glob(".screenshots-*.part"))
    legacy_outputs = {}
    for kind, apple in [("ot", False), ("apple", True)]:
        for adjustment in (0, -120):
            font = directory / f"legacy-{kind}-{adjustment}.ttf"
            legacy_kern_fixture(font, 1, apple=apple, adjustment=adjustment)
            profile = write(directory / f"legacy-{kind}-{adjustment}.json", scene(1280, 720, [layer(1, 0, 0, 128, 64, dict(tool="text", text=dict(spans=[dict(text="AV", style=text_style(["legacy"]))])))], fonts=[dict(id="legacy", path=font.name)]))
            output = directory / f"legacy-{kind}-{adjustment}.png"
            runner.render(source, profile, output)
            legacy_outputs[kind, adjustment] = decode(output)
        assert legacy_outputs[kind, 0] != legacy_outputs[kind, -120], "real legacy kerning must change glyph pixels"
    assert legacy_outputs["ot", -120] == legacy_outputs["apple", -120]
    font = directory / "legacy-80000.ttf"
    legacy_kern_fixture(font, 80_000)
    profile = write(directory / "legacy-80000.json", scene(1280, 720, [layer(1, 0, 0, 128, 64, dict(tool="text", text=dict(spans=[dict(text="AV", style=text_style(["legacy"]))])))], fonts=[dict(id="legacy", path=font.name)]))
    output = directory / "legacy-80000.png"
    error = runner.call("render", source, profile, output, ok=False)
    assert "legacy kern preprocessing" in error and not output.exists()
    assert not list(directory.glob(".screenshots-*.part"))
    return dict(legacy_kern_80000_attack_refused=True, real_ot_apple_kerning_pixels=True, final_output_preflight_before_asset_io=True, real_font_preprocessing_exhaustion_refused=True, all_tool_pixels=True, exact_edit_preset_pixels=True, effect_input_modes=True, source_aliases_refused=True, no_overwrite=True, format_mismatch_before_source=True, inspection_has_no_asset_io_or_text=True, sequential_preflight_before_decode=True, rotated_sheared_bilinear_clone_lens_privacy=rotated, formats=4, source_preserved=True, renders=runner.renders, **check_cancellation(runner, directory))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release-fast/screenshots")
    parser.add_argument("--report", type=Path, default=ROOT / "target/verification/editor-document-e2e.json")
    args = parser.parse_args()
    runner = Runner(args.binary)
    try:
        with tempfile.TemporaryDirectory(prefix="screenshots-editor-") as temp:
            report = run(runner, Path(temp))
        report.update(passed=True, binary_sha256=runner.digest, real_device_or_account_access=False, native_ui_acceptance=False)
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report))
    finally:
        runner.close()


if __name__ == "__main__":
    main()
