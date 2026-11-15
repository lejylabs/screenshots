//! Native fd-driven X11 task hotkeys, with no periodic idle polling.
use super::{Host, Key, Wake};
use anyhow::{Context, Result, ensure};
use std::{
    collections::HashSet,
    ffi::{CString, c_char, c_int, c_long, c_uint, c_ulong, c_void},
    io::Write,
    os::{fd::AsRawFd, unix::net::UnixStream},
    ptr,
    sync::{
        Arc,
        atomic::{AtomicI32, Ordering},
    },
};

type Display = c_void;
type Window = c_ulong;
#[repr(C)]
struct KeyEvent {
    kind: c_int,
    serial: c_ulong,
    send_event: c_int,
    display: *mut Display,
    window: Window,
    root: Window,
    subwindow: Window,
    time: c_ulong,
    x: c_int,
    y: c_int,
    x_root: c_int,
    y_root: c_int,
    state: c_uint,
    keycode: c_uint,
    same_screen: c_int,
}
#[repr(C)]
union Event {
    kind: c_int,
    key: std::mem::ManuallyDrop<KeyEvent>,
    pad: [c_long; 24],
}
#[repr(C)]
struct Error {
    kind: c_int,
    display: *mut Display,
    resource: c_ulong,
    serial: c_ulong,
    code: u8,
    request: u8,
    minor: u8,
}
#[repr(C)]
struct ModifierMap {
    max_keypermod: c_int,
    modifiermap: *mut u8,
}
type ErrorHandler = unsafe extern "C" fn(*mut Display, *mut c_void) -> c_int;
static ERROR: AtomicI32 = AtomicI32::new(0);
unsafe extern "C" fn error(_: *mut Display, value: *mut c_void) -> c_int {
    let value = value.cast::<Error>();
    if !value.is_null() {
        ERROR.store(unsafe { (*value).code } as i32, Ordering::Relaxed);
    }
    0
}
#[link(name = "X11")]
unsafe extern "C" {
    fn XOpenDisplay(name: *const c_char) -> *mut Display;
    fn XCloseDisplay(display: *mut Display) -> c_int;
    fn XDefaultScreen(display: *mut Display) -> c_int;
    fn XRootWindow(display: *mut Display, screen: c_int) -> Window;
    fn XConnectionNumber(display: *mut Display) -> c_int;
    fn XSetErrorHandler(handler: Option<ErrorHandler>) -> Option<ErrorHandler>;
    fn XSync(display: *mut Display, discard: c_int) -> c_int;
    fn XStringToKeysym(name: *const c_char) -> c_ulong;
    fn XKeysymToKeycode(display: *mut Display, symbol: c_ulong) -> u8;
    fn XGrabKey(
        display: *mut Display,
        key: c_int,
        modifiers: c_uint,
        window: Window,
        owner_events: c_int,
        pointer_mode: c_int,
        keyboard_mode: c_int,
    ) -> c_int;
    fn XUngrabKey(display: *mut Display, key: c_int, modifiers: c_uint, window: Window) -> c_int;
    fn XPending(display: *mut Display) -> c_int;
    fn XNextEvent(display: *mut Display, event: *mut Event) -> c_int;
    fn XGetModifierMapping(display: *mut Display) -> *mut ModifierMap;
    fn XFreeModifiermap(map: *mut ModifierMap) -> c_int;
    fn XkbSetDetectableAutoRepeat(
        display: *mut Display,
        detectable: c_int,
        supported: *mut c_int,
    ) -> c_int;
}
fn modifiers(key: &Key) -> u32 {
    (if key.modifiers & 1 != 0 { 4 } else { 0 })
        | (if key.modifiers & 2 != 0 { 8 } else { 0 })
        | (if key.modifiers & 4 != 0 { 1 } else { 0 })
        | (if key.modifiers & 8 != 0 { 64 } else { 0 })
}
fn keyboard_state(state: u32, locks: u32) -> u32 {
    state & 0xff & !locks
}

pub(super) struct Native {
    host: Arc<Host>,
    display: *mut Display,
    root: Window,
    previous_handler: Option<ErrorHandler>,
    grabs: Vec<(u8, u32)>,
    keys: Vec<(u8, u32, usize)>,
    lock_mask: u32,
    read: UnixStream,
    write: Arc<UnixStream>,
}
impl Native {
    pub(super) fn new(host: Arc<Host>) -> Result<Self> {
        crate::region_analysis::require_x11(
            std::env::var_os("DISPLAY").as_deref(),
            std::env::var_os("XDG_SESSION_TYPE").as_deref(),
            std::env::var_os("WAYLAND_DISPLAY").as_deref(),
        )?;
        let name = std::env::var("DISPLAY").context("Native task hotkeys require DISPLAY")?;
        ensure!(
            name.strip_prefix(':').is_some_and(|value| !value.is_empty()
                && value.len() <= 63
                && value.split('.').count() <= 2
                && value
                    .split('.')
                    .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))),
            "Task hotkeys require a local numeric X11 display"
        );
        let (read, write) = UnixStream::pair()?;
        write.set_nonblocking(true)?;
        let display = unsafe { XOpenDisplay(ptr::null()) };
        ensure!(!display.is_null(), "Could not connect task hotkeys to X11");
        ERROR.store(0, Ordering::Relaxed);
        let handler = unsafe { XSetErrorHandler(Some(error)) };
        let root = unsafe { XRootWindow(display, XDefaultScreen(display)) };
        let mut native = Self {
            host,
            display,
            root,
            previous_handler: handler,
            grabs: Vec::new(),
            keys: Vec::new(),
            lock_mask: 2,
            read,
            write: Arc::new(write),
        };
        let connection = unsafe { XConnectionNumber(display) };
        let flags = unsafe { libc::fcntl(connection, libc::F_GETFD) };
        ensure!(
            flags >= 0
                && unsafe { libc::fcntl(connection, libc::F_SETFD, flags | libc::FD_CLOEXEC) } == 0,
            "Could not protect the task-hotkey connection from child inheritance"
        );
        let mut supported = 0;
        unsafe {
            XkbSetDetectableAutoRepeat(display, 1, &mut supported);
        }
        native.sync()?;
        ensure!(
            supported != 0,
            "X11 task hotkeys require detectable autorepeat support"
        );
        let map = unsafe { XGetModifierMapping(display) };
        ensure!(!map.is_null(), "Could not inspect X11 lock modifiers");
        let mut locks = vec![2u32];
        let max = unsafe { (*map).max_keypermod };
        let valid = (1..=256).contains(&max) && unsafe { !(*map).modifiermap.is_null() };
        if valid {
            for symbol in [0xff7f, 0xff14] {
                let code = unsafe { XKeysymToKeycode(display, symbol) };
                if code == 0 {
                    continue;
                }
                for index in 0..8usize {
                    let keys = unsafe {
                        std::slice::from_raw_parts(
                            (*map).modifiermap.add(index * max as usize),
                            max as usize,
                        )
                    };
                    if keys.contains(&code) {
                        locks.push(1 << index);
                    }
                }
            }
        }
        unsafe {
            XFreeModifiermap(map);
        }
        native.sync()?;
        ensure!(valid, "X11 modifier map is invalid");
        locks.sort_unstable();
        locks.dedup();
        ensure!(locks.len() <= 3, "X11 lock modifier map is unsupported");
        let mut variants = vec![0u32];
        for mask in locks {
            native.lock_mask |= mask;
            let extra = variants
                .iter()
                .map(|value| *value | mask)
                .collect::<Vec<_>>();
            variants.extend(extra);
        }
        variants.sort_unstable();
        variants.dedup();
        for key in &native.host.config.keys {
            let symbol = CString::new(if key.name == "PrintScreen" {
                "Print"
            } else {
                &key.name
            })?;
            let code = unsafe { XKeysymToKeycode(display, XStringToKeysym(symbol.as_ptr())) };
            ensure!(
                code != 0,
                "Configured task hotkey is unavailable in this X11 keymap"
            );
            let mods = modifiers(key);
            ensure!(
                mods & native.lock_mask == 0,
                "Task shortcut modifier overlaps a lock modifier in this X11 keymap"
            );
            for lock in &variants {
                unsafe {
                    XGrabKey(display, code.into(), mods | lock, root, 0, 1, 1);
                }
                native.grabs.push((code, mods | lock));
                native.sync()?;
            }
            native.keys.push((code, mods, key.profile));
        }
        Ok(native)
    }
    fn sync(&self) -> Result<()> {
        unsafe {
            XSync(self.display, 0);
        }
        ensure!(
            ERROR.swap(0, Ordering::Relaxed) == 0,
            "A configured X11 task hotkey is unavailable or conflicted"
        );
        Ok(())
    }
    pub(super) fn wake(&self) -> Wake {
        let pipe = self.write.clone();
        Arc::new(move || {
            let _ = (&*pipe).write(&[1]);
        })
    }
    pub(super) fn run(&mut self) -> Result<()> {
        let mut down = HashSet::new();
        while !self.host.stopping.load(Ordering::SeqCst) {
            while unsafe { XPending(self.display) } > 0 {
                let mut event = Event { pad: [0; 24] };
                unsafe {
                    XNextEvent(self.display, &mut event);
                }
                let kind = unsafe { event.kind };
                if kind == 3 {
                    down.remove(&unsafe { event.key.keycode });
                }
                if kind == 2 {
                    let code = unsafe { event.key.keycode };
                    let state = keyboard_state(unsafe { event.key.state }, self.lock_mask);
                    if down.insert(code) {
                        if let Some((_, _, profile)) = self
                            .keys
                            .iter()
                            .find(|(key, mods, _)| u32::from(*key) == code && *mods == state)
                        {
                            let _ = self.host.trigger(*profile);
                        }
                    }
                }
            }
            self.sync()?;
            if self.host.stopping.load(Ordering::SeqCst) {
                break;
            }
            let mut descriptors = [
                libc::pollfd {
                    fd: unsafe { XConnectionNumber(self.display) },
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: self.read.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let ready = unsafe { libc::poll(descriptors.as_mut_ptr(), 2, -1) };
            if ready < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                anyhow::bail!("X11 task hotkey wait failed");
            }
            if descriptors[1].revents != 0 {
                break;
            }
            ensure!(
                descriptors[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) == 0,
                "X11 task hotkey connection closed"
            );
        }
        Ok(())
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        unsafe {
            for (key, mods) in &self.grabs {
                XUngrabKey(self.display, (*key).into(), *mods, self.root);
            }
            XSync(self.display, 0);
            XSetErrorHandler(self.previous_handler);
            XCloseDisplay(self.display);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn pointer_buttons_and_xkb_group_bits_do_not_change_keyboard_chords() {
        assert_eq!(
            super::keyboard_state(4 | 1 | 2 | 16 | (1 << 8) | (1 << 13), 2 | 16),
            4 | 1
        );
    }
}
