#!/usr/bin/env python3
"""Synthetic signing-flow tests: no real certificate, keychain, printer or account calls."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest import mock
import xml.etree.ElementTree as ET

ROOT=Path(__file__).resolve().parents[1]
def module(name,file):
 spec=importlib.util.spec_from_file_location(name,ROOT/'scripts'/file);value=importlib.util.module_from_spec(spec);spec.loader.exec_module(value);return value
SIGN=module('sign_test','native-signing.py');APP=module('app_sign_test','package-app.py')
MAC={'version':1,'platform':'macos','application_identity':'Developer ID Application: Test (ABCDEFGHIJ)','installer_identity':'Developer ID Installer: Test (ABCDEFGHIJ)','team_id':'ABCDEFGHIJ'}
WINDOWS={'version':1,'platform':'windows','thumbprint':'a'*40,'publisher':'CN=Test "quoted" & Company','timestamp_url':'https://timestamp.example.invalid/'}
class FakeTools:
 def __init__(self,reject=False,status='signed by a certificate trusted by macOS'):self.calls=[];self.reject=reject;self.entitlements=[];self.status=status
 def run(self,args,timeout=120,capture=False):
  args=list(map(str,args));self.calls.append(args)
  if args[0]=='codesign' and '--force' in args:
   path=Path(args[-1])
   if '--entitlements' in args:self.entitlements.append(plistlib.loads(Path(args[args.index('--entitlements')+1]).read_bytes()))
   if path.is_dir():path=path/'Contents/MacOS/screenshots'
   path.write_bytes(path.read_bytes()+b'SYNTHETIC-SIGNATURE')
  if args[0]=='signtool.exe' and args[1]=='sign':
   path=Path(args[-1]);path.write_bytes(path.read_bytes()+b'SYNTHETIC-SIGNATURE')
  if args[0]=='productsign':shutil.copyfile(args[-2],args[-1])
  if args[:2]==['xcrun','notarytool']:return json.dumps({'status':'Rejected' if self.reject else 'Accepted'}).encode()
  if args[:2]==['pkgutil','--check-signature']:return ('Status: '+self.status+'\nCertificate Chain:\n 1. '+MAC['installer_identity']+'\n 2. Developer ID Certification Authority\n').encode()
  return b''

def runtime(root,rid):
 root.mkdir(parents=True)
 entry='screenshots-sxcu.exe' if rid=='win-x64' else 'screenshots-sxcu'
 data={entry:b'\xcf\xfa\xed\xfe' + b'synthetic host','native.dylib':b'\xcf\xfa\xed\xfe' + b'synthetic library','managed.dll':b'synthetic managed bytes'}
 manifest={'version':1,'protocol':1,'sdk':'10.0.404','runtime':'10.0.15','rid':rid,'entrypoint':entry,'files':[]}
 for name,content in data.items():
  (root/name).write_bytes(content);manifest['files'].append({'path':name,'bytes':len(content),'sha256':hashlib.sha256(content).hexdigest(),'executable':name==entry})
 (root/'RUNTIME-MANIFEST.json').write_text(json.dumps(manifest))
 return manifest

class SigningTests(unittest.TestCase):
 def test_exact_public_profiles_reject_secrets_and_wrong_platform_before_tools(self):
  with tempfile.TemporaryDirectory() as temporary:
   path=Path(temporary)/'profile'
   for profile,platform in [(MAC,'macos'),(WINDOWS,'windows')]:
    path.write_text(json.dumps(profile));self.assertEqual(SIGN.profile(path,platform),profile)
   for changed in [dict(MAC,password='must-not-use'),dict(MAC,version=True),dict(MAC,team_id='wrong'),dict(MAC,application_identity='-'),dict(WINDOWS,timestamp_url='http://unsafe.invalid'),dict(WINDOWS,thumbprint='not-a-certificate')]:
    path.write_text(json.dumps(changed))
    with self.assertRaises(ValueError):SIGN.profile(path,changed['platform'])
   path.write_text(json.dumps(MAC))
   with self.assertRaises(ValueError):SIGN.profile(path,'linux')
 def test_publisher_is_exact_and_xml_escaped(self):
  root=ET.fromstring(APP.appx_manifest('0.1.0','x64',WINDOWS['publisher']))
  identity=list(root)[0];self.assertEqual(identity.attrib['Publisher'],WINDOWS['publisher'])
 def test_mac_nested_signatures_refresh_manifest_before_outer_bundle(self):
  with tempfile.TemporaryDirectory() as temporary:
   root=Path(temporary);app=root/'Screenshots.app';resources=app/'Contents/Resources';binary=app/'Contents/MacOS/screenshots';binary.parent.mkdir(parents=True);binary.write_bytes(b'unsigned main')
   private=resources/'lib/screenshots-sxcu';runtime(private,'osx-arm64')
   info={'version':'0.1.0','target':'aarch64-apple-darwin','binary_sha256':SIGN.digest(binary),'sharex_runtime':{'manifest_sha256':SIGN.digest(private/'RUNTIME-MANIFEST.json')}}
   tools=FakeTools();result=SIGN.mac_app(app,info,MAC,tools)
   manifest=json.loads((private/'RUNTIME-MANIFEST.json').read_text())
   for member in manifest['files']:self.assertEqual(member['sha256'],SIGN.digest(private/member['path']))
   self.assertEqual(result['sharex_runtime']['manifest_sha256'],SIGN.digest(private/'RUNTIME-MANIFEST.json'))
   self.assertNotEqual(result['binary_sha256'],SIGN.digest(binary))
   self.assertEqual(result['binary_sha256_scope'],'admitted executable before enclosing bundle signature')
   signed=[args for args in tools.calls if '--force' in args]
   self.assertEqual(Path(signed[-1][-1]).resolve(),app.resolve())
   helper=[args for args in signed if '--entitlements' in args];self.assertEqual(len(helper),1)
   self.assertEqual(tools.entitlements,[{'com.apple.security.cs.allow-jit':True}])
   self.assertTrue(any('ABCDEFGHIJ' in ' '.join(args) for args in tools.calls if '--verify' in args))
   self.assertFalse((root/'sxcu-jit-entitlements.plist').exists())
 def test_unknown_loader_refuses_before_signer(self):
  with tempfile.TemporaryDirectory() as temporary:
   root=Path(temporary);app=root/'Screenshots.app';resources=app/'Contents/Resources';private=resources/'lib/screenshots-sxcu';runtime(private,'osx-arm64');(private/'extra-loader').write_bytes(b'unowned');tools=FakeTools()
   with self.assertRaises(ValueError):SIGN.mac_app(app,{'target':'aarch64-apple-darwin'},MAC,tools)
   self.assertEqual(tools.calls,[])
 def test_windows_payload_tracks_signed_bytes_and_timestamp_algorithm(self):
  with tempfile.TemporaryDirectory() as temporary:
   root=Path(temporary);(root/'screenshots.exe').write_bytes(b'native exe');private=root/'lib/screenshots-sxcu';runtime(private,'win-x64');tools=FakeTools()
   info={'target':'x86_64-pc-windows-msvc','sharex_runtime':{'manifest_sha256':SIGN.digest(private/'RUNTIME-MANIFEST.json')}}
   result=SIGN.windows_payload(root,info,WINDOWS,tools)
   self.assertEqual(result['binary_sha256'],SIGN.digest(root/'screenshots.exe'))
   self.assertEqual(result['sharex_runtime']['manifest_sha256'],SIGN.digest(private/'RUNTIME-MANIFEST.json'))
   for args in tools.calls:
    if args[1]=='sign':self.assertLess(args.index('/tr'),args.index('/td'));self.assertIn('/sha1',args);self.assertNotIn('/p',args)
 def test_self_consistent_runtime_with_wrong_input_anchor_is_rejected_before_signing(self):
  with tempfile.TemporaryDirectory() as temporary:
   root=Path(temporary);(root/'screenshots.exe').write_bytes(b'original');private=root/'lib/screenshots-sxcu';runtime(private,'win-x64');tools=FakeTools()
   before=(private/'RUNTIME-MANIFEST.json').read_bytes()
   with self.assertRaises(ValueError):SIGN.windows_payload(root,{'target':'x86_64-pc-windows-msvc','sharex_runtime':{'manifest_sha256':'0'*64}},WINDOWS,tools)
   self.assertEqual(tools.calls,[]);self.assertEqual((private/'RUNTIME-MANIFEST.json').read_bytes(),before);self.assertEqual((root/'screenshots.exe').read_bytes(),b'original')
 def test_accepted_notary_staples_only_the_signed_package(self):
  with tempfile.TemporaryDirectory() as temporary:
   root=Path(temporary);source=root/'unsigned.pkg';source.write_bytes(b'unsigned');destination=root/'signed.pkg';tools=FakeTools()
   self.assertEqual(SIGN.mac_package(source,destination,MAC,'existing-profile',tools),{'signed':True,'notarized':True})
   self.assertTrue(any(args[:3]==['xcrun','stapler','staple'] and Path(args[-1]).resolve()==destination.resolve() for args in tools.calls))
   self.assertTrue(any(args[:3]==['xcrun','stapler','validate'] for args in tools.calls))
 def test_notary_rejection_does_not_staple_or_publish(self):
  with tempfile.TemporaryDirectory() as temporary:
   root=Path(temporary);source=root/'unsigned.pkg';source.write_bytes(b'unsigned');destination=root/'signed.pkg';tools=FakeTools(reject=True)
   with self.assertRaises(ValueError):SIGN.mac_package(source,destination,MAC,'already-existing-profile',tools)
   self.assertFalse(any(args[:2]==['xcrun','stapler'] for args in tools.calls));self.assertEqual(source.read_bytes(),b'unsigned')
 def test_installer_status_allows_exact_trusted_variants_and_refuses_other_statuses(self):
  trusted=['signed by a certificate trusted by macOS','signed by a certificate trusted by Mac OS X','signed by a developer certificate issued by Apple for distribution']
  refused=['no signature','signed by an untrusted certificate','signed by a developer certificate issued by Apple for distribution (expired)','unsigned\nNotarization: trusted by the Apple notary service']
  with tempfile.TemporaryDirectory() as temporary:
   root=Path(temporary);source=root/'unsigned';source.write_bytes(b'input')
   for index,status in enumerate(trusted+refused):
    with self.subTest(status=status):
     tools=FakeTools(status=status);destination=root/str(index)
     if status in trusted:self.assertEqual(SIGN.mac_package(source,destination,MAC,None,tools),{'signed':True,'notarized':False})
     else:
      with self.assertRaises(ValueError):SIGN.mac_package(source,destination,MAC,'existing-profile',tools)
      self.assertFalse(any(args[:2]==['xcrun','notarytool'] for args in tools.calls))
 def test_artifact_group_failure_retains_complete_published_members_for_review(self):
  with tempfile.TemporaryDirectory() as temporary:
   root=Path(temporary);out=root/'out';out.mkdir();a=root/'first';b=root/'second';a.write_bytes(b'one');b.write_bytes(b'two')
   original=APP.publish_file
   def fail_second(source,destination):
    if source==b:raise OSError('injected publish failure')
    return original(source,destination)
   with mock.patch.object(APP,'publish_file',side_effect=fail_second):
    with self.assertRaises(OSError):APP.publish_all([a,b],out)
   self.assertEqual((out/a.name).read_bytes(),b'one');self.assertTrue((out/(a.name+'.sha256')).is_file())
   (out/a.name).write_bytes(b'foreign')
   with self.assertRaises(ValueError):APP.publish_all([a,b],out)
   self.assertEqual((out/a.name).read_bytes(),b'foreign')
 def test_mismatched_installer_certificate_is_refused_before_notary(self):
  class WrongTeam(FakeTools):
   def run(self,args,**kwargs):
    if args[:2]==['pkgutil','--check-signature']:return b'Status: signed by a certificate trusted by macOS\n 1. Developer ID Installer: Other (ZZZZZZZZZZ)\n'
    return super().run(args,**kwargs)
  with tempfile.TemporaryDirectory() as temporary:
   root=Path(temporary);source=root/'unsigned';source.write_bytes(b'input');tools=WrongTeam()
   with self.assertRaises(ValueError):SIGN.mac_package(source,root/'signed',MAC,'existing-profile',tools)
   self.assertFalse(any(args[:2]==['xcrun','notarytool'] for args in tools.calls))
 def test_bare_path_lookalike_cannot_be_native_verifier(self):
  with tempfile.TemporaryDirectory() as temporary:
   root=Path(temporary);marker=root/'marker';fake=root/'codesign';fake.write_text('#!/bin/sh\ntouch "'+str(marker)+'"\n');fake.chmod(0o755)
   with mock.patch.dict(os.environ,{'PATH':str(root)}),mock.patch.dict(SIGN.MAC_TOOLS,{'codesign':str(root/'missing-protected-tool')}):
    with self.assertRaises(ValueError):SIGN.Tools().run(['codesign','--verify','file'])
   self.assertFalse(marker.exists())
 def test_native_runner_uses_protected_path_private_cwd_and_clean_environment(self):
  with tempfile.TemporaryDirectory() as temporary:
   root=Path(temporary);script=root/'controlled-tool.py';script.write_text('import os,json; print(json.dumps({"cwd":os.getcwd(),"path":os.environ["PATH"],"developer":os.environ.get("DEVELOPER_DIR"),"dyld":os.environ.get("DYLD_INSERT_LIBRARIES")}))\n')
   with mock.patch.dict(os.environ,{'PATH':str(root/'lookalikes'),'DEVELOPER_DIR':str(root/'untrusted'),'DYLD_INSERT_LIBRARIES':'must-not-load'}),mock.patch.dict(SIGN.MAC_TOOLS,{'codesign':str(Path(sys.executable).resolve())}):
    result=json.loads(SIGN.Tools().run(['codesign',str(script)],capture=True))
   self.assertNotEqual(result['cwd'],str(ROOT));self.assertNotIn('lookalikes',result['path']);self.assertIsNone(result['developer']);self.assertIsNone(result['dyld'])
 def test_group_rollback_preserves_replaced_checksum_and_artifact(self):
  with tempfile.TemporaryDirectory() as temporary:
   root=Path(temporary);out=root/'out';out.mkdir();a=root/'a';b=root/'b';a.write_bytes(b'a');b.write_bytes(b'b');original=APP.publish_file
   def replace_then_fail(source,destination):
    if source==b:
     for target in [out/'a',out/'a.sha256']:
      target.unlink();target.write_bytes(b'foreign replacement')
     raise OSError('injected')
    return original(source,destination)
   with mock.patch.object(APP,'publish_file',side_effect=replace_then_fail):
    with self.assertRaises(OSError):APP.publish_all([a,b],out)
   self.assertEqual((out/'a').read_bytes(),b'foreign replacement');self.assertEqual((out/'a.sha256').read_bytes(),b'foreign replacement')
 def test_single_publish_failure_preserves_foreign_artifact_replacement(self):
  with tempfile.TemporaryDirectory() as temporary:
   root=Path(temporary);source=root/'input';source.write_bytes(b'own');out=root/'out';out.mkdir();destination=out/'input';link=os.link
   def fail_checksum(origin,target):
    if target.name.endswith('.sha256'):
     destination.unlink();destination.write_bytes(b'foreign');raise OSError('injected')
    return link(origin,target)
   with mock.patch.object(APP.os,'link',side_effect=fail_checksum):
    with self.assertRaises(OSError):APP.publish_file(source,destination)
   self.assertEqual(destination.read_bytes(),b'foreign')
 def test_foreign_symlink_or_directory_and_prior_outputs_survive_failure(self):
  for foreign_kind in ['symlink','directory']:
   with self.subTest(kind=foreign_kind),tempfile.TemporaryDirectory() as temporary:
    root=Path(temporary);out=root/'out';out.mkdir();a=root/'a';b=root/'b';a.write_bytes(b'a');b.write_bytes(b'b');foreign=root/'foreign';foreign.write_bytes(b'foreign');original=APP.publish_file
    def fail_later(source,destination):
     if source==b:
      target=out/('a.sha256' if foreign_kind=='symlink' else 'a');target.unlink()
      if foreign_kind=='symlink':target.symlink_to(foreign)
      else:target.mkdir();(target/'keep').write_bytes(b'keep')
      raise OSError('injected later failure')
     return original(source,destination)
    with mock.patch.object(APP,'publish_file',side_effect=fail_later):
     with self.assertRaises(OSError):APP.publish_all([a,b],out)
    if foreign_kind=='symlink':self.assertTrue((out/'a.sha256').is_symlink());self.assertEqual((out/'a').read_bytes(),b'a')
    else:self.assertEqual((out/'a'/'keep').read_bytes(),b'keep');self.assertTrue((out/'a.sha256').is_file())
    self.assertEqual(foreign.read_bytes(),b'foreign')
 def test_failed_publication_never_unlinks_output_names_after_identity_checks(self):
  with tempfile.TemporaryDirectory() as temporary:
   root=Path(temporary);out=root/'out';out.mkdir();a=root/'a';b=root/'b';a.write_bytes(b'a');b.write_bytes(b'b');original=APP.publish_file;unlink=Path.unlink
   def no_output_unlink(path,*args,**kwargs):
    self.assertNotEqual(path.parent,out,'Shared output names must never be deleted during rollback')
    return unlink(path,*args,**kwargs)
   def fail_second(source,destination):
    if source==b:raise OSError('injected')
    return original(source,destination)
   with mock.patch.object(APP,'publish_file',side_effect=fail_second),mock.patch.object(Path,'unlink',new=no_output_unlink):
    with self.assertRaises(OSError):APP.publish_all([a,b],out)
   self.assertEqual((out/'a').read_bytes(),b'a')
 def test_ended_native_tool_cannot_leave_a_live_pipe_holding_descendant(self):
  with tempfile.TemporaryDirectory() as temporary:
   import time
   root=Path(temporary);pid=root/'pid'
   code='import subprocess,sys,pathlib; child=subprocess.Popen([sys.executable,"-c","import time;time.sleep(30)"]);pathlib.Path('+repr(str(pid))+').write_text(str(child.pid));sys.stdout.buffer.write(b"public\\n");sys.stdout.buffer.flush()'
   started=time.monotonic();self.assertEqual(SIGN.Tools().run([sys.executable,'-c',code],timeout=2,capture=True),b'public\n');self.assertLess(time.monotonic()-started,3)
   if sys.platform.startswith('linux'):
    state=Path('/proc')/pid.read_text()/'stat'
    try:remaining=state.read_text().rsplit(')',1)[1].split()[0]
    except FileNotFoundError:remaining=None
    self.assertIn(remaining,[None,'Z','X'])
 @unittest.skipIf(os.name=='nt','POSIX group startup guard; Windows atomic Job path has separate native gate')
 def test_reader_admission_failure_still_kills_and_reaps_owned_tool(self):
  children=[];create=subprocess.Popen
  def record(*args,**kwargs):
   child=create(*args,**kwargs);children.append(child);return child
  with mock.patch.object(SIGN.subprocess,'Popen',side_effect=record),mock.patch.object(SIGN.threading.Thread,'start',side_effect=RuntimeError('injected thread admission')):
   with self.assertRaises(RuntimeError):SIGN.Tools().run([sys.executable,'-c','import time;time.sleep(30)'],capture=True)
  self.assertEqual(len(children),1);self.assertIsNotNone(children[0].returncode);self.assertIsNotNone(children[0].poll())
 def test_bounded_native_result_and_timeout_use_only_private_subprocesses(self):
  tools=SIGN.Tools();self.assertEqual(tools.run([sys.executable,'-c','import sys;sys.stdout.buffer.write(b"public\\n")'],capture=True),b'public\n')
  with self.assertRaises(ValueError):tools.run([sys.executable,'-c','print("x"*200000)'],capture=True)
  with self.assertRaises(ValueError):tools.run([sys.executable,'-c','import time; time.sleep(10)'],timeout=.05)
 @unittest.skipIf(os.name=='nt','POSIX group reservation interruption regression')
 def test_reaping_wait_interruption_cannot_signal_a_reused_group(self):
  signals=[];wait=subprocess.Popen.wait;killpg=os.killpg;interrupted=[]
  def observed_signal(pid,signal):signals.append(pid);return killpg(pid,signal)
  def reap_then_interrupt(child,*args,**kwargs):
   result=wait(child,*args,**kwargs)
   if not interrupted:interrupted.append(child.pid);raise KeyboardInterrupt()
   return result
  with mock.patch.object(SIGN.os,'killpg',side_effect=observed_signal),mock.patch.object(SIGN.subprocess.Popen,'wait',new=reap_then_interrupt):
   with self.assertRaises(KeyboardInterrupt):SIGN.Tools().run([sys.executable,'-c','pass'])
  self.assertEqual(signals,interrupted)
 @unittest.skipIf(os.name=='nt','POSIX exceptional cleanup regression')
 def test_group_permission_failure_still_reaps_and_cleans_private_working_directory(self):
  children=[];directories=[];threads=[];create=subprocess.Popen;temporary=tempfile.TemporaryDirectory;thread=SIGN.threading.Thread
  def child(*args,**kwargs):
   value=create(*args,**kwargs);children.append(value);return value
  def directory(*args,**kwargs):
   value=temporary(*args,**kwargs);directories.append(value);return value
  def reader(*args,**kwargs):
   value=thread(*args,**kwargs);threads.append(value);return value
  with mock.patch.object(SIGN.subprocess,'Popen',side_effect=child),mock.patch.object(SIGN.tempfile,'TemporaryDirectory',side_effect=directory),mock.patch.object(SIGN.threading,'Thread',side_effect=reader),mock.patch.object(SIGN.os,'killpg',side_effect=PermissionError('injected')),mock.patch.object(SIGN,'darwin_only_held_zombie',return_value=False):
   with self.assertRaises(PermissionError):SIGN.Tools().run([sys.executable,'-c','pass'],capture=True)
  self.assertEqual(len(children),1);self.assertEqual(children[0].returncode,0)
  self.assertTrue(all(not Path(value.name).exists() for value in directories))
  self.assertTrue(all(not value.is_alive() for value in threads))
 @unittest.skipUnless(hasattr(os,'WNOWAIT'),'native nonreaping ABI test needs host wait flags')
 def test_native_waitid_marker_does_not_reap_the_owned_leader(self):
  import time
  child=subprocess.Popen([sys.executable,'-c','pass'])
  try:
   until=time.monotonic()+3
   while not SIGN.native_exit_marker(child.pid,os.WEXITED|os.WNOHANG|os.WNOWAIT):
    self.assertLess(time.monotonic(),until);time.sleep(.005)
   self.assertIsNone(child.returncode)
   self.assertIsNotNone(os.waitid(os.P_PID,child.pid,os.WEXITED|os.WNOHANG|os.WNOWAIT))
   self.assertEqual(child.wait(timeout=2),0)
  finally:
   if child.returncode is None:child.kill();child.wait(timeout=2)

if __name__=='__main__':unittest.main()
