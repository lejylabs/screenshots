//! One-attempt queue access and coalesced native notification for file logging.
use super::{Message, State};
use crossbeam_queue::ArrayQueue;
use std::{
    cell::UnsafeCell,
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SendError {
    Busy,
    Full,
}
/// A refused send hands the message back so a caller may retry it.
pub(super) type Rejected = (SendError, Message);
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ReceiveError {
    Busy,
    Empty,
}

pub(super) struct Mailbox {
    occupied: AtomicBool,
    queue: UnsafeCell<ArrayQueue<Message>>,
    wake: Wake,
    pub(super) state: Arc<State>,
    #[cfg(test)]
    pub(super) native_waits: std::sync::atomic::AtomicU64,
}
// SAFETY: Every access to `queue` goes through Gate's single strong CAS. Only
// that guard can obtain an exclusive mutable borrow, and it releases before any
// notification, I/O, formatting or wait. Message is Send; the queue never lends
// an element/reference beyond that exclusive borrow. A failed attempt never
// reads the UnsafeCell or retries. No shared ArrayQueue operation is used.
unsafe impl Sync for Mailbox where Message: Send {}

struct Gate<'a> {
    mailbox: &'a Mailbox,
    notify: bool,
}
impl Gate<'_> {
    fn queue(&mut self) -> &mut ArrayQueue<Message> {
        // SAFETY: This unique guard owns occupied until its Drop releases it.
        unsafe { &mut *self.mailbox.queue.get() }
    }
}
impl Drop for Gate<'_> {
    fn drop(&mut self) {
        self.mailbox.occupied.store(false, Ordering::Release);
        if self.notify {
            self.mailbox.signal();
        }
    }
}

impl Mailbox {
    pub(super) fn new(capacity: usize, state: Arc<State>) -> io::Result<Arc<Self>> {
        Ok(Arc::new(Self {
            occupied: AtomicBool::new(false),
            queue: UnsafeCell::new(ArrayQueue::new(capacity)),
            wake: Wake::new()?,
            state,
            #[cfg(test)]
            native_waits: std::sync::atomic::AtomicU64::new(0),
        }))
    }
    fn gate(&self, notify: bool) -> Option<Gate<'_>> {
        self.occupied
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Gate {
                mailbox: self,
                notify,
            })
    }
    pub(super) fn try_send(&self, message: Message) -> Result<(), Rejected> {
        let Some(mut gate) = self.gate(true) else {
            return Err((SendError::Busy, message));
        };
        // Pinned ArrayQueue0.3.14 push_mut is a finite, allocation-free operation;
        // shared push/pop APIs have retry loops and must not be used here.
        gate.queue().push_mut(message).map_err(|message| (SendError::Full, message))
    }
    pub(super) fn try_recv(&self) -> Result<Message, ReceiveError> {
        let Some(mut gate) = self.gate(false) else {
            return Err(ReceiveError::Busy);
        };
        gate.queue().pop_mut().ok_or(ReceiveError::Empty)
    }
    pub(super) fn signal(&self) {
        // One fixed retry also handles an interrupted native signal. There is
        // never a producer retry loop, allocation, mutex or wait-for-space.
        if !self.wake.notify() && !self.wake.notify() {
            self.state.accepting.store(false, Ordering::SeqCst);
            self.state.stopping.store(true, Ordering::SeqCst);
            self.state.failed.store(true, Ordering::Release);
        }
    }
    pub(super) fn wait(&self) -> io::Result<()> {
        #[cfg(test)]
        self.native_waits.fetch_add(1, Ordering::Release);
        self.wake.wait()
    }
    #[cfg(test)]
    pub(super) fn hold_gate<R>(&self, body: impl FnOnce() -> R) -> R {
        let _gate = self.gate(false).expect("test owns exclusive queue access");
        body()
    }
    #[cfg(test)]
    pub(super) fn signal_count(&self) -> u64 {
        self.wake.native_signals.load(Ordering::Acquire)
    }
    #[cfg(test)]
    pub(super) fn fail_next_signals(&self, count: usize) {
        self.wake.fail_signals.store(count, Ordering::SeqCst);
    }
    #[cfg(test)]
    pub(super) fn notification_pending(&self) -> bool {
        self.wake.pending.load(Ordering::Acquire)
    }
}

struct Wake {
    native: native::Event,
    pending: AtomicBool,
    #[cfg(test)]
    native_signals: std::sync::atomic::AtomicU64,
    #[cfg(test)]
    fail_signals: std::sync::atomic::AtomicUsize,
}
impl Wake {
    fn new() -> io::Result<Self> {
        let native = native::Event::new()?;
        // Warm native signal/wait before a GPUI callback can enter this layer.
        // A preposted credit makes the initialization wait immediately ready.
        if !native.signal() {
            return Err(io::ErrorKind::Other.into());
        }
        native.wait()?;
        Ok(Self {
            native,
            pending: AtomicBool::new(false),
            #[cfg(test)]
            native_signals: std::sync::atomic::AtomicU64::new(0),
            #[cfg(test)]
            fail_signals: std::sync::atomic::AtomicUsize::new(0),
        })
    }
    fn notify(&self) -> bool {
        // Publication precedes this RMW. A consumer clearing pending after the
        // native wake acquires that publication and rechecks the queue. A later
        // producer sees false and leaves a new native credit before the next wait.
        if self.pending.swap(true, Ordering::AcqRel) {
            return true;
        }
        #[cfg(test)]
        {
            let fail = self.fail_signals.load(Ordering::SeqCst);
            if fail != 0 {
                self.fail_signals.fetch_sub(1, Ordering::SeqCst);
                self.pending.store(false, Ordering::Release);
                return false;
            }
            self.native_signals.fetch_add(1, Ordering::Relaxed);
        }
        if self.native.signal() {
            true
        } else {
            self.pending.store(false, Ordering::Release);
            false
        }
    }
    fn wait(&self) -> io::Result<()> {
        self.native.wait()?;
        self.pending.swap(false, Ordering::AcqRel);
        Ok(())
    }
}

#[cfg(target_os = "linux")]
mod native {
    use std::{
        io,
        os::fd::{AsRawFd, FromRawFd, OwnedFd},
    };
    pub(super) struct Event(OwnedFd);
    impl Event {
        pub(super) fn new() -> io::Result<Self> {
            let raw = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
            if raw < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(unsafe { OwnedFd::from_raw_fd(raw) }))
        }
        pub(super) fn signal(&self) -> bool {
            let credit = 1_u64;
            let result = unsafe {
                libc::write(
                    self.0.as_raw_fd(),
                    (&credit as *const u64).cast(),
                    std::mem::size_of::<u64>(),
                )
            };
            result == std::mem::size_of::<u64>() as isize
                || (result < 0 && unsafe { *libc::__errno_location() } == libc::EAGAIN)
        }
        pub(super) fn wait(&self) -> io::Result<()> {
            loop {
                let mut watched = libc::pollfd {
                    fd: self.0.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                let ready = unsafe { libc::poll(&mut watched, 1, -1) };
                if ready < 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(error);
                }
                if watched.revents & libc::POLLIN == 0 {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
                let mut credit = 0_u64;
                let read = unsafe {
                    libc::read(
                        self.0.as_raw_fd(),
                        (&mut credit as *mut u64).cast(),
                        std::mem::size_of::<u64>(),
                    )
                };
                if read == std::mem::size_of::<u64>() as isize {
                    return Ok(());
                }
                let error = io::Error::last_os_error();
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock
                ) {
                    continue;
                }
                return Err(error);
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod native {
    use std::{
        io,
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    };
    use windows_sys::Win32::{Foundation::*, System::Threading::*};
    pub(super) struct Event(OwnedHandle);
    impl Event {
        pub(super) fn new() -> io::Result<Self> {
            let raw = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
            if raw.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(unsafe { OwnedHandle::from_raw_handle(raw) }))
        }
        pub(super) fn signal(&self) -> bool {
            unsafe { SetEvent(self.0.as_raw_handle()) != 0 }
        }
        pub(super) fn wait(&self) -> io::Result<()> {
            if unsafe { WaitForSingleObject(self.0.as_raw_handle(), INFINITE) } == WAIT_OBJECT_0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }
    }
}

#[cfg(target_os = "macos")]
mod native {
    use std::io;
    // Public Mach ABI: semaphore.h/task.defs. libSystem signal/wait are direct traps.
    unsafe extern "C" {
        fn semaphore_create(task: u32, semaphore: *mut u32, policy: i32, value: i32) -> i32;
        fn semaphore_destroy(task: u32, semaphore: u32) -> i32;
        fn semaphore_signal(semaphore: u32) -> i32;
        fn semaphore_wait(semaphore: u32) -> i32;
    }
    pub(super) struct Event(u32);
    impl Event {
        pub(super) fn new() -> io::Result<Self> {
            let mut semaphore = 0;
            let status = unsafe { semaphore_create(libc::mach_task_self(), &mut semaphore, 0, 0) };
            if status != libc::KERN_SUCCESS {
                return Err(io::ErrorKind::Other.into());
            }
            Ok(Self(semaphore))
        }
        pub(super) fn signal(&self) -> bool {
            unsafe { semaphore_signal(self.0) == libc::KERN_SUCCESS }
        }
        pub(super) fn wait(&self) -> io::Result<()> {
            loop {
                match unsafe { semaphore_wait(self.0) } {
                    libc::KERN_SUCCESS => return Ok(()),
                    libc::KERN_ABORTED => continue,
                    _ => return Err(io::ErrorKind::Other.into()),
                }
            }
        }
    }
    impl Drop for Event {
        fn drop(&mut self) {
            let _ = unsafe { semaphore_destroy(libc::mach_task_self(), self.0) };
        }
    }
}
