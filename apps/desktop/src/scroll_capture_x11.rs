//! Targeted X11 wheel messages on an operation-owned connection.

use super::{Geometry, Region};
use anyhow::{Context, Result, ensure};
use std::{
    ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void},
    ptr,
    sync::{
        Mutex, MutexGuard,
        atomic::{AtomicI32, Ordering},
    },
};

type Display = c_void;
type Window = c_ulong;
type ErrorHandler = unsafe extern "C" fn(*mut Display, *mut c_void) -> c_int;
static CONNECTION: Mutex<()> = Mutex::new(());
static X_ERROR: AtomicI32 = AtomicI32::new(0);

#[repr(C)]
struct XErrorEvent {
    kind: c_int,
    display: *mut Display,
    resource: c_ulong,
    serial: c_ulong,
    error: u8,
    request: u8,
    minor: u8,
}
#[repr(C)]
struct Attributes {
    x: c_int,
    y: c_int,
    width: c_int,
    height: c_int,
    border_width: c_int,
    depth: c_int,
    visual: *mut c_void,
    root: Window,
    class: c_int,
    bit_gravity: c_int,
    win_gravity: c_int,
    backing_store: c_int,
    backing_planes: c_ulong,
    backing_pixel: c_ulong,
    save_under: c_int,
    colormap: c_ulong,
    map_installed: c_int,
    map_state: c_int,
    all_event_masks: c_long,
    your_event_mask: c_long,
    do_not_propagate_mask: c_long,
    override_redirect: c_int,
    screen: *mut c_void,
}
#[repr(C)]
struct ButtonEvent {
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
    button: c_uint,
    same_screen: c_int,
}
#[repr(C)]
union Event {
    button: std::mem::ManuallyDrop<ButtonEvent>,
    pad: [c_long; 24],
}

#[link(name = "X11")]
unsafe extern "C" {
    fn XOpenDisplay(name: *const c_char) -> *mut Display;
    fn XCloseDisplay(display: *mut Display) -> c_int;
    fn XSync(display: *mut Display, discard: c_int) -> c_int;
    fn XSetErrorHandler(handler: Option<ErrorHandler>) -> Option<ErrorHandler>;
    fn XGetWindowAttributes(
        display: *mut Display,
        window: Window,
        attributes: *mut Attributes,
    ) -> c_int;
    fn XTranslateCoordinates(
        display: *mut Display,
        source: Window,
        destination: Window,
        x: c_int,
        y: c_int,
        translated_x: *mut c_int,
        translated_y: *mut c_int,
        child: *mut Window,
    ) -> c_int;
    fn XGetInputFocus(display: *mut Display, focus: *mut Window, revert: *mut c_int) -> c_int;
    fn XQueryTree(
        display: *mut Display,
        window: Window,
        root: *mut Window,
        parent: *mut Window,
        children: *mut *mut Window,
        count: *mut c_uint,
    ) -> c_int;
    fn XFree(data: *mut c_void) -> c_int;
    fn XInternAtom(display: *mut Display, name: *const c_char, only_if_exists: c_int) -> c_ulong;
    fn XGetWindowProperty(
        display: *mut Display,
        window: Window,
        property: c_ulong,
        offset: c_long,
        length: c_long,
        delete: c_int,
        requested: c_ulong,
        actual: *mut c_ulong,
        format: *mut c_int,
        items: *mut c_ulong,
        after: *mut c_ulong,
        data: *mut *mut u8,
    ) -> c_int;
    fn XSendEvent(
        display: *mut Display,
        window: Window,
        propagate: c_int,
        mask: c_long,
        event: *mut Event,
    ) -> c_int;
}

unsafe extern "C" fn error_handler(_: *mut Display, event: *mut c_void) -> c_int {
    let event = event.cast::<XErrorEvent>();
    if !event.is_null() {
        X_ERROR.store(unsafe { (*event).error } as i32, Ordering::Relaxed);
    }
    0
}

pub(super) struct Native {
    display: *mut Display,
    window: Window,
    handler: Option<ErrorHandler>,
    _guard: MutexGuard<'static, ()>,
}

impl Native {
    pub(super) fn new(id: u32) -> Result<Self> {
        crate::region_analysis::require_x11(
            std::env::var_os("DISPLAY").as_deref(),
            std::env::var_os("XDG_SESSION_TYPE").as_deref(),
            std::env::var_os("WAYLAND_DISPLAY").as_deref(),
        )?;
        let display_name = std::env::var("DISPLAY").context("X11 display is unavailable")?;
        let local = display_name.strip_prefix(':').is_some_and(|value| {
            !value.is_empty()
                && value.len() <= 63
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'.')
                && value.split('.').count() <= 2
                && value.split('.').all(|part| !part.is_empty())
        });
        ensure!(local, "Scrolling requires a local numeric X11 display");
        let guard = CONNECTION
            .lock()
            .map_err(|_| anyhow::anyhow!("X11 scrolling connection is unavailable"))?;
        let display = unsafe { XOpenDisplay(ptr::null()) };
        ensure!(
            !display.is_null(),
            "Could not open the X11 scrolling connection"
        );
        X_ERROR.store(0, Ordering::Relaxed);
        let handler = unsafe { XSetErrorHandler(Some(error_handler)) };
        Ok(Self {
            display,
            window: c_ulong::from(id),
            handler,
            _guard: guard,
        })
    }
    fn sync(&self) -> Result<()> {
        unsafe {
            XSync(self.display, 0);
        }
        ensure!(
            X_ERROR.swap(0, Ordering::Relaxed) == 0,
            "X11 target changed or rejected scrolling operations"
        );
        Ok(())
    }
    fn focused(&self) -> Result<bool> {
        let mut focus = 0;
        let mut revert = 0;
        unsafe {
            XGetInputFocus(self.display, &mut focus, &mut revert);
        }
        self.sync()?;
        for _ in 0..64 {
            if focus == self.window {
                return Ok(true);
            }
            if focus <= 1 {
                return Ok(false);
            }
            let (mut root, mut parent, mut children, mut count) = (0, 0, ptr::null_mut(), 0);
            let ok = unsafe {
                XQueryTree(
                    self.display,
                    focus,
                    &mut root,
                    &mut parent,
                    &mut children,
                    &mut count,
                )
            };
            if !children.is_null() {
                unsafe {
                    XFree(children.cast());
                }
            }
            self.sync()?;
            if ok == 0 || parent == focus || parent == 0 {
                return Ok(false);
            }
            focus = parent;
        }
        Ok(false)
    }
    pub(super) fn geometry(&self) -> Result<Geometry> {
        let mut attrs: Attributes = unsafe { std::mem::zeroed() };
        let ok = unsafe { XGetWindowAttributes(self.display, self.window, &mut attrs) };
        self.sync()?;
        ensure!(
            ok != 0 && attrs.map_state == 2 && attrs.width > 0 && attrs.height > 0,
            "Scrolling target is unavailable or not viewable"
        );
        ensure!(self.focused()?, "Scrolling target lost keyboard focus");
        let (mut x, mut y, mut child) = (0, 0, 0);
        let ok = unsafe {
            XTranslateCoordinates(
                self.display,
                self.window,
                attrs.root,
                0,
                0,
                &mut x,
                &mut y,
                &mut child,
            )
        };
        self.sync()?;
        ensure!(ok != 0, "Could not establish scrolling window coordinates");
        let atom = unsafe { XInternAtom(self.display, c"_NET_WM_PID".as_ptr(), 1) };
        let (mut actual, mut format, mut items, mut after, mut data) =
            (0, 0, 0, 0, ptr::null_mut());
        let status = unsafe {
            XGetWindowProperty(
                self.display,
                self.window,
                atom,
                0,
                1,
                0,
                6,
                &mut actual,
                &mut format,
                &mut items,
                &mut after,
                &mut data,
            )
        };
        let pid = if status == 0
            && actual == 6
            && format == 32
            && items == 1
            && after == 0
            && !data.is_null()
        {
            unsafe { *data.cast::<c_ulong>() }
        } else {
            0
        };
        if !data.is_null() {
            unsafe {
                XFree(data.cast());
            }
        }
        self.sync()?;
        let pid = u32::try_from(pid).context("Scrolling target process ID is invalid")?;
        ensure!(
            pid != 0 && pid != std::process::id(),
            "Scrolling requires a different identified application window"
        );
        Ok(Geometry {
            x,
            y,
            width: attrs.width as u32,
            height: attrs.height as u32,
            pid,
        })
    }
    pub(super) fn wheel(&self, expected: Geometry, region: Region, notches: u32) -> Result<()> {
        ensure!(
            self.geometry()? == expected,
            "Scrolling target geometry or process changed"
        );
        let x = i32::try_from(region.x + region.width / 2)?;
        let y = i32::try_from(region.y + region.height / 2)?;
        let x_root = expected
            .x
            .checked_add(x)
            .context("Scrolling point overflow")?;
        let y_root = expected
            .y
            .checked_add(y)
            .context("Scrolling point overflow")?;
        ensure!(
            i16::try_from(x).is_ok()
                && i16::try_from(y).is_ok()
                && i16::try_from(x_root).is_ok()
                && i16::try_from(y_root).is_ok(),
            "Scrolling point exceeds X11 wheel coordinates"
        );
        let mut attrs: Attributes = unsafe { std::mem::zeroed() };
        let ok = unsafe { XGetWindowAttributes(self.display, self.window, &mut attrs) };
        self.sync()?;
        ensure!(ok != 0, "Scrolling target is unavailable");
        for _ in 0..notches {
            for (kind, mask, state) in [(4, 1 << 2, 0), (5, 1 << 3, 1 << 12)] {
                let event = ButtonEvent {
                    kind,
                    serial: 0,
                    send_event: 1,
                    display: self.display,
                    window: self.window,
                    root: attrs.root,
                    subwindow: 0,
                    time: 0,
                    x,
                    y,
                    x_root,
                    y_root,
                    state,
                    button: 5,
                    same_screen: 1,
                };
                let mut event = Event {
                    button: std::mem::ManuallyDrop::new(event),
                };
                let sent = unsafe { XSendEvent(self.display, self.window, 0, mask, &mut event) };
                self.sync()?;
                ensure!(sent != 0, "X11 target refused the scroll message");
            }
        }
        Ok(())
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        unsafe {
            XSync(self.display, 0);
            XSetErrorHandler(self.handler);
            XCloseDisplay(self.display);
        }
    }
}
