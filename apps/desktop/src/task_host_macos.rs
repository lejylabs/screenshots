//! Carbon task hotkeys with an explicit native run-loop wake source.
use super::{Host, Wake};
use anyhow::{Result, ensure};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use std::{
    collections::HashSet,
    ffi::c_void,
    sync::atomic::Ordering,
    sync::{Arc, Mutex, OnceLock, Weak},
};

struct Dispatch {
    host: Weak<Host>,
    keys: Vec<(u32, usize)>,
    down: HashSet<u32>,
}
static DISPATCH: OnceLock<Mutex<Option<Dispatch>>> = OnceLock::new();
static HANDLER: std::sync::Once = std::sync::Once::new();
#[repr(C)]
struct SourceContext {
    version: isize,
    info: *mut c_void,
    retain: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
    release: Option<unsafe extern "C" fn(*const c_void)>,
    description: Option<unsafe extern "C" fn(*const c_void) -> *const c_void>,
    equal: Option<unsafe extern "C" fn(*const c_void, *const c_void) -> u8>,
    hash: Option<unsafe extern "C" fn(*const c_void) -> usize>,
    schedule: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void)>,
    cancel: Option<unsafe extern "C" fn(*mut c_void, *mut c_void, *const c_void)>,
    perform: Option<unsafe extern "C" fn(*mut c_void)>,
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFRunLoopCommonModes: *const c_void;
    fn CFRunLoopGetMain() -> *mut c_void;
    fn CFRunLoopSourceCreate(
        allocator: *const c_void,
        order: isize,
        context: *mut SourceContext,
    ) -> *mut c_void;
    fn CFRunLoopAddSource(runloop: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRunLoopRemoveSource(runloop: *mut c_void, source: *mut c_void, mode: *const c_void);
    fn CFRunLoopSourceSignal(source: *mut c_void);
    fn CFRunLoopWakeUp(runloop: *mut c_void);
    fn CFRelease(value: *const c_void);
}
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn RunApplicationEventLoop();
    fn QuitApplicationEventLoop();
}
unsafe extern "C" fn stop_loop(_: *mut c_void) {
    unsafe {
        QuitApplicationEventLoop();
    }
}
struct Source {
    source: usize,
    runloop: usize,
}
impl Drop for Source {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.source as *const c_void);
        }
    }
}
pub(super) struct Native {
    host: Arc<Host>,
    manager: GlobalHotKeyManager,
    keys: Vec<HotKey>,
    source: Arc<Source>,
}
impl Native {
    pub(super) fn new(host: Arc<Host>) -> Result<Self> {
        let manager = GlobalHotKeyManager::new()?;
        let mut context = SourceContext {
            version: 0,
            info: std::ptr::null_mut(),
            retain: None,
            release: None,
            description: None,
            equal: None,
            hash: None,
            schedule: None,
            cancel: None,
            perform: Some(stop_loop),
        };
        let source = unsafe { CFRunLoopSourceCreate(std::ptr::null(), 0, &mut context) };
        ensure!(
            !source.is_null(),
            "Could not create task-host run-loop source"
        );
        let runloop = unsafe { CFRunLoopGetMain() };
        let source = Arc::new(Source {
            source: source as usize,
            runloop: runloop as usize,
        });
        unsafe {
            CFRunLoopAddSource(runloop, source.source as *mut c_void, kCFRunLoopCommonModes);
        }
        let mut native = Self {
            host,
            manager,
            keys: Vec::new(),
            source,
        };
        for binding in &native.host.config.keys {
            let key: HotKey = binding.normalized.parse()?;
            native.manager.register(key)?;
            native.keys.push(key);
        }
        *DISPATCH.get_or_init(|| Mutex::new(None)).lock().unwrap() = Some(Dispatch {
            host: Arc::downgrade(&native.host),
            keys: native
                .keys
                .iter()
                .zip(&native.host.config.keys)
                .map(|(key, binding)| (key.id(), binding.profile))
                .collect(),
            down: HashSet::new(),
        });
        HANDLER.call_once(|| {
            GlobalHotKeyEvent::set_event_handler(Some(|event: GlobalHotKeyEvent| {
                let admitted = DISPATCH.get().and_then(|entry| {
                    let mut entry = entry.lock().ok()?;
                    let dispatch = entry.as_mut()?;
                    if event.state == HotKeyState::Released {
                        dispatch.down.remove(&event.id);
                        return None;
                    }
                    if !dispatch.down.insert(event.id) {
                        return None;
                    }
                    let profile = dispatch.keys.iter().find(|(id, _)| *id == event.id)?.1;
                    Some((dispatch.host.upgrade()?, profile))
                });
                if let Some((host, profile)) = admitted {
                    let _ = host.trigger(profile);
                }
            }))
        });
        Ok(native)
    }
    pub(super) fn wake(&self) -> Wake {
        let source = self.source.clone();
        Arc::new(move || unsafe {
            CFRunLoopSourceSignal(source.source as *mut c_void);
            CFRunLoopWakeUp(source.runloop as *mut c_void);
        })
    }
    pub(super) fn run(&mut self) -> Result<()> {
        if !self.host.stopping.load(Ordering::SeqCst) {
            unsafe {
                RunApplicationEventLoop();
            }
        }
        Ok(())
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        if let Some(dispatch) = DISPATCH.get() {
            *dispatch.lock().unwrap() = None;
        }
        for key in &self.keys {
            let _ = self.manager.unregister(*key);
        }
        unsafe {
            CFRunLoopRemoveSource(
                self.source.runloop as *mut c_void,
                self.source.source as *mut c_void,
                kCFRunLoopCommonModes,
            );
        }
    }
}
