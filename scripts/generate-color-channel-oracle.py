#!/usr/bin/env python3
"""Generate per-channel ICC fixtures with LittleCMS; no moxcms implementation is used."""
import ctypes as c
import ctypes.util
import json
from pathlib import Path
import struct
import zlib

out = Path('crates/app-core/tests/fixtures/color/channels')
out.mkdir(parents=True, exist_ok=True)
lcms = c.CDLL(ctypes.util.find_library('lcms2'))
class xyY(c.Structure):
    _fields_ = [('x', c.c_double), ('y', c.c_double), ('Y', c.c_double)]
class Triple(c.Structure):
    _fields_ = [('r', xyY), ('g', xyY), ('b', xyY)]
for name, args, result in [
    ('cmsBuildParametricToneCurve', [c.c_void_p,c.c_int,c.POINTER(c.c_double)], c.c_void_p),
    ('cmsBuildTabulatedToneCurve16', [c.c_void_p,c.c_uint32,c.POINTER(c.c_uint16)], c.c_void_p),
    ('cmsCreateRGBProfile', [c.POINTER(xyY),c.POINTER(Triple),c.POINTER(c.c_void_p)], c.c_void_p),
    ('cmsCreate_sRGBProfile', [], c.c_void_p),
    ('cmsSaveProfileToMem', [c.c_void_p,c.c_void_p,c.POINTER(c.c_uint32)], c.c_int),
    ('cmsOpenProfileFromMem', [c.c_void_p,c.c_uint32], c.c_void_p),
    ('cmsCreateTransform', [c.c_void_p,c.c_uint32,c.c_void_p,c.c_uint32,c.c_uint32,c.c_uint32], c.c_void_p),
    ('cmsDoTransform', [c.c_void_p,c.c_void_p,c.c_void_p,c.c_uint32], None),
    ('cmsDeleteTransform', [c.c_void_p], None),
    ('cmsCloseProfile', [c.c_void_p], c.c_int),
    ('cmsFreeToneCurve', [c.c_void_p], None),
    ('cmsGetEncodedCMMversion', [], c.c_uint32),
]:
    function = getattr(lcms, name)
    function.argtypes, function.restype = args, result

def chunk(kind, data):
    return struct.pack('>I',len(data))+kind+data+struct.pack('>I',zlib.crc32(kind+data))
def profile_bytes(profile):
    size = c.c_uint32()
    assert lcms.cmsSaveProfileToMem(profile, None, c.byref(size))
    data = c.create_string_buffer(size.value)
    assert lcms.cmsSaveProfileToMem(profile, data, c.byref(size))
    return data.raw[:size.value]
def trc_types(data):
    tags = {}
    for index in range(struct.unpack('>I',data[128:132])[0]):
        at = 132+index*12
        name, start, length = struct.unpack('>4sII',data[at:at+12])
        if name in [b'rTRC',b'gTRC',b'bTRC']:
            tags[name.decode()] = data[start:start+4].decode()
    return tags

samples = [(32896,32896,32896),(0,0,0),(65535,65535,65535),(1,128,257),
           (27886,60187,18886),(40000,15000,52000),(12345,34567,23456),
           (30000,30000,40000),(65535,0,0),(0,65535,0),(0,0,65535),(20000,40000,30000)]
alphas = [0,1,65535,128,12345,32768,65534,50000,257,32767,65535,40000]
raw = b''.join(b'\0'+b''.join(struct.pack('>4H',*samples[y*3+x],alphas[y*3+x]) for x in range(3)) for y in range(4))
input_rgb = (c.c_uint16*(len(samples)*3))(*(value for sample in samples for value in sample))
srgb = lcms.cmsCreate_sRGBProfile()
manifest = {'oracle':'LittleCMS', 'encoded_version':lcms.cmsGetEncodedCMMversion(),
            'source_rgb16':samples, 'source_alpha16':alphas, 'cases':[]}
for kind in ['para','curv']:
    for pair, gammas in [('rg',(1,1,2)),('gb',(2,1,1)),('rb',(1,2,1)),('equal1',(1,1,1)),('equal2',(2,2,2))]:
        curves = []
        for gamma in gammas:
            if kind == 'para':
                curve = lcms.cmsBuildParametricToneCurve(None,1,(c.c_double*1)(gamma))
            else:
                table = (c.c_uint16*257)(*(round((index/256)**gamma*65535) for index in range(257)))
                curve = lcms.cmsBuildTabulatedToneCurve16(None,257,table)
            assert curve
            curves.append(curve)
        profile = lcms.cmsCreateRGBProfile(c.byref(xyY(.3127,.329,1)),
            c.byref(Triple(xyY(.64,.33,1),xyY(.30,.60,1),xyY(.15,.06,1))),
            (c.c_void_p*3)(*curves))
        assert profile
        data = profile_bytes(profile)
        assert set(trc_types(data).values()) == {kind}
        # Reopen the exact serialized ICC, so the oracle includes its fixed-point matrix/TRCs.
        buffer = c.create_string_buffer(data)
        source_profile = lcms.cmsOpenProfileFromMem(buffer,len(data))
        assert source_profile
        transform = lcms.cmsCreateTransform(source_profile,(4<<16)|(3<<3)|2,
            srgb,(4<<16)|(3<<3)|1,1,0x0100|0x0040)
        assert transform
        output_rgb = (c.c_uint8*(len(samples)*3))()
        lcms.cmsDoTransform(transform,input_rgb,output_rgb,len(samples))
        expected = bytes(value for index in range(len(samples))
            for value in [*output_rgb[index*3:index*3+3],(alphas[index]+128)//257])
        name = f'{kind}-{pair}'
        (out/f'{name}.icc').write_bytes(data)
        (out/f'{name}.rgba').write_bytes(expected)
        png = b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',3,4,16,6,0,0,0))
        png += chunk(b'iCCP',b'channel-profile\0\0'+zlib.compress(data))+chunk(b'IDAT',zlib.compress(raw))+chunk(b'IEND',b'')
        (out/f'{name}.png').write_bytes(png)
        manifest['cases'].append({'name':name,'gamma':gammas,'trc_type':trc_types(data),
                                  'mid_rgb8':list(expected[:3]),'icc_bytes':len(data)})
        lcms.cmsDeleteTransform(transform)
        lcms.cmsCloseProfile(source_profile)
        lcms.cmsCloseProfile(profile)
        for curve in curves:lcms.cmsFreeToneCurve(curve)
lcms.cmsCloseProfile(srgb)
(out/'oracle.json').write_text(json.dumps(manifest,indent=2)+'\n')
(out/'README.txt').write_text('Generated by scripts/generate-color-channel-oracle.py with system LittleCMS 2.19.\n'
    'D65 sRGB primaries; independent channel gamma1/gamma2 combinations.\n'
    'para profiles use ICC parametric function0. curv profiles use 257-entry monotonic tables.\n'
    'The exact written ICC is reopened with LittleCMS before RGB16 -> sRGB8 conversion.\n'
    'Relative colorimetric, NOOPTIMIZE|NOCACHE; alpha is independently rounded, never premultiplied.\n'
    'oracle.json records source tuples, alpha, tag types and expected RGB128 midpoint.\n'
    'For r=g=gamma1 and b=gamma2, RGB128 maps to about [188,188,137]; reusing red for blue gives188.\n'
    'No moxcms code or result is used to generate the profiles or expected pixels.\n'
    'ICC creation timestamps may change on regeneration; pixel oracles stay stable.\n')
print(json.dumps({'oracle':'LittleCMS','version':manifest['encoded_version'],
                  'midpoints':{case['name']:case['mid_rgb8'] for case in manifest['cases']}}))
