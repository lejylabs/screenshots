#!/usr/bin/env python3
"""Independent Decimal(80) SDR rendition oracle; no Rust/module invocation."""
import json
import struct
from decimal import Decimal, localcontext, ROUND_FLOOR
from pathlib import Path
D = Decimal

def exact_f32(value):
    return D.from_float(struct.unpack('<f', struct.pack('<f', value))[0])

def reference(values, hdr):
    r, g, b, alpha = map(exact_f32, values)
    if not alpha:
        return [0, 0, 0, 0]
    rgb = [r / alpha, g / alpha, b / alpha]
    luminance = sum(a * b for a, b in zip(rgb, map(D, ['.2126', '.7152', '.0722'])))
    if hdr and luminance > D('.75'):
        d = luminance - D('.75')
        scale = (D('.75') + d / (1 + 4 * d)) / luminance
        rgb = [channel * scale for channel in rgb]
    encoded = []
    for value in rgb:
        value = max(D(0), min(D(1), value))
        value = value * D('12.92') if value <= D('.0031308') else D('1.055') * value ** (D(1) / D('2.4')) - D('.055')
        encoded.append(int((value * 255 + D('.5')).to_integral_value(rounding=ROUND_FLOOR)))
    return encoded + [int((alpha * 255 + D('.5')).to_integral_value(rounding=ROUND_FLOOR))]


cases = []
with localcontext() as context:
    context.prec = 80
    for white in [1000, 2000, 4000]:
        for hdr in [False, True]:
            for rows in [
                [[.125, .5, 2, 1], [12.5, 1, -.125, 1]],
                [[.125, .0625, 0, .5], [2, 1, .5, .5]],
                [[2**-24, .75, .25, 1], [12.5, 3, -2, 0]],
            ]:
                source = bytearray()
                expected = []
                normalized = []
                for rgba in rows:
                    packed = struct.pack('<4e', *rgba)
                    source.extend(packed + bytes([199] * 16))
                    native = list(struct.unpack('<4e', packed))
                    for c in range(3):
                        native[c] = float(exact_f32(native[c] / (white / 1000)))
                    normalized.append(native)
                extended = any(rgba[3] > 0 and sum(exact_f32(rgba[c]) * D(w) for c, w in enumerate(['.2126', '.7152', '.0722'])) / exact_f32(rgba[3]) > D(1) + D(1)/1024 for rgba in normalized)
                for native in normalized:
                    expected += reference(native, hdr and extended)
                cases.append([1, 2, 24, list(source), hdr, white, expected])
root = Path(__file__).resolve().parents[1]
output = root / 'crates/app-core/tests/fixtures/color/capture-half-oracle.json'
output.parent.mkdir(parents=True, exist_ok=True)
output.write_text('[\n' + ',\n'.join(json.dumps(case, separators=(',', ':')) for case in cases) + '\n]\n')
assert struct.unpack('<e', bytes([1, 0]))[0] == 2**-24
assert struct.unpack('<e', bytes([0x40, 0x4a]))[0] == 12.5
print(json.dumps({'cases': len(cases), 'precision': 80, 'oracle': 'CPython IEEE binary16; independent Decimal IEC sRGB and shoulder'}))
