//! Explicit service-only Dispatch wall timer and separately journaled, owned IOKit wake.
use anyhow::{Result, ensure};
use std::{
    ffi::{CString, c_void},
    io::Read,
    os::{fd::AsRawFd, unix::net::UnixStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
type Object = *mut c_void;
unsafe extern "C" {
    static _dispatch_source_type_timer: u8;
    fn dispatch_queue_create(label: *const i8, attr: *const c_void) -> Object;
    fn dispatch_source_create(
        kind: *const c_void,
        handle: usize,
        mask: usize,
        queue: Object,
    ) -> Object;
    fn dispatch_set_context(object: Object, context: *mut c_void);
    fn dispatch_source_set_event_handler_f(
        object: Object,
        handler: unsafe extern "C" fn(*mut c_void),
    );
    fn dispatch_source_set_cancel_handler_f(
        object: Object,
        handler: unsafe extern "C" fn(*mut c_void),
    );
    fn dispatch_source_set_timer(source: Object, start: u64, interval: u64, leeway: u64);
    fn dispatch_walltime(time: *const libc::timespec, delta: i64) -> u64;
    fn dispatch_resume(object: Object);
    fn dispatch_source_cancel(object: Object);
    fn dispatch_release(object: Object);
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDateCreate(allocator: *const c_void, at: f64) -> Object;
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        text: *const i8,
        encoding: u32,
    ) -> Object;
    fn CFRelease(object: *const c_void);
}
#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOPMSchedulePowerEvent(at: Object, owner: Object, kind: Object) -> u32;
    fn IOPMCancelScheduledPowerEvent(at: Object, owner: Object, kind: Object) -> u32;
}
unsafe extern "C" fn fire(context: *mut c_void) {
    // Box remains owned by the source until its cancellation handler, after event handlers finish.
    let stream = unsafe { &*context.cast::<UnixStream>() };
    let byte = 1u8;
    unsafe {
        libc::write(stream.as_raw_fd(), (&byte as *const u8).cast(), 1);
    }
}
unsafe extern "C" fn release_context(context: *mut c_void) {
    drop(unsafe { Box::from_raw(context.cast::<UnixStream>()) });
}
pub(super) struct Native {
    queue: Object,
    timer: Object,
    read: UnixStream,
    _write: UnixStream,
    stop: Arc<AtomicBool>,
    handlers: Vec<signal_hook::SigId>,
}
impl Native {
    pub fn new(stop: Arc<AtomicBool>) -> Result<Self> {
        let (read, write) = UnixStream::pair()?;
        read.set_nonblocking(true)?;
        write.set_nonblocking(true)?;
        let context = Box::new(write.try_clone()?);
        let queue = unsafe {
            dispatch_queue_create(
                c"dev.spitzli.screenshots.schedule".as_ptr(),
                std::ptr::null(),
            )
        };
        ensure!(
            !queue.is_null(),
            "Could not create native scheduler timer queue"
        );
        let timer = unsafe {
            dispatch_source_create(
                std::ptr::addr_of!(_dispatch_source_type_timer).cast(),
                0,
                0,
                queue,
            )
        };
        if timer.is_null() {
            unsafe {
                dispatch_release(queue);
            }
            anyhow::bail!("Could not create native scheduler wall timer");
        }
        let context = Box::into_raw(context);
        unsafe {
            dispatch_set_context(timer, context.cast());
            dispatch_source_set_event_handler_f(timer, fire);
            dispatch_source_set_cancel_handler_f(timer, release_context);
            dispatch_source_set_timer(timer, u64::MAX, u64::MAX, 0);
            dispatch_resume(timer);
        }
        let mut native = Self {
            queue,
            timer,
            read,
            _write: write,
            stop,
            handlers: Vec::new(),
        };
        for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            native
                .handlers
                .push(signal_hook::flag::register(signal, native.stop.clone())?);
            native.handlers.push(signal_hook::low_level::pipe::register(
                signal,
                native._write.try_clone()?,
            )?);
        }
        Ok(native)
    }
    pub fn arm(&mut self, next: Option<i64>, wake: Option<i64>) -> Result<()> {
        ensure!(
            wake.is_none(),
            "macOS power wake must be journaled separately"
        );
        let start = if let Some(ms) = next {
            let time = libc::timespec {
                tv_sec: ms.div_euclid(1000),
                tv_nsec: ms.rem_euclid(1000) * 1000000,
            };
            unsafe { dispatch_walltime(&time, 0) }
        } else {
            u64::MAX
        };
        unsafe {
            dispatch_source_set_timer(self.timer, start, u64::MAX, 0);
        }
        Ok(())
    }
    pub fn wait(&mut self) -> Result<()> {
        while !self.stop.load(Ordering::Relaxed) {
            let mut fd = libc::pollfd {
                fd: self.read.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let result = unsafe { libc::poll(&mut fd, 1, -1) };
            if result < 0 {
                let e = std::io::Error::last_os_error();
                if e.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e.into());
            }
            ensure!(
                fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) == 0,
                "Native timer event pipe closed unexpectedly"
            );
            let mut bytes = [0u8; 64];
            let _ = self.read.read(&mut bytes);
            return Ok(());
        }
        Ok(())
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        for id in self.handlers.drain(..) {
            signal_hook::low_level::unregister(id);
        }
        unsafe {
            dispatch_source_cancel(self.timer);
            dispatch_release(self.timer);
            dispatch_release(self.queue);
        }
    }
}
struct Cf(Object);
impl Drop for Cf {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.0);
        }
    }
}
fn string(value: &str) -> Result<Cf> {
    let value = CString::new(value)?;
    let raw = unsafe { CFStringCreateWithCString(std::ptr::null(), value.as_ptr(), 0x08000100) };
    ensure!(!raw.is_null(), "Could not allocate owned wake string");
    Ok(Cf(raw))
}
pub(super) fn power(wake: &super::store::Wake, cancel: bool) -> Result<()> {
    let date =
        Cf(unsafe { CFDateCreate(std::ptr::null(), wake.at_ms as f64 / 1000.0 - 978307200.0) });
    ensure!(!date.0.is_null(), "Could not allocate owned wake date");
    let owner = string(&format!("dev.spitzli.screenshots.schedule.{}", wake.id))?;
    let kind = string("wake")?;
    let result = unsafe {
        if cancel {
            IOPMCancelScheduledPowerEvent(date.0, owner.0, kind.0)
        } else {
            IOPMSchedulePowerEvent(date.0, owner.0, kind.0)
        }
    };
    // kIOReturnNotFound means a crash happened before publication, or the one-shot already fired.
    ensure!(
        result == 0 || (cancel && result == 0xe00002f0),
        "Could not {} explicit native wake (IOKit 0x{result:08x}); native privilege is required, no privilege prompt was requested",
        if cancel { "cancel" } else { "arm" }
    );
    Ok(())
}
