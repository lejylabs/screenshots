//! Finite native completion wakeup; no periodic run-loop timer or idle service.
use anyhow::{Result, ensure};
use core_foundation::{
    base::{CFRelease, CFRetain},
    runloop::{
        CFRunLoopAddSource, CFRunLoopGetCurrent, CFRunLoopRef, CFRunLoopRemoveSource,
        CFRunLoopRunInMode, CFRunLoopSourceContext, CFRunLoopSourceCreate,
        CFRunLoopSourceInvalidate, CFRunLoopSourceRef, CFRunLoopSourceSignal, CFRunLoopWakeUp,
        kCFRunLoopDefaultMode,
    },
};
use std::{
    ffi::c_void,
    ptr,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};
static TOPOLOGY_CHANGED: AtomicBool = AtomicBool::new(false);
static TOPOLOGY_WAKE: OnceLock<Arc<Wake>> = OnceLock::new();
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGDisplayRegisterReconfigurationCallback(
        callback: extern "C" fn(u32, u32, *mut c_void),
        context: *mut c_void,
    ) -> i32;
    fn CGDisplayRemoveReconfigurationCallback(
        callback: extern "C" fn(u32, u32, *mut c_void),
        context: *mut c_void,
    ) -> i32;
}
extern "C" fn topology(_: u32, _: u32, _: *mut c_void) {
    TOPOLOGY_CHANGED.store(true, Ordering::Release);
    if let Some(wake) = TOPOLOGY_WAKE.get() {
        wake.notify();
    }
}
pub(super) struct Topology;
impl Topology {
    pub fn new(wake: Arc<Wake>) -> Result<Self> {
        // Helpers handle one operation. Keep callback wake metadata until process exit:
        // CG's public remove API does not promise a drain of in-flight callbacks.
        ensure!(
            TOPOLOGY_WAKE.set(wake).is_ok(),
            "Native topology observer is already owned"
        );
        ensure!(
            unsafe { CGDisplayRegisterReconfigurationCallback(topology, ptr::null_mut()) } == 0,
            "Cannot observe native display topology"
        );
        Ok(Self)
    }
    pub fn check() -> Result<()> {
        ensure!(
            !TOPOLOGY_CHANGED.load(Ordering::Acquire),
            "Native display topology changed"
        );
        Ok(())
    }
}
impl Drop for Topology {
    fn drop(&mut self) {
        unsafe {
            CGDisplayRemoveReconfigurationCallback(topology, ptr::null_mut());
        }
    }
}
extern "C" fn perform(_: *const c_void) {}
pub(super) struct Wake {
    loop_: usize,
    source: usize,
    active: AtomicBool,
}
impl Wake {
    pub fn notify(&self) {
        if self.active.load(Ordering::Acquire) {
            unsafe {
                CFRunLoopSourceSignal(self.source as CFRunLoopSourceRef);
                CFRunLoopWakeUp(self.loop_ as CFRunLoopRef);
            }
        }
    }
}
impl Drop for Wake {
    fn drop(&mut self) {
        unsafe {
            CFRelease((self.source as CFRunLoopSourceRef).cast());
            CFRelease((self.loop_ as CFRunLoopRef).cast());
        }
    }
}
pub(super) struct EventLoop {
    wake: Arc<Wake>,
}
impl EventLoop {
    pub fn new() -> Result<Self> {
        crate::capture_macos_ax::main_thread()?;
        Self::current_thread()
    }
    #[cfg(test)]
    pub fn notification_test_loop() -> Result<Self> {
        // Private Foundation-notification tests need a real CF loop on the Rust
        // test thread; they never perform GUI/capture work or start a session.
        Self::current_thread()
    }
    fn current_thread() -> Result<Self> {
        let loop_ = unsafe { CFRunLoopGetCurrent() };
        ensure!(!loop_.is_null(), "Native run loop is unavailable");
        let mut context = CFRunLoopSourceContext {
            version: 0,
            info: ptr::null_mut(),
            retain: None,
            release: None,
            copyDescription: None,
            equal: None,
            hash: None,
            schedule: None,
            cancel: None,
            perform,
        };
        let source = unsafe { CFRunLoopSourceCreate(ptr::null(), 0, &mut context) };
        ensure!(!source.is_null(), "Cannot create native completion source");
        unsafe {
            CFRetain(loop_.cast());
            CFRunLoopAddSource(loop_, source, kCFRunLoopDefaultMode);
        }
        Ok(Self {
            wake: Arc::new(Wake {
                loop_: loop_ as usize,
                source: source as usize,
                active: AtomicBool::new(true),
            }),
        })
    }
    pub fn wake(&self) -> Arc<Wake> {
        self.wake.clone()
    }
    pub fn wait(&self, until: Instant, complete: impl Fn() -> bool) -> Result<()> {
        while !complete() {
            let remaining = until.saturating_duration_since(Instant::now());
            ensure!(!remaining.is_zero(), "Native operation deadline exceeded");
            unsafe {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, remaining.as_secs_f64(), 1);
            }
        }
        Ok(())
    }
}
impl Drop for EventLoop {
    fn drop(&mut self) {
        self.wake.active.store(false, Ordering::Release);
        unsafe {
            CFRunLoopRemoveSource(
                self.wake.loop_ as CFRunLoopRef,
                self.wake.source as CFRunLoopSourceRef,
                kCFRunLoopDefaultMode,
            );
            CFRunLoopSourceInvalidate(self.wake.source as CFRunLoopSourceRef);
        }
    }
}
