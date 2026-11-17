// Keep R10's actual custom marshaler/source. Only direct success now performs Drop.
#define wmain r10_prerequisite_main
#include "production_support.cpp"
#undef wmain
static void ordered_continue() {
    ULONGLONG until=GetTickCount64()+10000;
    for (;;) {
        ULONGLONG now=GetTickCount64();require(now<until,"owned source continue deadline");
        DWORD ready=MsgWaitForMultipleObjects(1,&stop_event,FALSE,static_cast<DWORD>(until-now),QS_ALLINPUT);
        if (ready==WAIT_OBJECT_0) return;
        require(ready==WAIT_OBJECT_0+1,"owned source continue wait");
        MSG message{};for(unsigned count=0;count<32 && PeekMessageW(&message,nullptr,0,0,PM_REMOVE);++count){TranslateMessage(&message);DispatchMessageW(&message);}
    }
}
int wmain(int argc, wchar_t** argv) {
    if (argc == 3 && !std::wcscmp(argv[1], L"--release-owned-ci-input")) return release_owned_ci_input(argv[2]);
    require(argc > 1 && !std::wcscmp(argv[1], L"--source"), "private production source route");
    attach(argc, argv); require(!baseline && (!ole_route || ci_input), "production OLE source requires owned hosted input");
    if (ci_input && ole_route) return source(argc, argv);
    state->source_pid = GetCurrentProcessId(); state->source_tid = GetCurrentThreadId(); require_hr(OleInitialize(nullptr), "source initialize");
    DWORD cookie = custom_source ? register_owned_unmarshaler() : 0;
    IDataObject* offered = custom_source ? static_cast<IDataObject*>(new CustomSource) : new Source;
    HGLOBAL global = GlobalAlloc(GMEM_MOVEABLE, state->marshal_bytes); require(global != nullptr, "source target stream");
    void* bytes = GlobalLock(global); require(bytes != nullptr, "source target stream lock"); memcpy(bytes, state->marshal, state->marshal_bytes); GlobalUnlock(global);
    IStream* stream{}; require_hr(CreateStreamOnHGlobal(global, TRUE, &stream), "source target stream creation");
    IDropTarget* target{}; require_hr(CoUnmarshalInterface(stream, IID_IDropTarget, reinterpret_cast<void**>(&target)), "source actual target proxy"); stream->Release();
    production_source_target = target;
    DWORD effect = DROPEFFECT_COPY; state->enter_result = target->DragEnter(offered, 0, POINTL{state->cursor.x, state->cursor.y}, &effect);
    state->returned_enter=1;state->returned_enter_effect=effect;
    if (!std::wcscmp(scenario, L"source-death")) ExitProcess(98);
    if (!wcscmp(scenario,L"ordered-enter-submit") || ordered_failure_case()) {InterlockedExchange(&state->ordered_enter_waiting,1);SetEvent(ready_event);ordered_continue();}
    if (!wcscmp(scenario,L"terminal-leave")) {target->DragLeave();}
    else
    if (SUCCEEDED(state->enter_result) && effect==DROPEFFECT_COPY) {state->returned_drop_hresult=target->Drop(offered,0,POINTL{state->cursor.x,state->cursor.y},&effect);state->returned_drop_effect=effect;state->returned_drop=1;}
    else target->DragLeave();
    production_source_target = nullptr; target->Release(); offered->Release(); if (cookie) CoRevokeClassObject(cookie); OleUninitialize();state->source_cleanup_done=1;SetEvent(done_event);return 0;
}
