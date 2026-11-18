// Owned hosted-CI supervisor and exact CLI launcher; never opens a user's queue.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <winspool.h>
#include <sddl.h>
#include <objbase.h>
#include <cstdio>
#include <memory>
#include <stdexcept>
#include <string>
#include <utility>
#include <vector>

struct Handle {
    HANDLE value{};
    explicit Handle(HANDLE v = nullptr) : value(v) {}
    ~Handle() { if (value && value != INVALID_HANDLE_VALUE) CloseHandle(value); }
    Handle(const Handle&) = delete;
    Handle& operator=(const Handle&) = delete;
};
static void need(bool value, const char* stage) { if (!value) throw std::runtime_error(stage); }
static std::wstring quote(const std::wstring& value) {
    need(value.find(L'"') == std::wstring::npos && !value.empty() && value.back() != L'\\', "unquotable owned argument");
    return L"\"" + value + L"\"";
}
static void hosted() {
    wchar_t gha[16]{}, os[16]{}, host[32]{};
    GetEnvironmentVariableW(L"GITHUB_ACTIONS", gha, 16);
    GetEnvironmentVariableW(L"RUNNER_OS", os, 16);
    GetEnvironmentVariableW(L"RUNNER_ENVIRONMENT", host, 32);
    need(!wcscmp(gha,L"true") && !wcscmp(os,L"Windows") && !wcscmp(host,L"github-hosted"), "disposable GitHub-hosted Windows only");
    SID_IDENTIFIER_AUTHORITY nt = SECURITY_NT_AUTHORITY; PSID admin{}; BOOL member{};
    need(AllocateAndInitializeSid(&nt,2,SECURITY_BUILTIN_DOMAIN_RID,DOMAIN_ALIAS_RID_ADMINS,0,0,0,0,0,0,&admin)!=0,"admin SID");
    const BOOL checked = CheckTokenMembership(nullptr,admin,&member); FreeSid(admin);
    need(checked && member,"already elevated host required; no UAC prompt");
}
static std::wstring default_printer() {
    DWORD count{}; GetDefaultPrinterW(nullptr,&count);
    need(count > 1 && count <= 513,"existing host default metadata required; never select/set it");
    std::vector<wchar_t> name(count); need(GetDefaultPrinterW(name.data(),&count)!=0,"host default metadata read");
    return name.data();
}
static std::wstring owned_queue(const std::wstring& directory) {
    const auto at = directory.find_last_of(L"\\/");
    const std::wstring name = directory.substr(at == std::wstring::npos ? 0 : at+1);
    constexpr wchar_t prefix[] = L"ScreenshotsPrintOwned-";
    need(name.rfind(prefix,0)==0,"exact owned directory prefix");
    const std::wstring guid = name.substr(wcslen(prefix)); GUID parsed{};
    need(guid.size()==36 && SUCCEEDED(CLSIDFromString((L"{"+guid+L"}").c_str(),&parsed)),"owned GUID identity");
    return name;
}
static std::unique_ptr<Handle> file(const std::wstring& path, bool inherit) {
    SECURITY_ATTRIBUTES sa{sizeof(sa),nullptr,inherit ? TRUE : FALSE};
    HANDLE result = CreateFileW(path.c_str(),GENERIC_WRITE,FILE_SHARE_READ,&sa,CREATE_NEW,FILE_ATTRIBUTE_NORMAL,nullptr);
    need(result!=INVALID_HANDLE_VALUE,"fresh private report"); return std::make_unique<Handle>(result);
}
static DWORD launch(std::wstring app, std::wstring command, const std::wstring& output,
                    const std::wstring& error, DWORD timeout, bool broken_stdout,
                    HANDLE job, const std::wstring& desktop = L"") {
    auto out = file(output,true); auto err = file(error,true);
    SECURITY_ATTRIBUTES sa{sizeof(sa),nullptr,TRUE};
    Handle in(CreateFileW(L"NUL",GENERIC_READ,FILE_SHARE_READ|FILE_SHARE_WRITE,&sa,OPEN_EXISTING,0,nullptr));
    need(in.value!=INVALID_HANDLE_VALUE,"null stdin");
    HANDLE read{},write{}; std::unique_ptr<Handle> broken;
    if (broken_stdout) {
        need(CreatePipe(&read,&write,&sa,4096)!=0,"owned broken stdout pipe");
        CloseHandle(read); broken=std::make_unique<Handle>(write);
    }
    SIZE_T size{}; InitializeProcThreadAttributeList(nullptr,job ? 2 : 1,0,&size);
    std::vector<BYTE> storage(size); auto* attributes=reinterpret_cast<LPPROC_THREAD_ATTRIBUTE_LIST>(storage.data());
    need(InitializeProcThreadAttributeList(attributes,job ? 2 : 1,0,&size)!=0,"launch attribute list");
    HANDLE inherited[]{broken ? broken->value : out->value,err->value,in.value};
    need(UpdateProcThreadAttribute(attributes,0,PROC_THREAD_ATTRIBUTE_HANDLE_LIST,inherited,sizeof(inherited),nullptr,nullptr)!=0,"exact inherited handles");
    if (job) need(UpdateProcThreadAttribute(attributes,0,PROC_THREAD_ATTRIBUTE_JOB_LIST,&job,sizeof(job),nullptr,nullptr)!=0,"atomic outer Job assignment");
    STARTUPINFOEXW start{}; start.StartupInfo.cb=sizeof(start); start.StartupInfo.dwFlags=STARTF_USESTDHANDLES;
    start.StartupInfo.hStdInput=in.value; start.StartupInfo.hStdOutput=inherited[0]; start.StartupInfo.hStdError=err->value;
    std::wstring desk=desktop; if (!desk.empty()) start.StartupInfo.lpDesktop=desk.data();
    start.lpAttributeList=attributes; PROCESS_INFORMATION process{};
    std::vector<wchar_t> mutable_command(command.begin(),command.end()); mutable_command.push_back(0);
    BOOL created=CreateProcessW(app.c_str(),mutable_command.data(),nullptr,nullptr,TRUE,EXTENDED_STARTUPINFO_PRESENT|CREATE_SUSPENDED,nullptr,nullptr,&start.StartupInfo,&process);
    DeleteProcThreadAttributeList(attributes); broken.reset();
    need(created!=0,"owned process creation"); Handle child(process.hProcess), thread(process.hThread);
    need(ResumeThread(thread.value)!=DWORD(-1),"owned process resume");
    const DWORD waited=WaitForSingleObject(child.value,timeout);
    if (waited!=WAIT_OBJECT_0) {
        // In invoke mode the outer supervisor still owns the whole family.
        if (job) TerminateJobObject(job,98); else TerminateProcess(child.value,98);
        need(WaitForSingleObject(child.value,10000)==WAIT_OBJECT_0,"owned process reap after deadline");
        throw std::runtime_error("owned process deadline");
    }
    DWORD code{}; need(GetExitCodeProcess(child.value,&code)!=0,"owned process exit");
    return code;
}

struct Ownership {
    std::wstring queue, port, sid;
    HANDLE printer{};
    bool port_created=false, queue_created=false, uncertain=false, pixels_authorized=false;
    unsigned destructive_calls=0;
    Ownership(std::wstring q,std::wstring p) : queue(std::move(q)),port(std::move(p)) {
        Handle token; HANDLE raw{}; need(OpenProcessToken(GetCurrentProcess(),TOKEN_QUERY,&raw)!=0,"owner token"); token.value=raw;
        DWORD bytes{}; GetTokenInformation(token.value,TokenUser,nullptr,0,&bytes); need(bytes && bytes<=65536,"owner SID bound");
        std::vector<BYTE> user(bytes); need(GetTokenInformation(token.value,TokenUser,user.data(),bytes,&bytes)!=0,"owner token SID");
        wchar_t* text{}; need(ConvertSidToStringSidW(reinterpret_cast<TOKEN_USER*>(user.data())->User.Sid,&text)!=0,"owner SID string"); sid=text; LocalFree(text);
    }
    ~Ownership() {if(printer) ClosePrinter(printer);}
    Ownership(const Ownership&)=delete;
    Ownership& operator=(const Ownership&)=delete;
};
static bool family_empty(HANDLE job) {
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION account{};
    return QueryInformationJobObject(job,JobObjectBasicAccountingInformation,&account,sizeof(account),nullptr) && account.ActiveProcesses==0;
}
static bool wait_empty(HANDLE job) {
    const ULONGLONG until=GetTickCount64()+10000;
    do {if(family_empty(job)) return true; Sleep(25);} while(GetTickCount64()<until);
    return false;
}
[[noreturn]] static void retain(const char* stage) {
    std::fprintf(stderr,"OWNED CLEANUP PENDING: %s; Job and acquired authority retained; destroy disposable host\n",stage);
    std::fflush(stderr); std::fflush(stdout); Sleep(INFINITE); std::terminate();
}
static std::wstring text_in(const std::vector<BYTE>& bytes,const wchar_t* text) {
    const auto begin=reinterpret_cast<ULONG_PTR>(bytes.data()), end=begin+bytes.size(), at=reinterpret_cast<ULONG_PTR>(text);
    need(at>=begin && at<end && at%alignof(wchar_t)==0,"bounded native text pointer");
    const size_t count=(end-at)/sizeof(wchar_t); size_t length=0;
    while(length<count && length<1024 && text[length]) ++length;
    need(length<count && length<1024,"bounded native text terminator"); return {text,length};
}
enum class Port { Missing, Local, Other };
static Port port_identity(const std::wstring& name) {
    DWORD bytes{},count{}; const BOOL first=EnumPortsW(nullptr,2,nullptr,0,&bytes,&count); const DWORD error=GetLastError();
    need(first || error==ERROR_INSUFFICIENT_BUFFER,"native port metadata available");
    need(bytes<=65536,"native port metadata bound"); if(!bytes) return Port::Missing;
    std::vector<BYTE> data(bytes); need(EnumPortsW(nullptr,2,data.data(),bytes,&bytes,&count)!=0,"native local port metadata");
    need(count<=128 && static_cast<size_t>(count)*sizeof(PORT_INFO_2W)<=data.size(),"native port count bound");
    const auto* ports=reinterpret_cast<const PORT_INFO_2W*>(data.data());
    for(DWORD index=0;index<count;++index) if(text_in(data,ports[index].pPortName)==name)
        return text_in(data,ports[index].pMonitorName)==L"Local Port" ? Port::Local : Port::Other;
    return Port::Missing;
}
static bool queue_exists(const std::wstring& name) {
    HANDLE printer{}; PRINTER_DEFAULTSW defaults{}; defaults.DesiredAccess=PRINTER_ACCESS_USE;
    if(OpenPrinterW(const_cast<wchar_t*>(name.c_str()),&printer,&defaults)) {ClosePrinter(printer);return true;}
    const DWORD error=GetLastError(); need(error==ERROR_INVALID_PRINTER_NAME || error==ERROR_FILE_NOT_FOUND,"queue collision/existence unavailable"); return false;
}
static HANDLE local_monitor() {
    HANDLE monitor{}; PRINTER_DEFAULTSW defaults{}; defaults.DesiredAccess=SERVER_ACCESS_ADMINISTER;
    need(OpenPrinterW(const_cast<wchar_t*>(L",XcvMonitor Local Port"),&monitor,&defaults)!=0,"Localmon administrator handle"); return monitor;
}
static DWORD port_command(HANDLE monitor,const wchar_t* command,const std::wstring& name) {
    DWORD needed{},status=ERROR_GEN_FAILURE;
    need(XcvDataW(monitor,command,reinterpret_cast<PBYTE>(const_cast<wchar_t*>(name.c_str())),static_cast<DWORD>((name.size()+1)*sizeof(wchar_t)),nullptr,0,&needed,&status)!=0,"Localmon command transport");
    return status;
}
static bool acquire(Ownership& owner) {
    // Metadata collisions confer no authority, even if identity happens to match.
    if(queue_exists(owner.queue) || port_identity(owner.port)!=Port::Missing) return false;
    HANDLE monitor=local_monitor(); DWORD status{};
    owner.uncertain=true;
    try {status=port_command(monitor,L"AddPort",owner.port);} catch(...) {ClosePrinter(monitor);throw;}
    ClosePrinter(monitor);
    if(status==ERROR_ALREADY_EXISTS || status==ERROR_FILE_EXISTS) {owner.uncertain=false;return false;}
    need(status==ERROR_SUCCESS,"exclusive native port creation"); owner.port_created=true; owner.uncertain=false;
    const std::wstring sddl=L"O:"+owner.sid+L"G:SYD:P(A;;0x000F000C;;;"+owner.sid+L")(A;;0x000F000C;;;SY)(A;;0x000F000C;;;BA)";
    PSECURITY_DESCRIPTOR security{}; need(ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.c_str(),SDDL_REVISION_1,&security,nullptr)!=0,"private queue descriptor");
    PRINTER_INFO_2W info{}; info.pPrinterName=owner.queue.data(); info.pPortName=owner.port.data();
    info.pDriverName=const_cast<wchar_t*>(L"Microsoft XPS Document Writer v4"); info.pPrintProcessor=const_cast<wchar_t*>(L"winprint"); info.pDatatype=const_cast<wchar_t*>(L"RAW");
    info.Attributes=PRINTER_ATTRIBUTE_LOCAL|PRINTER_ATTRIBUTE_QUEUED; info.Priority=1; info.DefaultPriority=1; info.pSecurityDescriptor=security;
    owner.uncertain=true; owner.printer=AddPrinterW(nullptr,2,reinterpret_cast<BYTE*>(&info)); const DWORD error=GetLastError(); LocalFree(security);
    if(!owner.printer && error==ERROR_PRINTER_ALREADY_EXISTS) {owner.uncertain=false;return false;}
    need(owner.printer!=nullptr,"exclusive native queue creation"); owner.queue_created=true; owner.uncertain=false;
    return true;
}
static bool private_security(PSECURITY_DESCRIPTOR descriptor,const std::wstring& expected) {
    if(!descriptor || !IsValidSecurityDescriptor(descriptor)) return false;
    PSID wanted{}; if(!ConvertStringSidToSidW(expected.c_str(),&wanted)) return false;
    PSID actual{}; BOOL defaulted{},present{}; PACL acl{}; SECURITY_DESCRIPTOR_CONTROL control{}; DWORD revision{};
    bool valid=GetSecurityDescriptorOwner(descriptor,&actual,&defaulted) && actual && IsValidSid(actual) && EqualSid(actual,wanted)
        && GetSecurityDescriptorControl(descriptor,&control,&revision) && (control&SE_DACL_PROTECTED)
        && GetSecurityDescriptorDacl(descriptor,&present,&acl,&defaulted) && present && acl && IsValidAcl(acl) && acl->AceCount==3;
    bool user=false,system=false,admin=false;
    if(valid) for(DWORD index=0;index<acl->AceCount;++index) {
        void* raw{}; if(!GetAce(acl,index,&raw)) {valid=false;break;}
        const auto* ace=static_cast<const ACCESS_ALLOWED_ACE*>(raw);
        if(ace->Header.AceType!=ACCESS_ALLOWED_ACE_TYPE || (ace->Header.AceFlags&INHERITED_ACE) || ace->Mask!=PRINTER_ALL_ACCESS) {valid=false;break;}
        PSID sid=const_cast<DWORD*>(&ace->SidStart);
        if(EqualSid(sid,wanted)) user=true;
        else if(IsWellKnownSid(sid,WinLocalSystemSid)) system=true;
        else if(IsWellKnownSid(sid,WinBuiltinAdministratorsSid)) admin=true;
        else {valid=false;break;}
    }
    LocalFree(wanted); return valid && user && system && admin;
}
static bool queue_identity(Ownership& owner) {
    if(!owner.queue_created || !owner.printer) return false;
    DWORD bytes{}; GetPrinterW(owner.printer,2,nullptr,0,&bytes); if(bytes<sizeof(PRINTER_INFO_2W) || bytes>65536) return false;
    std::vector<BYTE> data(bytes); if(!GetPrinterW(owner.printer,2,data.data(),bytes,&bytes)) return false;
    const auto* info=reinterpret_cast<const PRINTER_INFO_2W*>(data.data());
    if(text_in(data,info->pPrinterName)!=owner.queue || text_in(data,info->pPortName)!=owner.port || text_in(data,info->pDriverName)!=L"Microsoft XPS Document Writer v4"
        || (info->Attributes&(PRINTER_ATTRIBUTE_SHARED|PRINTER_ATTRIBUTE_NETWORK)) || !(info->Attributes&PRINTER_ATTRIBUTE_LOCAL)) return false;
    // Level 3 requests the actual owner/DACL through our retained creation handle.
    GetPrinterW(owner.printer,3,nullptr,0,&bytes); if(bytes<sizeof(PRINTER_INFO_3) || bytes>65536) return false;
    data.resize(bytes); if(!GetPrinterW(owner.printer,3,data.data(),bytes,&bytes)) return false;
    return private_security(reinterpret_cast<const PRINTER_INFO_3*>(data.data())->pSecurityDescriptor,owner.sid);
}
static bool cleanup_owned(Ownership& owner) {
    if(!owner.port_created && !owner.queue_created && !owner.pixels_authorized) return true;
    if(owner.port_created && port_identity(owner.port)!=Port::Local) return false;
    if(owner.queue_created) {
        if(!queue_identity(owner)) return false;
        ++owner.destructive_calls; if(!SetPrinterW(owner.printer,0,nullptr,PRINTER_CONTROL_PURGE)) return false;
        const ULONGLONG until=GetTickCount64()+15000; bool empty=false;
        do {DWORD bytes{},jobs{}; BYTE one[sizeof(JOB_INFO_1W)]{}; if(EnumJobsW(owner.printer,0,1,1,one,sizeof(one),&bytes,&jobs) && jobs==0) {empty=true;break;} Sleep(50);} while(GetTickCount64()<until);
        if(!empty) return false;
        ++owner.destructive_calls; if(!DeletePrinter(owner.printer)) return false;
        ClosePrinter(owner.printer); owner.printer=nullptr;
        const ULONGLONG gone=GetTickCount64()+15000;
        while(queue_exists(owner.queue)) {if(GetTickCount64()>=gone) return false;Sleep(50);}
        owner.queue_created=false;
    }
    if(owner.port_created) {
        // A queue collision never grants queue authority. Do not disturb a port
        // now referenced by that collision, even when this process created it.
        if(queue_exists(owner.queue) || port_identity(owner.port)!=Port::Local) return false;
        HANDLE monitor=local_monitor(); ++owner.destructive_calls; DWORD status{};
        try {status=port_command(monitor,L"DeletePort",owner.port);} catch(...) {ClosePrinter(monitor);throw;}
        ClosePrinter(monitor); if(status!=ERROR_SUCCESS || port_identity(owner.port)!=Port::Missing) return false; owner.port_created=false;
    }
    if(owner.pixels_authorized) {
        const auto directory=owner.port.substr(0,owner.port.find_last_of(L'\\'));
        for(const wchar_t* name : {L"output.xps",L"synthetic.png",L"oracle-control-golden.xps",L"oracle-control-shifted-fit.xps",L"oracle-control-wrong-color.xps",L"oracle-control-extra-page.xps",L"oracle-control-nested-brush-transform.xps",L"oracle-control-foreign-brush-namespace.xps",L"oracle-control-missing-width.xps",L"oracle-control-missing-height.xps"}) {
            ++owner.destructive_calls; if(!DeleteFileW((directory+L"\\"+name).c_str()) && GetLastError()!=ERROR_FILE_NOT_FOUND) return false;
        }
        WIN32_FIND_DATAW found{}; HANDLE listing=FindFirstFileW((directory+L"\\*").c_str(),&found);
        if(listing==INVALID_HANDLE_VALUE) return false;
        bool unexpected=false;
        do {
            const wchar_t* extension=wcsrchr(found.cFileName,L'.');
            if(extension && (!_wcsicmp(extension,L".png") || !_wcsicmp(extension,L".xps") || !_wcsicmp(extension,L".oxps"))) unexpected=true;
        } while(FindNextFileW(listing,&found));
        DWORD listing_error=GetLastError(); FindClose(listing);
        if(unexpected || listing_error!=ERROR_NO_MORE_FILES) return false;
        owner.pixels_authorized=false;
    }
    return true;
}
enum class Cleanup { Done, Pending, Failed };
static Cleanup guarded_cleanup(HANDLE job,Ownership& owner) {
    // This gate precedes all spool, port and pixel cleanup, including exceptions.
    if(!family_empty(job) || owner.uncertain) return Cleanup::Pending;
    return cleanup_owned(owner) ? Cleanup::Done : Cleanup::Failed;
}
static void controls(HANDLE job,Ownership& owner,const std::wstring& application,const std::wstring& desktop) {
    need(queue_identity(owner) && port_identity(owner.port)==Port::Local,"owned control baseline");
    Ownership collision(owner.queue,owner.port);
    need(!acquire(collision) && !collision.queue_created && !collision.port_created && !collision.uncertain,"collision grants no authority");
    need(guarded_cleanup(job,collision)==Cleanup::Done && collision.destructive_calls==0,"collision cleanup must perform no mutation");
    Ownership port_collision(owner.queue+L"-port-collision-control",owner.port);
    need(!acquire(port_collision) && !port_collision.queue_created && !port_collision.port_created && !port_collision.uncertain,"preexisting port grants no authority");
    need(guarded_cleanup(job,port_collision)==Cleanup::Done && port_collision.destructive_calls==0,"port collision cleanup must perform no mutation");
    need(queue_identity(owner) && port_identity(owner.port)==Port::Local,"collision resources remain intact");
    SIZE_T bytes{}; InitializeProcThreadAttributeList(nullptr,1,0,&bytes); std::vector<BYTE> buffer(bytes); auto* attributes=reinterpret_cast<LPPROC_THREAD_ATTRIBUTE_LIST>(buffer.data());
    need(InitializeProcThreadAttributeList(attributes,1,0,&bytes)!=0,"parked-child attributes");
    need(UpdateProcThreadAttribute(attributes,0,PROC_THREAD_ATTRIBUTE_JOB_LIST,&job,sizeof(job),nullptr,nullptr)!=0,"parked child atomic Job");
    STARTUPINFOEXW startup{}; startup.StartupInfo.cb=sizeof(startup);startup.lpAttributeList=attributes;std::wstring desk=desktop;startup.StartupInfo.lpDesktop=desk.data();
    std::wstring command=quote(application)+L" hold";std::vector<wchar_t> args(command.begin(),command.end());args.push_back(0);PROCESS_INFORMATION process{};
    const BOOL made=CreateProcessW(application.c_str(),args.data(),nullptr,nullptr,FALSE,EXTENDED_STARTUPINFO_PRESENT|CREATE_SUSPENDED,nullptr,nullptr,&startup.StartupInfo,&process);DeleteProcThreadAttributeList(attributes);
    need(made!=0,"actual owned parked child");Handle child(process.hProcess),thread(process.hThread);need(ResumeThread(thread.value)!=DWORD(-1),"parked child resume");
    const unsigned before=owner.destructive_calls;
    need(!family_empty(job) && guarded_cleanup(job,owner)==Cleanup::Pending && owner.destructive_calls==before && owner.queue_created && owner.port_created && owner.printer,"pending family retains authority without cleanup");
    need(queue_identity(owner) && port_identity(owner.port)==Port::Local,"pending family resources intact");
    need(TerminateProcess(child.value,97)!=0,"stop exact parked control child");need(WaitForSingleObject(child.value,10000)==WAIT_OBJECT_0 && wait_empty(job),"parked control child actually reaped");
}

int wmain(int argc,wchar_t** argv) {
    try {
        hosted(); need(argc>=2,"fixture mode");
        if(!wcscmp(argv[1],L"hold")) {BOOL assigned{};need(argc==2 && IsProcessInJob(GetCurrentProcess(),nullptr,&assigned) && assigned,"owned parked control Job");Sleep(INFINITE);return 1;}
        if(!wcscmp(argv[1],L"invoke")) {
            need(argc==9,"invoke app/input/queue/paper/stdout/stderr/mode");BOOL assigned{};need(IsProcessInJob(GetCurrentProcess(),nullptr,&assigned) && assigned,"invoke requires outer owned Job");
            const auto command=quote(argv[2])+L" print-image "+quote(argv[3])+L" --printer "+quote(argv[4])+L" --paper "+quote(argv[5]);
            const DWORD code=launch(argv[2],command,argv[6],argv[7],70000,!wcscmp(argv[8],L"broken-stdout"),nullptr);std::printf("{\"application_exit\":%lu}\n",code);return 0;
        }
        const bool ownership_only=!wcscmp(argv[1],L"ownership-only");
        need((!wcscmp(argv[1],L"supervise") || ownership_only) && argc==6,"supervise script/directory/shipping/instrumented");
        const std::wstring directory=argv[3],queue=owned_queue(directory),port=directory+L"\\output.xps",original_default=default_printer();Ownership owner(queue,port);
        Handle job(CreateJobObjectW(nullptr,nullptr));need(job.value!=nullptr,"outer owned Job");JOBOBJECT_EXTENDED_LIMIT_INFORMATION limits{};
        limits.BasicLimitInformation.LimitFlags=JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE|JOB_OBJECT_LIMIT_ACTIVE_PROCESS|JOB_OBJECT_LIMIT_PROCESS_MEMORY;limits.BasicLimitInformation.ActiveProcessLimit=8;limits.ProcessMemoryLimit=1024ull*1024*1024;
        need(SetInformationJobObject(job.value,JobObjectExtendedLimitInformation,&limits,sizeof(limits))!=0,"outer Job bounds");
        const std::wstring station_name=L"ScreenshotsPrintOwnedStation-"+std::to_wstring(GetCurrentProcessId()),desktop_path=station_name+L"\\owned";
        HWINSTA station=CreateWindowStationW(station_name.c_str(),0,WINSTA_ALL_ACCESS,nullptr);need(station!=nullptr,"private noninput station");HWINSTA previous=GetProcessWindowStation();need(SetProcessWindowStation(station)!=0,"private station setup");
        HDESK desktop=CreateDesktopW(L"owned",nullptr,nullptr,0,DESKTOP_CREATEWINDOW|DESKTOP_READOBJECTS|DESKTOP_WRITEOBJECTS|DESKTOP_ENUMERATE,nullptr);const BOOL restored=SetProcessWindowStation(previous);need(desktop && restored,"private desktop and station restoration");
        wchar_t system[MAX_PATH]{};need(GetSystemDirectoryW(system,MAX_PATH)>0,"System32 path");const std::wstring shell=std::wstring(system)+L"\\WindowsPowerShell\\v1.0\\powershell.exe";
        const auto prefix=quote(shell)+L" -NoLogo -NoProfile -NonInteractive -STA -File "+quote(argv[2]);
        const auto suffix=L" -FixtureDirectory "+quote(directory)+L" -ShippingApplication "+quote(argv[4])+L" -InstrumentedApplication "+quote(argv[5])+L" -Controller "+quote(argv[0])+L" -OwnerPid "+std::to_wstring(GetCurrentProcessId());
        DWORD exit=99;bool ran=false,checks=false;
        try {
            std::fprintf(stderr,"OWNED-PHASE preflight-start\n");
            need(launch(shell,prefix+L" -Stage Preflight"+suffix,directory+L"\\preflight.stdout.log",directory+L"\\preflight.stderr.log",60000,false,job.value,desktop_path)==0,"read-only preflight/prerequisite/collision refusal");
            need(wait_empty(job.value),"preflight family reaped before native acquisition");
            std::fprintf(stderr,"OWNED-PHASE preflight-complete-acquire-start\n");
            need(acquire(owner),"exclusive native ownership acquisition refused collision");need(queue_identity(owner) && port_identity(port)==Port::Local,"acquired private queue and Local Port identity");
            std::fprintf(stderr,"OWNED-PHASE acquire-complete-controls-start\n");
            controls(job.value,owner,argv[0],desktop_path);checks=true;
            std::fprintf(stderr,"OWNED-PHASE controls-complete\n");
            if(!ownership_only) {owner.pixels_authorized=true;exit=launch(shell,prefix+L" -Stage Execute"+suffix,directory+L"\\run.stdout.log",directory+L"\\run.stderr.log",300000,false,job.value,desktop_path);ran=true;}
            else exit=0;
        } catch(const std::exception& error) {std::fprintf(stderr,"supervised execution: %s\n",error.what());}
        TerminateJobObject(job.value,98);
        if(!wait_empty(job.value)) retain("family has not actually reaped; no spool/port/pixel cleanup attempted");
        std::fprintf(stderr,"OWNED-PHASE family-empty-cleanup-start\n");
        Cleanup result=Cleanup::Failed;
        try {result=guarded_cleanup(job.value,owner);} catch(...) {std::fprintf(stderr,"Owned cleanup raised; acquired authority retained\n");}
        if(result==Cleanup::Pending) retain("fresh family-empty/uncertain-acquisition gate; no destructive cleanup");
        if(result!=Cleanup::Done) retain("owned spool/port/pixel cleanup incomplete");
        bool same_default=false;try {same_default=default_printer()==original_default;} catch(...) {}
        std::fprintf(stderr,"OWNED-PHASE cleanup-complete\n");
        if(ownership_only) {
            need(!owner.pixels_authorized,"ownership-only granted no pixel authority");
            CloseDesktop(desktop);CloseWindowStation(station);
            const bool success=exit==0 && checks && same_default;
            std::printf("{\"scope\":\"ownership-controls-only\",\"product_executed\":false,\"pixel_output_tested\":false,\"outer_job_empty\":true,\"owned_spool_cleanup\":true,\"default_unchanged\":%s,\"ownership_controls_passed\":%s,\"passed\":%s}\n",same_default?"true":"false",checks?"true":"false",success?"true":"false");
            return success?0:1;
        }
        CloseDesktop(desktop);CloseWindowStation(station);const bool passed=ran && exit==0 && checks && same_default;
        std::printf("{\"inner_exit\":%lu,\"outer_job_empty\":true,\"owned_spool_cleanup\":true,\"default_unchanged\":%s,\"ownership_controls_passed\":%s,\"collision_resources_untouched\":%s,\"pending_family_no_cleanup\":%s,\"passed\":%s}\n",exit,same_default?"true":"false",checks?"true":"false",checks?"true":"false",checks?"true":"false",passed?"true":"false");return passed?0:1;
    } catch(const std::exception& error) {std::fprintf(stderr,"owned print fixture: %s\n",error.what());return 1;}
}
