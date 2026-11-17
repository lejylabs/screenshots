#!/usr/bin/env python3
"""Independent Decimal(80) xy->XYZ matrices, BT2100 and byte packing oracle."""
import json, struct
from decimal import Decimal as D, localcontext, ROUND_FLOOR
from pathlib import Path

def inverse(m):
    rows = [list(row) + [D(i == j) for j in range(3)] for i, row in enumerate(m)]
    for i in range(3):
        pivot = rows[i][i]
        rows[i] = [v/pivot for v in rows[i]]
        for j in range(3):
            if j != i:
                scale = rows[j][i]
                rows[j] = [a-scale*b for a,b in zip(rows[j], rows[i])]
    return [row[3:] for row in rows]
def mul(m, v): return [sum(a*b for a,b in zip(row,v)) for row in m]
def matrix(name):
    xy = {'bt709': [('0.64','0.33'),('0.30','0.60'),('0.15','0.06')],
          'display_p3': [('0.68','0.32'),('0.265','0.69'),('0.15','0.06')],
          'bt2020': [('0.708','0.292'),('0.170','0.797'),('0.131','0.046')],
          'adobe_rgb': [('0.64','0.33'),('0.21','0.71'),('0.15','0.06')]}[name]
    columns = [[D(x)/D(y),D(1),(1-D(x)-D(y))/D(y)] for x,y in xy]
    m = [list(row) for row in zip(*columns)]
    white = [D('.3127')/D('.3290'),D(1),(1-D('.3127')-D('.3290'))/D('.3290')]
    scale = mul(inverse(m),white)
    return [[v*scale[i] for i,v in enumerate(row)] for row in m]
def signal(v, transfer):
    if transfer == 'linear': return v
    if transfer == 'srgb': return (-1 if v<0 else 1)*(abs(v)/D('12.92') if abs(v)<=D('.04045') else ((abs(v)+D('.055'))/D('1.055'))**D('2.4'))
    if transfer in ['bt709','bt2020_twelve']:
        cutoff,a = (D('.081'),D('.099')) if transfer=='bt709' else (D('.08145'),D('.0993'))
        return (-1 if v<0 else 1)*(abs(v)/D('4.5') if abs(v)<cutoff else ((abs(v)+a)/(1+a))**(1/D('.45')))
    if transfer in ['gamma22','adobe_rgb']: return (-1 if v<0 else 1)*abs(v)**D('2.2' if transfer=='gamma22' else '2.19921875')
    if transfer == 'pq':
        p = v ** (1/D('78.84375'))
        return (max(p-D('.8359375'),D(0))/(D('18.8515625')-D('18.6875')*p))**(1/D('.1593017578125'))*10000/203
    if transfer == 'hlg': return v*v/3 if v<=D('.5') else (((v-D('.55991073'))/D('.17883277')).exp()+D('.28466892'))/12
    raise ValueError(transfer)
def linear(rgba, source):
    if rgba[3] == 0: return [D(0)]*3
    rgb = [v/rgba[3] if source['premultiplied'] else v for v in rgba[:3]]
    rgb = [signal(v,source['color']['transfer']) for v in rgb]
    xyz = matrix(source['color']['primaries'])
    if source['color']['transfer'] == 'hlg':
        y = sum(a*b for a,b in zip(rgb,xyz[1]))
        rgb = [v*(y**D('.2') if y>0 else 0)*1000/203 for v in rgb]
    return mul(inverse(matrix('bt709')),mul(xyz,rgb))
def encode(rgb, alpha, hdr):
    if not alpha: return [0]*4
    y=sum(a*b for a,b in zip(rgb,map(D,['.2126','.7152','.0722'])))
    if hdr and y>D('.75'):
        d=y-D('.75');scale=(D('.75')+d/(1+4*d))/y;rgb=[v*scale for v in rgb]
    result=[]
    for value in rgb:
        value=max(D(0),min(D(1),value))
        value=value*D('12.92') if value<=D('.0031308') else D('1.055')*value**(1/D('2.4'))-D('.055')
        result.append(int((value*255+D('.5')).to_integral_value(rounding=ROUND_FLOOR)))
    return result+[int((alpha*255+D('.5')).to_integral_value(rounding=ROUND_FLOOR))]
def pack(values, packing):
    if packing in ['rgba_f16_le','rgba_f32_le']:
        code='<4e' if packing=='rgba_f16_le' else '<4f';raw=struct.pack(code,*map(float,values));return raw,list(map(D.from_float,struct.unpack(code,raw)))
    if packing in ['rgba8','bgra8','rgbx8','bgrx8']:
        integers=[round(float(v)*255) for v in values];decoded=[D(v)/255 for v in integers]
        if packing in ['bgra8','bgrx8']: integers[0],integers[2]=integers[2],integers[0]
        if packing in ['rgbx8','bgrx8']: decoded[3]=D(1)
        return bytes(integers),decoded
    r,g,b=[round(float(v)*1023) for v in values[:3]];a=round(float(values[3])*3)
    if packing in ['xbgr210_le','bgrx102_le','abgr210_le','bgra102_le']:r,b=b,r
    if packing in ['rgbx102_le','bgrx102_le','rgba102_le','bgra102_le']:word=(r<<22)|(g<<12)|(b<<2)|a
    else:word=(a<<30)|(r<<20)|(g<<10)|b
    decoded=[D(round(float(v)*1023))/1023 for v in values[:3]]+[D(a)/3 if packing in ['argb210_le','abgr210_le','rgba102_le','bgra102_le'] else D(1)]
    return struct.pack('<I',word),decoded

cases=[]
with localcontext() as ctx:
    ctx.prec=80
    definitions=[('linear16','rgba_f16_le','linear','bt709',False),('linear32','rgba_f32_le','linear','display_p3',True),
        ('pq10','xbgr210_le','pq','bt2020',False),('hlg10','argb210_le','hlg','bt2020',False),('p3','rgba8','srgb','display_p3',False),
        ('adobe','rgba8','adobe_rgb','adobe_rgb',False),('bt709','rgba_f32_le','bt709','bt709',False),('bt2020-12','rgba_f16_le','bt2020_twelve','bt2020',False),
        ('pq16','rgba_f16_le','pq','display_p3',False),('hlg32','rgba_f32_le','hlg','bt2020',False),('gamma22','rgba_f32_le','gamma22','bt709',False),('exact-premul','bgra8','srgb','bt709',True)]
    for packing in ['rgba8','bgra8','rgbx8','bgrx8','xrgb210_le','xbgr210_le','rgbx102_le','bgrx102_le','argb210_le','abgr210_le','rgba102_le','bgra102_le']:
        definitions.append(('order-'+packing,packing,'srgb','bt709',False))
    definitions += [('portal-'+n,p,t,pr,False) for n,p,t,pr,pm in definitions[:5]]
    for name,packing,transfer,primaries,premultiplied in definitions:
        bpp=8 if packing=='rgba_f16_le' else 16 if packing=='rgba_f32_le' else 4
        source={'width':4,'height':3,'stride':4*bpp+8,'packing':packing,'premultiplied':premultiplied,'color':{'transfer':transfer,'primaries':primaries}}
        values=[['0','0','0','1'],['.5','.25','.125','1'],['1','1','1','1'],['.75','.75','.75','1'],
            ['.125','.0625','.03125','.5'],['.8','.4','.2','1'],['.0625','.25','.5','1'],['1','.25','.0625','1'],
            ['0','1','0','1'],['.5','.125','.75','1'],['.5','.5','.5','1'],['.5','.25','.125','0']]
        if transfer=='linear': values[2]=['2','2','2','1'];values[9]=['12.5','3','-.25','1']
        if name=='hlg32':values[2]=['1.1','1.1','1.1','1']
        if name.startswith('portal-'):
            source['opaque_alpha_only']=True
            for rgba in values:rgba[3]='1'
        native=bytearray();decoded=[]
        for i,rgba in enumerate(values):
            raw,rgba=pack(list(map(D,rgba)),packing);native+=raw;decoded.append(rgba)
            if i%4==3:native+=bytes([199]*8)
        rgb=[linear(rgba,source) for rgba in decoded]
        hdr_candidate=packing in ['rgba_f16_le','rgba_f32_le'] or transfer in ['linear','pq','hlg']
        hdr=hdr_candidate and any(sum(a*b for a,b in zip(p,map(D,['.2126','.7152','.0722'])))>1+D(1)/1024 for p in rgb)
        if transfer=='srgb' and primaries=='bt709' and packing in ['rgba8','bgra8','rgbx8','bgrx8']:
            output=[]
            for rgba in decoded:
                alpha=int((rgba[3]*255).to_integral_value());channels=[int((v*255).to_integral_value()) for v in rgba[:3]]
                if premultiplied and packing in ['rgba8','bgra8']:
                    channels=[min(255,(v*255+alpha//2)//alpha) if alpha else 0 for v in channels]
                output+=channels+[alpha]
        else:
            output=[v for p,rgba in zip(rgb,decoded) for v in encode(p,rgba[3],hdr)]
        cases.append({'name':name,'source':source,'bytes':list(native),'rgba':output})
root=Path(__file__).resolve().parents[1]
out=root/'crates/app-core/tests/fixtures/color/native-rgb-oracle.json';out.parent.mkdir(parents=True,exist_ok=True)
out.write_text('[\n'+',\n'.join(json.dumps(c,separators=(',',':')) for c in cases)+'\n]\n')
print(json.dumps({'cases':len(cases),'precision':80,'oracle':'independent Decimal xy inversion/BT2100 and CPython binary packing'}))
