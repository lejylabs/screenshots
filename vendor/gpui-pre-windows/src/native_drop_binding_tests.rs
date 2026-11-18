//! Actual Rust target/QI/two-STA regression. Run only in an owned Windows test process/Job.
use super::*;
use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    thread,
    time::{Duration, Instant},
};
use windows::Win32::System::{
    Com::{
        Marshal::{CoMarshalInterThreadInterfaceInStream, IMarshal},
        StructuredStorage::CoGetInterfaceAndReleaseStream,
    },
    SystemServices::MODIFIERKEYS_FLAGS,
};
pub(super) struct Trace {
    calls: AtomicU32,
    owner: u32,
    wrong_thread: AtomicU32,
    pid: AtomicU32,
    tid: AtomicU32,
    unguarded: AtomicU32,
}
impl Trace {
    pub(super) fn record(&self, armed: bool) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if unsafe { GetCurrentThreadId() } != self.owner {
            self.wrong_thread.fetch_add(1, Ordering::SeqCst);
        }
        self.pid
            .store(unsafe { GetCurrentProcessId() }, Ordering::SeqCst);
        self.tid
            .store(unsafe { GetCurrentThreadId() }, Ordering::SeqCst);
        if !armed {
            self.unguarded.fetch_add(1, Ordering::SeqCst);
        }
    }
}
#[implement(IDropTargetHelper, Agile = false)]
struct PrivateHelper;
#[allow(non_snake_case)]
impl IDropTargetHelper_Impl for PrivateHelper_Impl {
    fn DragEnter(
        &self,
        _: HWND,
        _: Ref<IDataObject>,
        _: *const POINT,
        _: DROPEFFECT,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn DragLeave(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn DragOver(&self, _: *const POINT, _: DROPEFFECT) -> windows::core::Result<()> {
        Ok(())
    }
    fn Drop(
        &self,
        _: Ref<IDataObject>,
        _: *const POINT,
        _: DROPEFFECT,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn Show(&self, _: BOOL) -> windows::core::Result<()> {
        Ok(())
    }
}
#[test]
#[ignore = "owned Windows process/Job only; no clipboard, window, input or user desktop access"]
fn actual_rust_target_is_non_agile_and_marshals_callbacks_to_guarded_owner_sta() {
    unsafe {
        OleInitialize(None).unwrap();
    }
    let owner_tid = unsafe { GetCurrentThreadId() };
    let owner_pid = unsafe { GetCurrentProcessId() };
    let trace = Arc::new(Trace {
        calls: AtomicU32::new(0),
        owner: owner_tid,
        wrong_thread: AtomicU32::new(0),
        pid: AtomicU32::new(0),
        tid: AtomicU32::new(0),
        unguarded: AtomicU32::new(0),
    });
    let parent = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, owner_pid).unwrap() };
    let parent = unsafe { OwnedHandle::from_raw_handle(parent.0) };
    let receiver = Rc::new(Receiver {
        formats: Formats::register().unwrap(),
        output: RefCell::new(File::options().write(true).open("NUL").unwrap()),
        parent,
        permit: event::event(false).unwrap(),
        armed: Cell::new(true),
        sequence: Cell::new(0),
        version: Cell::new(1),
        busy: Cell::new(None),
        generation: Cell::new(0),
        command_sequence: Cell::new(0),
        enabled: Cell::new(false),
        pending: RefCell::new(None),
        parent_pid: owner_pid,
        thread: owner_tid,
        trace: Some(trace.clone()),
    });
    let helper: IDropTargetHelper = PrivateHelper.into();
    let target = Rc::new(Target {
        receiver: receiver.clone(),
        window: 1,
        hwnd: HWND::default(),
        version: 1,
        helper: RefCell::new(Some(Rc::new(helper))),
        generation: Cell::new(0),
        phase: Cell::new(GesturePhase::Retired),
        serial: Cell::new(0),
        operation: Cell::new(None),
        identity_override: false,
        helper_factory: None,
        format: Cell::new(None),
        metadata: RefCell::new(None),
        open: Cell::new(false),
        retired: Cell::new(false),
    });
    let interface: IDropTarget = DropTarget(target.clone()).into();
    assert_eq!(
        interface.cast::<IAgileObject>().unwrap_err().code(),
        E_NOINTERFACE
    );
    assert_eq!(
        interface.cast::<IMarshal>().unwrap_err().code(),
        E_NOINTERFACE
    );
    let unknown: IUnknown = interface.cast().unwrap();
    let stream =
        unsafe { CoMarshalInterThreadInterfaceInStream(&IDropTarget::IID, &unknown).unwrap() };
    // This is COM's transfer stream, never a raw IDataObject or target pointer.
    let stream = stream.into_raw() as usize;
    let done = Arc::new(event::event(false).unwrap());
    let completed = done.clone();
    receiver.armed.set(false);
    let client = thread::spawn(move || {
        unsafe {
            OleInitialize(None).unwrap();
        }
        let client_tid = unsafe { GetCurrentThreadId() };
        let stream = ManuallyDrop::new(unsafe { IStream::from_raw(stream as _) });
        let proxy: IDropTarget = unsafe { CoGetInterfaceAndReleaseStream(&*stream).unwrap() };
        let mut effect = DROPEFFECT_COPY;
        unsafe {
            proxy
                .DragOver(MODIFIERKEYS_FLAGS(0), POINTL::default(), &mut effect)
                .unwrap();
            proxy.DragLeave().unwrap();
        }
        drop(proxy);
        unsafe {
            OleUninitialize();
        }
        event::signal(&*completed).unwrap();
        client_tid
    });
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let remain = until
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min(5000) as u32;
        let ready = unsafe {
            MsgWaitForMultipleObjects(Some(&[event::raw(&*done)]), false, remain, QS_ALLINPUT)
        };
        if ready == WAIT_OBJECT_0 {
            break;
        }
        // A failure exits this owned test process; no blocked COM thread is abandoned.
        if ready != WAIT_EVENT(1) {
            unsafe { ExitProcess(91) }
        }
        receiver.armed.set(true);
        let mut message = MSG::default();
        if unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        receiver.armed.set(false);
    }
    let client_tid = client.join().unwrap();
    assert_ne!(client_tid, owner_tid);
    assert_eq!(trace.calls.load(Ordering::SeqCst), 2);
    assert_eq!(trace.wrong_thread.load(Ordering::SeqCst), 0);
    assert_eq!(trace.pid.load(Ordering::SeqCst), owner_pid);
    assert_eq!(trace.tid.load(Ordering::SeqCst), owner_tid);
    assert_eq!(trace.unguarded.load(Ordering::SeqCst), 0);
    receiver.armed.set(true);
    drop(unknown);
    drop(interface);
    drop(target);
    drop(receiver);
    unsafe {
        OleUninitialize();
    }
}

#[implement(IErrorInfo, Agile = false)]
struct PrivateError {
    releases: Arc<AtomicU32>,
    descriptions: Arc<AtomicU32>,
}
impl Drop for PrivateError {
    fn drop(&mut self) {
        self.releases.fetch_add(1, Ordering::SeqCst);
    }
}
#[allow(non_snake_case)]
impl IErrorInfo_Impl for PrivateError_Impl {
    fn GetGUID(&self) -> windows::core::Result<GUID> {
        Ok(GUID::zeroed())
    }
    fn GetSource(&self) -> windows::core::Result<BSTR> {
        Ok(BSTR::new())
    }
    fn GetDescription(&self) -> windows::core::Result<BSTR> {
        self.descriptions.fetch_add(1, Ordering::SeqCst);
        Ok(BSTR::from("private fixture sentinel"))
    }
    fn GetHelpFile(&self) -> windows::core::Result<BSTR> {
        Ok(BSTR::new())
    }
    fn GetHelpContext(&self) -> windows::core::Result<u32> {
        Ok(0)
    }
}
#[test]
fn private_source_error_reference_released_without_description() {
    thread::spawn(|| {
        unsafe {
            OleInitialize(None).unwrap();
        }
        let releases = Arc::new(AtomicU32::new(0));
        let descriptions = Arc::new(AtomicU32::new(0));
        let info: IErrorInfo = PrivateError {
            releases: releases.clone(),
            descriptions: descriptions.clone(),
        }
        .into();
        unsafe {
            SetErrorInfo(0, &info).unwrap();
        }
        drop(info);
        clear_error_info().unwrap();
        assert_eq!(releases.load(Ordering::SeqCst), 1);
        assert_eq!(descriptions.load(Ordering::SeqCst), 0);
        unsafe {
            OleUninitialize();
        }
    })
    .join()
    .unwrap();
}

// These exercise the actual Rust target and HGLOBAL adapter through generated COM
// vtables. Only HWND identity and helper creation are replaced; no window/input is used.
#[derive(Clone, Copy, Eq, PartialEq)]
enum ReentryAt {
    GetData,
    HelperEnter,
    HelperDrop,
    MissingVisualMetadata,
    MissingVisualMetadataEnterReentry,
    MissingVisualMetadataDropReentry,
    UnknownVisualFailure,
    UnknownVisualDropFailure,
}
struct Reentry {
    at: ReentryAt,
    fired: Cell<bool>,
    interface: RefCell<Option<IDropTarget>>,
    source: RefCell<Option<IDataObject>>,
    target: RefCell<std::rc::Weak<Target>>,
    png: u16,
    queries: Cell<u32>,
    reads: Cell<u32>,
    helper_enters: Cell<u32>,
    helper_drops: Cell<u32>,
    helper_leaves: Cell<u32>,
    helper_releases: Cell<u32>,
}
impl Reentry {
    fn leave_during_foreign_call(&self) {
        if self.fired.replace(true) {
            return;
        }
        let interface = self.interface.borrow().as_ref().unwrap().clone();
        let source = self.source.borrow().as_ref().unwrap().clone();
        let queries = self.queries.get();
        let reads = self.reads.get();
        // Nested acquisition is refused both before and after Leave.
        for after_leave in [false, true] {
            if after_leave {
                unsafe {
                    interface.DragLeave().unwrap();
                }
            }
            let mut effect = DROPEFFECT_COPY;
            unsafe {
                interface
                    .DragEnter(
                        &source,
                        MODIFIERKEYS_FLAGS(0),
                        POINTL::default(),
                        &mut effect,
                    )
                    .unwrap();
            }
            assert_eq!(effect, DROPEFFECT_NONE);
            effect = DROPEFFECT_COPY;
            unsafe {
                interface
                    .Drop(
                        &source,
                        MODIFIERKEYS_FLAGS(0),
                        POINTL::default(),
                        &mut effect,
                    )
                    .unwrap();
            }
            assert_eq!(effect, DROPEFFECT_NONE);
            effect = DROPEFFECT_COPY;
            unsafe {
                interface
                    .DragOver(MODIFIERKEYS_FLAGS(0), POINTL::default(), &mut effect)
                    .unwrap();
            }
            assert_eq!(effect, DROPEFFECT_NONE);
            assert_eq!(self.queries.get(), queries);
            assert_eq!(self.reads.get(), reads);
        }
        let target = self.target.borrow().upgrade().unwrap();
        assert!(target.phase.get() == GesturePhase::Retired);
        assert!(target.operation.get().is_none());
    }
}
#[implement(IDropTargetHelper, Agile = false)]
struct ReentrantHelper {
    context: std::rc::Weak<Reentry>,
}
impl Drop for ReentrantHelper {
    fn drop(&mut self) {
        if let Some(context) = self.context.upgrade() {
            context
                .helper_releases
                .set(context.helper_releases.get() + 1);
            let target = context.target.borrow().upgrade().unwrap();
            assert!(target.phase.get() == if matches!(context.at, ReentryAt::MissingVisualMetadata | ReentryAt::UnknownVisualDropFailure) {GesturePhase::Dropping} else {GesturePhase::Retired});
        }
    }
}
#[allow(non_snake_case)]
impl IDropTargetHelper_Impl for ReentrantHelper_Impl {
    fn DragEnter(
        &self,
        _: HWND,
        _: Ref<IDataObject>,
        _: *const POINT,
        _: DROPEFFECT,
    ) -> windows::core::Result<()> {
        let context = self.context.upgrade().unwrap();
        context.helper_enters.set(context.helper_enters.get() + 1);
        if matches!(context.at, ReentryAt::HelperEnter | ReentryAt::MissingVisualMetadataEnterReentry) {
            context.leave_during_foreign_call();
        }
        match context.at {
            ReentryAt::MissingVisualMetadata | ReentryAt::MissingVisualMetadataEnterReentry | ReentryAt::MissingVisualMetadataDropReentry | ReentryAt::UnknownVisualDropFailure => Err(DV_E_FORMATETC.into()),
            ReentryAt::UnknownVisualFailure => Err(E_UNEXPECTED.into()),
            _ => Ok(()),
        }
    }
    fn DragLeave(&self) -> windows::core::Result<()> {
        let context = self.context.upgrade().unwrap();
        context.helper_leaves.set(context.helper_leaves.get() + 1);
        let target = context.target.borrow().upgrade().unwrap();
        assert!(target.phase.get() == GesturePhase::Retired);
        assert!(target.operation.get().is_none());
        if context.at == ReentryAt::MissingVisualMetadata {Err(DV_E_FORMATETC.into())} else {Ok(())}
    }
    fn DragOver(&self, _: *const POINT, _: DROPEFFECT) -> windows::core::Result<()> {
        let context = self.context.upgrade().unwrap();
        assert!(context.at == ReentryAt::MissingVisualMetadata, "Recursive helper movement must be refused");
        Err(DV_E_FORMATETC.into())
    }
    fn Drop(
        &self,
        _: Ref<IDataObject>,
        _: *const POINT,
        _: DROPEFFECT,
    ) -> windows::core::Result<()> {
        let context = self.context.upgrade().unwrap();
        context.helper_drops.set(context.helper_drops.get() + 1);
        let target = context.target.borrow().upgrade().unwrap();
        assert!(target.phase.get() == GesturePhase::Dropping);
        if matches!(context.at, ReentryAt::HelperDrop | ReentryAt::MissingVisualMetadataDropReentry) {
            context.leave_during_foreign_call();
        }
        if context.at == ReentryAt::UnknownVisualDropFailure {Err(E_UNEXPECTED.into())} else if matches!(context.at, ReentryAt::MissingVisualMetadata | ReentryAt::MissingVisualMetadataDropReentry) {Err(DV_E_FORMATETC.into())} else {Ok(())}
    }
    fn Show(&self, _: BOOL) -> windows::core::Result<()> {
        Ok(())
    }
}
#[implement(IDataObject, Agile = false)]
struct ReentrantSource {
    context: std::rc::Weak<Reentry>,
}
#[allow(non_snake_case)]
impl IDataObject_Impl for ReentrantSource_Impl {
    fn GetData(&self, format: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
        use windows::Win32::System::Memory::{
            GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalUnlock,
        };
        let context = self.context.upgrade().unwrap();
        assert!(!format.is_null());
        assert_eq!(unsafe { (*format).cfFormat }, context.png);
        context.reads.set(context.reads.get() + 1);
        if context.at == ReentryAt::GetData {
            context.leave_during_foreign_call();
        }
        let bytes = b"\x89PNG\r\n\x1a\n";
        let memory = unsafe { GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, bytes.len()) }?;
        let locked = unsafe { GlobalLock(memory) };
        assert!(!locked.is_null());
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), locked.cast(), bytes.len());
            let _ = GlobalUnlock(memory);
        }
        Ok(STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: memory },
            ..Default::default()
        })
    }
    fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn QueryGetData(&self, format: *const FORMATETC) -> HRESULT {
        let context = self.context.upgrade().unwrap();
        context.queries.set(context.queries.get() + 1);
        if !format.is_null() && unsafe { (*format).cfFormat } == context.png {
            S_OK
        } else {
            S_FALSE
        }
    }
    fn GetCanonicalFormatEtc(&self, _: *const FORMATETC, _: *mut FORMATETC) -> HRESULT {
        E_NOTIMPL
    }
    fn SetData(
        &self,
        _: *const FORMATETC,
        _: *const STGMEDIUM,
        _: BOOL,
    ) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn EnumFormatEtc(&self, _: u32) -> windows::core::Result<IEnumFORMATETC> {
        Err(E_NOTIMPL.into())
    }
    fn DAdvise(
        &self,
        _: *const FORMATETC,
        _: u32,
        _: Ref<IAdviseSink>,
    ) -> windows::core::Result<u32> {
        Err(E_NOTIMPL.into())
    }
    fn DUnadvise(&self, _: u32) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
        Err(E_NOTIMPL.into())
    }
}
fn actual_reentrant_leave(at: ReentryAt) {
    thread::spawn(move || {
        unsafe {
            OleInitialize(None).unwrap();
        }
        let pid = unsafe { GetCurrentProcessId() };
        let parent = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid).unwrap() };
        let receiver = Rc::new(Receiver {
            formats: Formats::register().unwrap(),
            output: RefCell::new(File::options().write(true).open("NUL").unwrap()),
            parent: unsafe { OwnedHandle::from_raw_handle(parent.0) },
            permit: event::event(false).unwrap(),
            armed: Cell::new(true),
            sequence: Cell::new(0),
            version: Cell::new(1),
            busy: Cell::new(None),
            generation: Cell::new(0),
            command_sequence: Cell::new(0),
            enabled: Cell::new(true),
            pending: RefCell::new(None),
            parent_pid: pid,
            thread: unsafe { GetCurrentThreadId() },
            trace: None,
        });
        let png =
            unsafe { windows::Win32::System::DataExchange::RegisterClipboardFormatW(w!("PNG")) };
        assert!(png > 0 && png <= u16::MAX as u32);
        let context = Rc::new(Reentry {
            at,
            fired: Cell::new(false),
            interface: RefCell::new(None),
            source: RefCell::new(None),
            target: RefCell::new(std::rc::Weak::new()),
            png: png as u16,
            queries: Cell::new(0),
            reads: Cell::new(0),
            helper_enters: Cell::new(0),
            helper_drops: Cell::new(0),
            helper_leaves: Cell::new(0),
            helper_releases: Cell::new(0),
        });
        let factory_context = Rc::downgrade(&context);
        let target = Rc::new(Target {
            receiver: receiver.clone(),
            window: 1,
            hwnd: HWND::default(),
            version: 1,
            helper: RefCell::new(None),
            generation: Cell::new(0),
            phase: Cell::new(GesturePhase::Retired),
            serial: Cell::new(0),
            operation: Cell::new(None),
            format: Cell::new(None),
            metadata: RefCell::new(None),
            open: Cell::new(false),
            retired: Cell::new(false),
            identity_override: true,
            helper_factory: Some(Rc::new(move || {
                Ok(ReentrantHelper {
                    context: factory_context.clone(),
                }
                .into())
            })),
        });
        *context.target.borrow_mut() = Rc::downgrade(&target);
        let interface: IDropTarget = DropTarget(target.clone()).into();
        let source: IDataObject = ReentrantSource {
            context: Rc::downgrade(&context),
        }
        .into();
        *context.interface.borrow_mut() = Some(interface.clone());
        *context.source.borrow_mut() = Some(source.clone());
        let mut effect = DROPEFFECT_COPY;
        unsafe {
            interface
                .DragEnter(
                    &source,
                    MODIFIERKEYS_FLAGS(0),
                    POINTL::default(),
                    &mut effect,
                )
                .unwrap();
        }
        let enter_refused = matches!(at, ReentryAt::HelperEnter | ReentryAt::MissingVisualMetadataEnterReentry | ReentryAt::UnknownVisualFailure);
        if !enter_refused {
            assert_eq!(effect, DROPEFFECT_COPY);
            let entered = receiver.pending.borrow_mut().take().unwrap();
            assert_eq!(entered.header.kind, wire::ENTER);
            if at == ReentryAt::MissingVisualMetadata {
                effect = DROPEFFECT_COPY;
                unsafe {interface.DragOver(MODIFIERKEYS_FLAGS(0), POINTL::default(), &mut effect).unwrap();}
                assert_eq!(effect, DROPEFFECT_COPY);
                assert_eq!(receiver.pending.borrow_mut().take().unwrap().header.kind, wire::MOTION);
            }
            effect = DROPEFFECT_COPY;
            unsafe {
                interface
                    .Drop(
                        &source,
                        MODIFIERKEYS_FLAGS(0),
                        POINTL::default(),
                        &mut effect,
                    )
                    .unwrap();
            }
        }
        let success = at == ReentryAt::MissingVisualMetadata;
        let unknown = matches!(at, ReentryAt::UnknownVisualFailure | ReentryAt::UnknownVisualDropFailure);
        assert_eq!(effect, if success {DROPEFFECT_COPY} else {DROPEFFECT_NONE});
        assert_eq!(context.fired.get(), !success && !unknown);
        let terminal = receiver.pending.borrow_mut().take().unwrap();
        assert_eq!(terminal.header.kind, if success {wire::SUBMIT} else if unknown {wire::FAILED} else {wire::LEAVE});
        assert_eq!(terminal.header.generation, 1);
        assert_eq!(terminal.bytes.is_empty(), !success);
        assert!(target.phase.get() == GesturePhase::Retired);
        assert!(target.operation.get().is_none());
        assert!(target.helper.borrow().is_none());
        assert!(!target.open.get());
        assert_eq!(receiver.busy.get(), Some((1, 1)));
        assert!(!receiver.enabled.get());
        assert_eq!(context.helper_releases.get(), 1);
        assert_eq!(
            context.reads.get(),
            if enter_refused { 0 } else { 1 }
        );
        assert_eq!(context.helper_enters.get(), 1);
        assert_eq!(
            context.helper_drops.get(),
            u32::from(matches!(at, ReentryAt::HelperDrop | ReentryAt::MissingVisualMetadataDropReentry | ReentryAt::MissingVisualMetadata | ReentryAt::UnknownVisualDropFailure))
        );
        assert_eq!(
            context.helper_leaves.get(),
            u32::from(!matches!(at, ReentryAt::HelperDrop | ReentryAt::MissingVisualMetadataDropReentry | ReentryAt::MissingVisualMetadata | ReentryAt::UnknownVisualDropFailure))
        );
        context.interface.borrow_mut().take();
        context.source.borrow_mut().take();
        drop(interface);
        drop(source);
        drop(target);
        drop(context);
        drop(receiver);
        clear_error_info().unwrap();
        unsafe {
            OleUninitialize();
        }
    })
    .join()
    .unwrap();
}
#[test]
fn actual_target_leave_during_get_data_cannot_publish_late_submit() {
    actual_reentrant_leave(ReentryAt::GetData);
}
#[test]
fn actual_target_leave_during_helper_enter_cannot_publish_late_enter() {
    actual_reentrant_leave(ReentryAt::HelperEnter);
}
#[test]
fn actual_target_leave_during_helper_drop_cannot_publish_late_submit() {
    actual_reentrant_leave(ReentryAt::HelperDrop);
}

#[test]
fn missing_shell_visual_metadata_does_not_reject_valid_payload() {
    actual_reentrant_leave(ReentryAt::MissingVisualMetadata);
}
#[test]
fn missing_shell_visual_metadata_cannot_revive_reentrant_enter_or_drop() {
    actual_reentrant_leave(ReentryAt::MissingVisualMetadataEnterReentry);
    actual_reentrant_leave(ReentryAt::MissingVisualMetadataDropReentry);
}
#[test]
fn unknown_shell_visual_failure_still_rejects_payload() {
    actual_reentrant_leave(ReentryAt::UnknownVisualFailure);
    actual_reentrant_leave(ReentryAt::UnknownVisualDropFailure);
}

#[test]
fn multiple_prequeued_messages_wake_without_new_input_after_first_peek() {
    let parent = event::event(true).unwrap();
    let command = event::event(false).unwrap();
    let handles = [event::raw(&parent), event::raw(&command)];
    let mut message = MSG::default();
    unsafe {
        // Own test-thread queue; neither input injection nor a user's window is used.
        let _ = PeekMessageW(&mut message, None, WM_APP, WM_APP + 1, PM_NOREMOVE);
        let thread = GetCurrentThreadId();
        PostThreadMessageW(thread, WM_APP, WPARAM(0), LPARAM(0)).unwrap();
        PostThreadMessageW(thread, WM_APP + 1, WPARAM(0), LPARAM(0)).unwrap();
        for expected in [WM_APP, WM_APP + 1] {
            assert_eq!(
                wait_for_work(&handles, 1000),
                2,
                "Already-seen queued input must wake without another producer"
            );
            assert!(PeekMessageW(&mut message, None, WM_APP, WM_APP + 1, PM_REMOVE).as_bool());
            assert_eq!(message.message, expected);
        }
    }
}
