//! All incoming COM, including argument unmarshaling and release, stays here.
#[path = "native_drop_payload.rs"]
mod payload;
use super::{native_drop_events as event, native_drop_wire as wire};
use anyhow::{Result, ensure};
use payload::{Format, Formats, Payload};
use std::{
    cell::{Cell, RefCell},
    fs::File,
    mem::ManuallyDrop,
    os::windows::io::{FromRawHandle, OwnedHandle},
    rc::Rc,
};
use windows::{
    Win32::{
        Foundation::*,
        System::{Com::*, Ole::*, SystemServices::MODIFIERKEYS_FLAGS, Threading::*},
        UI::{Shell::*, WindowsAndMessaging::*},
    },
    core::*,
};

pub(super) const PROPERTY: PCWSTR = w!("Screenshots.NativeDrop.Target.v1");
const ARM: u32 = 30;
const DONE: u32 = 31;
const READY: u32 = 32;

struct Receiver {
    formats: Formats,
    output: RefCell<File>,
    parent: OwnedHandle,
    permit: OwnedHandle,
    armed: Cell<bool>,
    sequence: Cell<u64>,
    version: Cell<u64>,
    busy: Cell<Option<(u64, u64)>>,
    generation: Cell<u64>,
    command_sequence: Cell<u64>,
    enabled: Cell<bool>,
    pending: RefCell<Option<wire::Packet>>,
    parent_pid: u32,
    thread: u32,
    #[cfg(test)]
    trace: Option<std::sync::Arc<binding_tests::Trace>>,
}
impl Receiver {
    fn send(&self, header: wire::Header, bytes: &[u8]) -> Result<()> {
        wire::write(&mut *self.output.borrow_mut(), header, bytes)
    }
    fn control(&self, kind: u32) -> Result<()> {
        self.send(
            wire::Header {
                kind,
                reader: self.sequence.get(),
                generation: self.command_sequence.get(),
                ..Default::default()
            },
            &[],
        )
    }
    fn permit(&self) -> Result<()> {
        ensure!(
            event::wait(&[event::raw(&self.parent), event::raw(&self.permit)], 5000)? == 1,
            "Native controller unavailable"
        );
        Ok(())
    }
    fn arm(&self) -> Result<()> {
        ensure!(!self.armed.replace(true), "Nested native pump guard");
        self.sequence.set(
            self.sequence
                .get()
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("Native interval exhausted"))?,
        );
        self.control(ARM)?;
        self.permit()?;
        #[cfg(screenshots_native_drop_acceptance)]
        super::native_drop_acceptance::hook(1, self.sequence.get(), std::ptr::null_mut());
        Ok(())
    }
    fn done(&self) -> Result<()> {
        #[cfg(screenshots_native_drop_acceptance)]
        super::native_drop_acceptance::error_reference();
        clear_error_info()?;
        // Source argument wrappers and STGMEDIUM have already unwound before this.
        if let Some(packet) = self.pending.borrow_mut().take() {
            self.send(packet.header, &packet.bytes)?;
        }
        self.control(DONE)?;
        self.permit()?;
        #[cfg(screenshots_native_drop_acceptance)]
        super::native_drop_acceptance::hook(2, self.sequence.get(), std::ptr::null_mut());
        self.armed.set(false);
        Ok(())
    }
    fn check(&self) -> Result<()> {
        ensure!(
            unsafe { GetCurrentThreadId() } == self.thread
                && self.armed.get()
                && unsafe { WaitForSingleObject(event::raw(&self.parent), 0) } == WAIT_TIMEOUT,
            "Native receiver canceled"
        );
        Ok(())
    }
    fn stage(&self, packet: wire::Packet) -> Result<()> {
        let mut pending = self.pending.borrow_mut();
        if packet.header.kind == wire::MOTION {
            if let Some(old) = pending.as_mut() {
                ensure!(
                    old.header.window == packet.header.window
                        && old.header.generation == packet.header.generation,
                    "Nested native target changed"
                );
                if matches!(old.header.kind, wire::ENTER | wire::MOTION) {
                    old.header.x = packet.header.x;
                    old.header.y = packet.header.y;
                }
                return Ok(());
            }
        }
        ensure!(
            pending
                .as_ref()
                .is_none_or(|old| old.header.kind != wire::SUBMIT),
            "Native result slot occupied"
        );
        #[cfg(screenshots_native_drop_acceptance)]
        if packet.header.kind == wire::SUBMIT {
            super::native_drop_acceptance::hook(
                56,
                packet.bytes.len() as u64,
                std::ptr::null_mut(),
            );
        }
        *pending = Some(packet);
        Ok(())
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum GesturePhase {
    Retired,
    Active,
    Dropping,
}
#[derive(Clone, Copy, Eq, PartialEq)]
struct Operation {
    generation: u64,
    serial: u64,
}
struct Target {
    receiver: Rc<Receiver>,
    window: u64,
    hwnd: HWND,
    version: u64,
    // Rust reference counting does not call COM while the RefCell is borrowed.
    helper: RefCell<Option<Rc<IDropTargetHelper>>>,
    generation: Cell<u64>,
    phase: Cell<GesturePhase>,
    serial: Cell<u64>,
    operation: Cell<Option<Operation>>,
    format: Cell<Option<Format>>,
    metadata: RefCell<Option<(u32, Vec<u8>)>>,
    open: Cell<bool>,
    retired: Cell<bool>,
    #[cfg(test)]
    identity_override: bool,
    #[cfg(test)]
    helper_factory: Option<Rc<dyn Fn() -> Result<IDropTargetHelper>>>,
}
impl Target {
    fn observe_callback(&self) {
        #[cfg(test)]
        if let Some(trace) = self.receiver.trace.as_ref() {
            trace.record(self.receiver.armed.get());
        }
    }
    fn live(&self) -> bool {
        if self.retired.get()
            || unsafe { GetCurrentThreadId() } != self.receiver.thread
            || !self.receiver.armed.get()
            || self.version != self.receiver.version.get()
        {
            return false;
        }
        #[cfg(test)]
        if self.identity_override {
            return true;
        }
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(self.hwnd, Some(&mut pid));
        }
        pid == self.receiver.parent_pid
            && unsafe { GetPropW(self.hwnd, PROPERTY) }.0 as usize as u64 == self.window
    }
    fn owns_admission(&self, generation: u64) -> bool {
        self.receiver.busy.get() == Some((self.window, generation))
    }
    fn operation_current(&self, operation: Operation) -> bool {
        self.live()
            && self.phase.get() != GesturePhase::Retired
            && self.operation.get() == Some(operation)
            && self.generation.get() == operation.generation
            && self.owns_admission(operation.generation)
    }
    fn check_operation(&self, operation: Operation) -> Result<()> {
        self.receiver.check()?;
        ensure!(
            self.operation_current(operation),
            "Native operation retired"
        );
        Ok(())
    }
    fn begin_operation(&self) -> Result<Option<Operation>> {
        if !self.live()
            || self.phase.get() != GesturePhase::Active
            || self.operation.get().is_some()
            || !self.owns_admission(self.generation.get())
        {
            return Ok(None);
        }
        let serial = self
            .serial
            .get()
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Native operation exhausted"))?;
        let operation = Operation {
            generation: self.generation.get(),
            serial,
        };
        self.serial.set(serial);
        self.operation.set(Some(operation));
        Ok(Some(operation))
    }
    fn stage(
        &self,
        generation: u64,
        kind: u32,
        format: u32,
        pt: POINTL,
        bytes: Vec<u8>,
    ) -> Result<()> {
        self.receiver.stage(wire::Packet {
            header: wire::Header {
                kind,
                format,
                window: self.window,
                generation,
                version: self.version,
                reader: 0,
                x: pt.x,
                y: pt.y,
            },
            bytes,
        })
    }
    fn retire_gesture(&self) -> Option<u64> {
        let previous = self.phase.replace(GesturePhase::Retired);
        self.operation.set(None);
        self.clear();
        (previous != GesturePhase::Retired).then_some(self.generation.get())
    }
    fn leave_helper(&self) -> Result<()> {
        let helper = self.helper.borrow_mut().take();
        let open = self.open.replace(false);
        if let Some(helper) = helper {
            if open {
                unsafe { helper.DragLeave() }
                    .map_err(|_| anyhow::anyhow!("Native drag helper failed"))?;
            }
            drop(helper);
        }
        Ok(())
    }
    fn clear(&self) {
        self.format.set(None);
        self.metadata.borrow_mut().take();
    }
    fn make_helper(&self) -> Result<IDropTargetHelper> {
        #[cfg(screenshots_native_drop_acceptance)]
        if let Some(helper) = super::native_drop_acceptance::helper()? {
            return Ok(helper);
        }
        #[cfg(test)]
        if let Some(factory) = self.helper_factory.as_ref() {
            return factory();
        }
        unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER) }
            .map_err(|_| anyhow::anyhow!("Native helper unavailable"))
    }
    fn failed(&self, operation: Operation) -> Result<()> {
        if self.operation.get() != Some(operation) {
            return Ok(());
        }
        // Admission remains occupied, but no foreign cleanup may revive a gesture.
        let generation = self.retire_gesture();
        self.leave_helper()?;
        if let Some(generation) = generation {
            self.stage(generation, wire::FAILED, 0, POINTL::default(), Vec::new())?;
        }
        Ok(())
    }
    fn finish_operation(&self, operation: Operation, result: Result<bool>) -> Result<bool> {
        match result {
            Ok(accepted) => {
                if self.operation.get() == Some(operation) {
                    self.operation.set(None);
                }
                Ok(accepted)
            }
            Err(_) => {
                self.failed(operation)?;
                Ok(false)
            }
        }
    }
    fn entered(&self, source: &IDataObject, pt: POINTL, effect: DROPEFFECT) -> Result<bool> {
        if !self.live()
            || effect == DROPEFFECT_NONE
            || self.receiver.busy.get().is_some()
            || !self.receiver.enabled.replace(false)
        {
            return Ok(false);
        }
        let generation = self
            .receiver
            .generation
            .get()
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("Native gesture exhausted"))?;
        self.receiver.generation.set(generation);
        self.generation.set(generation);
        self.phase.set(GesturePhase::Active);
        self.receiver.busy.set(Some((self.window, generation)));
        let operation = self
            .begin_operation()?
            .ok_or_else(|| anyhow::anyhow!("Native gesture unavailable"))?;
        let result = (|| {
            let format = self
                .receiver
                .formats
                .negotiate(source, &|| self.check_operation(operation))?;
            self.check_operation(operation)?;
            self.format.set(Some(format));
            let (code, body) =
                if matches!(format, Format::Files | Format::UrlWide | Format::UrlAscii) {
                    payload_bytes(
                        self.receiver
                            .formats
                            .acquire(source, format, &|| self.check_operation(operation))?,
                    )?
                } else {
                    (format_code(format), Vec::new())
                };
            self.check_operation(operation)?;
            *self.metadata.borrow_mut() = Some((code, body.clone()));
            let helper = Rc::new(self.make_helper()?);
            self.check_operation(operation)?;
            *self.helper.borrow_mut() = Some(helper.clone());
            self.open.set(true);
            let result =
                unsafe { helper.DragEnter(self.hwnd, source, &POINT { x: pt.x, y: pt.y }, effect) }
                    .map_err(|_| anyhow::anyhow!("Native drag helper failed"));
            self.check_operation(operation)?;
            drop(helper);
            self.check_operation(operation)?;
            result?;
            self.stage(operation.generation, wire::ENTER, code, pt, body)?;
            Ok(true)
        })();
        self.finish_operation(operation, result)
    }
    fn moved(&self, pt: POINTL, effect: DROPEFFECT) -> Result<bool> {
        let Some(operation) = self.begin_operation()? else {
            return Ok(false);
        };
        let result = (|| {
            ensure!(self.open.get(), "Native drag helper is closed");
            let helper = self
                .helper
                .borrow()
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Native drag helper unavailable"))?;
            let result = unsafe { helper.DragOver(&POINT { x: pt.x, y: pt.y }, effect) }
                .map_err(|_| anyhow::anyhow!("Native drag helper failed"));
            self.check_operation(operation)?;
            drop(helper);
            self.check_operation(operation)?;
            result?;
            self.stage(operation.generation, wire::MOTION, 0, pt, Vec::new())?;
            Ok(true)
        })();
        self.finish_operation(operation, result)
    }
    fn leave(&self) -> Result<()> {
        self.receiver.check()?;
        if let Some(generation) = self.retire_gesture() {
            // Retire first: helper DragLeave/final Release can call us again.
            self.leave_helper()?;
            self.stage(generation, wire::LEAVE, 0, POINTL::default(), Vec::new())?;
        }
        Ok(())
    }
    fn refuse_drop(&self) -> Result<bool> {
        if let Some(operation) = self.begin_operation()? {
            self.failed(operation)?;
        }
        Ok(false)
    }
    fn dropped(&self, source: &IDataObject, pt: POINTL, effect: DROPEFFECT) -> Result<bool> {
        let Some(operation) = self.begin_operation()? else {
            return Ok(false);
        };
        let result = (|| {
            ensure!(effect != DROPEFFECT_NONE, "Native copy was refused");
            let format = self
                .format
                .get()
                .ok_or_else(|| anyhow::anyhow!("Missing native format"))?;
            let (code, bytes) =
                if matches!(format, Format::Files | Format::UrlWide | Format::UrlAscii) {
                    self.metadata
                        .borrow_mut()
                        .take()
                        .ok_or_else(|| anyhow::anyhow!("Missing native metadata"))?
                } else {
                    payload_bytes(
                        self.receiver
                            .formats
                            .acquire(source, format, &|| self.check_operation(operation))?,
                    )?
                };
            self.check_operation(operation)?;
            // No new acquisition may begin during terminal helper calls. Leave
            // still invalidates this distinct ticket and suppresses publication.
            self.phase.set(GesturePhase::Dropping);
            let helper = self.helper.borrow_mut().take();
            let open = self.open.replace(false);
            if let Some(helper) = helper {
                let result = if open {
                    unsafe { helper.Drop(source, &POINT { x: pt.x, y: pt.y }, effect) }
                        .map_err(|_| anyhow::anyhow!("Native drag helper failed"))
                } else {
                    Ok(())
                };
                self.check_operation(operation)?;
                drop(helper);
                self.check_operation(operation)?;
                result?;
            }
            self.check_operation(operation)?;
            self.retire_gesture();
            self.stage(operation.generation, wire::SUBMIT, code, pt, bytes)?;
            Ok(true)
        })();
        self.finish_operation(operation, result)
    }
}
fn format_code(format: Format) -> u32 {
    match format {
        Format::Files => wire::FILES,
        Format::Png => wire::PNG,
        Format::Jpeg => wire::JPEG,
        Format::Webp => wire::WEBP,
        Format::Dib | Format::DibV5 => wire::BMP,
        Format::UrlWide | Format::UrlAscii => wire::URL,
    }
}
fn payload_bytes(value: Payload) -> Result<(u32, Vec<u8>)> {
    Ok(match value {
        Payload::Files(paths) => (wire::FILES, wire::encode_files(&paths)?),
        Payload::Image { format, bytes } => (
            wire::image_code(format)?,
            std::sync::Arc::try_unwrap(bytes)
                .map_err(|_| anyhow::anyhow!("Native image ownership failed"))?,
        ),
        Payload::Url(url) => (wire::URL, url.as_bytes().to_vec()),
    })
}
#[implement(IDropTarget, Agile = false)]
struct DropTarget(Rc<Target>);
#[allow(non_snake_case)]
impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        source: Ref<IDataObject>,
        _: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        fatal_if(self.0.receiver.check());
        self.0.observe_callback();
        #[cfg(screenshots_native_drop_acceptance)]
        super::native_drop_acceptance::hook(
            5,
            self.0.receiver.sequence.get(),
            std::ptr::null_mut(),
        );
        let copy = unsafe { *effect & DROPEFFECT_COPY };
        unsafe {
            *effect = DROPEFFECT_NONE;
        }
        if source
            .ok()
            .ok()
            .is_some_and(|source| fatal_if(self.0.entered(source, *pt, copy)))
        {
            unsafe {
                *effect = copy;
            }
        }
        #[cfg(screenshots_native_drop_acceptance)]
        {
            if super::native_drop_acceptance::source_mutation() == Some("busy-get")
                && unsafe { *effect } == DROPEFFECT_NONE
                && self.0.receiver.busy.get().is_some()
            {
                if let Ok(source) = source.ok() {
                    super::native_drop_acceptance::hook(70, 6, std::ptr::null_mut());
                    let format = fatal_if(
                        self.0
                            .receiver
                            .formats
                            .negotiate(source, &|| self.0.receiver.check()),
                    );
                    let extra = fatal_if(
                        self.0
                            .receiver
                            .formats
                            .acquire(source, format, &|| self.0.receiver.check()),
                    );
                    drop(extra);
                }
            }
            super::native_drop_acceptance::mutate_busy_effect(effect);
        }
        Ok(())
    }
    fn DragOver(
        &self,
        _: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        fatal_if(self.0.receiver.check());
        self.0.observe_callback();
        let copy = unsafe { *effect & DROPEFFECT_COPY };
        unsafe {
            *effect = DROPEFFECT_NONE;
        }
        if fatal_if(self.0.moved(*pt, copy)) {
            unsafe {
                *effect = copy;
            }
        }
        Ok(())
    }
    fn DragLeave(&self) -> windows::core::Result<()> {
        fatal_if(self.0.receiver.check());
        self.0.observe_callback();
        fatal_if(self.0.leave());
        Ok(())
    }
    fn Drop(
        &self,
        source: Ref<IDataObject>,
        _: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        fatal_if(self.0.receiver.check());
        self.0.observe_callback();
        let copy = unsafe { *effect & DROPEFFECT_COPY };
        unsafe {
            *effect = DROPEFFECT_NONE;
        }
        let accepted = match source.ok() {
            Ok(source) => fatal_if(self.0.dropped(source, *pt, copy)),
            Err(_) => fatal_if(self.0.refuse_drop()),
        };
        if accepted {
            unsafe {
                *effect = copy;
            }
        }
        #[cfg(screenshots_native_drop_acceptance)]
        if accepted && super::native_drop_acceptance::mutate_drop_effect(effect) {
            return Err(windows::core::Error::from_hresult(E_FAIL));
        }
        Ok(())
    }
}
fn fatal_if<T>(result: Result<T>) -> T {
    match result {
        Ok(value) => value,
        Err(_) => unsafe { ExitProcess(70) },
    }
}
struct Registration {
    target: Rc<Target>,
    _interface: IDropTarget,
}
fn remove(registration: Registration) -> Result<()> {
    let target = &registration.target;
    // Parent normal close holds the exact HWND until this acknowledgement.
    ensure!(target.live(), "Native target identity changed");
    target.retired.set(true);
    target.retire_gesture();
    target.leave_helper()?;
    #[cfg(screenshots_native_drop_acceptance)]
    super::native_drop_acceptance::hook(21, target.receiver.sequence.get(), std::ptr::null_mut());
    unsafe { RevokeDragDrop(target.hwnd) }
        .map_err(|_| anyhow::anyhow!("Native target removal failed"))?;
    if target
        .receiver
        .busy
        .get()
        .is_some_and(|(window, _)| window == target.window)
    {
        target.receiver.busy.set(None);
    }
    drop(registration);
    Ok(())
}
/// Called only by the early authenticated inherited-pipe entry; no UI/logger exists.
pub fn serve(
    mut input: File,
    mut output: File,
    nonce: [u8; 16],
    parent_pid: u32,
    command: usize,
    permit: usize,
) -> Result<()> {
    ensure!(
        command > 3 && permit > 3 && command != permit,
        "Invalid native event handles"
    );
    wire::handshake(&mut input, &mut output, &nonce, false)?;
    let command = unsafe { OwnedHandle::from_raw_handle(command as _) };
    let permit = unsafe { OwnedHandle::from_raw_handle(permit as _) };
    for handle in [&command, &permit] {
        unsafe {
            SetHandleInformation(event::raw(handle), HANDLE_FLAG_INHERIT.0, HANDLE_FLAGS(0))
        }?;
    }
    let parent = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, parent_pid) }?;
    let parent = unsafe { OwnedHandle::from_raw_handle(parent.0) };
    // Initialization is native work too. Parent must acknowledge ARM first.
    let sequence = 1u64;
    wire::write(
        &mut output,
        wire::Header {
            kind: ARM,
            reader: sequence,
            ..Default::default()
        },
        &[],
    )?;
    ensure!(
        event::wait(&[event::raw(&parent), event::raw(&permit)], 5000)? == 1,
        "Native controller unavailable"
    );
    #[cfg(screenshots_native_drop_acceptance)]
    super::native_drop_acceptance::hook(1, sequence, std::ptr::null_mut());
    fatal_if(
        unsafe { OleInitialize(None) }
            .map_err(|_| anyhow::anyhow!("Native receiver initialization failed")),
    );
    #[cfg(screenshots_native_drop_acceptance)]
    super::native_drop_acceptance::hook(3, sequence, std::ptr::null_mut());
    let formats = fatal_if(Formats::register());
    let receiver = ManuallyDrop::new(Rc::new(Receiver {
        formats,
        output: RefCell::new(output),
        parent,
        permit,
        armed: Cell::new(true),
        sequence: Cell::new(sequence),
        version: Cell::new(0),
        busy: Cell::new(None),
        generation: Cell::new(0),
        command_sequence: Cell::new(0),
        enabled: Cell::new(false),
        pending: RefCell::new(None),
        parent_pid,
        thread: unsafe { GetCurrentThreadId() },
        #[cfg(test)]
        trace: None,
    }));
    let mut targets = ManuallyDrop::new(Vec::<Registration>::new());
    fatal_if(receiver.send(
        wire::Header {
            kind: READY,
            ..Default::default()
        },
        &[],
    ));
    fatal_if(receiver.done());
    loop {
        // Non-alertable native wait does not retrieve or dispatch a message.
        let handles = [event::raw(&receiver.parent), event::raw(&command)];
        let ready = wait_for_work(&handles, INFINITE);
        if ready == 0 {
            unsafe { ExitProcess(70) }
        }
        ensure_or_exit(ready == 1 || ready == 2);
        fatal_if(receiver.arm());
        if ready == 1 {
            let packet = fatal_if(wire::read(&mut input));
            let h = packet.header;
            ensure_or_exit(h.reader == receiver.command_sequence.get() + 1);
            receiver.command_sequence.set(h.reader);
            match h.kind {
                wire::REGISTER => {
                    ensure_or_exit(
                        targets.len() < 32
                            && h.window > 0
                            && h.version > 0
                            && h.generation == 0
                            && packet.bytes.len() == 8
                            && !targets.iter().any(|entry| entry.target.window == h.window),
                    );
                    if targets.is_empty() {
                        receiver.version.set(h.version);
                    }
                    ensure_or_exit(receiver.version.get() == h.version);
                    let hwnd = HWND(
                        u64::from_le_bytes(packet.bytes[..8].try_into().unwrap()) as usize as _
                    );
                    let target = Rc::new(Target {
                        receiver: Rc::clone(&receiver),
                        window: h.window,
                        hwnd,
                        version: h.version,
                        helper: RefCell::new(None),
                        generation: Cell::new(0),
                        phase: Cell::new(GesturePhase::Retired),
                        serial: Cell::new(0),
                        operation: Cell::new(None),
                        #[cfg(test)]
                        identity_override: false,
                        #[cfg(test)]
                        helper_factory: None,
                        format: Cell::new(None),
                        metadata: RefCell::new(None),
                        open: Cell::new(false),
                        retired: Cell::new(false),
                    });
                    ensure_or_exit(target.live());
                    // A reaped receiver may have left an OLE registration on this
                    // still-live parent HWND. Reclaim it only inside the new guard.
                    fatal_if(match unsafe { RevokeDragDrop(hwnd) } {
                        Err(error) if error.code() == DRAGDROP_E_NOTREGISTERED => Ok(()),
                        result => {
                            result.map_err(|_| anyhow::anyhow!("Native target reclaim failed"))
                        }
                    });
                    ensure_or_exit(target.live());
                    let interface: IDropTarget = DropTarget(target.clone()).into();
                    #[cfg(screenshots_native_drop_acceptance)]
                    super::native_drop_acceptance::hook(
                        20,
                        receiver.sequence.get(),
                        std::ptr::null_mut(),
                    );
                    fatal_if(
                        unsafe { RegisterDragDrop(hwnd, &interface) }
                            .map_err(|_| anyhow::anyhow!("Native registration failed")),
                    );
                    #[cfg(screenshots_native_drop_acceptance)]
                    super::native_drop_acceptance::hook(
                        4,
                        receiver.sequence.get(),
                        interface.as_raw(),
                    );
                    targets.push(Registration {
                        target,
                        _interface: interface,
                    });
                    #[cfg(screenshots_native_drop_acceptance)]
                    if receiver.enabled.get() {
                        // A newly registered B may share an already-enabled receiver;
                        // registration alone must never manufacture admission.
                        super::native_drop_acceptance::hook(
                            9,
                            receiver.sequence.get(),
                            std::ptr::null_mut(),
                        );
                    }
                    fatal_if(receiver.send(
                        wire::Header {
                            kind: wire::REGISTERED,
                            ..h
                        },
                        &[],
                    ));
                }
                wire::REMOVE => {
                    ensure_or_exit(h.version == receiver.version.get());
                    let index = targets
                        .iter()
                        .position(|entry| entry.target.window == h.window)
                        .unwrap_or_else(|| unsafe { ExitProcess(70) });
                    fatal_if(remove(targets.remove(index)));
                    fatal_if(receiver.send(
                        wire::Header {
                            kind: wire::REMOVED,
                            ..h
                        },
                        &[],
                    ));
                }
                wire::RELEASE => {
                    if (receiver.busy.get().is_none()
                        || receiver.busy.get() == Some((h.window, h.generation)))
                        && h.version == receiver.version.get()
                    {
                        receiver.busy.set(None);
                        receiver.enabled.set(true);
                        #[cfg(screenshots_native_drop_acceptance)]
                        super::native_drop_acceptance::hook(
                            9,
                            receiver.sequence.get(),
                            std::ptr::null_mut(),
                        );
                    }
                }
                wire::ALLOW => {
                    ensure_or_exit(
                        receiver.busy.get().is_none() && h.version == receiver.version.get(),
                    );
                    receiver.enabled.set(true);
                    #[cfg(screenshots_native_drop_acceptance)]
                    super::native_drop_acceptance::hook(
                        9,
                        receiver.sequence.get(),
                        std::ptr::null_mut(),
                    );
                }
                #[cfg(screenshots_native_drop_acceptance)]
                40 => {
                    ensure_or_exit(
                        h.version == receiver.version.get() && h.window == 0 && h.generation == 0,
                    );
                    if !super::native_drop_acceptance::omit_completion_reply() {
                        fatal_if(receiver.send(wire::Header { kind: 41, ..h }, &[]));
                    }
                }
                wire::STOP => {
                    while let Some(registration) = targets.pop() {
                        fatal_if(remove(registration));
                    }
                    #[cfg(screenshots_native_drop_acceptance)]
                    super::native_drop_acceptance::hook(
                        8,
                        receiver.sequence.get(),
                        std::ptr::null_mut(),
                    );
                    unsafe {
                        ManuallyDrop::drop(&mut targets);
                        #[cfg(screenshots_native_drop_acceptance)]
                        super::native_drop_acceptance::hook(
                            22,
                            receiver.sequence.get(),
                            std::ptr::null_mut(),
                        );
                        OleUninitialize();
                    }
                    // Receiver/output objects contain no source COM interfaces now.
                    fatal_if(receiver.done());
                    unsafe { ExitProcess(0) }
                }
                _ => unsafe { ExitProcess(70) },
            }
        } else {
            // PeekMessage itself can unmarshal a custom source: ARM precedes it.
            let mut message = MSG::default();
            if unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
                ensure_or_exit(message.message != WM_QUIT);
                unsafe {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }
        fatal_if(receiver.done());
    }
}
fn wait_for_work(handles: &[HANDLE], timeout: u32) -> u32 {
    // Previously inspected queue input must still wake the next guarded dispatch.
    unsafe { MsgWaitForMultipleObjectsEx(Some(handles), timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE) }
        .0
}
fn ensure_or_exit(value: bool) {
    if !value {
        unsafe { ExitProcess(70) }
    }
}

#[cfg(test)]
#[path = "native_drop_binding_tests.rs"]
mod binding_tests;

#[link(name = "oleaut32")]
unsafe extern "system" {
    #[link_name = "GetErrorInfo"]
    fn take_error_info(reserved: u32, output: *mut *mut std::ffi::c_void) -> HRESULT;
}
fn clear_error_info() -> Result<()> {
    // Raw HRESULT avoids asking an untrusted error object for diagnostic text.
    // A reentrant Release may install another object; bounded churn fails closed.
    for _ in 0..16 {
        let mut pointer = std::ptr::null_mut();
        let result = unsafe { take_error_info(0, &mut pointer) };
        ensure!(
            result == S_OK || result == S_FALSE,
            "Native error cleanup failed"
        );
        if pointer.is_null() {
            ensure!(result == S_FALSE, "Invalid native error reference");
            return Ok(());
        }
        let value = unsafe { IErrorInfo::from_raw(pointer) };
        drop(value);
    }
    anyhow::bail!("Native error cleanup did not settle")
}
