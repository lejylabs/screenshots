// Private COM topology probe. This is held verification source, not production.
// Exact optimized compiler/link arguments are in run_probe.ps1.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <ole2.h>
#include <psapi.h>
#include <rpc.h>
#include <rpcasync.h>
#include <shlobj.h>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cwchar>
#include <string>
#include <memory>
#include <vector>

constexpr SIZE_T growth = 96 * 1024 * 1024;
constexpr DWORD deadline_ms = 5000;
struct QueryTrace { DWORD format, aspect, tymed; LONG index, after_enter; DWORD pid, tid; };
struct Shared {
    DWORD receiver_pid, receiver_tid, source_pid, source_tid;
    DWORD enter_pid, enter_tid, query_pid, query_tid, data_pid, data_tid, query_client_pid;
    DWORD owner_pid, owner_tid; UINT_PTR owner_hwnd;
    LONG query_calls, data_calls, medium_releases, native_bound, small_probe, large_probe;
    LONG registration, enter_result, query_result, get_result, source_data_result, cleanup_result, copied, completed;
    DWORD failure_pid, failure_win32_error; LONG failure_hresult; char failure_stage[96];
    ULONGLONG private_baseline, offered_bytes;
    LONG preflight_query_calls, preflight_get_calls, preflight_medium_releases, native_stage, enum_calls, enum_result, source_continue_calls, source_feedback_calls, private_move_calls, ci_cursor_moves, query_trace_count;
    LONG drag_call_started, mover_started, mover_stopped, cursor_window_is_owner;
    LONG drag_call_done, source_window_ready, injected_button_downs, injected_button_ups, controller_button_ups, input_stage;
    DWORD mover_pid, mover_tid, cursor_window_pid; char cursor_window_class[96];
    LONG source_qi_calls, drop_source_qi_calls, source_gui_ok, source_capture_nonzero;
    LONG enum_next_calls, enum_next_result, enum_next_fetched;
    GUID last_source_iid, last_drop_source_iid;
    DWORD source_active_pid, source_capture_pid; char source_capture_class[96];
    QueryTrace query_trace[16];
    LONG cursor_ok; DWORD cursor_error; POINT cursor; DWORD message_position; UINT_PTR point_window;
    DWORD marshal_bytes;
    BYTE marshal[16 * 1024];
};
static Shared* state;
static HANDLE ready_event, done_event, stop_event;
static const wchar_t* scenario;
static bool ole_route = false, baseline = false, ci_input = false;
static IDataObject* source_data;
static IDropSource* source_drop;
static DWORD source_effect;
static SRWLOCK qi_trace_lock = SRWLOCK_INIT;
static void require_owned_ci() {
    wchar_t actions[8]{}, operating_system[32]{}, runner_environment[32]{}, explicit_gate[8]{};
    GetEnvironmentVariableW(L"GITHUB_ACTIONS", actions, 8);
    GetEnvironmentVariableW(L"RUNNER_OS", operating_system, 32);
    GetEnvironmentVariableW(L"RUNNER_ENVIRONMENT", runner_environment, 32);
    GetEnvironmentVariableW(L"SCREENSHOTS_OWNED_HOSTED_CI_INPUT", explicit_gate, 8);
    if (std::wcscmp(actions, L"true") || std::wcscmp(operating_system, L"Windows") || std::wcscmp(runner_environment, L"github-hosted") || std::wcscmp(explicit_gate, L"1")) {
        std::fprintf(stderr, "Owned hosted Windows CI input mode is not authorized by its environment gates\n"); ExitProcess(91);
    }
}
static const wchar_t* parse_scenario(const wchar_t* value) {
    if (!std::wcsncmp(value, L"ci-", 3)) { ci_input = true; require_owned_ci(); value += 3; }
    if (!std::wcsncmp(value, L"baseline-", 9)) { baseline = true; value += 9; }
    if (!std::wcsncmp(value, L"ole-", 4)) { ole_route = true; value += 4; }
    return value;
}
static void require(bool value, const char* what) {
    if (!value) {
        DWORD error = GetLastError();
        if (state) {
            state->failure_pid = GetCurrentProcessId(); state->failure_win32_error = error;
            strncpy_s(state->failure_stage, what, _TRUNCATE);
        }
        std::fprintf(stderr, "%s: %lu\n", what, error); ExitProcess(90);
    }
}
static void require_hr(HRESULT result, const char* what) {
    if (FAILED(result)) { if (state) state->failure_hresult = result; require(false, what); }
}
static bool owned_left_button(DWORD flags) {
    require_owned_ci();
    INPUT input{}; input.type = INPUT_MOUSE; input.mi.dwFlags = flags;
    return SendInput(1, &input, sizeof(input)) == 1;
}
static int release_owned_ci_input(const wchar_t* controller_text) {
    require_owned_ci(); wchar_t* end = nullptr;
    for (const wchar_t* unit = controller_text; *unit; ++unit) require(*unit >= L'0' && *unit <= L'9', "owned CI release controller digits");
    unsigned long controller = std::wcstoul(controller_text, &end, 10);
    require(controller != 0 && end != controller_text && *end == 0, "owned CI release controller ID");
    std::wstring expected = L"ScreenshotsDropProbe-" + std::to_wstring(controller);
    HDESK input = OpenInputDesktop(0, FALSE, DESKTOP_READOBJECTS | DESKTOP_WRITEOBJECTS | DESKTOP_SWITCHDESKTOP);
    require(input != nullptr, "owned CI external release input desktop");
    wchar_t name[128]{}; DWORD actual = 0;
    require(GetUserObjectInformationW(input, UOI_NAME, name, sizeof(name), &actual), "owned CI external release desktop identity");
    if (expected != name) { CloseDesktop(input); std::printf("{\"released\":false,\"owned_desktop_not_current\":true}\n"); return 0; }
    HDESK original = GetThreadDesktop(GetCurrentThreadId());
    require(SetThreadDesktop(input), "attach external release to exact owned CI desktop");
    require(owned_left_button(MOUSEEVENTF_LEFTUP), "external owned CI left-button release");
    std::printf("{\"released\":true,\"owned_desktop_not_current\":false}\n");
    require(SetThreadDesktop(original), "restore external release thread desktop"); CloseDesktop(input); return 0;
}
static HANDLE from_arg(const wchar_t* text) {
    return reinterpret_cast<HANDLE>(_wcstoui64(text, nullptr, 10));
}
static void attach(int argc, wchar_t** argv) {
    require(argc == 8, "child arguments");
    state = static_cast<Shared*>(MapViewOfFile(from_arg(argv[2]), FILE_MAP_ALL_ACCESS, 0, 0, sizeof(Shared)));
    require(state != nullptr, "child mapping");
    ready_event = from_arg(argv[3]); done_event = from_arg(argv[4]); stop_event = from_arg(argv[5]);
    scenario = parse_scenario(argv[7]);
}
static LRESULT CALLBACK owner_proc(HWND hwnd, UINT message, WPARAM wparam, LPARAM lparam) {
    if (message == WM_APP + 2) { SetEvent(ready_event); return 0; }
    return DefWindowProcW(hwnd, message, wparam, lparam);
}
static HWND owner_window() {
    WNDCLASSW cls{}; cls.lpfnWndProc = owner_proc; cls.hInstance = GetModuleHandleW(nullptr); cls.lpszClassName = L"ScreenshotsPrivateDropProbe";
    require(RegisterClassW(&cls) != 0, "private window class");
    // Hosted CI input mode is separately gated; ordinary cases are noninteractive.
    HWND window = CreateWindowExW(0, cls.lpszClassName, L"owned synthetic receiver", WS_POPUP | WS_VISIBLE, 0, 0, 128, 128, nullptr, nullptr, cls.hInstance, nullptr);
    require(window != nullptr, "private owner window");
    return window;
}
static HANDLE contain_native() {
    PROCESS_MEMORY_COUNTERS_EX counters{}; counters.cb = sizeof(counters);
    require(GetProcessMemoryInfo(GetCurrentProcess(), reinterpret_cast<PROCESS_MEMORY_COUNTERS*>(&counters), sizeof(counters)), "private memory baseline");
    state->private_baseline = counters.PrivateUsage;
    HANDLE job = CreateJobObjectW(nullptr, nullptr); require(job != nullptr, "native job");
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION limits{};
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY;
    limits.ProcessMemoryLimit = counters.PrivateUsage + growth;
    require(SetInformationJobObject(job, JobObjectExtendedLimitInformation, &limits, sizeof(limits)), "native job limit");
    require(AssignProcessToJobObject(job, GetCurrentProcess()), "native job assignment");
    state->native_bound = 1;
    void* small_allocation = VirtualAlloc(nullptr, 4096, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    require(small_allocation != nullptr, "small bounded commit"); VirtualFree(small_allocation, 0, MEM_RELEASE); state->small_probe = 1;
    void* large = VirtualAlloc(nullptr, growth + 65536, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE);
    if (large) VirtualFree(large, 0, MEM_RELEASE);
    require(large == nullptr, "large commit must fail before COM acquisition"); state->large_probe = 1;
    return job;
}

struct MediumRelease final : IUnknown {
    LONG refs = 1; HGLOBAL global; bool target_medium;
    MediumRelease(HGLOBAL value, bool target) : global(value), target_medium(target) {}
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override {
        if (!out) return E_POINTER; *out = nullptr;
        if (iid != IID_IUnknown) return E_NOINTERFACE;
        *out = this; AddRef(); return S_OK;
    }
    ULONG STDMETHODCALLTYPE AddRef() override { return InterlockedIncrement(&refs); }
    ULONG STDMETHODCALLTYPE Release() override {
        LONG count = InterlockedDecrement(&refs);
        if (!count) {
            if (target_medium) InterlockedIncrement(&state->medium_releases);
            else InterlockedIncrement(&state->preflight_medium_releases);
            if (target_medium && !std::wcscmp(scenario, L"release-stall")) Sleep(INFINITE);
            GlobalFree(global); delete this;
        }
        return count;
    }
};
struct FormatsEnum final : IEnumFORMATETC {
    LONG refs = 1; bool consumed = false;
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override {
        if (!out) return E_POINTER; *out = nullptr;
        if (iid != IID_IUnknown && iid != IID_IEnumFORMATETC) return E_NOINTERFACE;
        *out = this; AddRef(); return S_OK;
    }
    ULONG STDMETHODCALLTYPE AddRef() override { return InterlockedIncrement(&refs); }
    ULONG STDMETHODCALLTYPE Release() override { LONG n = InterlockedDecrement(&refs); if (!n) delete this; return n; }
    HRESULT STDMETHODCALLTYPE Next(ULONG requested, FORMATETC* out, ULONG* fetched) override {
        InterlockedIncrement(&state->enum_next_calls);
        if (!out || (!fetched && requested != 1)) { state->enum_next_result = E_POINTER; return E_POINTER; }
        ULONG count = 0;
        if (requested && !consumed) { *out = FORMATETC{CF_HDROP, nullptr, DVASPECT_CONTENT, -1, TYMED_HGLOBAL}; consumed = true; count = 1; }
        if (fetched) *fetched = count;
        state->enum_next_fetched = count; state->enum_next_result = count == requested ? S_OK : S_FALSE;
        return state->enum_next_result;
    }
    HRESULT STDMETHODCALLTYPE Skip(ULONG count) override { bool available = !consumed; if (count) consumed = true; return count == 0 || (count == 1 && available) ? S_OK : S_FALSE; }
    HRESULT STDMETHODCALLTYPE Reset() override { consumed = false; return S_OK; }
    HRESULT STDMETHODCALLTYPE Clone(IEnumFORMATETC** out) override { if (!out) return E_POINTER; auto* copy = new FormatsEnum; copy->consumed = consumed; *out = copy; return S_OK; }
};
struct Source final : IDataObject {
    LONG refs = 1;
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override {
        InterlockedIncrement(&state->source_qi_calls); AcquireSRWLockExclusive(&qi_trace_lock); state->last_source_iid = iid; ReleaseSRWLockExclusive(&qi_trace_lock);
        if (!out) return E_POINTER; *out = nullptr;
        if (iid != IID_IUnknown && iid != IID_IDataObject) return E_NOINTERFACE;
        *out = this; AddRef(); return S_OK;
    }
    ULONG STDMETHODCALLTYPE AddRef() override { return InterlockedIncrement(&refs); }
    ULONG STDMETHODCALLTYPE Release() override { LONG n = InterlockedDecrement(&refs); if (!n) delete this; return n; }
    HRESULT STDMETHODCALLTYPE QueryGetData(FORMATETC* format) override {
        bool target_entered = state->enter_pid != 0 && state->native_stage == 2;
        LONG slot = InterlockedIncrement(&state->query_trace_count) - 1;
        if (slot >= 0 && slot < 16) state->query_trace[slot] = QueryTrace{static_cast<DWORD>(format ? format->cfFormat : 0), format ? format->dwAspect : 0, format ? format->tymed : 0, format ? format->lindex : 0, target_entered ? 1 : 0, GetCurrentProcessId(), GetCurrentThreadId()};
        if (!target_entered) {
            InterlockedIncrement(&state->preflight_query_calls);
            return format && format->cfFormat == CF_HDROP && (format->tymed & TYMED_HGLOBAL) != 0 ? S_OK : DV_E_FORMATETC;
        }
        state->query_pid = GetCurrentProcessId(); state->query_tid = GetCurrentThreadId();
        RPC_CALL_ATTRIBUTES_V2_W attributes{}; attributes.Version = 2; attributes.Flags = RPC_QUERY_CLIENT_PID;
        if (!baseline && RpcServerInqCallAttributesW(nullptr, &attributes) == RPC_S_OK)
            state->query_client_pid = static_cast<DWORD>(reinterpret_cast<UINT_PTR>(attributes.ClientPID));
        InterlockedIncrement(&state->query_calls);
        SetEvent(ready_event);
        if (!std::wcscmp(scenario, L"query-stall")) Sleep(INFINITE);
        return format && format->cfFormat == CF_HDROP && (format->tymed & TYMED_HGLOBAL) != 0 ? S_OK : DV_E_FORMATETC;
    }
    HRESULT STDMETHODCALLTYPE GetData(FORMATETC* format, STGMEDIUM* medium) override {
        if (!format || !medium) return E_POINTER;
        *medium = STGMEDIUM{};
        if (format->cfFormat != CF_HDROP || (format->tymed & TYMED_HGLOBAL) == 0) return DV_E_FORMATETC;
        bool target_entered = state->enter_pid != 0 && state->native_stage == 3;
        if (!target_entered) InterlockedIncrement(&state->preflight_get_calls);
        state->data_pid = GetCurrentProcessId(); state->data_tid = GetCurrentThreadId();
        if (target_entered) InterlockedIncrement(&state->data_calls);
        if (target_entered && !std::wcscmp(scenario, L"get-stall")) Sleep(INFINITE);
        SIZE_T bytes = target_entered && !std::wcscmp(scenario, L"over-memory") ? 128 * 1024 * 1024 : 64;
        state->offered_bytes = bytes;
        HGLOBAL global = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes);
        if (!global) { state->source_data_result = E_OUTOFMEMORY; return E_OUTOFMEMORY; }
        auto* data = static_cast<BYTE*>(GlobalLock(global));
        if (!data) { GlobalFree(global); state->source_data_result = E_OUTOFMEMORY; return E_OUTOFMEMORY; }
        // Synthetic DROPFILES, including native wide terminators. Never opens this path.
        const wchar_t path[] = L"C:\\owned-fixture.png";
        DWORD offset = 20, wide = 1;
        memcpy(data, &offset, sizeof(offset)); memcpy(data + 16, &wide, sizeof(wide));
        memcpy(data + 20, path, sizeof(path)); GlobalUnlock(global);
        medium->tymed = TYMED_HGLOBAL; medium->hGlobal = global;
        medium->pUnkForRelease = new MediumRelease(global, target_entered);
        state->source_data_result = S_OK;
        return S_OK;
    }
    HRESULT STDMETHODCALLTYPE GetDataHere(FORMATETC*, STGMEDIUM*) override { return E_NOTIMPL; }
    HRESULT STDMETHODCALLTYPE GetCanonicalFormatEtc(FORMATETC*, FORMATETC* out) override { if (out) out->ptd = nullptr; return E_NOTIMPL; }
    HRESULT STDMETHODCALLTYPE SetData(FORMATETC*, STGMEDIUM*, BOOL) override { return E_NOTIMPL; }
    HRESULT STDMETHODCALLTYPE EnumFormatEtc(DWORD direction, IEnumFORMATETC** out) override {
        InterlockedIncrement(&state->enum_calls);
        if (!out) return E_POINTER; *out = nullptr;
        if (direction != DATADIR_GET) return E_NOTIMPL;
        *out = new FormatsEnum;
        state->enum_result = S_OK;
        return state->enum_result;
    }
    HRESULT STDMETHODCALLTYPE DAdvise(FORMATETC*, DWORD, IAdviseSink*, DWORD*) override { return OLE_E_ADVISENOTSUPPORTED; }
    HRESULT STDMETHODCALLTYPE DUnadvise(DWORD) override { return OLE_E_ADVISENOTSUPPORTED; }
    HRESULT STDMETHODCALLTYPE EnumDAdvise(IEnumSTATDATA**) override { return OLE_E_ADVISENOTSUPPORTED; }
};
struct DropSource final : IDropSource {
    LONG refs = 1;
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override {
        InterlockedIncrement(&state->drop_source_qi_calls); AcquireSRWLockExclusive(&qi_trace_lock); state->last_drop_source_iid = iid; ReleaseSRWLockExclusive(&qi_trace_lock);
        if (!out) return E_POINTER; *out = nullptr;
        if (iid != IID_IUnknown && iid != IID_IDropSource) return E_NOINTERFACE;
        *out = this; AddRef(); return S_OK;
    }
    ULONG STDMETHODCALLTYPE AddRef() override { return InterlockedIncrement(&refs); }
    ULONG STDMETHODCALLTYPE Release() override { LONG n = InterlockedDecrement(&refs); if (!n) delete this; return n; }
    HRESULT STDMETHODCALLTYPE QueryContinueDrag(BOOL, DWORD) override { InterlockedIncrement(&state->source_continue_calls); return state->enter_pid ? DRAGDROP_S_DROP : S_OK; }
    HRESULT STDMETHODCALLTYPE GiveFeedback(DWORD) override { InterlockedIncrement(&state->source_feedback_calls); return DRAGDROP_S_USEDEFAULTCURSORS; }
};
static void CALLBACK private_move(HWND window, UINT, UINT_PTR, DWORD) {
    // A window message in the owned source only; never system input injection.
    InterlockedIncrement(&state->private_move_calls); state->message_position = GetMessagePos();
    PostMessageW(window, WM_MOUSEMOVE, MK_LBUTTON, MAKELPARAM(-256, -256));
}
struct CiMoveState { HANDLE stop; HDESK desktop; HWND source_window; };
static DWORD WINAPI owned_ci_mover(void* context) {
    require_owned_ci(); require(ci_input, "CI mover requires its private scenario");
    auto* move = static_cast<CiMoveState*>(context);
    // A new, operation-owned thread has created no native windows/COM objects.
    require(SetThreadDesktop(move->desktop), "attach mover to source owned CI desktop");
    state->mover_pid = GetCurrentProcessId(); state->mover_tid = GetCurrentThreadId(); state->mover_started = 1;
    HANDLE timer = CreateWaitableTimerW(nullptr, FALSE, nullptr); require(timer != nullptr, "owned CI mover timer");
    LARGE_INTEGER due{}; due.QuadPart = -20 * 10000;
    require(SetWaitableTimer(timer, &due, 20, nullptr, nullptr, FALSE), "arm owned CI mover timer");
    HANDLE waits[]{move->stop, stop_event, timer};
    bool button_down = false;
    for (;;) {
        DWORD wait = WaitForMultipleObjects(3, waits, FALSE, INFINITE);
        if (wait == WAIT_OBJECT_0 || wait == WAIT_OBJECT_0 + 1) break;
        require(wait == WAIT_OBJECT_0 + 2, "owned CI mover wait");
        if (!button_down) {
            if (InterlockedCompareExchange(&state->source_window_ready, 0, 0) == 0) continue;
            require(SetCursorPos(272, 272), "owned CI initiating source position");
            require(WindowFromPoint(POINT{272, 272}) == move->source_window, "owned source must receive initiating gesture");
            InterlockedExchange(&state->input_stage, 1);
            require(owned_left_button(MOUSEEVENTF_LEFTDOWN), "owned CI initiating left-button down");
            InterlockedIncrement(&state->injected_button_downs); button_down = true;
            continue;
        }
        // A source callback establishes that DoDragDrop has actually entered,
        // rather than merely that the caller is about to invoke it.
        if (InterlockedCompareExchange(&state->drag_call_started, 0, 0) == 0 || (InterlockedCompareExchange(&state->enum_calls, 0, 0) == 0 && InterlockedCompareExchange(&state->preflight_query_calls, 0, 0) == 0 && InterlockedCompareExchange(&state->source_continue_calls, 0, 0) == 0)) continue;
        LONG count = InterlockedIncrement(&state->ci_cursor_moves);
        int coordinate = count % 2 ? 32 : 16;
        require(SetCursorPos(coordinate, coordinate), "owned CI actual drag movement");
        HWND under_cursor = WindowFromPoint(POINT{coordinate, coordinate});
        state->cursor_window_is_owner = reinterpret_cast<UINT_PTR>(under_cursor) == state->owner_hwnd;
        GetWindowThreadProcessId(under_cursor, &state->cursor_window_pid);
        GetClassNameA(under_cursor, state->cursor_window_class, sizeof(state->cursor_window_class));
        GUITHREADINFO gui{}; gui.cbSize = sizeof(gui);
        state->source_gui_ok = GetGUIThreadInfo(state->source_tid, &gui);
        if (state->source_gui_ok) {
            state->source_capture_nonzero = gui.hwndCapture != nullptr;
            if (gui.hwndActive) GetWindowThreadProcessId(gui.hwndActive, &state->source_active_pid);
            if (gui.hwndCapture) { GetWindowThreadProcessId(gui.hwndCapture, &state->source_capture_pid); GetClassNameA(gui.hwndCapture, state->source_capture_class, sizeof(state->source_capture_class)); }
        }
    }
    if (button_down) { require(owned_left_button(MOUSEEVENTF_LEFTUP), "owned CI source left-button release"); InterlockedIncrement(&state->injected_button_ups); }
    CancelWaitableTimer(timer); CloseHandle(timer); state->mover_stopped = 1; return 0;
}
static LRESULT CALLBACK source_proc(HWND hwnd, UINT message, WPARAM wparam, LPARAM lparam) {
    if (message == WM_LBUTTONDOWN && ci_input && source_data && source_drop && InterlockedCompareExchange(&state->drag_call_started, 1, 0) == 0) {
        InterlockedExchange(&state->input_stage, 2);
        state->enter_result = DoDragDrop(source_data, source_drop, DROPEFFECT_COPY, &source_effect);
        InterlockedExchange(&state->drag_call_done, 1);
        return 0;
    }
    return DefWindowProcW(hwnd, message, wparam, lparam);
}
struct Target final : IDropTarget {
    LONG refs = 1;
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID iid, void** out) override {
        if (!out) return E_POINTER; *out = nullptr;
        if (iid != IID_IUnknown && iid != IID_IDropTarget) return E_NOINTERFACE;
        *out = this; AddRef(); return S_OK;
    }
    ULONG STDMETHODCALLTYPE AddRef() override { return InterlockedIncrement(&refs); }
    ULONG STDMETHODCALLTYPE Release() override { LONG n = InterlockedDecrement(&refs); if (!n) delete this; return n; }
    HRESULT STDMETHODCALLTYPE DragEnter(IDataObject* source, DWORD, POINTL, DWORD* effect) override {
        state->enter_pid = GetCurrentProcessId(); state->enter_tid = GetCurrentThreadId();
        state->native_stage = 1;
        // The failure control executes an owned source directly on the HWND's
        // foreground STA; remote COM waits may pump window messages themselves.
        std::unique_ptr<Source> local_baseline(baseline ? new Source : nullptr);
        if (local_baseline) source = local_baseline.get();
        *effect = DROPEFFECT_NONE;
        FORMATETC format{CF_HDROP, nullptr, DVASPECT_CONTENT, -1, TYMED_HGLOBAL};
        state->native_stage = 2;
        state->query_result = source->QueryGetData(&format);
        if (state->query_result != S_OK) return S_OK;
        STGMEDIUM medium{};
        state->native_stage = 3;
        state->get_result = source->GetData(&format, &medium);
        if (FAILED(state->get_result)) return S_OK;
        // This deliberately mirrors the old reader's post-acquisition boundary.
        bool admitted = medium.tymed == TYMED_HGLOBAL && GlobalSize(medium.hGlobal) <= 65536;
        state->native_stage = 4;
        ReleaseStgMedium(&medium);
        state->native_stage = 5;
        if (admitted) { state->copied = 1; *effect = DROPEFFECT_COPY; }
        return S_OK;
    }
    HRESULT STDMETHODCALLTYPE DragOver(DWORD, POINTL, DWORD* effect) override { *effect = DROPEFFECT_COPY; return S_OK; }
    HRESULT STDMETHODCALLTYPE DragLeave() override { return S_OK; }
    HRESULT STDMETHODCALLTYPE Drop(IDataObject*, DWORD, POINTL, DWORD* effect) override { *effect = state->copied ? DROPEFFECT_COPY : DROPEFFECT_NONE; return S_OK; }
};

static int receiver(int argc, wchar_t** argv) {
    attach(argc, argv); state->receiver_pid = GetCurrentProcessId(); state->receiver_tid = GetCurrentThreadId();
    HANDLE budget = contain_native();
    require_hr(OleInitialize(nullptr), "receiver OleInitialize");
    HWND owner = reinterpret_cast<HWND>(_wcstoui64(argv[6], nullptr, 10));
    if (baseline) owner = owner_window();
    state->owner_hwnd = reinterpret_cast<UINT_PTR>(owner);
    state->owner_tid = GetWindowThreadProcessId(owner, &state->owner_pid);
    auto* target = new Target;
    HRESULT registered = RegisterDragDrop(owner, target);
    state->registration = registered;
    if (FAILED(registered)) {
        state->failure_win32_error = GetLastError(); state->failure_pid = GetCurrentProcessId(); state->failure_hresult = registered;
        strcpy_s(state->failure_stage, "RegisterDragDrop");
        SetEvent(ready_event); target->Release(); OleUninitialize(); CloseHandle(budget); return 20;
    }
    IStream* stream = nullptr;
    require_hr(CreateStreamOnHGlobal(nullptr, TRUE, &stream), "marshal stream");
    require_hr(CoMarshalInterface(stream, IID_IDropTarget, target, MSHCTX_LOCAL, nullptr, MSHLFLAGS_TABLESTRONG), "marshal owned target");
    HGLOBAL global = nullptr; require_hr(GetHGlobalFromStream(stream, &global), "marshal extent");
    STATSTG stat{}; require_hr(stream->Stat(&stat, STATFLAG_NONAME), "marshal stat");
    require(stat.cbSize.HighPart == 0 && stat.cbSize.LowPart <= sizeof(state->marshal), "marshal bound");
    void* bytes = GlobalLock(global); require(bytes != nullptr, "marshal lock");
    state->marshal_bytes = stat.cbSize.LowPart; memcpy(state->marshal, bytes, state->marshal_bytes); GlobalUnlock(global);
    SetEvent(ready_event);
    for (;;) {
        DWORD wait = MsgWaitForMultipleObjects(1, &stop_event, FALSE, INFINITE, QS_ALLINPUT);
        if (wait == WAIT_OBJECT_0) break;
        require(wait == WAIT_OBJECT_0 + 1, "receiver wait");
        MSG msg{}; while (PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) { TranslateMessage(&msg); DispatchMessageW(&msg); }
    }
    // These potentially foreign cleanup calls remain inside the same hard deadline.
    state->cleanup_result = RevokeDragDrop(owner);
    LARGE_INTEGER zero{}; stream->Seek(zero, STREAM_SEEK_SET, nullptr);
    CoReleaseMarshalData(stream); stream->Release(); target->Release(); OleUninitialize(); CloseHandle(budget);
    if (baseline) DestroyWindow(owner);
    state->completed = 1; return 0;
}
static int source(int argc, wchar_t** argv) {
    attach(argc, argv); state->source_pid = GetCurrentProcessId(); state->source_tid = GetCurrentThreadId();
    require_hr(OleInitialize(nullptr), "source OleInitialize");
    auto* offered = new Source; DWORD effect = DROPEFFECT_COPY;
    if (ole_route) {
        WNDCLASSW cls{}; cls.lpfnWndProc = source_proc; cls.hInstance = GetModuleHandleW(nullptr); cls.lpszClassName = L"ScreenshotsPrivateDropSource";
        require(RegisterClassW(&cls) != 0, "private source class");
        HWND source_window = CreateWindowExW(0, cls.lpszClassName, L"owned synthetic source", WS_POPUP | WS_VISIBLE, 256, 256, 64, 64, nullptr, nullptr, cls.hInstance, nullptr);
        require(source_window != nullptr, "private source window");
        state->point_window = reinterpret_cast<UINT_PTR>(WindowFromPoint(POINT{0, 0}));
        state->cursor_ok = GetCursorPos(&state->cursor); state->cursor_error = state->cursor_ok ? 0 : GetLastError();
        auto* drag_source = new DropSource;
        source_data = offered; source_drop = drag_source; source_effect = DROPEFFECT_COPY;
        UINT_PTR timer = 0; HANDLE mover = nullptr;
        CiMoveState move{nullptr, GetThreadDesktop(GetCurrentThreadId()), source_window};
        if (ci_input) {
            move.stop = CreateEventW(nullptr, TRUE, FALSE, nullptr); require(move.stop != nullptr, "owned CI mover stop");
            mover = CreateThread(nullptr, 256 * 1024, owned_ci_mover, &move, STACK_SIZE_PARAM_IS_A_RESERVATION, nullptr);
            require(mover != nullptr, "owned CI mover thread");
            InterlockedExchange(&state->source_window_ready, 1);
        } else {
            timer = SetTimer(source_window, 1, 10, private_move); require(timer != 0, "private operation timer");
            private_move(source_window, 0, 0, 0);
        }
        if (ci_input) {
            while (InterlockedCompareExchange(&state->drag_call_done, 0, 0) == 0) {
                MSG message{}; require(GetMessageW(&message, nullptr, 0, 0) > 0, "owned source gesture message loop");
                TranslateMessage(&message); DispatchMessageW(&message);
            }
            effect = source_effect;
        } else {
            InterlockedExchange(&state->drag_call_started, 1);
            state->enter_result = DoDragDrop(offered, drag_source, DROPEFFECT_COPY, &effect);
        }
        if (mover) { SetEvent(move.stop); require(WaitForSingleObject(mover, 1000) == WAIT_OBJECT_0, "join owned CI mover"); CloseHandle(mover); CloseHandle(move.stop); }
        else KillTimer(source_window, timer);
        source_data = nullptr; source_drop = nullptr;
        drag_source->Release(); DestroyWindow(source_window); UnregisterClassW(cls.lpszClassName, cls.hInstance);
    } else {
        HGLOBAL global = GlobalAlloc(GMEM_MOVEABLE, state->marshal_bytes); require(global != nullptr, "unmarshal storage");
        void* bytes = GlobalLock(global); require(bytes != nullptr, "unmarshal lock");
        memcpy(bytes, state->marshal, state->marshal_bytes); GlobalUnlock(global);
        IStream* stream = nullptr; require_hr(CreateStreamOnHGlobal(global, TRUE, &stream), "unmarshal stream");
        IDropTarget* target = nullptr;
        require_hr(CoUnmarshalInterface(stream, IID_IDropTarget, reinterpret_cast<void**>(&target)), "unmarshal target");
        stream->Release();
        state->enter_result = target->DragEnter(offered, 0, POINTL{0, 0}, &effect);
        target->DragLeave(); target->Release();
    }
    offered->Release();
    // Final proxy and custom-medium rundown is deliberately observed before success.
    OleUninitialize(); SetEvent(done_event); return 0;
}
static PROCESS_INFORMATION launch(const std::wstring& command, const std::wstring& desktop, HANDLE job, const std::vector<HANDLE>& inherited) {
    SIZE_T size = 0; InitializeProcThreadAttributeList(nullptr, 2, 0, &size);
    std::vector<BYTE> storage(size); auto* attrs = reinterpret_cast<LPPROC_THREAD_ATTRIBUTE_LIST>(storage.data());
    require(InitializeProcThreadAttributeList(attrs, 2, 0, &size), "attribute list");
    require(UpdateProcThreadAttribute(attrs, 0, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, const_cast<HANDLE*>(inherited.data()), inherited.size() * sizeof(HANDLE), nullptr, nullptr), "handle whitelist");
    require(UpdateProcThreadAttribute(attrs, 0, PROC_THREAD_ATTRIBUTE_JOB_LIST, &job, sizeof(job), nullptr, nullptr), "atomic child job");
    STARTUPINFOEXW startup{}; startup.StartupInfo.cb = sizeof(startup); startup.StartupInfo.lpDesktop = const_cast<wchar_t*>(desktop.c_str()); startup.lpAttributeList = attrs;
    std::vector<wchar_t> mutable_command(command.begin(), command.end()); mutable_command.push_back(0);
    PROCESS_INFORMATION child{};
    require(CreateProcessW(nullptr, mutable_command.data(), nullptr, nullptr, TRUE, EXTENDED_STARTUPINFO_PRESENT | CREATE_SUSPENDED, nullptr, nullptr, &startup.StartupInfo, &child), "child create");
    DeleteProcThreadAttributeList(attrs); require(ResumeThread(child.hThread) != static_cast<DWORD>(-1), "child resume"); CloseHandle(child.hThread); return child;
}
static bool pump_until(HANDLE event, ULONGLONG limit, ULONG& dispatched) {
    for (;;) {
        ULONGLONG now = GetTickCount64(); if (now >= limit) return false;
        DWORD wait = MsgWaitForMultipleObjects(1, &event, FALSE, static_cast<DWORD>(limit - now), QS_ALLINPUT);
        if (wait == WAIT_OBJECT_0) return true;
        if (wait == WAIT_TIMEOUT) return false;
        require(wait == WAIT_OBJECT_0 + 1, "owner wait");
        MSG msg{}; while (PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) { ++dispatched; TranslateMessage(&msg); DispatchMessageW(&msg); }
    }
}
int wmain(int argc, wchar_t** argv) {
    if (argc == 3 && !std::wcscmp(argv[1], L"--release-owned-ci-input")) return release_owned_ci_input(argv[2]);
    if (argc > 1 && !std::wcscmp(argv[1], L"--receiver")) return receiver(argc, argv);
    if (argc > 1 && !std::wcscmp(argv[1], L"--source")) return source(argc, argv);
    bool explicit_ci_flag = argc == 3 && !std::wcscmp(argv[1], L"--owned-ci-input-desktop");
    require(argc == 2 || explicit_ci_flag, "supply one scenario, with the explicit flag only for owned hosted CI");
    const wchar_t* case_name = explicit_ci_flag ? argv[2] : argv[1]; scenario = parse_scenario(case_name);
    require(!ci_input || explicit_ci_flag, "CI input scenario requires --owned-ci-input-desktop");
    std::wstring station_name = L"ScreenshotsDropProbe-" + std::to_wstring(GetCurrentProcessId());
    HWINSTA original_station = GetProcessWindowStation(); HDESK original_desktop = GetThreadDesktop(GetCurrentThreadId());
    HWINSTA station = original_station; HDESK original_input = nullptr; POINT original_cursor{};
    std::wstring desktop_name = ci_input ? station_name : L"probe", desktop_path;
    if (ci_input) {
        wchar_t station_label[128]{}; DWORD actual = 0;
        require(GetUserObjectInformationW(original_station, UOI_NAME, station_label, sizeof(station_label), &actual) && !std::wcscmp(station_label, L"WinSta0"), "CI input mode needs its interactive WinSta0");
        original_input = OpenInputDesktop(0, FALSE, DESKTOP_SWITCHDESKTOP | DESKTOP_READOBJECTS | DESKTOP_WRITEOBJECTS);
        require(original_input != nullptr && GetCursorPos(&original_cursor), "owned CI original input state");
        desktop_path = L"WinSta0\\" + desktop_name;
    } else {
        station = CreateWindowStationW(station_name.c_str(), 0, WINSTA_ALL_ACCESS, nullptr);
        require(station != nullptr, "private window station"); require(SetProcessWindowStation(station), "select private station");
        desktop_path = station_name + L"\\probe";
    }
    DWORD desktop_access = DESKTOP_CREATEWINDOW | DESKTOP_READOBJECTS | DESKTOP_WRITEOBJECTS | DESKTOP_ENUMERATE | (ci_input ? DESKTOP_SWITCHDESKTOP : 0);
    HDESK desktop = CreateDesktopW(desktop_name.c_str(), nullptr, nullptr, 0, desktop_access, nullptr);
    require(desktop != nullptr && SetThreadDesktop(desktop), "private desktop");
    if (ci_input) { require(SwitchDesktop(desktop), "select owned CI input desktop"); require(SetCursorPos(16, 16), "owned CI private cursor position"); }
    HWND window = owner_window();
    SECURITY_ATTRIBUTES security{sizeof(security), nullptr, TRUE};
    HANDLE mapping = CreateFileMappingW(INVALID_HANDLE_VALUE, &security, PAGE_READWRITE, 0, sizeof(Shared), nullptr);
    state = static_cast<Shared*>(MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, sizeof(Shared))); require(state != nullptr, "private shared metadata");
    state->registration = state->enter_result = state->query_result = state->get_result = state->source_data_result = state->cleanup_result = E_PENDING;
    state->enum_result = state->enum_next_result = E_PENDING;
    ready_event = CreateEventW(&security, TRUE, FALSE, nullptr); done_event = CreateEventW(&security, TRUE, FALSE, nullptr); stop_event = CreateEventW(&security, TRUE, FALSE, nullptr);
    require(ready_event && done_event && stop_event, "private events");
    HANDLE job = CreateJobObjectW(nullptr, nullptr); require(job != nullptr, "outer job");
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION limits{}; limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
    limits.BasicLimitInformation.ActiveProcessLimit = 2; limits.ProcessMemoryLimit = 512 * 1024 * 1024;
    require(SetInformationJobObject(job, JobObjectExtendedLimitInformation, &limits, sizeof(limits)), "outer job configuration");
    wchar_t executable[32768]; require(GetModuleFileNameW(nullptr, executable, 32768) > 0, "executable path");
    auto command = [&](const wchar_t* mode) {
        return L"\"" + std::wstring(executable) + L"\" " + mode + L" " + std::to_wstring(reinterpret_cast<UINT_PTR>(mapping)) + L" " + std::to_wstring(reinterpret_cast<UINT_PTR>(ready_event)) + L" " + std::to_wstring(reinterpret_cast<UINT_PTR>(done_event)) + L" " + std::to_wstring(reinterpret_cast<UINT_PTR>(stop_event)) + L" " + std::to_wstring(reinterpret_cast<UINT_PTR>(window)) + L" " + case_name;
    };
    auto handles = std::vector<HANDLE>{mapping, ready_event, done_event, stop_event};
    auto receiver_process = launch(command(L"--receiver"), desktop_path, job, handles);
    ULONG dispatched = 0; ULONGLONG begin = GetTickCount64();
    bool ready = pump_until(ready_event, begin + deadline_ms, dispatched);
    PROCESS_INFORMATION source_process{}; bool delivered = false, cleaned = false, ping = false;
    if (ready && state->registration == S_OK) {
        ResetEvent(ready_event);
        source_process = launch(command(L"--source"), desktop_path, job, handles);
        bool queried = pump_until(ready_event, begin + deadline_ms, dispatched);
        if (queried) {
            ResetEvent(ready_event);
            PostMessageW(reinterpret_cast<HWND>(state->owner_hwnd), WM_APP + 2, 0, 0);
            ping = pump_until(ready_event, GetTickCount64() + 50, dispatched);
        }
        delivered = pump_until(done_event, begin + deadline_ms, dispatched);
        SetEvent(stop_event);
        cleaned = pump_until(receiver_process.hProcess, begin + deadline_ms, dispatched);
    }
    TerminateJobObject(job, 99);
    require(WaitForSingleObject(receiver_process.hProcess, 2000) == WAIT_OBJECT_0, "receiver reaped");
    if (source_process.hProcess) require(WaitForSingleObject(source_process.hProcess, 2000) == WAIT_OBJECT_0, "source reaped");
    if (ci_input) { require(owned_left_button(MOUSEEVENTF_LEFTUP), "owned CI controller left-button release"); InterlockedIncrement(&state->controller_button_ups); }
    DWORD receiver_exit = STILL_ACTIVE, source_exit = STILL_ACTIVE;
    require(GetExitCodeProcess(receiver_process.hProcess, &receiver_exit), "receiver exit code");
    if (source_process.hProcess) require(GetExitCodeProcess(source_process.hProcess, &source_exit), "source exit code");
    bool topology = state->enter_pid == state->receiver_pid && state->enter_tid == state->receiver_tid && (baseline ? state->query_pid == state->owner_pid && state->query_tid == state->owner_tid : state->query_pid == state->source_pid && state->query_tid == state->source_tid);
    wchar_t source_iid[40]{}, drop_iid[40]{};
    StringFromGUID2(state->last_source_iid, source_iid, 40); StringFromGUID2(state->last_drop_source_iid, drop_iid, 40);
    std::printf("{\"input_stage\":%ld,\"source_window_ready\":%ld,\"drag_call_done\":%ld,\"injected_button_downs\":%ld,\"injected_button_ups\":%ld,\"controller_button_ups\":%ld,\"source_qi_calls\":%ld,\"drop_source_qi_calls\":%ld,\"last_source_iid\":\"%ls\",\"last_drop_source_iid\":\"%ls\",\"source_gui_ok\":%ld,\"source_capture_nonzero\":%ld,\"source_active_pid\":%lu,\"source_capture_pid\":%lu,\"source_capture_class\":\"%s\",", state->input_stage, state->source_window_ready, state->drag_call_done, state->injected_button_downs, state->injected_button_ups, state->controller_button_ups, state->source_qi_calls, state->drop_source_qi_calls, source_iid, drop_iid, state->source_gui_ok, state->source_capture_nonzero, state->source_active_pid, state->source_capture_pid, state->source_capture_class);
    std::printf("\"drag_call_started\":%ld,\"enum_result\":%ld,\"enum_next_calls\":%ld,\"enum_next_result\":%ld,\"enum_next_fetched\":%ld,\"mover_started\":%ld,\"mover_stopped\":%ld,\"mover_pid\":%lu,\"mover_tid\":%lu,\"cursor_window_is_owner\":%ld,\"cursor_window_pid\":%lu,\"cursor_window_class\":\"%s\",", state->drag_call_started, state->enum_result, state->enum_next_calls, state->enum_next_result, state->enum_next_fetched, state->mover_started, state->mover_stopped, state->mover_pid, state->mover_tid, state->cursor_window_is_owner, state->cursor_window_pid, state->cursor_window_class);
    std::printf("\"preflight_query_calls\":%ld,\"preflight_get_calls\":%ld,\"preflight_medium_releases\":%ld,\"native_stage\":%ld,\"enum_calls\":%ld,\"source_continue_calls\":%ld,\"source_feedback_calls\":%ld,\"private_move_calls\":%ld,\"ci_cursor_moves\":%ld,\"cursor_ok\":%ld,\"cursor_error\":%lu,\"cursor_x\":%ld,\"cursor_y\":%ld,\"message_position\":%lu,\"point_zero_is_owner\":%s,\"query_trace_count\":%ld,\"query_trace\":[", state->preflight_query_calls, state->preflight_get_calls, state->preflight_medium_releases, state->native_stage, state->enum_calls, state->source_continue_calls, state->source_feedback_calls, state->private_move_calls, state->ci_cursor_moves, state->cursor_ok, state->cursor_error, state->cursor.x, state->cursor.y, state->message_position, state->point_window == state->owner_hwnd ? "true" : "false", state->query_trace_count);
    for (LONG index = 0; index < state->query_trace_count && index < 16; ++index) {
        const auto& trace = state->query_trace[index];
        std::printf("%s{\"format\":%lu,\"aspect\":%lu,\"tymed\":%lu,\"index\":%ld,\"after_enter\":%ld,\"pid\":%lu,\"tid\":%lu}", index ? "," : "", trace.format, trace.aspect, trace.tymed, trace.index, trace.after_enter, trace.pid, trace.tid);
    }
    std::printf("],");
    std::printf("\"scope\":\"%s\",\"baseline_foreground\":%s,\"baseline_uses_local_owned_source\":%s,\"owner_ping_within_50ms\":%s,\"registration\":%ld,\"ready\":%s,\"delivered\":%s,\"cleaned\":%s,\"topology\":%s,\"native_bound\":%ld,\"small_probe\":%ld,\"large_probe\":%ld,\"baseline_private\":%llu,\"growth_bytes\":%llu,\"copied\":%ld,\"query_calls\":%ld,\"data_calls\":%ld,\"medium_releases\":%ld,\"elapsed_ms\":%llu,\"owner_messages\":%lu,\"controller_pid\":%lu,\"owner_pid\":%lu,\"receiver_pid\":%lu,\"enter_pid\":%lu,\"source_pid\":%lu,\"query_pid\":%lu,\"query_client_pid\":%lu,\"query_client_pid_available\":%s,\"query_result\":%ld,\"get_result\":%ld,\"source_data_result\":%ld,\"offered_bytes\":%llu,\"cleanup_result\":%ld,\"receiver_exit\":%lu,\"source_exit\":%lu,\"failure_pid\":%lu,\"failure_win32_error\":%lu,\"failure_hresult\":%ld,\"failure_stage\":\"%s\",\"all_children_reaped\":true}\n", ci_input ? "owned-hosted-CI-input-DoDragDrop" : ole_route ? "private-DoDragDrop" : "private-direct-COM-proxy", baseline ? "true" : "false", baseline ? "true" : "false", ping ? "true" : "false", state->registration, ready ? "true" : "false", delivered ? "true" : "false", cleaned ? "true" : "false", topology ? "true" : "false", state->native_bound, state->small_probe, state->large_probe, state->private_baseline, static_cast<ULONGLONG>(growth), state->copied, state->query_calls, state->data_calls, state->medium_releases, GetTickCount64() - begin, dispatched, GetCurrentProcessId(), state->owner_pid, state->receiver_pid, state->enter_pid, state->source_pid, state->query_pid, state->query_client_pid, state->query_client_pid ? "true" : "false", state->query_result, state->get_result, state->source_data_result, state->offered_bytes, state->cleanup_result, receiver_exit, source_exit, state->failure_pid, state->failure_win32_error, state->failure_hresult, state->failure_stage);
    bool expected = ready && state->registration == S_OK && state->native_bound && state->small_probe && state->large_probe && state->query_calls == 1 && (baseline ? state->owner_pid == state->receiver_pid : state->owner_pid != state->receiver_pid);
    if (state->query_client_pid) expected = expected && state->query_client_pid == state->receiver_pid;
    if (!std::wcscmp(scenario, L"normal")) expected = expected && topology && delivered && cleaned && state->copied && state->medium_releases == 1 && state->data_calls == 1 && state->get_result == S_OK && state->cleanup_result == S_OK && (!ole_route || state->enter_result == DRAGDROP_S_DROP);
    else expected = expected && topology && !state->copied && (!delivered || !std::wcscmp(scenario, L"over-memory"));
    if (!std::wcscmp(scenario, L"query-stall")) expected = expected && state->data_calls == 0 && state->query_result == E_PENDING && (baseline ? !ping : ping);
    if (!std::wcscmp(scenario, L"get-stall")) expected = expected && state->data_calls == 1 && state->get_result == E_PENDING;
    if (!std::wcscmp(scenario, L"release-stall")) expected = expected && state->data_calls == 1 && state->medium_releases == 1;
    if (!std::wcscmp(scenario, L"over-memory")) {
        bool memory_refused = state->get_result == E_OUTOFMEMORY || state->get_result == HRESULT_FROM_WIN32(ERROR_NOT_ENOUGH_MEMORY) || state->get_result == HRESULT_FROM_WIN32(ERROR_NO_SYSTEM_RESOURCES) || state->get_result == HRESULT_FROM_WIN32(ERROR_COMMITMENT_LIMIT);
        expected = expected && state->data_calls == 1 && state->offered_bytes == 128 * 1024 * 1024 && state->source_data_result == S_OK && memory_refused;
    }
    if (ci_input) {
        expected = expected && state->mover_started == 1 && state->mover_pid == state->source_pid && state->mover_tid != state->source_tid && state->ci_cursor_moves >= 1 && state->cursor_window_is_owner == 1 && state->cursor_window_pid == state->owner_pid;
        expected = expected && state->injected_button_downs == 1 && state->input_stage == 2 && state->controller_button_ups == 1;
        if (delivered) expected = expected && state->mover_stopped == 1 && state->injected_button_ups == 1;
    }
    CloseHandle(receiver_process.hProcess); if (source_process.hProcess) CloseHandle(source_process.hProcess); CloseHandle(job);
    UnmapViewOfFile(state); state = nullptr; CloseHandle(mapping); CloseHandle(ready_event); CloseHandle(done_event); CloseHandle(stop_event);
    DestroyWindow(window); UnregisterClassW(L"ScreenshotsPrivateDropProbe", GetModuleHandleW(nullptr));
    if (ci_input) require(SwitchDesktop(original_input), "restore owned CI input desktop");
    require(SetThreadDesktop(original_desktop), "restore desktop");
    if (ci_input) { require(SetCursorPos(original_cursor.x, original_cursor.y), "restore owned CI cursor"); CloseDesktop(original_input); }
    require(SetProcessWindowStation(original_station), "restore station"); CloseDesktop(desktop); if (!ci_input) CloseWindowStation(station);
    return expected ? 0 : 1;
}
