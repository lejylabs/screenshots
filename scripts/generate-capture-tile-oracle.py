#!/usr/bin/env python3
"""Independent Decimal(80) rendition of exact CPython binary16/32 samples."""
from decimal import Decimal as D, localcontext, ROUND_FLOOR
from pathlib import Path
import json,struct

def f32(v):return struct.unpack('<f',struct.pack('<f',v))[0]
def expected(rgba,hdr):
    p=list(map(D.from_float,rgba));a=p[3]
    if a==0:return [0]*4
    rgb=[c/a for c in p[:3]];y=sum(c*w for c,w in zip(rgb,map(D,['.2126','.7152','.0722'])))
    if hdr and y>D('.75'):
        d=y-D('.75');scale=(D('.75')+d/(1+4*d))/y;rgb=[c*scale for c in rgb]
    out=[]
    for c in rgb:
        c=max(D(0),min(D(1),c));c=c*D('12.92') if c<=D('.0031308') else D('1.055')*c**(1/D('2.4'))-D('.055')
        out.append(int((c*255+D('.5')).to_integral_value(rounding=ROUND_FLOOR)))
    return out+[int((a*255+D('.5')).to_integral_value(rounding=ROUND_FLOOR))]
def sample(rgba,white):
    scale=f32(white/1000)
    packed=struct.pack('<4e',*[v*scale if i<3 else v for i,v in enumerate(rgba)])
    half=struct.unpack('<4H',packed);native=struct.unpack('<4e',packed)
    normalized=[f32(v/scale) if i<3 else v for i,v in enumerate(native)]
    return {'half':half,'sdr':expected(normalized,False),'hdr':expected(normalized,True)}
base=[[0,0,0,1],[.125,.25,.5,1],[1,1,1,1],[.25,.125,0,.5],[2**-24,0,0,2**-24],[-.125,.75,.125,1],[.5,.5,.5,1],[12.5,3,-.5,0]]
with localcontext() as ctx:
    ctx.prec=80
    cases=[{'white':white,'samples':[sample(p,white) for p in base],'highlight':sample([12.5,12.5,12.5,1],white)} for white in [1000,1375,2000,4000]]
root=Path(__file__).resolve().parents[1];output=root/'crates/app-core/tests/fixtures/color/capture-tile-oracle.json';output.parent.mkdir(parents=True,exist_ok=True)
output.write_text('[\n'+',\n'.join(json.dumps(case,separators=(',',':')) for case in cases)+'\n]\n')
assert all(c['samples'][2]['sdr']==[255,255,255,255] and c['samples'][2]['hdr']==[240,240,240,255] for c in cases)
print(json.dumps({'whites':len(cases),'samples':sum(len(c['samples'])+1 for c in cases),'oracle':'Decimal80; CPython exact binary16/32 normalization'}))
