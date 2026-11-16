"""Operation-owned native release-tool process; atomic job/handle association."""
import ctypes as C
from ctypes import wintypes as W
import json
import msvcrt
import os
import subprocess
import threading
import time

H=C.c_void_p
K=C.WinDLL('kernel32',use_last_error=True)
def api(name,arguments,result=W.BOOL):
 value=getattr(K,name);value.argtypes=arguments;value.restype=result;return value
Close=api('CloseHandle',[H]);Terminate=api('TerminateJobObject',[H,W.UINT])
Wait=api('WaitForSingleObject',[H,W.DWORD],W.DWORD)
class Security(C.Structure):_fields_=[('length',W.DWORD),('descriptor',H),('inherit',W.BOOL)]
class Startup(C.Structure):
 _fields_=[('cb',W.DWORD),('reserved',W.LPWSTR),('desktop',W.LPWSTR),('title',W.LPWSTR),
 ('x',W.DWORD),('y',W.DWORD),('width',W.DWORD),('height',W.DWORD),('xc',W.DWORD),('yc',W.DWORD),
 ('fill',W.DWORD),('flags',W.DWORD),('show',W.WORD),('reserved_len',W.WORD),('reserved_data',H),('stdin',H),('stdout',H),('stderr',H)]
class ExtendedStartup(C.Structure):_fields_=[('base',Startup),('attributes',H)]
class Process(C.Structure):_fields_=[('process',H),('thread',H),('pid',W.DWORD),('tid',W.DWORD)]
class BasicLimit(C.Structure):
 _fields_=[('process_time',C.c_int64),('job_time',C.c_int64),('flags',W.DWORD),('minimum',C.c_size_t),('maximum',C.c_size_t),('active_limit',W.DWORD),('affinity',C.c_size_t),('priority',W.DWORD),('scheduling',W.DWORD)]
class Limits(C.Structure):_fields_=[('basic',BasicLimit),('io',C.c_uint64*6),('process_memory',C.c_size_t),('job_memory',C.c_size_t),('peak_process',C.c_size_t),('peak_job',C.c_size_t)]
class Accounting(C.Structure):_fields_=[('times',C.c_int64*4),('faults',W.DWORD),('total',W.DWORD),('active',W.DWORD),('terminated',W.DWORD)]
CreateJob=api('CreateJobObjectW',[H,W.LPCWSTR],H)
SetJob=api('SetInformationJobObject',[H,C.c_int,H,W.DWORD])
QueryJob=api('QueryInformationJobObject',[H,C.c_int,H,W.DWORD,H])
CreateFile=api('CreateFileW',[W.LPCWSTR,W.DWORD,W.DWORD,C.POINTER(Security),W.DWORD,W.DWORD,H],H)
CreatePipe=api('CreatePipe',[C.POINTER(H),C.POINTER(H),C.POINTER(Security),W.DWORD])
SetHandle=api('SetHandleInformation',[H,W.DWORD,W.DWORD])
Initialize=api('InitializeProcThreadAttributeList',[H,W.DWORD,W.DWORD,C.POINTER(C.c_size_t)])
Update=api('UpdateProcThreadAttribute',[H,W.DWORD,C.c_size_t,H,C.c_size_t,H,H])
Delete=api('DeleteProcThreadAttributeList',[H],None)
Create=api('CreateProcessW',[W.LPCWSTR,W.LPWSTR,H,H,W.BOOL,W.DWORD,H,W.LPCWSTR,C.POINTER(Startup),C.POINTER(Process)])
Exit=api('GetExitCodeProcess',[H,C.POINTER(W.DWORD)])

def require(value,message):
 if not value:raise ValueError(message)

def run(arguments,environment,working,timeout,capture):
 require(os.path.isabs(arguments[0]),'Release-tool image must be absolute')
 command=subprocess.list2cmdline(arguments)
 require(len(command)<32767 and '\0' not in command,'Release-tool command exceeds native limits')
 block='\0'.join(key+'='+value for key,value in sorted(environment.items(),key=lambda item:item[0].upper()))+'\0\0'
 require(len(block.encode('utf-16-le',errors='surrogatepass'))<=1024*1024,'Release-tool environment exceeds limit')
 handles=[];job=None;attributes=None;attributes_ready=False;process=Process();reader=None;pipe=None;output=bytearray();excess=threading.Event()
 try:
  job=CreateJob(None,None);require(job,'Could not create release-tool job');handles.append(job)
  require(SetHandle(job,1,0),'Could not isolate release-tool job handle')
  limits=Limits();limits.basic.flags=0x2000
  require(SetJob(job,9,C.byref(limits),C.sizeof(limits)),'Could not configure owned release-tool job')
  security=Security(C.sizeof(Security),None,1)
  null=CreateFile('NUL',0xc0000000,3,C.byref(security),3,0x80,None)
  require(null and null!=H(-1).value,'Could not create private release-tool null streams');handles.append(null)
  inherited=[null];out=null
  if capture:
   read=H();write=H();require(CreatePipe(C.byref(read),C.byref(write),C.byref(security),0),'Could not create bounded release-tool pipe')
   handles.extend([read.value,write.value]);require(SetHandle(read,1,0),'Could not isolate parent result pipe')
   inherited.append(write.value);out=write.value
  size=C.c_size_t();Initialize(None,2,0,C.byref(size))
  require(0<size.value<=65536,'Invalid release-tool attribute storage requirement')
  storage=(C.c_size_t*((size.value+C.sizeof(C.c_size_t)-1)//C.sizeof(C.c_size_t)))()
  attributes=C.cast(storage,H);require(Initialize(attributes,2,0,C.byref(size)),'Could not initialize release-tool attributes');attributes_ready=True
  jobs=(H*1)(job);allowed=(H*len(inherited))(*inherited)
  require(Update(attributes,0,0x2000d,jobs,C.sizeof(jobs),None,None),'Could not attach owned job atomically')
  require(Update(attributes,0,0x20002,allowed,C.sizeof(allowed),None,None),'Could not restrict inherited handles')
  startup=ExtendedStartup();startup.base.cb=C.sizeof(startup);startup.base.flags=0x100
  startup.base.stdin=null;startup.base.stdout=out;startup.base.stderr=null;startup.attributes=attributes
  env=C.create_unicode_buffer(block);cmd=C.create_unicode_buffer(command)
  require(Create(arguments[0],cmd,None,None,1,0x08000000|0x00080000|0x00000400,env,working,C.cast(C.byref(startup),C.POINTER(Startup)),C.byref(process)),
          'Could not create contained release-tool process')
  handles.extend([process.process,process.thread])
  if capture:
   Close(write);handles.remove(write.value)
   fd=msvcrt.open_osfhandle(read.value,os.O_RDONLY|os.O_BINARY);handles.remove(read.value);pipe=os.fdopen(fd,'rb')
   def consume():
    try:
     while chunk:=pipe.read(4096):
      if len(chunk)>128*1024-len(output):excess.set();return
      output.extend(chunk)
    finally:pipe.close()
   reader=threading.Thread(target=consume,daemon=True);reader.start()
  deadline=time.monotonic()+timeout
  while True:
   state=Wait(process.process,10)
   require(state in (0,258),'Release-tool wait failed')
   if state==0:break
   require(not excess.is_set() and time.monotonic()<deadline,'Release-tool exceeded its output or time budget')
  status=W.DWORD();require(Exit(process.process,C.byref(status)),'Could not read release-tool outcome')
  # Job identity is an owned handle, so leader exit/reaping cannot target another family.
  require(Terminate(job,3),'Could not stop remaining release-tool descendants')
  until=time.monotonic()+5
  while True:
   accounting=Accounting();require(QueryJob(job,1,C.byref(accounting),C.sizeof(accounting),None),'Could not confirm release-tool cleanup')
   if accounting.active==0:break
   require(time.monotonic()<until,'Release-tool family termination is pending');time.sleep(.01)
  if reader:reader.join(2);require(not reader.is_alive(),'Release-tool result pipe cleanup is pending')
  require(not excess.is_set() and status.value==0,'Native signing/verification failed')
  return bytes(output)
 finally:
  if job:Terminate(job,3)
  if process.process:Wait(process.process,5000)
  if reader is not None and reader.ident is not None:reader.join(2)
  elif pipe is not None:pipe.close()
  if attributes_ready:Delete(attributes)
  for handle in reversed(handles):Close(handle)
