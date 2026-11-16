#!/usr/bin/env python3
"""Real isolated image output ENOSPC/fsync/unavailable-parent checks; no disk filling."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import shutil
import struct
import subprocess
import tempfile

ROOT=Path(__file__).resolve().parents[1]

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary",type=Path,required=True)
    parser.add_argument("--report",type=Path)
    args=parser.parse_args()
    if not os.sys.platform.startswith("linux"):
        raise RuntimeError("Native interposer fixture requires Linux")
    binary=args.binary.resolve()
    with tempfile.TemporaryDirectory(prefix="screenshots-image-fault-") as temporary:
        root=Path(temporary)
        frozen=root/"screenshots"
        shutil.copyfile(binary,frozen)
        frozen.chmod(0o755)
        library=root/"fault.so"
        subprocess.run(["cc","-shared","-fPIC","-O2",str(ROOT/"scripts/image-fault-preload.c"),"-ldl","-o",str(library)],check=True,timeout=30)
        width,height=256,128
        data=random.Random(42).randbytes(width*height*3)
        image=root/"input.bmp"
        image.write_bytes(struct.pack("<2sIHHI",b"BM",54+len(data),0,0,54)+struct.pack("<IiiHHIIiiII",40,width,height,1,24,0,len(data),2835,2835,0,0)+data)
        recipe=root/"recipe.json"
        recipe.write_text(json.dumps({"version":1,"steps":[{"op":"crop","x":0,"y":0,"width":width,"height":height}]}))
        before=hashlib.sha256(image.read_bytes()).hexdigest()
        env=dict(os.environ)
        for name in ["DISPLAY","WAYLAND_DISPLAY","DBUS_SESSION_BUS_ADDRESS"]:env.pop(name,None)
        for name in ["HOME","USERPROFILE","APPDATA","LOCALAPPDATA","XDG_CONFIG_HOME","XDG_DATA_HOME","XDG_CACHE_HOME"]:env[name]=str(root/name)
        checks=[]
        for mode in ["enospc","fsync"]:
            folder=root/mode
            folder.mkdir()
            log=root/(mode+".log")
            run_env=dict(env,LD_PRELOAD=str(library),SCREENSHOTS_FIXTURE_IMAGE_MODE=mode,SCREENSHOTS_FIXTURE_IMAGE_PREFIX=str(folder/".screenshots-"),SCREENSHOTS_FIXTURE_IMAGE_LOG=str(log))
            result=subprocess.run([str(frozen),"workflow",str(recipe),str(image),str(folder/"out.png")],env=run_env,capture_output=True,timeout=15)
            assert result.returncode!=0 and not result.stdout,(mode,result.stdout,result.stderr)
            assert log.read_text().startswith("injected"),mode
            assert list(folder.iterdir())==[],mode
            assert hashlib.sha256(image.read_bytes()).hexdigest()==before
            checks.append(mode)
        unavailable=root/"unavailable"
        unavailable.write_bytes(b"owned fixture data stays unchanged")
        result=subprocess.run([str(frozen),"workflow",str(recipe),str(image),str(unavailable/"out.png")],env=env,capture_output=True,timeout=15)
        assert result.returncode!=0 and not result.stdout
        assert unavailable.read_bytes()==b"owned fixture data stays unchanged"
        assert hashlib.sha256(image.read_bytes()).hexdigest()==before
        checks.append("unavailable_parent")
        for name in ["HOME","USERPROFILE","APPDATA","LOCALAPPDATA","XDG_CONFIG_HOME","XDG_DATA_HOME","XDG_CACHE_HOME"]:
            assert not (root/name).exists(),name
        report={"passed":True,"checks":checks,"binary_sha256":hashlib.sha256(frozen.read_bytes()).hexdigest(),"source_unchanged":True,"no_completed_output_or_parts":True,"scope":"owned output paths only; injected Linux write/fsync errors, not physical disk-full or power-loss acceptance"}
        if args.report:
            args.report.parent.mkdir(parents=True,exist_ok=True)
            args.report.write_text(json.dumps(report,indent=2)+"\n")
        print(json.dumps(report))

if __name__=="__main__":main()
