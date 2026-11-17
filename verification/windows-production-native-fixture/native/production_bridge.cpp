// Acceptance DLL: the R10 synthetic source/marshaler, attached to the ACTUAL Rust receiver.
// Its C++ Target/receiver/wmain are compiled only as unused prerequisite code.
#define wmain r10_prerequisite_main
#include "production_support.cpp"
#undef wmain
#include <new>

static IDropTarget* actual_target;
static HANDLE context_mapping;
static IStream* exported_target;
static DWORD factory_cookie;
static PROCESS_INFORMATION offered_process{};
static HANDLE exact_receiver;
static HANDLE ordered_source_job;
static DWORD ordered_helper,ordered_helper_tid;
static LONG ordered_prior_queries,ordered_prior_data;
static std::wstring bridge_context, bridge_source, bridge_case;
static bool bridge_attached;
static bool expected_busy;
static PayloadCounters source_baseline;
static LONG mutation_baseline;
static void mutation_applied(unsigned id) {
    auto& mutation=state->source_mutation;
    if (InterlockedCompareExchange(&mutation.id,id,0)==0) {mutation.actor=0;mutation.pid=GetCurrentProcessId();mutation.tid=GetCurrentThreadId();mutation.guard=state->pump_guard_active;mutation.sequence=state->pump_sequence;mutation.armed=state->controller_armed_sequence;}
    InterlockedIncrement(&mutation.applied);
}
static void payload_begin(unsigned phase,unsigned format) {
    if (GetCurrentProcessId()!=state->receiver_pid || GetCurrentThreadId()!=state->receiver_tid || state->pump_guard_active!=1 || state->pump_sequence<=0 || state->pump_sequence!=state->controller_armed_sequence || state->payload_phase!=0) InterlockedIncrement(&state->payload.phase_bad);
    state->payload_format=format;InterlockedExchange(&state->payload_phase,phase);
    if (phase==1) InterlockedIncrement(&state->payload.helper_queries);
    if (phase==2) {InterlockedIncrement(&state->payload.helper_gets);InterlockedIncrement(&state->payload_ticket);}
    if (phase==3) InterlockedIncrement(&state->payload.helper_releases);
}
static void payload_end(unsigned phase,HRESULT result) {
    if (state->payload_phase!=static_cast<LONG>(phase)) InterlockedIncrement(&state->payload.phase_bad);
    if (phase==1 && result==S_OK && state->payload_format==production_format()) InterlockedIncrement(&state->payload.helper_positive);
    if (phase==2 && result==S_OK) InterlockedIncrement(&state->payload.helper_get_success);
    if (phase==3) InterlockedIncrement(&state->payload.helper_release_ends);
    InterlockedExchange(&state->payload_phase,0);state->payload_format=0;
}
static std::wstring object_name(const wchar_t* suffix) { return bridge_context + suffix; }
static void initialize_events(bool create) {
    auto event = [&](const wchar_t* suffix, bool manual) {
        std::wstring name = object_name(suffix);
        HANDLE value = create ? CreateEventW(nullptr, manual, FALSE, name.c_str()) : OpenEventW(EVENT_ALL_ACCESS, FALSE, name.c_str());
        require(value != nullptr, "acceptance named event"); return value;
    };
    ready_event = event(L"-ready", true); done_event = event(L"-done", true); stop_event = event(L"-stop", true);
    pump_enter_event = event(L"-pump-enter", false); pump_armed_event = event(L"-pump-armed", false); pump_done_event = event(L"-pump-done", false);
}
extern "C" __declspec(dllexport) void __cdecl acceptance_begin(const wchar_t* context, const wchar_t* source_exe, const wchar_t* selected_case, HWND owner) {
    bridge_context = context; bridge_source = source_exe; bridge_case = selected_case;
    scenario = parse_scenario(bridge_case.c_str()); require(!baseline && (!ole_route || ci_input), "production OLE requires owned hosted input opt-in");
    if (ci_input) { wchar_t owner[32]{}; require(GetEnvironmentVariableW(L"SCREENSHOTS_DROP_ACCEPTANCE_DESKTOP_OWNER",owner,32)>0,"explicit desktop owner"); expected_ci_desktop = L"ScreenshotsDropProbe-" + std::wstring(owner); }
    context_mapping = CreateFileMappingW(INVALID_HANDLE_VALUE, nullptr, PAGE_READWRITE, 0, sizeof(Shared), context);
    require(context_mapping && GetLastError() != ERROR_ALREADY_EXISTS, "fresh owned acceptance mapping");
    state = static_cast<Shared*>(MapViewOfFile(context_mapping, FILE_MAP_ALL_ACCESS, 0, 0, sizeof(Shared))); require(state != nullptr, "acceptance mapping");
    initialize_events(true);
    if (!wcsncmp(bridge_case.c_str(),L"noop-",5)) {const auto* spec=expected_injection();require(spec!=nullptr,"no-op control needs a mandatory injection");state->omit_injection=spec->id;}
    state->owner_pid = GetCurrentProcessId();
    wchar_t desktop_owner[32]{}; require(GetEnvironmentVariableW(L"SCREENSHOTS_DROP_ACCEPTANCE_DESKTOP_OWNER",desktop_owner,32)>0,"owned controller identity"); state->controller_pid = wcstoul(desktop_owner,nullptr,10); state->owner_tid = GetCurrentThreadId(); state->owner_hwnd = reinterpret_cast<UINT_PTR>(owner);
    state->cursor = POINT{48, 40};
    state->enter_result = state->query_result = state->get_result = state->registration = E_PENDING;
}
extern "C" __declspec(dllexport) void __cdecl acceptance_attach(const wchar_t* context, const wchar_t* selected_case) {
    bridge_context = context; bridge_case = selected_case; scenario = parse_scenario(bridge_case.c_str());
    require(!baseline && (!ole_route || ci_input), "private receiver owned acceptance route");
    context_mapping = OpenFileMappingW(FILE_MAP_ALL_ACCESS, FALSE, context); require(context_mapping != nullptr, "open exact owned acceptance mapping");
    state = static_cast<Shared*>(MapViewOfFile(context_mapping, FILE_MAP_ALL_ACCESS, 0, 0, sizeof(Shared))); require(state != nullptr && state->owner_pid != GetCurrentProcessId(), "receiver mapping identity");
    initialize_events(false); bridge_attached = true;
    state->receiver_pid=GetCurrentProcessId();state->receiver_tid=GetCurrentThreadId();InterlockedExchange(&state->payload_phase,0);state->payload_format=0;
}
extern "C" __declspec(dllexport) void __cdecl acceptance_hook(unsigned code, unsigned long long sequence, void* pointer) {
    if (!bridge_attached) return;
    if (code>=50 && code<=55) {unsigned phase=(code-50)/2+1;if ((code&1)==0) payload_begin(phase,static_cast<unsigned>(sequence));else payload_end(phase,static_cast<HRESULT>(sequence));return;}
    if (code==56) {if (sequence>0) InterlockedIncrement(&state->payload.submits);else InterlockedIncrement(&state->payload.phase_bad);return;}
    if (code==70) {mutation_applied(static_cast<unsigned>(sequence));return;}
    if (code == 1) {
        InterlockedExchange(&state->pump_sequence, static_cast<LONG>(sequence));
        InterlockedExchange(&state->controller_armed_sequence, static_cast<LONG>(sequence));
        InterlockedExchange(&state->pump_guard_active, 1);
    } else if (code == 2) {
        InterlockedExchange(&state->pump_guard_active, 0);
    } else if (code == 3) {
        factory_cookie = register_owned_unmarshaler();
        if (!std::wcscmp(scenario,L"startup-stall") && try_injection(I_STARTUP)) {SetEvent(ready_event);Sleep(INFINITE);}
    } else if (code == 4) {
        actual_target = static_cast<IDropTarget*>(pointer);
        // Only the actual Rust target, after real RegisterDragDrop, supplies this pointer.
        if (exported_target) { LARGE_INTEGER zero{}; exported_target->Seek(zero, STREAM_SEEK_SET, nullptr); CoReleaseMarshalData(exported_target); exported_target->Release(); }
        require_hr(CreateStreamOnHGlobal(nullptr, TRUE, &exported_target), "actual target marshal stream");
        require_hr(CoMarshalInterface(exported_target, IID_IDropTarget, static_cast<IUnknown*>(pointer), MSHCTX_LOCAL, nullptr, MSHLFLAGS_TABLESTRONG), "marshal actual Rust target");
        STATSTG info{}; require_hr(exported_target->Stat(&info, STATFLAG_NONAME), "actual target stream size");
        require(info.cbSize.HighPart == 0 && info.cbSize.LowPart <= sizeof(state->marshal), "bounded target OBJREF");
        HGLOBAL data{}; require_hr(GetHGlobalFromStream(exported_target, &data), "actual target global");
        void* bytes = GlobalLock(data); require(bytes != nullptr, "actual target bytes");
        memcpy(state->marshal, bytes, info.cbSize.LowPart); GlobalUnlock(data); state->marshal_bytes = info.cbSize.LowPart; state->registration = S_OK;
        if (state->source_pid) SetEvent(ready_event);
    } else if (code == 5) {
        state->enter_pid = GetCurrentProcessId(); state->enter_tid = GetCurrentThreadId(); InterlockedExchange(&state->native_stage, 1);
    } else if (code == 6) {
        InterlockedExchange(&state->native_stage, 2);
    } else if (code == 7) {
        InterlockedExchange(&state->native_stage, 3);
    } else if (code == 8) {
        // Export ownership is released while the real receiver guard is still armed.
        if (exported_target) { LARGE_INTEGER zero{}; exported_target->Seek(zero, STREAM_SEEK_SET, nullptr); CoReleaseMarshalData(exported_target); exported_target->Release(); exported_target = nullptr; }
        if (factory_cookie) { CoRevokeClassObject(factory_cookie); factory_cookie = 0; }
        actual_target = nullptr;
    } else if ((code == 20 && !wcscmp(scenario,L"register-stall")) || (code == 21 && !wcscmp(scenario,L"revoke-stall")) || (code == 22 && !wcscmp(scenario,L"uninitialize-stall"))) {
        InjectionId id=code==20?I_REGISTER:code==21?I_REVOKE:I_UNINITIALIZE;
        if (try_injection(id)) {SetEvent(ready_event);Sleep(INFINITE);}
    } else if (code == 11) {
        state->get_result = static_cast<LONG>(sequence);
    } else if (code == 9) {
        SetEvent(ready_event);
    }
}
static bool bridge_wait(HANDLE watched, DWORD timeout) {
    ULONGLONG end = GetTickCount64() + timeout;
    for (;;) {
        ULONGLONG now = GetTickCount64(); if (now >= end) return false;
        DWORD ready = MsgWaitForMultipleObjects(1, &watched, FALSE, static_cast<DWORD>(end - now), QS_ALLINPUT);
        if (ready == WAIT_OBJECT_0) return true;
        require(ready == WAIT_OBJECT_0 + 1, "owned production event wait");
        MSG message{}; while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) { TranslateMessage(&message); DispatchMessageW(&message); }
    }
}
static PayloadCounters payload_delta() {return {state->payload.helper_queries-source_baseline.helper_queries,state->payload.helper_positive-source_baseline.helper_positive,state->payload.helper_gets-source_baseline.helper_gets,state->payload.helper_get_success-source_baseline.helper_get_success,state->payload.helper_releases-source_baseline.helper_releases,state->payload.helper_release_ends-source_baseline.helper_release_ends,state->payload.source_selected_queries-source_baseline.source_selected_queries,state->payload.source_positive-source_baseline.source_positive,state->payload.source_gets-source_baseline.source_gets,state->payload.source_created-source_baseline.source_created,state->payload.source_released-source_baseline.source_released,state->payload.outside_positive-source_baseline.outside_positive,state->payload.outside_gets-source_baseline.outside_gets,state->payload.outside_created-source_baseline.outside_created,state->payload.outside_released-source_baseline.outside_released,state->payload.submits-source_baseline.submits,state->payload.phase_bad-source_baseline.phase_bad};}
static bool final_source_oracle(DWORD helper,DWORD helper_tid,bool helper_reaped,bool source_finished,DWORD source_exit,bool source_natural) {
    PayloadCounters d=payload_delta();const auto* spec=expected_injection();
    bool startup=spec && (spec->id==I_STARTUP || spec->id==I_REGISTER);
    bool teardown=spec && (spec->id==I_REVOKE || spec->id==I_UNINITIALIZE);
    bool killed=spec && spec->kind==InjectionKind::Stall && !teardown;
    bool death=!wcscmp(scenario,L"source-death");bool leave=!wcscmp(scenario,L"terminal-leave");
    bool rejected=spec && !teardown && !killed;
    const char* outcome=expected_busy?"capacity-refusal":startup?"no-source":death?"source-death":killed?"isolated-failure":rejected?"rejection":leave?"source-leave":"success";
    bool source_done=source_finished && source_natural && source_exit==0 && state->source_cleanup_done==1;
    bool direct_success=state->returned_enter==1 && state->enter_result==S_OK && state->returned_enter_effect==DROPEFFECT_COPY && state->returned_drop==1 && state->returned_drop_hresult==S_OK && state->returned_drop_effect==DROPEFFECT_COPY;
    bool ole_success=state->returned_drop==1 && state->returned_drop_hresult==DRAGDROP_S_DROP && state->returned_drop_effect==DROPEFFECT_COPY;
    bool direct_refusal=state->returned_enter==1 && state->enter_result==S_OK && state->returned_enter_effect==DROPEFFECT_NONE && state->returned_drop==0;
    bool ole_refusal=state->returned_drop==1 && state->returned_drop_effect==DROPEFFECT_NONE && (state->returned_drop_hresult==DRAGDROP_S_DROP || state->returned_drop_hresult==DRAGDROP_S_CANCEL);
    bool direct_reject=state->returned_drop ? (FAILED(state->returned_drop_hresult)||state->returned_drop_effect==DROPEFFECT_NONE) : state->returned_enter && (FAILED(state->enter_result)||state->returned_enter_effect==DROPEFFECT_NONE);
    bool ole_reject=state->returned_drop && (FAILED(state->returned_drop_hresult)||state->returned_drop_effect==DROPEFFECT_NONE);
    bool result=expected_busy?source_done && (ole_route?ole_refusal:direct_refusal):startup?helper_reaped && state->source_pid==0:death?source_natural && source_exit==98 && state->returned_enter==1 && state->enter_result==S_OK && state->returned_enter_effect==DROPEFFECT_COPY && !state->returned_drop:killed?helper_reaped:rejected?source_done && (ole_route?ole_reject:direct_reject):leave?source_done && state->returned_enter && state->enter_result==S_OK && state->returned_enter_effect==DROPEFFECT_COPY && !state->returned_drop:source_done && (ole_route?ole_success:direct_success);
    bool cached_query=ole_route && d.source_selected_queries==0 && d.source_positive==0 && (d.outside_positive>0 || d.outside_created>0);
    bool cached_get=ole_route && d.source_gets==0 && d.source_created==0 && d.source_released==0 && d.outside_created>0;
    bool query=d.helper_queries>=1 && d.helper_queries<=8 && d.helper_positive==1 && ((d.source_selected_queries==1 && d.source_positive==1)||cached_query);
    bool acquisition=d.helper_gets==1 && d.helper_get_success==1 && d.helper_releases==1 && d.helper_release_ends==1 && ((d.source_gets==1 && d.source_created==1 && d.source_released==1)||cached_get);
    bool zero=d.helper_queries==0 && d.helper_positive==0 && d.helper_gets==0 && d.helper_get_success==0 && d.helper_releases==0 && d.helper_release_ends==0 && d.source_selected_queries==0 && d.source_positive==0 && d.source_gets==0 && d.source_created==0 && d.source_released==0 && d.submits==0;
    bool prefix=d.helper_queries>=0 && d.helper_queries<=8 && d.helper_positive>=0 && d.helper_positive<=1 && d.helper_gets>=0 && d.helper_gets<=1 && d.helper_get_success>=0 && d.helper_get_success<=d.helper_gets && d.helper_releases>=0 && d.helper_releases<=d.helper_get_success && d.helper_release_ends>=0 && d.helper_release_ends<=d.helper_releases && d.source_selected_queries>=0 && d.source_selected_queries<=1 && d.source_positive>=0 && d.source_positive<=d.source_selected_queries && d.source_gets>=0 && d.source_gets<=1 && d.source_created>=0 && d.source_created<=d.source_gets && d.source_released>=0 && d.source_released<=d.source_created && d.submits>=0 && d.submits<=(killed?1:0);
    bool counts=d.phase_bad==0 && ((expected_busy||startup)?zero:death?query && d.helper_gets==1 && d.helper_get_success==1 && d.helper_releases==1 && d.helper_release_ends==1 && d.source_gets==1 && d.source_created==1 && d.source_released>=0 && d.source_released<=1 && d.submits==0:leave?query && acquisition && d.submits==0:(killed||rejected)?prefix:query && acquisition && d.submits==1);
    auto& mutation=state->source_mutation;LONG applied=mutation.applied-mutation_baseline;
    bool mutation_guard=applied>0 && mutation.pid==helper && mutation.tid==helper_tid && mutation.guard==1 && mutation.sequence>0 && mutation.sequence==mutation.armed;
    bool mutation_context=mutation.actor==0?mutation_guard:applied>0 && mutation.pid==state->source_pid && mutation.tid==state->source_tid && state->source_adapter_applied>0 && state->raw_ole_returned==1;
    bool ok=result && counts;
    if(expected_busy && ok) return true; // Every refusal is checked; successful detail rows are coalesced below.
    std::printf("{\"source_final_oracle\":true,\"expected_outcome\":\"%s\",\"ole_route\":%s,\"result_verified\":%s,\"payload_counts_verified\":%s,\"source_oracle_passed\":%s,\"source_cleanup_done\":%ld,\"source_exit\":%lu,\"source_natural_exit\":%s,\"enter_returned\":%ld,\"enter_hresult\":%ld,\"enter_effect\":%ld,\"drop_returned\":%ld,\"drop_hresult\":%ld,\"drop_effect\":%ld,\"cached_provider_query\":%s,\"cached_provider_get\":%s,\"mutation_id\":%ld,\"mutation_applied\":%ld,\"mutation_guard_verified\":%s,\"duplicate_release_calls\":%ld,\"mutation_actor\":%ld,\"mutation_context_verified\":%s,\"raw_ole_returned\":%ld,\"raw_ole_hresult\":%ld,\"raw_ole_effect\":%ld,\"source_adapter_applied\":%ld,",outcome,ole_route?"true":"false",result?"true":"false",counts?"true":"false",ok?"true":"false",state->source_cleanup_done,source_exit,source_natural?"true":"false",state->returned_enter,state->enter_result,state->returned_enter_effect,state->returned_drop,state->returned_drop_hresult,state->returned_drop_effect,cached_query?"true":"false",cached_get?"true":"false",mutation.id,applied,mutation_guard?"true":"false",mutation.duplicate_release_calls,mutation.actor,mutation_context?"true":"false",state->raw_ole_returned,state->raw_ole_hresult,state->raw_ole_effect,state->source_adapter_applied);
    std::printf("\"helper_queries\":%ld,\"helper_positive\":%ld,\"helper_gets\":%ld,\"helper_get_success\":%ld,\"helper_releases\":%ld,\"helper_release_ends\":%ld,\"source_selected_queries\":%ld,\"source_positive\":%ld,\"source_gets\":%ld,\"source_created\":%ld,\"source_released\":%ld,\"outside_positive\":%ld,\"outside_gets\":%ld,\"outside_created\":%ld,\"outside_released\":%ld,\"submits\":%ld,\"phase_bad\":%ld}\n",d.helper_queries,d.helper_positive,d.helper_gets,d.helper_get_success,d.helper_releases,d.helper_release_ends,d.source_selected_queries,d.source_positive,d.source_gets,d.source_created,d.source_released,d.outside_positive,d.outside_gets,d.outside_created,d.outside_released,d.submits,d.phase_bad);std::fflush(stdout);return ok;
}
static int finish_drive(HANDLE source_job,DWORD helper,DWORD helper_tid,LONG prior_queries,LONG prior_data) {
    bool source_death = !std::wcscmp(scenario, L"source-death");
    bool finished = source_death ? bridge_wait(offered_process.hProcess, 2000) : bridge_wait(done_event, 7000);
    if (source_death) { DWORD code{}; require(GetExitCodeProcess(offered_process.hProcess, &code) && code == 98, "observed exact source death"); }
    const auto* spec=expected_injection();
    bool deferred=spec && (spec->id==I_REVOKE || spec->id==I_UNINITIALIZE);
    bool stall=spec && spec->kind==InjectionKind::Stall && !deferred;
    bool helper_reaped=stall ? (state->injections[spec->id].fired>0 && WaitForSingleObject(exact_receiver,7000)==WAIT_OBJECT_0) : false;
    bool source_natural=finished && bridge_wait(offered_process.hProcess,2000);
    if (!source_natural) TerminateJobObject(source_job,99);
    require(WaitForSingleObject(offered_process.hProcess,2000)==WAIT_OBJECT_0,"exact source reaping");
    DWORD source_exit=STILL_ACTIVE;require(GetExitCodeProcess(offered_process.hProcess,&source_exit),"actual final source exit");
    bool custom = custom_source;
    bool first = !custom || (state->first_factory_pid == static_cast<LONG>(helper) && state->first_unmarshal_pid == static_cast<LONG>(helper) && state->first_factory_tid == helper_tid && state->first_unmarshal_tid == helper_tid && state->first_factory_guard == 1 && state->first_unmarshal_guard == 1 && state->first_factory_sequence > 0 && state->first_factory_sequence == state->first_factory_armed_sequence && state->first_unmarshal_sequence == state->first_unmarshal_armed_sequence && state->first_factory_native_stage == (ole_route ? 2 : 0) && state->first_unmarshal_native_stage == (ole_route ? 2 : 0) && state->unmarshal_release_outside_guard == 0);
    bool fault_before_entry = !std::wcscmp(scenario, L"unmarshal-stall") || !std::wcscmp(scenario, L"unmarshal-memory");
    bool provider = fault_before_entry ? state->query_calls == 0 && state->data_calls == 0 && (ole_route ? state->enter_pid == helper : state->enter_pid == 0) : state->enter_pid == helper && state->enter_tid == helper_tid && state->query_pid == state->source_pid;
    if (expected_busy) provider = state->query_calls == prior_queries && state->data_calls == prior_data;
    bool allocation = std::wcscmp(scenario, L"unmarshal-memory") || state->unmarshal_memory_refused == 1;
    if (!std::wcscmp(scenario,L"over-memory")) allocation = state->offered_bytes == 128 * 1024 * 1024 && state->source_data_result == S_OK && FAILED(state->get_result);
    if (ci_input) { require(owned_left_button(MOUSEEVENTF_LEFTUP), "owned production OLE controller button release"); InterlockedIncrement(&state->controller_button_ups); }
    bool owned_input = !ci_input || (state->injected_button_downs == 1 && state->ci_cursor_moves >= 1 && state->controller_button_ups == 1 && state->input_not_owned == 0 && state->source_input_error == 0 && state->controller_input_error == 0);
    bool no_description = state->error_descriptions == 0;
    DWORD helper_exit=STILL_ACTIVE;bool exit_ok=!stall || (helper_reaped && GetExitCodeProcess(exact_receiver,&helper_exit) && helper_exit==70);
    bool positive=deferred || (exit_ok && injection_evidence(helper,helper_tid));
    if (!deferred) report_injection(helper,helper_tid,positive);
    bool alive_after_delivery=WaitForSingleObject(exact_receiver,0)==WAIT_TIMEOUT;
    if (deferred) {std::printf("{\"teardown_validation_deferred\":true,\"helper_alive_after_delivery\":%s,\"helper_pid\":%lu}\n",alive_after_delivery?"true":"false",helper);std::fflush(stdout);}
    bool source_valid=final_source_oracle(helper,helper_tid,helper_reaped,finished,source_exit,source_natural);
    bool ok=source_valid && owned_input && no_description && positive && helper!=state->owner_pid && first && provider && allocation && (!stall || helper_reaped) && (stall || finished) && (!deferred || alive_after_delivery);
    if (expected_busy && ok) {CloseHandle(offered_process.hProcess);CloseHandle(source_job);return 0;}
    std::printf("{\"helper_tid\":%lu,\"first_factory_pid\":%ld,\"first_factory_tid\":%lu,\"first_factory_native_stage\":%ld,\"first_factory_guard\":%ld,\"first_factory_sequence\":%ld,\"first_factory_armed_sequence\":%ld,\"first_unmarshal_pid\":%ld,\"first_unmarshal_tid\":%lu,\"first_unmarshal_native_stage\":%ld,\"first_unmarshal_guard\":%ld,\"first_unmarshal_sequence\":%ld,\"first_unmarshal_armed_sequence\":%ld,\"error_releases\":%ld,\"error_descriptions\":%ld,\"helper_reentries\":%ld,\"get_hresult\":%ld,",helper_tid,state->first_factory_pid,state->first_factory_tid,state->first_factory_native_stage,state->first_factory_guard,state->first_factory_sequence,state->first_factory_armed_sequence,state->first_unmarshal_pid,state->first_unmarshal_tid,state->first_unmarshal_native_stage,state->first_unmarshal_guard,state->first_unmarshal_sequence,state->first_unmarshal_armed_sequence,state->error_releases,state->error_descriptions,state->helper_reentries,state->get_result);
    std::printf("\"actual_rust_helper\":true,\"helper_pid\":%lu,\"source_pid\":%lu,\"first_boundary\":%s,\"provider_topology\":%s,\"helper_reaped_on_fault\":%s,\"source_reaped\":true,\"source_finished\":%s,\"memory_refused\":%ld,\"query_calls\":%ld,\"data_calls\":%ld,\"medium_releases\":%ld,\"unmarshal_release_outside_guard\":%ld}\n", helper, state->source_pid, first ? "true" : "false", provider ? "true" : "false", helper_reaped ? "true" : "false", finished ? "true" : "false", state->unmarshal_memory_refused, state->query_calls, state->data_calls, state->medium_releases, state->unmarshal_release_outside_guard); std::fflush(stdout);
    CloseHandle(offered_process.hProcess); CloseHandle(source_job); return ok ? 0 : 1;

}
extern "C" __declspec(dllexport) int __cdecl acceptance_drive() {
    if (!expected_busy) require(bridge_wait(ready_event, 7000), "actual helper ALLOW ready"); ResetEvent(ready_event);
    LONG prior_queries=state->query_calls,prior_data=state->data_calls;
    source_baseline=state->payload;mutation_baseline=state->source_mutation.applied;
    state->returned_enter=0;state->returned_enter_effect=0;state->returned_drop=0;state->returned_drop_effect=0;state->returned_drop_hresult=E_PENDING;state->source_cleanup_done=0;state->raw_ole_returned=0;state->source_adapter_applied=0;
    DWORD helper = state->receiver_pid, helper_tid = state->receiver_tid;
    if (exact_receiver) CloseHandle(exact_receiver);
    exact_receiver = OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION, FALSE, helper); require(exact_receiver != nullptr, "exact production receiver process");
    if (!wcscmp(scenario,L"startup-stall") || !wcscmp(scenario,L"register-stall")) {
        const auto* spec=expected_injection();
        bool killed=state->injections[spec->id].fired>0 && WaitForSingleObject(exact_receiver,7000)==WAIT_OBJECT_0;
        DWORD exit=STILL_ACTIVE;bool exit_ok=killed && GetExitCodeProcess(exact_receiver,&exit) && exit==70;
        bool positive=exit_ok && injection_evidence(helper,helper_tid);
        report_injection(helper,helper_tid,positive);
        bool source_valid=final_source_oracle(helper,helper_tid,killed,false,STILL_ACTIVE,false);
        bool ok=source_valid && killed && positive && state->source_pid==0 && state->query_calls==0 && state->data_calls==0;
        std::printf("{\"actual_lifecycle_guard\":true,\"helper_reaped\":%s,\"provider_calls\":0}\n",killed?"true":"false"); std::fflush(stdout); return ok?0:1;
    }
    HANDLE source_job = CreateJobObjectW(nullptr, nullptr); require(source_job != nullptr, "source fixture Job");
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION limit{}; limit.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS | JOB_OBJECT_LIMIT_PROCESS_MEMORY; limit.BasicLimitInformation.ActiveProcessLimit = 1; limit.ProcessMemoryLimit = 512 * 1024 * 1024;
    require(SetInformationJobObject(source_job, JobObjectExtendedLimitInformation, &limit, sizeof(limit)), "source Job limits");
    std::vector<HANDLE> inherited{context_mapping, ready_event, done_event, stop_event, pump_enter_event, pump_armed_event, pump_done_event};
    for (HANDLE handle : inherited) require(SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT), "source inherited whitelist");
    std::wstring command = L"\"" + bridge_source + L"\" --source " + std::to_wstring(reinterpret_cast<UINT_PTR>(context_mapping)) + L" " + std::to_wstring(reinterpret_cast<UINT_PTR>(ready_event)) + L" " + std::to_wstring(reinterpret_cast<UINT_PTR>(done_event)) + L" " + std::to_wstring(reinterpret_cast<UINT_PTR>(stop_event)) + L" " + std::to_wstring(state->owner_hwnd) + L" " + bridge_case;
    wchar_t station[256]{}, desktop[256]{}; DWORD actual{};
    require(GetUserObjectInformationW(GetProcessWindowStation(), UOI_NAME, station, sizeof(station), &actual), "source station identity");
    require(GetUserObjectInformationW(GetThreadDesktop(GetCurrentThreadId()), UOI_NAME, desktop, sizeof(desktop), &actual), "source desktop identity");
    offered_process = launch(command, std::wstring(station) + L"\\" + desktop, source_job, inherited);
    if (!wcscmp(scenario,L"ordered-enter-submit") || ordered_failure_case()) {
        ULONGLONG until=GetTickCount64()+7000;
        while (InterlockedCompareExchange(&state->ordered_enter_waiting,0,0)==0) {
            ULONGLONG now=GetTickCount64();require(now<until && bridge_wait(ready_event,static_cast<DWORD>(until-now)),"actual ENTER source gate");ResetEvent(ready_event);
        }
        ordered_source_job=source_job;ordered_helper=helper;ordered_helper_tid=helper_tid;ordered_prior_queries=prior_queries;ordered_prior_data=prior_data;std::printf("{\"actual_ordered_source_waiting\":true,\"source_pid\":%lu}\n",state->source_pid);std::fflush(stdout);return 0;
    }
    return finish_drive(source_job,helper,helper_tid,prior_queries,prior_data);
}

extern "C" __declspec(dllexport) void __cdecl acceptance_end() {
    if (exact_receiver) { CloseHandle(exact_receiver); exact_receiver = nullptr; }
    UnmapViewOfFile(state); state = nullptr; CloseHandle(context_mapping);
    for (HANDLE handle : {ready_event, done_event, stop_event, pump_enter_event, pump_armed_event, pump_done_event}) CloseHandle(handle);
}
extern "C" __declspec(dllexport) void __cdecl acceptance_input(HWND owner, const wchar_t* path, const char* url, int x, int y) {
    state->owner_hwnd = reinterpret_cast<UINT_PTR>(owner);
    if (path) wcscpy_s(state->fixture_file, path);
    if (url) strcpy_s(state->fixture_url, url);
    state->cursor = POINT{x, y};
}
struct ProductionError final : IErrorInfo {
    LONG refs = 1;
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override { if (!out) return E_POINTER; *out = nullptr; if (iid != IID_IUnknown && iid != IID_IErrorInfo) return E_NOINTERFACE; *out = this; AddRef(); return S_OK; }
    ULONG STDMETHODCALLTYPE AddRef() override { return InterlockedIncrement(&refs); }
    ULONG STDMETHODCALLTYPE Release() override {
        LONG count = InterlockedDecrement(&refs);
        if (!count) {
            InjectionId id=!wcscmp(scenario,L"error-release-stall")?I_ERROR_STALL:I_ERROR_REENTER;
            InterlockedIncrement(&state->injections[id].fired);InterlockedIncrement(&state->error_releases);
            if (id==I_ERROR_STALL) Sleep(INFINITE);
            if (actual_target) injection_completed(id,actual_target->DragLeave());
            delete this;
        }
        return count;
    }
    HRESULT STDMETHODCALLTYPE GetGUID(GUID* value) override { if (!value) return E_POINTER; *value = GUID_NULL; return S_OK; }
    HRESULT STDMETHODCALLTYPE GetSource(BSTR*) override { return E_NOTIMPL; }
    HRESULT STDMETHODCALLTYPE GetDescription(BSTR*) override { InterlockedIncrement(&state->error_descriptions); return E_NOTIMPL; }
    HRESULT STDMETHODCALLTYPE GetHelpFile(BSTR*) override { return E_NOTIMPL; }
    HRESULT STDMETHODCALLTYPE GetHelpContext(DWORD*) override { return E_NOTIMPL; }
};
static bool injected_error;
extern "C" __declspec(dllexport) void __cdecl acceptance_error() {
    if (!bridge_attached || injected_error || !state->enter_pid || !wcsstr(scenario, L"error-release")) return;
    injected_error=true;
    InjectionId id=!wcscmp(scenario,L"error-release-stall")?I_ERROR_STALL:I_ERROR_REENTER;
    if (!injection_enabled(id)) return;
    auto* value=new ProductionError;require_hr(SetErrorInfo(0,value),"owned TLS error reference");
    InterlockedIncrement(&state->error_installations);value->Release();
}
struct ProductionHelper final : IDropTargetHelper {
    LONG refs = 1; IDropTargetHelper* inner;
    explicit ProductionHelper(IDropTargetHelper* value) : inner(value) {}
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override { if (!out) return E_POINTER; *out = nullptr; if (iid != IID_IUnknown && iid != IID_IDropTargetHelper) return E_NOINTERFACE; *out = this; AddRef(); return S_OK; }
    ULONG STDMETHODCALLTYPE AddRef() override { return InterlockedIncrement(&refs); }
    ULONG STDMETHODCALLTYPE Release() override {
        LONG count = InterlockedDecrement(&refs); if (!count) {
            if (!wcscmp(scenario,L"helper-release-stall") && try_injection(I_HELPER_RELEASE_STALL)) Sleep(INFINITE);
            if (!wcscmp(scenario,L"helper-release-reenter") && actual_target && try_injection(I_HELPER_RELEASE_REENTER)) {InterlockedIncrement(&state->helper_reentries);injection_completed(I_HELPER_RELEASE_REENTER,actual_target->DragLeave());}
            inner->Release(); delete this;
        } return count;
    }
    void fault(const wchar_t* stage) {
        InterlockedIncrement(&state->helper_calls);
        std::wstring prefix = std::wstring(L"helper-") + stage;
        if (std::wstring(scenario)==prefix+L"-stall") {
            InjectionId id=!wcscmp(stage,L"enter")?I_HELPER_ENTER_STALL:I_HELPER_DROP_STALL;
            if (try_injection(id)) Sleep(INFINITE);
        }
        if (std::wstring(scenario)==prefix+L"-reenter" && actual_target) {
            InjectionId id=!wcscmp(stage,L"enter")?I_HELPER_ENTER_REENTER:I_HELPER_DROP_REENTER;
            if (try_injection(id)) {InterlockedIncrement(&state->helper_reentries);injection_completed(id,actual_target->DragLeave());}
        }
    }
    HRESULT STDMETHODCALLTYPE DragEnter(HWND hwnd, IDataObject* data, POINT* point, DWORD effect) override { fault(L"enter"); return inner->DragEnter(hwnd, data, point, effect); }
    HRESULT STDMETHODCALLTYPE DragLeave() override { fault(L"leave"); return inner->DragLeave(); }
    HRESULT STDMETHODCALLTYPE DragOver(POINT* point, DWORD effect) override { fault(L"over"); return inner->DragOver(point, effect); }
    HRESULT STDMETHODCALLTYPE Drop(IDataObject* data, POINT* point, DWORD effect) override { fault(L"drop"); return inner->Drop(data, point, effect); }
    HRESULT STDMETHODCALLTYPE Show(BOOL visible) override { return inner->Show(visible); }
};
extern "C" __declspec(dllexport) HRESULT __cdecl acceptance_helper(IDropTargetHelper** output) {
    if (!bridge_attached || !wcsstr(scenario, L"helper-")) return S_FALSE;
    if (!wcscmp(scenario,L"helper-factory-stall") && try_injection(I_HELPER_FACTORY)) Sleep(INFINITE);
    IDropTargetHelper* inner{}; HRESULT result = CoCreateInstance(CLSID_DragDropHelper, nullptr, CLSCTX_INPROC_SERVER, IID_PPV_ARGS(&inner));
    if (FAILED(result)) return result;
    *output = new ProductionHelper(inner); return S_OK;
}
extern "C" __declspec(dllexport) void __cdecl acceptance_rearm(HWND owner) {
    ResetEvent(ready_event); ResetEvent(done_event); ResetEvent(stop_event);
    state->ordered_enter_waiting=0;state->source_cleanup_done=0;state->returned_enter=0;state->returned_drop=0;
    state->owner_hwnd = reinterpret_cast<UINT_PTR>(owner); state->enter_pid = 0; state->native_stage = 0;
}
extern "C" __declspec(dllexport) int __cdecl acceptance_wait_reaped() {
    if (!exact_receiver) return 1;
    DWORD result = WaitForSingleObject(exact_receiver, 7000);
    return result == WAIT_OBJECT_0 ? 0 : 1;
}

extern "C" __declspec(dllexport) int __cdecl acceptance_busy_drive() {
    static unsigned iteration=0;ResetEvent(done_event);expected_busy=true;int result=acceptance_drive();expected_busy=false;
    std::printf("{\"capacity_refusal_iteration\":%u,\"source_result_and_counts_verified\":%s}\n",++iteration,result==0?"true":"false");std::fflush(stdout);return result;
}
// Called only from the actual broker timeout branch, before its Job reap.
extern "C" __declspec(dllexport) void __cdecl acceptance_deadline() {broker_deadline_observed();}
extern "C" __declspec(dllexport) int __cdecl acceptance_begin_teardown() {
    const auto* spec=expected_injection();
    if (!spec || (spec->id!=I_REVOKE && spec->id!=I_UNINITIALIZE) || !exact_receiver) return 1;
    if (WaitForSingleObject(exact_receiver,0)!=WAIT_TIMEOUT || state->injections[spec->id].visits!=0 || state->deadline_count!=0) return 1;
    state->teardown_helper_pid=GetProcessId(exact_receiver);state->teardown_helper_tid=state->receiver_tid;
    state->teardown_before_sequence=InterlockedCompareExchange(&state->pump_sequence,0,0);
    state->teardown_tick=GetTickCount64();MemoryBarrier();InterlockedExchange(&state->teardown_requested,1);
    std::printf("{\"initial_delivery_acknowledged\":true,\"helper_pid\":%lu,\"teardown_before_sequence\":%ld}\n",state->teardown_helper_pid,state->teardown_before_sequence);std::fflush(stdout);return 0;
}
extern "C" __declspec(dllexport) int __cdecl acceptance_finish_teardown() {
    const auto* spec=expected_injection();if (!spec || !state->teardown_requested || !exact_receiver) return 1;
    // No fixture kill occurs here: the caller has closed the target and joined the real broker.
    DWORD exit=STILL_ACTIVE;bool reaped=WaitForSingleObject(exact_receiver,0)==WAIT_OBJECT_0 && GetExitCodeProcess(exact_receiver,&exit);
    const auto& trace=state->injections[spec->id];
    bool ordered=trace.tick>=state->teardown_tick && trace.sequence>state->teardown_before_sequence && state->deadline_tick>=trace.tick && GetTickCount64()-state->teardown_tick<=8000;
    bool positive=injection_evidence(state->teardown_helper_pid,state->teardown_helper_tid) && reaped && exit==70 && ordered;
    report_injection(state->teardown_helper_pid,state->teardown_helper_tid,positive);
    std::printf("{\"post_close_teardown\":true,\"exact_receiver_reaped\":%s,\"receiver_exit\":%lu,\"ordered_after_close\":%s,\"mandatory_injection_evidence\":%s}\n",reaped?"true":"false",exit,ordered?"true":"false",positive?"true":"false");std::fflush(stdout);return positive?0:1;
}

extern "C" __declspec(dllexport) int __cdecl acceptance_rejection_effect() {
    DWORD exit=STILL_ACTIVE;bool reaped=exact_receiver && WaitForSingleObject(exact_receiver,0)==WAIT_OBJECT_0 && GetExitCodeProcess(exact_receiver,&exit) && exit==70;
    bool returned=state->source_cleanup_done && (state->returned_drop ? (FAILED(state->returned_drop_hresult) || state->returned_drop_effect==DROPEFFECT_NONE) : (state->returned_enter && (FAILED(state->enter_result)||state->returned_enter_effect==DROPEFFECT_NONE)));
    bool death=!wcscmp(scenario,L"source-death") && !state->returned_drop;
    bool verified=returned || reaped || death;
    std::printf("{\"rejection_effect_verified\":%s,\"source_cleanup_done\":%ld,\"enter_returned\":%ld,\"enter_hresult\":%ld,\"enter_effect\":%ld,\"drop_returned\":%ld,\"drop_hresult\":%ld,\"drop_effect\":%ld,\"exact_helper_reaped\":%s}\n",verified?"true":"false",state->source_cleanup_done,state->returned_enter,state->enter_result,state->returned_enter_effect,state->returned_drop,state->returned_drop_hresult,state->returned_drop_effect,reaped?"true":"false");std::fflush(stdout);return verified?0:1;
}

extern "C" __declspec(dllexport) void __cdecl acceptance_order_next() {bridge_case=L"ordered-enter-submit";scenario=parse_scenario(bridge_case.c_str());}
extern "C" __declspec(dllexport) int __cdecl acceptance_finish_ordered() {
    if (!ordered_source_job || !state->ordered_enter_waiting) return 1;
    require(SetEvent(stop_event),"release source after real consumed ENTER");
    bool done=bridge_wait(done_event,7000) && WaitForSingleObject(offered_process.hProcess,2000)==WAIT_OBJECT_0;
    DWORD exit=STILL_ACTIVE;bool observed=done && GetExitCodeProcess(offered_process.hProcess,&exit);
    bool source_valid=final_source_oracle(ordered_helper,ordered_helper_tid,false,done,exit,observed);
    bool ok=observed && source_valid;
    if (!done) TerminateJobObject(ordered_source_job,99);
    require(WaitForSingleObject(offered_process.hProcess,2000)==WAIT_OBJECT_0,"exact ordered source reap");
    CloseHandle(offered_process.hProcess);CloseHandle(ordered_source_job);ordered_source_job=nullptr;
    std::printf("{\"actual_drop_after_consumed_enter\":%s,\"returned_drop_effect\":%ld,\"source_reaped\":true,\"ordered_source_passed\":%s}\n",state->returned_drop?"true":"false",state->returned_drop_effect,ok?"true":"false");std::fflush(stdout);return ok?0:1;
}

extern "C" __declspec(dllexport) int __cdecl acceptance_finish_ordered_failure() {
    if (!ordered_source_job || !ordered_failure_case() || !state->ordered_enter_waiting) return 1;
    require(SetEvent(stop_event),"release Drop only after real ENTER publication");
    HANDLE job=ordered_source_job;ordered_source_job=nullptr;
    return finish_drive(job,ordered_helper,ordered_helper_tid,ordered_prior_queries,ordered_prior_data);
}

struct SafeReleaseOwner final : IUnknown {
    LONG refs=2;STGMEDIUM original;
    explicit SafeReleaseOwner(STGMEDIUM value):original(value){}
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid,void** out) override {if(!out)return E_POINTER;*out=nullptr;if(iid!=IID_IUnknown)return E_NOINTERFACE;*out=this;AddRef();return S_OK;}
    ULONG STDMETHODCALLTYPE AddRef() override{return InterlockedIncrement(&refs);}
    ULONG STDMETHODCALLTYPE Release() override {InterlockedIncrement(&state->source_mutation.duplicate_release_calls);LONG count=InterlockedDecrement(&refs);if(!count){ReleaseStgMedium(&original);delete this;}return count;}
};
extern "C" __declspec(dllexport) HRESULT __cdecl acceptance_duplicate_release(STGMEDIUM* original,STGMEDIUM* copy) {
    if(!original||!copy)return E_POINTER;
    auto* owner=new(std::nothrow) SafeReleaseOwner(*original);if(!owner)return E_OUTOFMEMORY;
    original->pUnkForRelease=owner;*copy=*original;mutation_applied(4);return S_OK;
}
