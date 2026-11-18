// Included after Source: a controlled, registry-free custom-marshaling fixture.
static const CLSID owned_unmarshaler_id = {0xf6ce2d35, 0x6d76, 0x4be6, {0xa9, 0x41, 0x37, 0x71, 0xcf, 0x45, 0x82, 0xf1}};
constexpr DWORD custom_magic = 0x53434450;
static HRESULT write_custom_header(IStream* stream) {
    ULONG actual = 0;
    HRESULT result = stream->Write(&custom_magic, sizeof(custom_magic), &actual);
    return SUCCEEDED(result) && actual != sizeof(custom_magic) ? STG_E_WRITEFAULT : result;
}
static HRESULT read_custom_header(IStream* stream) {
    DWORD magic = 0; ULONG actual = 0;
    HRESULT result = stream->Read(&magic, sizeof(magic), &actual);
    return FAILED(result) ? result : actual == sizeof(magic) && magic == custom_magic ? S_OK : STG_E_READFAULT;
}
struct OwnedUnmarshaler final : IMarshal {
    LONG refs = 1;
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override {
        if (!out) return E_POINTER; *out = nullptr;
        if (iid != IID_IUnknown && iid != IID_IMarshal) return E_NOINTERFACE;
        *out = static_cast<IMarshal*>(this); AddRef(); return S_OK;
    }
    ULONG STDMETHODCALLTYPE AddRef() override { return InterlockedIncrement(&refs); }
    ULONG STDMETHODCALLTYPE Release() override {
        LONG n = InterlockedDecrement(&refs);
        if (!n) {
            if (GetCurrentProcessId() != state->source_pid && (GetCurrentProcessId() != state->receiver_pid || InterlockedCompareExchange(&state->pump_guard_active, 0, 0) != 1)) InterlockedIncrement(&state->unmarshal_release_outside_guard);
            delete this;
        }
        return n;
    }
    HRESULT STDMETHODCALLTYPE GetUnmarshalClass(REFIID, void*, DWORD, void*, DWORD, CLSID*) override { return E_NOTIMPL; }
    HRESULT STDMETHODCALLTYPE GetMarshalSizeMax(REFIID, void*, DWORD, void*, DWORD, DWORD*) override { return E_NOTIMPL; }
    HRESULT STDMETHODCALLTYPE MarshalInterface(IStream*, REFIID, void*, DWORD, void*, DWORD) override { return E_NOTIMPL; }
    HRESULT STDMETHODCALLTYPE UnmarshalInterface(IStream* stream, REFIID iid, void** out) override {
        if (!out) return E_POINTER; *out = nullptr;
        HRESULT header = read_custom_header(stream); if (FAILED(header)) return header;
        DWORD pid = GetCurrentProcessId();
        if (pid != state->source_pid) {
            LONG previous = InterlockedCompareExchange(&state->first_unmarshal_pid, static_cast<LONG>(pid), 0);
            if (previous == 0) {
                state->first_unmarshal_tid = GetCurrentThreadId();
                state->first_unmarshal_native_stage = InterlockedCompareExchange(&state->native_stage, 0, 0);
                state->first_unmarshal_guard = InterlockedCompareExchange(&state->pump_guard_active, 0, 0);
                state->first_unmarshal_sequence = InterlockedCompareExchange(&state->pump_sequence, 0, 0);
                state->first_unmarshal_armed_sequence = InterlockedCompareExchange(&state->controller_armed_sequence, 0, 0);
            }
            InterlockedIncrement(&state->external_unmarshal_calls);
            // A foreground observation is already a failed containment proof.
            // Return an error there rather than deliberately hanging the observer.
            if (pid != state->receiver_pid) { SetEvent(ready_event); return E_ACCESSDENIED; }
            if (!std::wcscmp(scenario,L"unmarshal-stall") && try_injection(I_UNMARSHAL_STALL)) {SetEvent(ready_event);Sleep(INFINITE);}
            if (!std::wcscmp(scenario,L"unmarshal-memory") && try_injection(I_UNMARSHAL_MEMORY)) {
                void* allocation = VirtualAlloc(nullptr, 128 * 1024 * 1024, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
                state->unmarshal_memory_refused = allocation == nullptr;
                if (allocation) VirtualFree(allocation, 0, MEM_RELEASE);
                SetEvent(ready_event);
                injection_completed(I_UNMARSHAL_MEMORY,E_OUTOFMEMORY);
                return E_OUTOFMEMORY;
            }
        }
        return CoUnmarshalInterface(stream, iid, out);
    }
    HRESULT STDMETHODCALLTYPE ReleaseMarshalData(IStream* stream) override {
        if (GetCurrentProcessId() != state->source_pid && (GetCurrentProcessId() != state->receiver_pid || InterlockedCompareExchange(&state->pump_guard_active, 0, 0) != 1)) InterlockedIncrement(&state->unmarshal_release_outside_guard);
        HRESULT header = read_custom_header(stream);
        return FAILED(header) ? header : CoReleaseMarshalData(stream);
    }
    HRESULT STDMETHODCALLTYPE DisconnectObject(DWORD) override { return S_OK; }
};
struct OwnedUnmarshalFactory final : IClassFactory {
    LONG refs = 1;
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override {
        if (!out) return E_POINTER; *out = nullptr;
        if (iid != IID_IUnknown && iid != IID_IClassFactory) return E_NOINTERFACE;
        *out = static_cast<IClassFactory*>(this); AddRef(); return S_OK;
    }
    ULONG STDMETHODCALLTYPE AddRef() override { return InterlockedIncrement(&refs); }
    ULONG STDMETHODCALLTYPE Release() override { LONG n = InterlockedDecrement(&refs); if (!n) delete this; return n; }
    HRESULT STDMETHODCALLTYPE CreateInstance(IUnknown* outer, REFIID iid, void** out) override {
        if (outer) return CLASS_E_NOAGGREGATION;
        DWORD pid = GetCurrentProcessId();
        if (pid != state->source_pid && InterlockedCompareExchange(&state->first_factory_pid, static_cast<LONG>(pid), 0) == 0) {
            state->first_factory_tid = GetCurrentThreadId();
            state->first_factory_native_stage = InterlockedCompareExchange(&state->native_stage, 0, 0);
            state->first_factory_guard = InterlockedCompareExchange(&state->pump_guard_active, 0, 0);
            state->first_factory_sequence = InterlockedCompareExchange(&state->pump_sequence, 0, 0);
            state->first_factory_armed_sequence = InterlockedCompareExchange(&state->controller_armed_sequence, 0, 0);
        }
        auto* instance = new OwnedUnmarshaler;
        HRESULT result = instance->QueryInterface(iid, out); instance->Release(); return result;
    }
    HRESULT STDMETHODCALLTYPE LockServer(BOOL) override { return S_OK; }
};
static DWORD register_owned_unmarshaler() {
    auto* factory = new OwnedUnmarshalFactory; DWORD cookie = 0;
    HRESULT result = CoRegisterClassObject(owned_unmarshaler_id, factory, CLSCTX_INPROC_SERVER, REGCLS_MULTIPLEUSE, &cookie);
    factory->Release(); require_hr(result, "owned in-process custom marshaler factory");
    return cookie;
}
struct CustomSource final : IDataObject, IMarshal {
    LONG refs = 1; IDataObject* inner = new Source;
    ~CustomSource() { inner->Release(); }
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override {
        if (!out) return E_POINTER; *out = nullptr;
        if (iid == IID_IUnknown || iid == IID_IDataObject) *out = static_cast<IDataObject*>(this);
        else if (iid == IID_IMarshal) *out = static_cast<IMarshal*>(this);
        else return E_NOINTERFACE;
        AddRef(); return S_OK;
    }
    ULONG STDMETHODCALLTYPE AddRef() override { return InterlockedIncrement(&refs); }
    ULONG STDMETHODCALLTYPE Release() override { LONG n = InterlockedDecrement(&refs); if (!n) delete this; return n; }
    HRESULT STDMETHODCALLTYPE GetData(FORMATETC* format, STGMEDIUM* medium) override { return inner->GetData(format, medium); }
    HRESULT STDMETHODCALLTYPE GetDataHere(FORMATETC* format, STGMEDIUM* medium) override { return inner->GetDataHere(format, medium); }
    HRESULT STDMETHODCALLTYPE QueryGetData(FORMATETC* format) override { return inner->QueryGetData(format); }
    HRESULT STDMETHODCALLTYPE GetCanonicalFormatEtc(FORMATETC* in, FORMATETC* out) override { return inner->GetCanonicalFormatEtc(in, out); }
    HRESULT STDMETHODCALLTYPE SetData(FORMATETC* format, STGMEDIUM* medium, BOOL release) override { return inner->SetData(format, medium, release); }
    HRESULT STDMETHODCALLTYPE EnumFormatEtc(DWORD direction, IEnumFORMATETC** out) override { return inner->EnumFormatEtc(direction, out); }
    HRESULT STDMETHODCALLTYPE DAdvise(FORMATETC* format, DWORD flags, IAdviseSink* sink, DWORD* connection) override { return inner->DAdvise(format, flags, sink, connection); }
    HRESULT STDMETHODCALLTYPE DUnadvise(DWORD connection) override { return inner->DUnadvise(connection); }
    HRESULT STDMETHODCALLTYPE EnumDAdvise(IEnumSTATDATA** out) override { return inner->EnumDAdvise(out); }
    HRESULT STDMETHODCALLTYPE GetUnmarshalClass(REFIID, void*, DWORD, void*, DWORD, CLSID* out) override { if (!out) return E_POINTER; *out = owned_unmarshaler_id; return S_OK; }
    HRESULT STDMETHODCALLTYPE GetMarshalSizeMax(REFIID iid, void*, DWORD context, void* reserved, DWORD flags, DWORD* out) override {
        if (!out) return E_POINTER;
        DWORD nested = 0; HRESULT result = CoGetMarshalSizeMax(&nested, iid, inner, context, reserved, flags);
        if (FAILED(result)) return result;
        if (nested > MAXDWORD - sizeof(custom_magic)) return E_OUTOFMEMORY;
        *out = nested + sizeof(custom_magic); return S_OK;
    }
    HRESULT STDMETHODCALLTYPE MarshalInterface(IStream* stream, REFIID iid, void*, DWORD context, void* reserved, DWORD flags) override {
        InterlockedIncrement(&state->custom_marshal_calls);
        HRESULT header = write_custom_header(stream);
        return FAILED(header) ? header : CoMarshalInterface(stream, iid, inner, context, reserved, flags);
    }
    HRESULT STDMETHODCALLTYPE UnmarshalInterface(IStream*, REFIID, void**) override { return E_NOTIMPL; }
    HRESULT STDMETHODCALLTYPE ReleaseMarshalData(IStream* stream) override {
        HRESULT header = read_custom_header(stream);
        return FAILED(header) ? header : CoReleaseMarshalData(stream);
    }
    HRESULT STDMETHODCALLTYPE DisconnectObject(DWORD reserved) override { return CoDisconnectObject(inner, reserved); }
};
