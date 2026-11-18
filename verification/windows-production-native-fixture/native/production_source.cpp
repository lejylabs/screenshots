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
struct CheckRelease final : IUnknown {
    LONG refs=1;HGLOBAL global;LONG* released;
    CheckRelease(HGLOBAL value,LONG* count):global(value),released(count){}
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid,void** out) override {if(!out)return E_POINTER;*out=nullptr;if(iid!=IID_IUnknown)return E_NOINTERFACE;*out=this;AddRef();return S_OK;}
    ULONG STDMETHODCALLTYPE AddRef() override {return InterlockedIncrement(&refs);}
    ULONG STDMETHODCALLTYPE Release() override {LONG count=InterlockedDecrement(&refs);if(!count){GlobalFree(global);InterlockedIncrement(released);delete this;}return count;}
};
static STGMEDIUM check_medium(SIZE_T bytes,BYTE value,LONG* released=nullptr) {
    HGLOBAL global=GlobalAlloc(GMEM_MOVEABLE|GMEM_ZEROINIT,bytes);require(global!=nullptr,"self-check medium allocation");
    void* data=GlobalLock(global);require(data!=nullptr,"self-check medium lock");memset(data,value,bytes);GlobalUnlock(global);
    STGMEDIUM medium{};medium.tymed=TYMED_HGLOBAL;medium.hGlobal=global;if(released)medium.pUnkForRelease=new CheckRelease(global,released);return medium;
}
static void check_private_value(Source* source,FORMATETC* format,BYTE expected) {
    STGMEDIUM copy{};require_hr(source->GetData(format,&copy),"self-check private copy");
    require(copy.tymed==TYMED_HGLOBAL && GlobalSize(copy.hGlobal)>=4,"self-check copied private medium");
    auto* bytes=static_cast<BYTE*>(GlobalLock(copy.hGlobal));require(bytes!=nullptr,"self-check private medium lock");for(unsigned index=0;index<4;++index)require(bytes[index]==expected,"self-check exact private bytes");GlobalUnlock(copy.hGlobal);ReleaseStgMedium(&copy);
}
static int private_formats_self_check() {
    Shared local{};state=&local;scenario=L"normal";require_hr(OleInitialize(nullptr),"self-check source STA");HANDLE budget=contain_native();
    FORMATETC formats[9]{};
    for(unsigned index=0;index<9;++index){std::wstring name=L"Screenshots.Owned.Private."+std::to_wstring(index);formats[index]={static_cast<CLIPFORMAT>(RegisterClipboardFormatW(name.c_str())),nullptr,DVASPECT_CONTENT,-1,TYMED_HGLOBAL};require(formats[index].cfFormat>=0xc000,"self-check registered private format");}
    LONG refused_release=0;STGMEDIUM refused=check_medium(4,7,&refused_release);auto* plain=new Source;
    require(plain->SetData(&formats[0],&refused,TRUE)==E_NOTIMPL && refused_release==0,"basic source legitimately refuses private storage without taking ownership");
    STGMEDIUM absent{};require(plain->GetData(&formats[0],&absent)==DV_E_FORMATETC && absent.tymed==TYMED_NULL,"basic source refuses missing private format");
    plain->Release();ReleaseStgMedium(&refused);require(refused_release==1,"caller releases refused private medium");
    scenario=L"shell-private-normal";auto* source=new Source;
    STGMEDIUM borrowed=check_medium(4,11);require_hr(source->SetData(&formats[0],&borrowed,FALSE),"self-check borrowed private store");
    auto* changed=static_cast<BYTE*>(GlobalLock(borrowed.hGlobal));require(changed!=nullptr,"self-check borrowed change");changed[0]=99;GlobalUnlock(borrowed.hGlobal);ReleaseStgMedium(&borrowed);check_private_value(source,&formats[0],11);
    LONG transferred_release=0;STGMEDIUM transferred=check_medium(4,23,&transferred_release);require_hr(source->SetData(&formats[1],&transferred,TRUE),"self-check transferred private store");check_private_value(source,&formats[1],23);
    STGMEDIUM replacement=check_medium(4,37);require_hr(source->SetData(&formats[1],&replacement,FALSE),"self-check private replacement");ReleaseStgMedium(&replacement);check_private_value(source,&formats[1],37);
    STGMEDIUM small=check_medium(4,43);
    for(unsigned index=2;index<8;++index)require_hr(source->SetData(&formats[index],&small,FALSE),"self-check bounded distinct private format");
    require(source->SetData(&formats[8],&small,FALSE)==STG_E_MEDIUMFULL,"ninth private format refused");
    FORMATETC invalid=formats[0];invalid.lindex=0;require(source->SetData(&invalid,&small,FALSE)==DV_E_LINDEX,"private lindex validated");
    invalid=formats[0];invalid.dwAspect=DVASPECT_ICON;require(source->SetData(&invalid,&small,FALSE)==DV_E_DVASPECT,"private aspect validated");
    invalid=formats[0];invalid.tymed=TYMED_ISTREAM;require(source->SetData(&invalid,&small,FALSE)==DV_E_TYMED,"private medium descriptor validated");ReleaseStgMedium(&small);
    LONG oversize_release=0;STGMEDIUM oversize=check_medium(64*1024+1,59,&oversize_release);
    require(source->SetData(&formats[0],&oversize,TRUE)==STG_E_MEDIUMFULL && oversize_release==0,"oversized private medium refused without ownership transfer");ReleaseStgMedium(&oversize);require(oversize_release==1,"caller owns rejected oversized medium");
    source->Release();require(transferred_release==1 && state->private_live_slots==0 && state->private_live_bytes==0,"accepted private ownership released exactly once");
    auto* total=new Source;STGMEDIUM large=check_medium(64*1024,71);
    for(unsigned index=0;index<4;++index)require_hr(total->SetData(&formats[index],&large,FALSE),"self-check aggregate admission");
    require(total->SetData(&formats[4],&large,FALSE)==STG_E_MEDIUMFULL,"private aggregate bytes refused");ReleaseStgMedium(&large);total->Release();
    require(state->private_peak_slots==8 && state->private_peak_bytes==256*1024 && state->private_live_slots==0 && state->private_live_bytes==0,"private retained bytes and slots bounded and released");
    OleUninitialize();CloseHandle(budget);std::printf("{\"private_formats_self_check\":true,\"passed\":true,\"basic_refusal_owns_nothing\":true,\"borrowed_copy_independent\":true,\"transferred_release_exact\":true,\"descriptor_validated\":true,\"slots_limit\":8,\"per_format_bytes\":65536,\"aggregate_bytes\":262144}\n");return 0;
}

int wmain(int argc, wchar_t** argv) {
    if (argc == 2 && !std::wcscmp(argv[1], L"--private-formats-self-check")) return private_formats_self_check();
    if (argc == 3 && !std::wcscmp(argv[1], L"--release-owned-ci-input")) return release_owned_ci_input(argv[2]);
    require(argc > 1 && !std::wcscmp(argv[1], L"--source"), "private production source route");
    attach(argc, argv); require(!baseline && (!ole_route || ci_input), "production OLE source requires owned hosted input");
    if (ci_input && ole_route) return source(argc, argv);
    state->source_pid = GetCurrentProcessId(); state->source_tid = GetCurrentThreadId(); require_hr(OleInitialize(nullptr), "source initialize");
    DWORD cookie = custom_source ? register_owned_unmarshaler() : 0;
    IDataObject* offered = custom_source ? static_cast<IDataObject*>(new CustomSource) : new Source;
    initialize_owned_drag_image(offered);
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
