// Outer owned-process/Job watchdog. It creates exact private desktops; no input injection.
#define wmain r10_prerequisite_main
#include "production_support.cpp"
#undef wmain
#include <atomic>
struct Drain { HANDLE input, output, ready; std::atomic<DWORD> helper{0}; bool overflow = false; };
static DWORD WINAPI drain_output(void* raw) {
    auto* drain = static_cast<Drain*>(raw); char bytes[4096]; DWORD read{}, written{}; std::string line; size_t total = 0;
    while (ReadFile(drain->input, bytes, sizeof(bytes), &read, nullptr) && read) {
        total += read; if (total > 512 * 1024) {drain->overflow = true; return 1;}
        require(WriteFile(drain->output, bytes, read, &written, nullptr) && written == read, "owned report write");
        for (DWORD index = 0; index < read; ++index) {
            if (bytes[index] == '\n') {
                constexpr char marker[] = "{\"owned_helper_ready\":true,\"pid\":";
                if (line.rfind(marker, 0) == 0) {char* end{}; unsigned long pid = strtoul(line.c_str() + sizeof(marker) - 1, &end, 10); if (pid && end && *end == '}') {drain->helper.store(pid); SetEvent(drain->ready);} }
                line.clear();
            } else {require(line.size() < 65536, "bounded report line"); line.push_back(bytes[index]);}
        }
    }
    return 0;
}
int wmain(int argc, wchar_t** argv) {
    wchar_t actions[8]{}, os[32]{}, runner[32]{}, gate[8]{};
    GetEnvironmentVariableW(L"GITHUB_ACTIONS",actions,8); GetEnvironmentVariableW(L"RUNNER_OS",os,32); GetEnvironmentVariableW(L"RUNNER_ENVIRONMENT",runner,32); GetEnvironmentVariableW(L"SCREENSHOTS_OWNED_PRODUCTION_ACCEPTANCE",gate,8);
    require(!wcscmp(actions,L"true") && !wcscmp(os,L"Windows") && !wcscmp(runner,L"github-hosted") && !wcscmp(gate,L"1"),"owned hosted production controller gate");
    require(argc == 6, "controller app/case/stdout/stderr/identity");
    std::wstring selected = argv[2]; bool parent_death = selected == L"protocol-parent-death"; bool negative_control=selected.rfind(L"noop-",0)==0 || selected.rfind(L"control-",0)==0 || selected.rfind(L"gpui-control-",0)==0 || selected.rfind(L"ci-control-",0)==0;
    SECURITY_ATTRIBUTES security{sizeof(security), nullptr, TRUE};
    HANDLE read{}, write{}; require(CreatePipe(&read, &write, &security, 65536), "owned stdout pipe"); require(SetHandleInformation(read, HANDLE_FLAG_INHERIT, 0), "controller-only read");
    HANDLE output = CreateFileW(argv[3], GENERIC_WRITE, 0, nullptr, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr); require(output != INVALID_HANDLE_VALUE, "fresh owned report");
    HANDLE error = CreateFileW(argv[4], GENERIC_WRITE, FILE_SHARE_READ, &security, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, nullptr); require(error != INVALID_HANDLE_VALUE, "fresh owned error log");
    HANDLE null_input = CreateFileW(L"NUL", GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE, &security, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, nullptr); require(null_input != INVALID_HANDLE_VALUE, "owned null stdin");
    HANDLE job = CreateJobObjectW(nullptr, nullptr); require(job != nullptr, "outer owned Job");
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION limits{}; limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS | JOB_OBJECT_LIMIT_PROCESS_MEMORY; limits.BasicLimitInformation.ActiveProcessLimit = 3; limits.ProcessMemoryLimit = 1024ull * 1024 * 1024;
    require(SetInformationJobObject(job, JobObjectExtendedLimitInformation, &limits, sizeof(limits)), "outer Job limits");
    HANDLE completion = CreateIoCompletionPort(INVALID_HANDLE_VALUE, nullptr, 0, 1); require(completion != nullptr, "outer Job completion port");
    JOBOBJECT_ASSOCIATE_COMPLETION_PORT association{job, completion}; require(SetInformationJobObject(job, JobObjectAssociateCompletionPortInformation, &association, sizeof(association)), "outer Job completion binding");
    bool input_mode = selected.rfind(L"ci-",0) == 0 || selected.rfind(L"gpui-ole-",0) == 0;
    bool graphical_mode = selected.rfind(L"gpui-",0) == 0;
    bool interactive_station = input_mode || graphical_mode;
    if (input_mode) require_owned_ci();
    HWINSTA original_station = GetProcessWindowStation();
    if (interactive_station) {
        wchar_t name[256]{};DWORD actual{};
        require(GetUserObjectInformationW(original_station,UOI_NAME,name,sizeof(name),&actual) && !wcscmp(name,L"WinSta0"),"exact owned interactive startup station");
    }
    std::wstring owner_id = std::to_wstring(GetCurrentProcessId());
    std::wstring station_name = L"ScreenshotsProductionAcceptance-" + owner_id;
    HWINSTA private_station = interactive_station ? original_station : CreateWindowStationW(station_name.c_str(),0,WINSTA_ALL_ACCESS,nullptr);
    require(private_station != nullptr,"owned startup station");
    require(SetProcessWindowStation(private_station),"owned startup station selection");
    std::wstring desktop_name = interactive_station ? L"ScreenshotsDropProbe-" + owner_id : L"owned";
    DWORD access = DESKTOP_CREATEWINDOW | DESKTOP_READOBJECTS | DESKTOP_WRITEOBJECTS | DESKTOP_ENUMERATE | (input_mode ? DESKTOP_SWITCHDESKTOP | DESKTOP_JOURNALPLAYBACK : 0);
    HDESK owned_desktop = CreateDesktopW(desktop_name.c_str(),nullptr,nullptr,0,access,nullptr); require(owned_desktop != nullptr,"owned startup desktop");
    require(SetProcessWindowStation(original_station),"restore controller station before app startup");
    std::wstring desktop_path = (interactive_station ? L"WinSta0" : station_name) + L"\\" + desktop_name;
    require(SetEnvironmentVariableW(L"SCREENSHOTS_DROP_ACCEPTANCE_DESKTOP",desktop_path.c_str()),"explicit owned startup desktop");
    require(SetEnvironmentVariableW(L"SCREENSHOTS_DROP_ACCEPTANCE_DESKTOP_OWNER",owner_id.c_str()),"explicit owned startup desktop owner");
    SIZE_T size{}; InitializeProcThreadAttributeList(nullptr, 2, 0, &size); std::vector<BYTE> storage(size); auto* attrs = reinterpret_cast<LPPROC_THREAD_ATTRIBUTE_LIST>(storage.data()); require(InitializeProcThreadAttributeList(attrs,2,0,&size), "outer launch attributes");
    HANDLE inherited[]{write,error,null_input}; require(UpdateProcThreadAttribute(attrs,0,PROC_THREAD_ATTRIBUTE_HANDLE_LIST,inherited,sizeof(inherited),nullptr,nullptr), "outer exact handle whitelist"); require(UpdateProcThreadAttribute(attrs,0,PROC_THREAD_ATTRIBUTE_JOB_LIST,&job,sizeof(job),nullptr,nullptr), "outer atomic Job assignment");
    STARTUPINFOEXW startup{}; startup.StartupInfo.cb = sizeof(startup); startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES; startup.StartupInfo.hStdOutput = write; startup.StartupInfo.hStdError = error; startup.StartupInfo.hStdInput = null_input; startup.lpAttributeList = attrs; startup.StartupInfo.lpDesktop = desktop_path.data();
    std::wstring command = L"\"" + std::wstring(argv[1]) + L"\" --owned-native-drop-acceptance " + selected;
    std::vector<wchar_t> mutable_command(command.begin(),command.end()); mutable_command.push_back(0); PROCESS_INFORMATION app{};
    require(CreateProcessW(argv[1],mutable_command.data(),nullptr,nullptr,TRUE,EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED,nullptr,nullptr,&startup.StartupInfo,&app), "actual acceptance app suspended in Job"); DeleteProcThreadAttributeList(attrs); CloseHandle(write);
    HANDLE identity = CreateFileW(argv[5],GENERIC_WRITE,0,nullptr,CREATE_NEW,FILE_ATTRIBUTE_NORMAL,nullptr); require(identity != INVALID_HANDLE_VALUE,"fresh owned identity");
    char identity_text[96]{}; int identity_bytes = sprintf_s(identity_text,"{\"application_pid\":%lu,\"desktop_owner_pid\":%lu}\n",app.dwProcessId,GetCurrentProcessId()); DWORD identity_written{};
    require(identity_bytes > 0 && WriteFile(identity,identity_text,identity_bytes,&identity_written,nullptr) && identity_written == static_cast<DWORD>(identity_bytes) && FlushFileBuffers(identity),"durable owned identity before resume"); CloseHandle(identity);
    HANDLE ready = CreateEventW(nullptr,TRUE,FALSE,nullptr); require(ready != nullptr, "owned parent-death ready"); Drain drain{read,output,ready}; HANDLE reader = CreateThread(nullptr,256*1024,drain_output,&drain,STACK_SIZE_PARAM_IS_A_RESERVATION,nullptr); require(reader != nullptr,"owned bounded output reader");
    require(ResumeThread(app.hThread) != static_cast<DWORD>(-1),"resume actual acceptance app"); CloseHandle(app.hThread);
    bool parent_observed = !parent_death, timed_out = false;
    if (parent_death) {
        HANDLE watched[]{ready,app.hProcess}; DWORD result = WaitForMultipleObjects(2,watched,FALSE,10000);
        if (result == WAIT_OBJECT_0) {
            HANDLE helper = OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,FALSE,drain.helper.load()); require(helper != nullptr,"exact live helper handle before parent death");
            require(WaitForSingleObject(helper,0) == WAIT_TIMEOUT,"helper alive before parent death"); require(TerminateProcess(app.hProcess,98),"terminate exact owned parent"); parent_observed = WaitForSingleObject(helper,7000) == WAIT_OBJECT_0; CloseHandle(helper);
        }
    }
    DWORD timeout = (selected == L"withheld-body-256" || selected == L"hwnd-reuse") ? 240000 : 60000;
    if (WaitForSingleObject(app.hProcess,timeout) != WAIT_OBJECT_0) {timed_out = true; TerminateJobObject(job,99);}
    require(WaitForSingleObject(app.hProcess,7000) == WAIT_OBJECT_0,"exact application reaping"); DWORD exit{}; require(GetExitCodeProcess(app.hProcess,&exit),"actual application exit code");
    TerminateJobObject(job,99);
    bool empty = false; ULONGLONG until = GetTickCount64()+7000;
    while (true) {ULONGLONG now=GetTickCount64();if(now>=until)break;DWORD message{}; ULONG_PTR key{}; LPOVERLAPPED value{}; BOOL result = GetQueuedCompletionStatus(completion,&message,&key,&value,static_cast<DWORD>(until-now)); if (!result) break; if (key == reinterpret_cast<ULONG_PTR>(job) && message == JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO) {empty=true;break;} }
    require(WaitForSingleObject(reader,2000) == WAIT_OBJECT_0,"exact reader thread reaping"); DWORD reader_exit{}; require(GetExitCodeThread(reader,&reader_exit),"reader exit result");
    bool ok = !timed_out && empty && parent_observed && !drain.overflow && reader_exit == 0 && (parent_death ? exit == 98 : negative_control ? exit==1 : exit==0);
    std::printf("{\"outer_job_empty\":%s,\"outer_timeout\":%s,\"parent_death_observed\":%s,\"application_pid\":%lu,\"application_exit\":%lu,\"negative_control_rejected\":%s,\"passed\":%s}\n",empty?"true":"false",timed_out?"true":"false",parent_observed?"true":"false",app.dwProcessId,exit,(negative_control && exit==1)?"true":"false",ok?"true":"false");
    CloseDesktop(owned_desktop); if (!interactive_station) CloseWindowStation(private_station);
    for (HANDLE handle : {app.hProcess,reader,read,output,error,null_input,ready,completion,job}) CloseHandle(handle); return ok?0:1;
}
