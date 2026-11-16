#!/usr/bin/env python3
"""Explicit native identity selectors and owned-staging signing; never configures trust."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import tempfile
import sys
import errno
import signal
import threading
import time
from urllib.parse import urlsplit

ROOT=Path(__file__).resolve().parents[1]
SPEC=importlib.util.spec_from_file_location('signing_package',ROOT/'scripts/package.py')
PACKAGE=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(PACKAGE)
SPEC=importlib.util.spec_from_file_location('signing_install',ROOT/'scripts/install-user.py')
INSTALL=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(INSTALL)


def text(value,limit=1024):
    INSTALL.require(type(value) is str and 0<len(value.encode('utf-8'))<=limit and not value.startswith('-')
                    and not any(ord(char)<32 or ord(char)==127 for char in value),'Invalid public signing selector')
    return value


def profile(path,platform):
    value=INSTALL.read_json(path,16384)
    common={'version','platform'}
    fields={'macos':{'application_identity','installer_identity','team_id'},
            'windows':{'thumbprint','publisher','timestamp_url'}}
    INSTALL.require(type(value) is dict and platform in fields and set(value)==common|fields[platform]
                    and type(value['version']) is int and value['version']==1 and value['platform']==platform,
                    'Signing profile must match the native platform and exact version1 schema')
    for key in fields[platform]:text(value[key],4096 if key=='timestamp_url' else 1024)
    if platform=='macos':
        INSTALL.require(re.fullmatch('[A-Z0-9]{10}',value['team_id']), 'Invalid application team')
        for field,role in [('application_identity','Application'),('installer_identity','Installer')]:
            INSTALL.require(value[field].startswith('Developer ID '+role+': ')
                            and value[field].endswith(' ('+value['team_id']+')'),
                            'Exact Developer ID identity names must match the configured team')
    else:
        INSTALL.require(re.fullmatch('[a-fA-F0-9]{40}',value['thumbprint']),'Certificate thumbprint must contain40 hexadecimal digits')
        url=urlsplit(value['timestamp_url'])
        INSTALL.require(url.scheme=='https' and url.hostname and not url.username and not url.password and not url.fragment,
                        'Timestamp endpoint must be HTTPS without credentials/fragment')
    return value


MAC_TOOLS={'codesign':'/usr/bin/codesign','productsign':'/usr/bin/productsign','pkgbuild':'/usr/bin/pkgbuild',
           'pkgutil':'/usr/sbin/pkgutil','xcrun':'/usr/bin/xcrun'}

def native_tool(name):
    if name in MAC_TOOLS:
        path=Path(MAC_TOOLS[name])
        INSTALL.require(path.is_file() and not path.is_symlink(),'Required protected macOS tool is unavailable')
        return str(path)
    INSTALL.require(name in ('signtool.exe','makeappx.exe') and os.name=='nt','Unsupported native signing tool')
    import winreg
    with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE,r'SOFTWARE\Microsoft\Windows Kits\Installed Roots',0,
                        winreg.KEY_READ|winreg.KEY_WOW64_32KEY) as key:
        root,kind=winreg.QueryValueEx(key,'KitsRoot10')
    INSTALL.require(kind==winreg.REG_SZ and Path(root).is_absolute(),'Invalid protected SDK registry root')
    candidates=list((Path(root)/'bin').glob('*/x64/'+name))
    candidates=[path for path in candidates if re.fullmatch(r'[0-9]+(?:\.[0-9]+)+',path.parent.parent.name)
                and path.is_file() and not path.is_symlink()]
    INSTALL.require(candidates,'Windows SDK signing tool is unavailable')
    return str(max(candidates,key=lambda path:tuple(map(int,path.parent.parent.name.split('.')))).resolve())


def native_environment():
    environment=dict(os.environ)
    for name in list(environment):
        upper=name.upper()
        if upper.startswith(('DYLD_','LD_','XCRUN_')) or upper in ('DEVELOPER_DIR','SDKROOT','TOOLCHAINS','CODESIGN_ALLOCATE'):
            environment.pop(name,None)
    if os.name=='nt':
        import ctypes
        buffer=ctypes.create_unicode_buffer(32768)
        length=ctypes.windll.kernel32.GetSystemDirectoryW(buffer,len(buffer))
        INSTALL.require(0<length<len(buffer),'Could not resolve protected system directory')
        environment['PATH']=buffer.value
    else:environment['PATH']='/usr/bin:/bin:/usr/sbin:/sbin'
    environment['LC_ALL']='C';environment['LANG']='C'
    return environment


def native_exit_marker(pid,flags):
    # Darwin Python omits waitid, but libSystem exposes the public XNU ABI.
    # siginfo_t starts with int si_signo; aligned512B storage exceeds its104B
    # Darwin64 size (also accommodates Linux128B for the direct native regression).
    import ctypes
    class Info(ctypes.Union):
        _fields_=[('words',ctypes.c_uint64*64),('signo',ctypes.c_int)]
    library=ctypes.CDLL(None,use_errno=True)
    function=library.waitid
    function.argtypes=[ctypes.c_int,ctypes.c_uint,ctypes.c_void_p,ctypes.c_int]
    function.restype=ctypes.c_int
    info=Info()
    if function(1,pid,ctypes.byref(info),flags)!=0: # P_PID
        code=ctypes.get_errno()
        if code==errno.EINTR:return False
        raise OSError(code,'Native nonreaping child observation failed')
    pid_offset=12 if sys.platform=='darwin' else 16 if sys.platform.startswith('linux') else None
    INSTALL.require(pid_offset is not None,'Unsupported native child-info layout')
    return info.signo==signal.SIGCHLD and ctypes.c_int.from_buffer(info,pid_offset).value==pid


def child_exited_unreaped(pid):
    if hasattr(os,'waitid') and hasattr(os,'WNOWAIT'):
        result=os.waitid(os.P_PID,pid,os.WEXITED|os.WNOHANG|os.WNOWAIT)
        return result is not None and result.si_pid==pid
    INSTALL.require(sys.platform=='darwin','Native signer needs nonreaping child observation')
    return native_exit_marker(pid,0x04|0x01|0x20) # XNU WEXITED|WNOHANG|WNOWAIT


def darwin_only_held_zombie(pid):
    # XNU killpg excludes zombies and reports EPERM for a zombie-only group.
    # Accept only an exact snapshot containing our still-unreaped leader alone;
    # any extra member, truncation, native error or lost reservation fails closed.
    if sys.platform!='darwin' or not child_exited_unreaped(pid):return False
    import ctypes
    library=ctypes.CDLL('/usr/lib/libproc.dylib',use_errno=True)
    function=library.proc_listpgrppids
    function.argtypes=[ctypes.c_int,ctypes.c_void_p,ctypes.c_int];function.restype=ctypes.c_int
    members=(ctypes.c_int*2)();ctypes.set_errno(0)
    count=function(pid,ctypes.byref(members),ctypes.sizeof(members))
    return ctypes.get_errno()==0 and count==1 and members[0]==pid and child_exited_unreaped(pid)


class Tools:
    def run(self,args,timeout=120,capture=False):
        # Explicit release operation, with no signer diagnostics copied into application logs.
        args=list(map(str,args))
        is_native=args[0] in MAC_TOOLS or args[0] in ('signtool.exe','makeappx.exe')
        if is_native:args[0]=native_tool(args[0])
        working=tempfile.TemporaryDirectory(prefix='screenshots-sign-tool-')
        if os.name=='nt':
            spec=importlib.util.spec_from_file_location('native_signing_windows',ROOT/'scripts/native-signing-windows.py')
            native=importlib.util.module_from_spec(spec);spec.loader.exec_module(native)
            try:return native.run(args,native_environment() if is_native else dict(os.environ),working.name,timeout,capture)
            finally:working.cleanup()
        if os.name!='nt':
            INSTALL.require((hasattr(os,'waitid') and hasattr(os,'WNOWAIT')) or sys.platform=='darwin','Native signer needs nonreaping child observation')
        process=subprocess.Popen(args,stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE if capture else subprocess.DEVNULL,stderr=subprocess.DEVNULL,
            start_new_session=os.name!='nt',cwd=working.name,
            env=native_environment() if is_native else None)
        output=bytearray();excess=threading.Event();reader=None
        reaped=False;wait_started=False
        def stop():
            if os.name!='nt':
                if reaped or wait_started or process.returncode is not None:return
                # wait() can reap then raise KeyboardInterrupt; never signal a
                # numeric group once any reaping wait has started.
                try:os.killpg(process.pid,signal.SIGKILL)
                except ProcessLookupError:pass
                except PermissionError:
                    if not darwin_only_held_zombie(process.pid):raise
            elif process.poll() is None:process.kill()
        def read():
            try:
                while chunk:=process.stdout.read(4096):
                    if len(chunk)>128*1024-len(output):excess.set();return
                    output.extend(chunk)
            finally:process.stdout.close()
        try:
            if capture:
                reader=threading.Thread(target=read,daemon=True);reader.start()
            if os.name!='nt':
                deadline=time.monotonic()+timeout
                while True:
                    if child_exited_unreaped(process.pid):break
                    if excess.is_set() or time.monotonic()>=deadline:
                        stop()
                        wait_started=True
                        process.wait(timeout=5);reaped=True
                        raise ValueError('Native signing tool exceeded its output or time budget')
                    time.sleep(.01)
                # The unreaped leader reserves its group until all owned descendants are signaled.
                stop()
                wait_started=True
                code=process.wait(timeout=5);reaped=True
            else:
                try:code=process.wait(timeout=timeout)
                except subprocess.TimeoutExpired as error:
                    stop();process.wait(timeout=5)
                    raise ValueError('Native signing/verification tool exceeded its deadline') from error
            if reader:
                reader.join(timeout=2)
                INSTALL.require(not reader.is_alive(),'Native signer left its result pipe open')
            INSTALL.require(not excess.is_set(),'Native signing result exceeds128KiB')
            INSTALL.require(code==0,'Native signing/verification failed; no signed artifact was published')
            return bytes(output)
        finally:
            try:
                if not reaped:
                    try:stop()
                    finally:
                        wait_started=True
                        process.wait(timeout=5)
                        reaped=True
            finally:
                try:
                    if reader is not None and reader.ident is not None:
                        reader.join(timeout=2)
                    elif capture:
                        process.stdout.close()
                finally:working.cleanup()



def digest(path):
    with path.open('rb') as source:return hashlib.file_digest(source,'sha256').hexdigest()


def runtime_copy(source,destination,target,expected_anchor):
    # Reuse the complete bounded manifest verifier rather than trusting an archive's sidecar alone.
    _,runtime=PACKAGE.runtime_bundle(source,destination,target)
    INSTALL.require(runtime['manifest_sha256']==expected_anchor,
                    'Incoming private runtime does not match its admitted build-info anchor')


def refresh_runtime(runtime):
    manifest=INSTALL.read_json(runtime/'RUNTIME-MANIFEST.json',128*1024)
    for entry in manifest['files']:
        path=runtime/entry['path'];metadata=path.stat()
        INSTALL.require(path.is_file() and not path.is_symlink() and metadata.st_size<=64*1024*1024,
                        'Signed runtime member changed type or exceeded its size limit')
        entry['bytes']=metadata.st_size;entry['sha256']=digest(path)
    INSTALL.require(sum(entry['bytes'] for entry in manifest['files'])<=256*1024*1024,'Signed runtime exceeds aggregate limit')
    (runtime/'RUNTIME-MANIFEST.json').write_text(json.dumps(manifest,separators=(',',':'))+'\n',encoding='utf-8',newline='\n')
    return digest(runtime/'RUNTIME-MANIFEST.json')


def native_macho(path):
    with path.open('rb') as source:magic=source.read(4)
    return magic in (b'\xcf\xfa\xed\xfe',b'\xce\xfa\xed\xfe',b'\xfe\xed\xfa\xcf',b'\xfe\xed\xfa\xce',b'\xca\xfe\xba\xbe',b'\xbe\xba\xfe\xca')


def verify_mac(tools,path,signing):
    requirement='anchor apple generic and certificate leaf[subject.OU] = "'+signing['team_id']+'"'
    tools.run(['codesign','--verify','--strict','-R',requirement,path])


def mac_app(app,info,signing,tools=None):
    tools=tools or Tools();app=app.resolve();resources=app/'Contents/Resources';runtime=resources/'lib/screenshots-sxcu'
    with tempfile.TemporaryDirectory(prefix='.verified-runtime-',dir=app.parent) as temporary:
        verified=Path(temporary)/'runtime';runtime_copy(runtime,verified,info['target'],info.get('sharex_runtime',{}).get('manifest_sha256'))
        shutil.rmtree(runtime);shutil.copytree(verified,runtime)
    entitlements=app.parent/'sxcu-jit-entitlements.plist'
    with entitlements.open('xb') as output:plistlib.dump({'com.apple.security.cs.allow-jit':True},output)
    try:
        # Runtime files are flat/native plus a bounded license directory, with no nested bundles.
        manifest=INSTALL.read_json(runtime/'RUNTIME-MANIFEST.json',128*1024)
        for entry in manifest['files']:
            path=runtime/entry['path']
            if native_macho(path):
                args=['codesign','--force','--sign',signing['application_identity'],'--options','runtime','--timestamp']
                if entry['path']==manifest['entrypoint']:
                    args+=['--identifier','me.lejy.screenshots.sxcu','--entitlements',entitlements]
                tools.run([*args,path]);verify_mac(tools,path,signing)
        updated=dict(info)
        updated['sharex_runtime']=dict(info['sharex_runtime'],manifest_sha256=refresh_runtime(runtime))
        updated['binary_sha256_scope']='admitted executable before enclosing bundle signature'
        updated['signing']='Developer ID team '+signing['team_id']+'; native signature verified before publication'
        (resources/'BUILD-INFO.json').write_text(json.dumps(updated,indent=2)+'\n',encoding='utf-8',newline='\n')
        tools.run(['codesign','--force','--sign',signing['application_identity'],'--identifier','me.lejy.screenshots',
                   '--options','runtime','--timestamp',app])
        verify_mac(tools,app,signing)
        tools.run(['codesign','--verify','--deep','--strict',app])
        return updated
    finally:entitlements.unlink(missing_ok=True)


def windows_payload(portable,info,signing,tools=None):
    tools=tools or Tools();portable=portable.resolve();runtime=portable/'lib/screenshots-sxcu'
    with tempfile.TemporaryDirectory(prefix='.verified-runtime-',dir=portable.parent) as temporary:
        verified=Path(temporary)/'runtime';runtime_copy(runtime,verified,info['target'],info.get('sharex_runtime',{}).get('manifest_sha256'))
        shutil.rmtree(runtime);shutil.copytree(verified,runtime)
    manifest=INSTALL.read_json(runtime/'RUNTIME-MANIFEST.json',128*1024)
    executables=[portable/'screenshots.exe',runtime/manifest['entrypoint']]
    for path in executables:
        tools.run(['signtool.exe','sign','/fd','SHA256','/sha1',signing['thumbprint'],
                   '/tr',signing['timestamp_url'],'/td','SHA256',path])
        tools.run(['signtool.exe','verify','/pa','/all',path])
    updated=dict(info,binary_sha256=digest(portable/'screenshots.exe'),signing='Authenticode verified before packaging')
    updated['sharex_runtime']=dict(info['sharex_runtime'],manifest_sha256=refresh_runtime(runtime))
    (portable/'BUILD-INFO.json').write_text(json.dumps(updated,indent=2)+'\n',encoding='utf-8',newline='\n')
    return updated


def windows_package(path,signing,tools=None):
    tools=tools or Tools();path=path.resolve()
    tools.run(['signtool.exe','sign','/fd','SHA256','/sha1',signing['thumbprint'],
               '/tr',signing['timestamp_url'],'/td','SHA256',path])
    tools.run(['signtool.exe','verify','/pa','/all',path])


def mac_package(unsigned,signed,signing,notary_profile=None,tools=None):
    tools=tools or Tools();unsigned=unsigned.resolve();signed=signed.resolve()
    tools.run(['productsign','--sign',signing['installer_identity'],unsigned,signed])
    certificate=tools.run(['pkgutil','--check-signature',signed],capture=True).decode('utf-8')
    leaf=re.search(r'^\s*1\.\s+(.+?)\s*$',certificate,re.MULTILINE)
    INSTALL.require(leaf and leaf.group(1)==signing['installer_identity']
                    and re.search(r'^\s*Status:\s+signed by (?:a certificate trusted by (?:macOS|Mac OS X)|a developer certificate issued by Apple for distribution)\s*$',certificate,re.MULTILINE),
                    'Signed installer certificate/team does not match the requested trusted identity')
    if notary_profile is not None:
        text(notary_profile,256)
        response=tools.run(['xcrun','notarytool','submit',signed,'--keychain-profile',notary_profile,
                            '--wait','--timeout','15m','--output-format','json'],timeout=930,capture=True)
        result=json.loads(response)
        INSTALL.require(type(result) is dict and result.get('status')=='Accepted','Notarization did not accept the package')
        tools.run(['xcrun','stapler','staple',signed]);tools.run(['xcrun','stapler','validate',signed])
    return {'signed':True,'notarized':notary_profile is not None}
