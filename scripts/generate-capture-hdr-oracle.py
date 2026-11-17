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
    for y in [0, 2**-24, 2**-12, .003, .03125, .125, .25, .5, .75, .7501, 1, 2, 4, 12.5, 125]:
        for hdr in [False, True]:
            rgba = [y, y, y, 1]
            cases.append([rgba, hdr, reference(rgba, hdr)])
    for rgba in [[1, .125, .03125, 1], [4, 1, -.125, 1], [-.25, 1, .5, 1],
                 [.125, .0625, 0, .5], [2, 1, .5, .5], [12.5, 3, -2, 0]]:
        for hdr in [False, True]:
            cases.append([rgba, hdr, reference(rgba, hdr)])
root = Path(__file__).resolve().parents[1]
output = root / 'crates/app-core/tests/fixtures/color/capture-hdr-oracle.json'
output.parent.mkdir(parents=True, exist_ok=True)
output.write_text('[\n' + ',\n'.join(json.dumps(case, separators=(',', ':')) for case in cases) + '\n]\n')
# Analytic boundaries independent of production implementation.
assert reference([0, 0, 0, 1], True) == [0, 0, 0, 255]
assert reference([1, 1, 1, 1], False) == [255, 255, 255, 255]
assert reference([1, 1, 1, 1], True) == [240, 240, 240, 255]
print(json.dumps({'cases': len(cases), 'precision': 80, 'oracle': 'Python Decimal; f32 input normalization; IEC sRGB output'}))

# Exhaustive independent CPython binary16 decoder digest (exclude NaN payloads,
# whose propagation is intentionally implementation-specific).
import hashlib
hash_value = hashlib.sha256()
count = 0
for bits in range(65536):
    if bits & 0x7c00 == 0x7c00 and bits & 1023:
        continue
    value = struct.unpack('<e', struct.pack('<H', bits))[0]
    hash_value.update(struct.pack('<f', value))
    count += 1
metadata = {'non_nan_encodings': count, 'f32_le_sha256': hash_value.hexdigest(), 'decoder': 'CPython struct IEEE binary16'}
(root / 'crates/app-core/tests/fixtures/color/capture-half-decoder-oracle.json').write_text(json.dumps(metadata, indent=2) + '\n')
print(json.dumps(metadata))
