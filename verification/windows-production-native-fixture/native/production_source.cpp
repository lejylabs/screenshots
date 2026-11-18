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
struct StreamStoreReentry {
    Source* source{};FORMATETC format{};bool on_stat=false,on_release=false,active=false,fired=false,guard_seen=false;LONG attempts=0,nested_methods=0;HRESULT nested_result=E_PENDING;
    bool nested_method() {if(!active)return false;++nested_methods;return true;}
    void invoke(IStream* stream,LONG& refs,LONG* released) {
        fired=true;++attempts;guard_seen=source->private_busy;
        LONG caller_refs=refs,caller_releases=released?*released:0,sets=state->private_set_calls,gets=state->private_get_calls;
        LONG live_slots=state->private_live_slots,live_bytes=state->private_live_bytes,peak_slots=state->private_peak_slots,peak_bytes=state->private_peak_bytes;
        LONG refused=state->private_refusals,queries=state->query_calls,data=state->data_calls,media=state->medium_releases;PayloadCounters payload=state->payload;
        IDataObject* cache=source->private_data;SIZE_T bytes=source->private_bytes;Source::PrivateSlot slots[8];memcpy(slots,source->private_slots,sizeof(slots));
        STGMEDIUM nested{};nested.tymed=TYMED_ISTREAM;nested.pstm=stream;
        active=true;nested_result=source->SetData(&format,&nested,TRUE);active=false;
        require(nested_result==E_UNEXPECTED && nested_methods==0 && guard_seen,"reentrant SetData rejects before any nested stream method");
        require(refs==caller_refs && (!released || *released==caller_releases),"reentrant SetData never takes caller stream ownership");
        require(source->private_data==cache && source->private_bytes==bytes,"reentrant SetData leaves native storage identity and bytes unchanged");
        for(unsigned index=0;index<8;++index)require(source->private_slots[index].format==slots[index].format && source->private_slots[index].bytes==slots[index].bytes && source->private_slots[index].tymed==slots[index].tymed,"reentrant SetData leaves all retained slot metadata unchanged");
        require(state->private_live_slots==live_slots && state->private_live_bytes==live_bytes && state->private_peak_slots==peak_slots && state->private_peak_bytes==peak_bytes && state->private_set_calls==sets+1 && state->private_get_calls==gets && state->private_refusals==refused && state->query_calls==queries && state->data_calls==data && state->medium_releases==media && !memcmp(&state->payload,&payload,sizeof(payload)),"nested rejection preserves bounds and acquisition counters");
    }
};

struct CheckedStream final : IStream {
    LONG refs=1;IStream* inner;unsigned fault;LONG* released;StreamStoreReentry* reentry;
    CheckedStream(IStream* stream,unsigned mode,LONG* count=nullptr,StreamStoreReentry* control=nullptr):inner(stream),fault(mode),released(count),reentry(control){}
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid,void** out) override {if(reentry && reentry->nested_method())return E_UNEXPECTED;if(!out)return E_POINTER;*out=nullptr;if(iid!=IID_IUnknown && iid!=IID_IStream && iid!=IID_ISequentialStream)return E_NOINTERFACE;*out=this;AddRef();return S_OK;}
    ULONG STDMETHODCALLTYPE AddRef() override {if(reentry && reentry->nested_method())return refs;return InterlockedIncrement(&refs);}
    ULONG STDMETHODCALLTYPE Release() override {if(reentry && reentry->nested_method())return refs;LONG count=InterlockedDecrement(&refs);if(!count){if(reentry && reentry->on_release && !reentry->fired)reentry->invoke(this,refs,released);inner->Release();if(released)InterlockedIncrement(released);delete this;}return count;}
    HRESULT STDMETHODCALLTYPE Read(void* data,ULONG bytes,ULONG* read) override {if(reentry && reentry->nested_method())return E_UNEXPECTED;HRESULT result=inner->Read(data,fault==1 && bytes?bytes-1:bytes,read);if(fault==4 && stop_event)SetEvent(stop_event);return result;}
    HRESULT STDMETHODCALLTYPE Write(const void* data,ULONG bytes,ULONG* written) override {if(reentry && reentry->nested_method())return E_UNEXPECTED;return inner->Write(data,bytes,written);}
    HRESULT STDMETHODCALLTYPE Seek(LARGE_INTEGER offset,DWORD from,ULARGE_INTEGER* position) override {if(reentry && reentry->nested_method())return E_UNEXPECTED;return fault==2?STG_E_SEEKERROR:inner->Seek(offset,from,position);}
    HRESULT STDMETHODCALLTYPE SetSize(ULARGE_INTEGER size) override {if(reentry && reentry->nested_method())return E_UNEXPECTED;return inner->SetSize(size);}
    HRESULT STDMETHODCALLTYPE CopyTo(IStream* out,ULARGE_INTEGER count,ULARGE_INTEGER* read,ULARGE_INTEGER* written) override {if(reentry && reentry->nested_method())return E_UNEXPECTED;return inner->CopyTo(out,count,read,written);}
    HRESULT STDMETHODCALLTYPE Commit(DWORD flags) override {if(reentry && reentry->nested_method())return E_UNEXPECTED;return inner->Commit(flags);}
    HRESULT STDMETHODCALLTYPE Revert() override {if(reentry && reentry->nested_method())return E_UNEXPECTED;return inner->Revert();}
    HRESULT STDMETHODCALLTYPE LockRegion(ULARGE_INTEGER offset,ULARGE_INTEGER count,DWORD flags) override {if(reentry && reentry->nested_method())return E_UNEXPECTED;return inner->LockRegion(offset,count,flags);}
    HRESULT STDMETHODCALLTYPE UnlockRegion(ULARGE_INTEGER offset,ULARGE_INTEGER count,DWORD flags) override {if(reentry && reentry->nested_method())return E_UNEXPECTED;return inner->UnlockRegion(offset,count,flags);}
    HRESULT STDMETHODCALLTYPE Stat(STATSTG* info,DWORD flags) override {if(reentry && reentry->nested_method())return E_UNEXPECTED;if(reentry && reentry->on_stat && !reentry->fired)reentry->invoke(this,refs,released);if(fault==6)return E_UNEXPECTED;HRESULT result=inner->Stat(info,flags);if(SUCCEEDED(result) && fault==3)info->cbSize.QuadPart=64*1024+1;if(SUCCEEDED(result) && fault==5 && info->cbSize.QuadPart)--info->cbSize.QuadPart;return result;}
    HRESULT STDMETHODCALLTYPE Clone(IStream** out) override {if(reentry && reentry->nested_method())return E_UNEXPECTED;if(!out)return E_POINTER;*out=nullptr;IStream* copy{};HRESULT result=inner->Clone(&copy);if(SUCCEEDED(result))*out=new CheckedStream(copy,fault,nullptr,reentry);return result;}
};
static STGMEDIUM check_stream(unsigned fault,LONG* released=nullptr,StreamStoreReentry* reentry=nullptr) {
    IStream* stream{};require_hr(CreateStreamOnHGlobal(nullptr,TRUE,&stream),"self-check owned stream");
    const BYTE bytes[]{11,11,11,11};ULONG written{};require_hr(stream->Write(bytes,sizeof(bytes),&written),"self-check stream bytes");require(written==sizeof(bytes),"self-check stream complete write");
    LARGE_INTEGER position{};position.QuadPart=1;require_hr(stream->Seek(position,STREAM_SEEK_SET,nullptr),"self-check initial stream position");
    STGMEDIUM medium{};medium.tymed=TYMED_ISTREAM;medium.pstm=new CheckedStream(stream,fault,released,reentry);return medium;
}
static void check_stream_value(Source* source,FORMATETC* format) {
    STGMEDIUM media[2]{};LARGE_INTEGER zero{};
    for(unsigned copy=0;copy<2;++copy) {
        auto& medium=media[copy];require_hr(source->GetData(format,&medium),"self-check copied private stream");require(medium.tymed==TYMED_ISTREAM && medium.pstm,"self-check private stream type");
        STATSTG info{};require_hr(medium.pstm->Stat(&info,STATFLAG_NONAME),"self-check returned stream size");require(info.cbSize.QuadPart==4,"private stream snapshot has exact four-byte size");
        ULARGE_INTEGER position{};require_hr(medium.pstm->Seek(zero,STREAM_SEEK_CUR,&position),"self-check returned stream cursor");require(position.QuadPart==4,"GetData cursor marks end of complete snapshot");
        BYTE extra{};ULONG read{};HRESULT result=medium.pstm->Read(&extra,1,&read);require(SUCCEEDED(result) && read==0,"returned stream cursor is at payload end");
        require_hr(medium.pstm->Seek(zero,STREAM_SEEK_SET,nullptr),"self-check returned stream rewind");BYTE bytes[5]{};result=medium.pstm->Read(bytes,sizeof(bytes),&read);require(SUCCEEDED(result) && read==4,"private stream exact length");for(unsigned index=0;index<read;++index)require(bytes[index]==11,"private stream exact original bytes");
        if(copy==0) {
            require_hr(medium.pstm->Seek(zero,STREAM_SEEK_SET,nullptr),"self-check returned stream mutation position");const BYTE changed=99;ULONG written{};require_hr(medium.pstm->Write(&changed,1,&written),"self-check returned stream mutation");require(written==1,"self-check returned stream mutation complete");
        }
    }
    ULARGE_INTEGER position{};require_hr(media[0].pstm->Seek(zero,STREAM_SEEK_CUR,&position),"self-check first returned stream cursor remains independent");require(position.QuadPart==1,"second GetData never changes first returned stream seek position");
    require_hr(media[0].pstm->Seek(zero,STREAM_SEEK_SET,nullptr),"self-check first returned stream mutation preserved");BYTE changed{};ULONG read{};require_hr(media[0].pstm->Read(&changed,1,&read),"self-check first returned stream remains caller-owned");require(read==1 && changed==99,"second GetData leaves first returned stream bytes unchanged");
    for(auto& medium:media)ReleaseStgMedium(&medium);
}
static void private_stream_self_check(FORMATETC format) {
    format.tymed=TYMED_ISTREAM;LONG borrowed_release=0;STGMEDIUM borrowed=check_stream(0,&borrowed_release);auto* source=new Source;
    require_hr(source->SetData(&format,&borrowed,FALSE),"self-check borrowed stream snapshot");require(borrowed_release==0,"borrowed stream stays caller-owned");
    FORMATETC changed_format=format;changed_format.tymed=TYMED_HGLOBAL;STGMEDIUM changed_medium=check_medium(4,99);require(source->SetData(&changed_format,&changed_medium,FALSE)==DV_E_TYMED,"one private format cannot accumulate alternate retained media");ReleaseStgMedium(&changed_medium);
    LARGE_INTEGER zero{};ULARGE_INTEGER position{};require_hr(borrowed.pstm->Seek(zero,STREAM_SEEK_CUR,&position),"self-check original stream cursor");require(position.QuadPart==1,"snapshot never changes caller seek position");
    require_hr(borrowed.pstm->Seek(zero,STREAM_SEEK_SET,nullptr),"self-check caller stream rewrite");const BYTE changed[]{99,99,99,99};ULONG written{};require_hr(borrowed.pstm->Write(changed,sizeof(changed),&written),"self-check caller stream mutation");ReleaseStgMedium(&borrowed);require(borrowed_release==1,"borrowed stream owner releases exactly once");check_stream_value(source,&format);check_stream_value(source,&format);source->Release();
    LONG transferred_release=0;STGMEDIUM transferred=check_stream(0,&transferred_release);source=new Source;require_hr(source->SetData(&format,&transferred,TRUE),"self-check transferred stream snapshot");require(transferred_release==1,"accepted transferred stream released exactly once");check_stream_value(source,&format);source->Release();
    stop_event=CreateEventW(nullptr,TRUE,FALSE,nullptr);require(stop_event!=nullptr,"self-check stream cancellation event");
    const HRESULT expected[]{S_OK,STG_E_READFAULT,STG_E_SEEKERROR,STG_E_MEDIUMFULL,E_ABORT,STG_E_MEDIUMFULL};
    for(unsigned fault=1;fault<=5;++fault) {
        LONG released=0;STGMEDIUM medium=check_stream(fault,&released);source=new Source;
        require(source->SetData(&format,&medium,TRUE)==expected[fault] && released==0,"failed short-read/seek/size/canceled/grown stream retains caller ownership");
        STGMEDIUM missing{};require(!source->private_busy && source->private_bytes==0 && state->private_live_slots==0 && state->private_live_bytes==0 && source->GetData(&format,&missing)==DV_E_FORMATETC,"failed stream store clears guard and admits no retained entry");
        ResetEvent(stop_event);STGMEDIUM healthy=check_stream(0);require_hr(source->SetData(&format,&healthy,FALSE),"healthy store succeeds after each rejected stream");ReleaseStgMedium(&healthy);check_stream_value(source,&format);
        source->Release();ReleaseStgMedium(&medium);require(released==1,"rejected stream caller releases once");
    }
    SetEvent(stop_event);LONG canceled_release=0;STGMEDIUM canceled=check_stream(6,&canceled_release);source=new Source;require(source->SetData(&format,&canceled,TRUE)==E_ABORT && canceled_release==0,"pre-canceled stream is rejected before its failing Stat");source->Release();ReleaseStgMedium(&canceled);require(canceled_release==1,"pre-canceled stream remains caller-owned");
    CloseHandle(stop_event);stop_event=nullptr;
}

struct CacheReentryRelease final : IUnknown {
    LONG refs=1;HGLOBAL global;IStream* stream;StreamStoreReentry* control;LONG* released;
    CacheReentryRelease(HGLOBAL data,IStream* input,StreamStoreReentry* callback,LONG* count):global(data),stream(input),control(callback),released(count){}
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid,void** out) override {if(!out)return E_POINTER;*out=nullptr;if(iid!=IID_IUnknown)return E_NOINTERFACE;*out=this;AddRef();return S_OK;}
    ULONG STDMETHODCALLTYPE AddRef() override {return InterlockedIncrement(&refs);}
    ULONG STDMETHODCALLTYPE Release() override {LONG count=InterlockedDecrement(&refs);if(!count){auto* input=static_cast<CheckedStream*>(stream);control->invoke(stream,input->refs,input->released);GlobalFree(global);stream->Release();InterlockedIncrement(released);delete this;}return count;}
};
static void private_stream_reentry_self_check(FORMATETC seed_format,FORMATETC stream_format) {
    stream_format.tymed=TYMED_ISTREAM;
    for(unsigned mode=0;mode<2;++mode) {
        auto* source=new Source;STGMEDIUM seed=check_medium(4,11);require_hr(source->SetData(&seed_format,&seed,FALSE),"self-check reentry retained seed");ReleaseStgMedium(&seed);
        StreamStoreReentry control;control.source=source;control.format=stream_format;control.on_stat=mode==0;control.on_release=mode==1;
        LONG released=0;STGMEDIUM medium=check_stream(0,&released,&control);
        require_hr(source->SetData(&stream_format,&medium,FALSE),"guarded Stat/Release reentry allows only the outer stream store");
        require(control.attempts==1 && control.nested_methods==0 && control.nested_result==E_UNEXPECTED && control.guard_seen && released==0 && !source->private_busy,"Stat/Release nested store rejects immediately and outer guard resets");
        LARGE_INTEGER zero{};ULARGE_INTEGER position{};require_hr(medium.pstm->Seek(zero,STREAM_SEEK_CUR,&position),"reentry caller stream cursor");require(position.QuadPart==1,"reentrant store leaves caller cursor unchanged");
        require(state->private_live_slots==2 && source->private_bytes==static_cast<SIZE_T>(state->private_live_bytes) && source->private_bytes<=256*1024 && state->private_peak_slots<=8 && state->private_peak_bytes<=256*1024,"only outer store is admitted within unchanged cache bounds");
        check_private_value(source,&seed_format,11);check_stream_value(source,&stream_format);
        ReleaseStgMedium(&medium);require(released==1,"reentry caller releases its original stream once");source->Release();require(state->private_live_slots==0 && state->private_live_bytes==0,"reentry storage releases fully");
    }
    auto* source=new Source;StreamStoreReentry control;control.source=source;control.format=stream_format;
    LONG stream_released=0,owner_released=0;STGMEDIUM nested=check_stream(0,&stream_released,&control);STGMEDIUM retained=check_medium(4,11);retained.pUnkForRelease=new CacheReentryRelease(retained.hGlobal,nested.pstm,&control,&owner_released);
    require_hr(source->SetData(&seed_format,&retained,TRUE),"self-check native release-owner storage");
    source->Release();require(control.attempts==1 && control.nested_methods==0 && control.guard_seen && control.nested_result==E_UNEXPECTED && owner_released==1 && stream_released==1 && state->private_live_slots==0 && state->private_live_bytes==0,"native storage owner release rejects destructor/store reentry without leaks");
}

static int private_formats_self_check() {
    Shared local{};state=&local;scenario=L"normal";require_hr(OleInitialize(nullptr),"self-check source STA");HANDLE budget=contain_native();
    FORMATETC formats[9]{};
    for(unsigned index=0;index<9;++index){std::wstring name=L"Screenshots.Owned.Private."+std::to_wstring(index);formats[index]={static_cast<CLIPFORMAT>(RegisterClipboardFormatW(name.c_str())),nullptr,DVASPECT_CONTENT,-1,TYMED_HGLOBAL};require(formats[index].cfFormat>=0xc000,"self-check registered private format");}
    LONG refused_release=0;STGMEDIUM refused=check_medium(4,7,&refused_release);auto* plain=new Source;
    require(plain->SetData(&formats[0],&refused,TRUE)==E_NOTIMPL && refused_release==0,"basic source legitimately refuses private storage without taking ownership");
    STGMEDIUM absent{};require(plain->GetData(&formats[0],&absent)==DV_E_FORMATETC && absent.tymed==TYMED_NULL,"basic source refuses missing private format");
    FORMATETC unsupported_medium=formats[0];unsupported_medium.tymed=TYMED_ISTREAM;
    require(plain->QueryGetData(&unsupported_medium)==DV_E_FORMATETC,"basic unknown private format query rejects format");
    require(plain->GetData(&unsupported_medium,&absent)==DV_E_FORMATETC,"unsupported private format precedes unsupported medium on basic source");
    plain->Release();ReleaseStgMedium(&refused);require(refused_release==1,"caller releases refused private medium");
    scenario=L"shell-private-normal";auto* source=new Source;
    require(source->cooperative(),"self-check cooperative scenario active before native store");
    require(source->GetData(&unsupported_medium,&absent)==DV_E_FORMATETC,"unknown cooperative private format precedes unsupported medium");
    LONG borrowed_release=0;STGMEDIUM borrowed=check_medium(4,11,&borrowed_release);require_hr(source->SetData(&formats[0],&borrowed,FALSE),"self-check borrowed private store");
    LONG trace_index=state->private_trace_count-1;require(trace_index>=0 && trace_index<32,"self-check borrowed native trace present");
    const auto& borrowed_trace=state->private_trace[trace_index];require(borrowed_release==0 && borrowed_trace.cooperative==1 && borrowed_trace.native_called==1 && borrowed_trace.release==0 && borrowed_trace.native_release==1 && borrowed_trace.native_result==S_OK,"borrowed source ownership preserved while native store accepts owned copy");
    auto* changed=static_cast<BYTE*>(GlobalLock(borrowed.hGlobal));require(changed!=nullptr,"self-check borrowed change");changed[0]=99;GlobalUnlock(borrowed.hGlobal);ReleaseStgMedium(&borrowed);require(borrowed_release==1,"caller releases borrowed private owner exactly once");check_private_value(source,&formats[0],11);
    require(source->QueryGetData(&unsupported_medium)==DV_E_TYMED,"stored private format query still rejects unsupported medium");
    require(source->GetData(&unsupported_medium,&absent)==DV_E_TYMED && absent.tymed==TYMED_NULL,"stored private format still rejects unsupported medium");
    LONG transferred_release=0;STGMEDIUM transferred=check_medium(4,23,&transferred_release);require_hr(source->SetData(&formats[1],&transferred,TRUE),"self-check transferred private store");check_private_value(source,&formats[1],23);
    STGMEDIUM replacement=check_medium(4,37);require_hr(source->SetData(&formats[1],&replacement,FALSE),"self-check private replacement");ReleaseStgMedium(&replacement);check_private_value(source,&formats[1],37);
    STGMEDIUM private_medium=check_medium(4,43);
    for(unsigned index=2;index<8;++index)require_hr(source->SetData(&formats[index],&private_medium,FALSE),"self-check bounded distinct private format");
    require(source->SetData(&formats[8],&private_medium,FALSE)==STG_E_MEDIUMFULL,"ninth private format refused");
    FORMATETC invalid=formats[0];invalid.lindex=0;require(source->SetData(&invalid,&private_medium,FALSE)==DV_E_LINDEX,"private lindex validated");
    invalid=formats[0];invalid.dwAspect=DVASPECT_ICON;require(source->SetData(&invalid,&private_medium,FALSE)==DV_E_DVASPECT,"private aspect validated");
    invalid=formats[0];invalid.tymed=TYMED_ISTREAM;require(source->SetData(&invalid,&private_medium,FALSE)==DV_E_TYMED,"private medium descriptor validated");ReleaseStgMedium(&private_medium);
    LONG oversize_release=0;STGMEDIUM oversize=check_medium(64*1024+1,59,&oversize_release);
    require(source->SetData(&formats[0],&oversize,TRUE)==STG_E_MEDIUMFULL && oversize_release==0,"oversized private medium refused without ownership transfer");ReleaseStgMedium(&oversize);require(oversize_release==1,"caller owns rejected oversized medium");
    source->Release();require(transferred_release==1 && state->private_live_slots==0 && state->private_live_bytes==0,"accepted private ownership released exactly once");
    auto* total=new Source;STGMEDIUM large=check_medium(64*1024,71);
    for(unsigned index=0;index<4;++index)require_hr(total->SetData(&formats[index],&large,FALSE),"self-check aggregate admission");
    require(total->SetData(&formats[4],&large,FALSE)==STG_E_MEDIUMFULL,"private aggregate bytes refused");ReleaseStgMedium(&large);total->Release();
    require(state->private_peak_slots==8 && state->private_peak_bytes==256*1024 && state->private_live_slots==0 && state->private_live_bytes==0,"private retained bytes and slots bounded and released");
    private_stream_self_check(formats[0]);
    private_stream_reentry_self_check(formats[0],formats[1]);
    require(state->private_live_slots==0 && state->private_live_bytes==0,"private stream storage released");
    OleUninitialize();CloseHandle(budget);std::printf("{\"private_formats_self_check\":true,\"passed\":true,\"basic_refusal_owns_nothing\":true,\"borrowed_copy_independent\":true,\"transferred_release_exact\":true,\"descriptor_validated\":true,\"stream_ownership_seek_short_read_cancel\":true,\"stream_getdata_end_cursor_and_independent_snapshots\":true,\"stream_stat_release_reentry_guard\":true,\"slots_limit\":8,\"per_format_bytes\":65536,\"aggregate_bytes\":262144}\n");return 0;
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
