//! DraggingInfo descriptors stay on AppKit's thread; promised data stays in the owned worker.
use super::*;
use gpui::{NativeDropData, NativeDropSource};
use std::cell::RefCell;

static READ_ACTIVE: AtomicBool = AtomicBool::new(false);
thread_local! {
    static CURRENT: RefCell<Weak<Mutex<MacWindowState>>> = RefCell::new(Weak::new());
}
struct Lease;
impl Drop for Lease {
    fn drop(&mut self) {
        READ_ACTIVE.store(false, Ordering::Release);
    }
}

#[derive(Default)]
pub(super) struct State {
    generation: u64,
    sequence: Option<NSInteger>,
    source: Option<NativeDropSource>,
    position: Point<Pixels>,
    cancel: Option<Arc<AtomicBool>>,
    reading: bool,
    ready: bool,
    image: bool,
    dropped: bool,
}
impl State {
    pub(super) fn retire(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Release);
        }
        self.generation = self.generation.saturating_add(1);
        self.sequence = None;
        self.source.take();
        self.reading = false;
        self.ready = false;
        self.image = false;
        self.dropped = false;
    }
    fn current(&self, generation: u64) -> bool {
        self.generation == generation && self.source.is_some()
    }
}

/// Explicit application cancellation never dispatches a reentrant window input callback.
pub(crate) fn cancel_pending_native_drop() {
    CURRENT.with(|current| {
        if let Some(window) = current.borrow_mut().upgrade() {
            window.lock().native_drop.retire();
        }
    });
}

unsafe fn descriptor(sender: *mut AnyObject) -> Option<(NSInteger, NativeDropSource)> {
    if sender.is_null() {
        return None;
    }
    let sequence: NSInteger = unsafe { msg_send![sender, draggingSequenceNumber] };
    let board: *mut AnyObject = unsafe { msg_send![sender, draggingPasteboard] };
    if board.is_null() {
        return None;
    }
    let name: *const NSString = unsafe { msg_send![board, name] };
    let name = unsafe { name.as_ref() }?;
    if name.length() == 0 || name.length() > 128 {
        return None;
    }
    // NSString's UTF-16 extent bounds the temporary UTF-8 conversion to 512 bytes.
    let name = name.to_string();
    if name.is_empty()
        || name.len() > 128
        || name.contains('\0')
        || matches!(
            name.as_str(),
            "NSGeneralPboard" | "com.apple.pasteboard.clipboard"
        )
    {
        return None;
    }
    let change_count: NSInteger = unsafe { msg_send![board, changeCount] };
    if change_count < 0 {
        return None;
    }
    Some((
        sequence,
        NativeDropSource::MacPasteboard {
            name: name.into(),
            change_count: change_count as i64,
        },
    ))
}

pub(super) fn entered(
    window: Rc<Mutex<MacWindowState>>,
    sender: *mut AnyObject,
) -> NSDragOperation {
    if window.lock().closed.load(Ordering::Acquire) || gpui::native_drop_reader().is_none() {
        return NSDragOperation::None;
    }
    let old = CURRENT.with(|current| {
        let old = current.borrow().upgrade();
        *current.borrow_mut() = Rc::downgrade(&window);
        old
    });
    if let Some(old) = old {
        old.lock().native_drop.retire();
        send_file_drop_event(old, FileDropEvent::Exited);
    }
    let Some((sequence, source)) = (unsafe { descriptor(sender) }) else {
        return NSDragOperation::None;
    };
    let position = drag_event_position(&window, sender);
    let generation = {
        let mut lock = window.lock();
        lock.native_drop.retire();
        let state = &mut lock.native_drop;
        if state.generation == u64::MAX {
            return NSDragOperation::None;
        }
        state.sequence = Some(sequence);
        state.source = Some(source);
        state.position = position;
        state.generation
    };
    if start_read(&window, generation, false) {
        NSDragOperation::Copy
    } else {
        NSDragOperation::None
    }
}

pub(super) fn updated(
    window: Rc<Mutex<MacWindowState>>,
    sender: *mut AnyObject,
) -> NSDragOperation {
    let sequence: NSInteger = unsafe { msg_send![sender, draggingSequenceNumber] };
    let position = drag_event_position(&window, sender);
    let accepted = {
        let mut lock = window.lock();
        if lock.closed.load(Ordering::Acquire)
            || lock.native_drop.sequence != Some(sequence)
            || lock.native_drop.dropped
        {
            return NSDragOperation::None;
        }
        lock.native_drop.position = position;
        lock.native_drop.source.is_some()
    };
    if accepted {
        send_file_drop_event(window, FileDropEvent::Pending { position });
        NSDragOperation::Copy
    } else {
        NSDragOperation::None
    }
}

pub(super) fn exited(window: Rc<Mutex<MacWindowState>>, sender: *mut AnyObject) {
    if sender.is_null() {
        return;
    }
    let sequence: NSInteger = unsafe { msg_send![sender, draggingSequenceNumber] };
    let mut lock = window.lock();
    if lock.native_drop.sequence != Some(sequence) || lock.native_drop.dropped {
        return;
    }
    lock.native_drop.retire();
    drop(lock);
    send_file_drop_event(window, FileDropEvent::Exited);
}

pub(super) fn perform(window: Rc<Mutex<MacWindowState>>, sender: *mut AnyObject) -> Bool {
    let Some((sequence, source)) = (unsafe { descriptor(sender) }) else {
        return Bool::NO;
    };
    let position = drag_event_position(&window, sender);
    let (generation, begin, image) = {
        let mut lock = window.lock();
        if lock.closed.load(Ordering::Acquire)
            || lock.native_drop.sequence != Some(sequence)
            || lock.native_drop.dropped
        {
            return Bool::NO;
        }
        let state = &mut lock.native_drop;
        state.source = Some(source);
        state.position = position;
        state.dropped = true;
        (state.generation, !state.reading, state.ready && state.image)
    };
    // AppKit's synchronous Boolean reports acquisition admission, never editor success.
    (!begin || start_read(&window, generation, image)).to_objc()
}

fn start_read(window: &Rc<Mutex<MacWindowState>>, generation: u64, image: bool) -> bool {
    let Some(reader) = gpui::native_drop_reader() else {
        return false;
    };
    let (source, cancel, background, foreground, final_read) = {
        let mut lock = window.lock();
        if lock.closed.load(Ordering::Acquire)
            || !lock.native_drop.current(generation)
            || lock.native_drop.reading
        {
            return false;
        }
        if READ_ACTIVE
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            lock.native_drop.retire();
            return false;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        lock.native_drop.cancel = Some(cancel.clone());
        lock.native_drop.reading = true;
        (
            lock.native_drop.source.clone().unwrap(),
            cancel,
            lock.background_executor.clone(),
            lock.foreground_executor.clone(),
            lock.native_drop.dropped,
        )
    };
    let weak = Rc::downgrade(window);
    let lease = Lease;
    let read = background.spawn(async move { (reader(source, image, &cancel), lease) });
    foreground
        .spawn(async move {
            let (value, lease) = read.await;
            let Some(window) = weak.upgrade() else {
                return;
            };
            let mut lock = window.lock();
            if lock.closed.load(Ordering::Acquire) || !lock.native_drop.current(generation) {
                return;
            }
            let position = lock.native_drop.position;
            let data = value.and_then(|value| match value.data {
                NativeDropData::Image { format, bytes } => {
                    lock.native_drop.image = true;
                    let needs_image = bytes.is_none();
                    Some((
                        FileDropEvent::ImageEntered {
                            position,
                            image: gpui::ExternalImageDrop { format, bytes },
                        },
                        needs_image,
                    ))
                }
                NativeDropData::UriList(bytes) => {
                    lock.native_drop.image = false;
                    match gpui::NativeDropMetadata::parse(&bytes).ok()? {
                        gpui::NativeDropMetadata::Files(paths) => {
                            Some((FileDropEvent::Entered { position, paths }, false))
                        }
                        gpui::NativeDropMetadata::Url(url) => {
                            Some((FileDropEvent::UrlEntered { position, url }, false))
                        }
                    }
                }
            });
            let Some((entered, needs_image)) = data else {
                lock.native_drop.retire();
                drop(lock);
                send_file_drop_event(window, FileDropEvent::Exited);
                return;
            };
            lock.native_drop.reading = false;
            lock.native_drop.cancel.take();
            lock.native_drop.ready = true;
            let dropped = lock.native_drop.dropped;
            let image = lock.native_drop.image;
            let submit = dropped && final_read && !needs_image;
            if submit {
                lock.native_drop.retire();
            }
            let terminal_generation = lock.native_drop.generation;
            drop(lock);
            send_file_drop_event(window.clone(), entered);
            let current = {
                let lock = window.lock();
                !lock.closed.load(Ordering::Acquire)
                    && lock.native_drop.generation == terminal_generation
            };
            if submit && current {
                send_file_drop_event(window.clone(), FileDropEvent::Submit { position });
            }
            // The sole body lease survives completed acquisition through delivery/discard.
            drop(lease);
            if dropped && (!final_read || needs_image) {
                start_read(&window, generation, image);
            }
        })
        .detach();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_drop_retirement_cancels_and_never_reuses_a_generation() {
        let mut state = State::default();
        let cancel = Arc::new(AtomicBool::new(false));
        state.cancel = Some(cancel.clone());
        state.sequence = Some(17);
        state.source = Some(NativeDropSource::MacPasteboard {
            name: "private-drag-fixture".into(),
            change_count: 1,
        });
        let generation = state.generation;
        assert!(state.current(generation));
        state.retire();
        assert!(cancel.load(Ordering::Acquire));
        assert!(!state.current(generation));
        assert!(state.source.is_none());
    }
}
