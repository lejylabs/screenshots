//! Isolated X11 CLIPBOARD transfer. Finite properties/INCR chunks, no ownership or manager thread.
use super::*;
use anyhow::{Context, bail};
use std::{
    ffi::{CString, c_char, c_int, c_long, c_ulong, c_void},
    os::unix::ffi::OsStrExt,
};
type Display = c_void;
type Window = c_ulong;
type Atom = c_ulong;
#[repr(C)]
struct SelectionEvent {
    kind: c_int,
    serial: c_ulong,
    send_event: c_int,
    display: *mut Display,
    requestor: Window,
    selection: Atom,
    target: Atom,
    property: Atom,
    time: c_ulong,
}
#[repr(C)]
struct PropertyEvent {
    kind: c_int,
    serial: c_ulong,
    send_event: c_int,
    display: *mut Display,
    window: Window,
    atom: Atom,
    time: c_ulong,
    state: c_int,
}
#[repr(C)]
union Event {
    kind: c_int,
    selection: std::mem::ManuallyDrop<SelectionEvent>,
    property: std::mem::ManuallyDrop<PropertyEvent>,
    pad: [c_long; 24],
}
#[link(name = "X11")]
unsafe extern "C" {
    fn XOpenDisplay(name: *const c_char) -> *mut Display;
    fn XCloseDisplay(display: *mut Display) -> c_int;
    fn XDefaultRootWindow(display: *mut Display) -> Window;
    fn XCreateSimpleWindow(
        display: *mut Display,
        parent: Window,
        x: c_int,
        y: c_int,
        w: u32,
        h: u32,
        border: u32,
        color: c_ulong,
        background: c_ulong,
    ) -> Window;
    fn XDestroyWindow(display: *mut Display, window: Window) -> c_int;
    fn XSelectInput(display: *mut Display, window: Window, mask: c_long) -> c_int;
    fn XInternAtom(display: *mut Display, name: *const c_char, only_if_exists: c_int) -> Atom;
    fn XGetSelectionOwner(display: *mut Display, selection: Atom) -> Window;
    fn XConvertSelection(
        display: *mut Display,
        selection: Atom,
        target: Atom,
        property: Atom,
        window: Window,
        time: c_ulong,
    ) -> c_int;
    fn XFlush(display: *mut Display) -> c_int;
    fn XEventsQueued(display: *mut Display, mode: c_int) -> c_int;
    fn XNextEvent(display: *mut Display, event: *mut Event) -> c_int;
    fn XConnectionNumber(display: *mut Display) -> c_int;
    fn XGetWindowProperty(
        display: *mut Display,
        window: Window,
        property: Atom,
        offset: c_long,
        length: c_long,
        delete: c_int,
        requested: Atom,
        actual: *mut Atom,
        format: *mut c_int,
        count: *mut c_ulong,
        after: *mut c_ulong,
        data: *mut *mut u8,
    ) -> c_int;
    fn XDeleteProperty(display: *mut Display, window: Window, property: Atom) -> c_int;
    fn XFree(value: *mut c_void) -> c_int;
    fn XGetAtomName(display: *mut Display, atom: Atom) -> *mut c_char;
}
struct Property {
    kind: Atom,
    format: c_int,
    count: usize,
    after: usize,
    data: *mut u8,
}
impl Drop for Property {
    fn drop(&mut self) {
        if !self.data.is_null() {
            unsafe {
                XFree(self.data.cast());
            }
        }
    }
}
struct Session {
    display: *mut Display,
    window: Window,
    selection: Atom,
    property: Atom,
    targets: Atom,
    incr: Atom,
    owner: Window,
    deadline: Instant,
    events: usize,
}
impl Session {
    fn new(backend: NativeClipboardBackend, deadline: Instant) -> Result<Self> {
        if backend == NativeClipboardBackend::Automatic
            && (std::env::var_os("WAYLAND_DISPLAY").is_some()
                || std::env::var("XDG_SESSION_TYPE")
                    .is_ok_and(|v| v.eq_ignore_ascii_case("wayland")))
        {
            bail!(
                "Wayland clipboard requires focused application Paste; no ambient clipboard fallback was attempted"
            );
        }
        let display = std::env::var_os("DISPLAY")
            .ok_or_else(|| anyhow::anyhow!("X11 clipboard is unavailable"))?;
        let bytes = display.as_bytes();
        ensure!(
            bytes.len() <= 64
                && bytes.first() == Some(&b':')
                && !bytes[1..].is_empty()
                && bytes[1..].iter().all(|b| b.is_ascii_digit() || *b == b'.'),
            "X11 clipboard requires a local numeric display"
        );
        let name = CString::new(bytes)?;
        let display = unsafe { XOpenDisplay(name.as_ptr()) };
        ensure!(!display.is_null(), "Could not connect to X11 clipboard");
        let window = unsafe {
            XCreateSimpleWindow(display, XDefaultRootWindow(display), 0, 0, 1, 1, 0, 0, 0)
        };
        let mut session = Self {
            display,
            window,
            selection: 0,
            property: 0,
            targets: 0,
            incr: 0,
            owner: 0,
            deadline,
            events: 0,
        };
        ensure!(window != 0, "Could not create clipboard requestor");
        unsafe {
            XSelectInput(display, window, 1 << 22);
        }
        session.selection = session.atom("CLIPBOARD")?;
        session.property = session.atom("_SCREENSHOTS_CLIP_INPUT")?;
        session.targets = session.atom("TARGETS")?;
        session.incr = session.atom("INCR")?;
        session.owner = unsafe { XGetSelectionOwner(display, session.selection) };
        Ok(session)
    }
    fn atom(&self, name: &str) -> Result<Atom> {
        let name = CString::new(name)?;
        let atom = unsafe { XInternAtom(self.display, name.as_ptr(), 0) };
        ensure!(atom != 0, "Could not prepare clipboard atom");
        Ok(atom)
    }
    fn check(&self, check: &crate::NativeCaptureCheck<'_>) -> Result<()> {
        check()?;
        ensure!(
            Instant::now() < self.deadline,
            "Clipboard transfer deadline exceeded"
        );
        ensure!(
            unsafe { XGetSelectionOwner(self.display, self.selection) } == self.owner,
            "Clipboard owner changed during transfer"
        );
        Ok(())
    }
    fn event(&mut self, check: &crate::NativeCaptureCheck<'_>) -> Result<Event> {
        loop {
            self.check(check)?;
            ensure!(self.events < 4096, "Clipboard event bound exceeded");
            let queued = unsafe { XEventsQueued(self.display, 0) };
            if queued > 0 {
                let mut event = Event { pad: [0; 24] };
                unsafe {
                    XNextEvent(self.display, &mut event);
                }
                self.events += 1;
                return Ok(event);
            }
            let remaining = self.deadline.saturating_duration_since(Instant::now());
            let millis = remaining
                .as_millis()
                .saturating_add(1)
                .min(i32::MAX as u128) as i32;
            let mut fd = libc::pollfd {
                fd: unsafe { XConnectionNumber(self.display) },
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = unsafe { libc::poll(&mut fd, 1, millis) };
            if ready < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error.into());
            }
            ensure!(
                ready > 0 && fd.revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) == 0,
                "Clipboard transfer timed out or disconnected"
            );
            // Read only after native readiness, then consume one finite X event.
            let mut event = Event { pad: [0; 24] };
            unsafe {
                XNextEvent(self.display, &mut event);
            }
            self.events += 1;
            return Ok(event);
        }
    }
    fn property(&self, max: usize) -> Result<Property> {
        self.property_at(0, max)
    }
    fn property_at(&self, offset: usize, max: usize) -> Result<Property> {
        let (mut kind, mut format, mut count, mut after, mut data) =
            (0, 0, 0, 0, std::ptr::null_mut());
        let result = unsafe {
            XGetWindowProperty(
                self.display,
                self.window,
                self.property,
                offset as c_long,
                (max.div_ceil(4)) as c_long,
                0,
                0,
                &mut kind,
                &mut format,
                &mut count,
                &mut after,
                &mut data,
            )
        };
        let value = Property {
            kind,
            format,
            count: usize::try_from(count)?,
            after: usize::try_from(after)?,
            data,
        };
        ensure!(result == 0, "Could not read clipboard property");
        let native_bytes = value
            .count
            .checked_mul(match format {
                8 => 1,
                16 => 2,
                32 => std::mem::size_of::<c_ulong>(),
                _ => 0,
            })
            .ok_or_else(|| anyhow::anyhow!("Clipboard property size overflow"))?;
        ensure!(
            value.count == 0 || (!value.data.is_null() && native_bytes <= max.max(8) * 2),
            "Invalid clipboard property extent"
        );
        Ok(value)
    }
    fn delete(&self) {
        unsafe {
            XDeleteProperty(self.display, self.window, self.property);
            XFlush(self.display);
        }
    }
    fn begin(&mut self, target: Atom, check: &crate::NativeCaptureCheck<'_>) -> Result<()> {
        self.check(check)?;
        ensure!(self.owner != 0, "Clipboard has no owner");
        self.delete();
        unsafe {
            XConvertSelection(
                self.display,
                self.selection,
                target,
                self.property,
                self.window,
                0,
            );
            XFlush(self.display);
        }
        loop {
            let event = self.event(check)?;
            if unsafe { event.kind } == 31 {
                let event = unsafe { &event.selection };
                if event.requestor == self.window
                    && event.selection == self.selection
                    && event.target == target
                {
                    ensure!(
                        event.property == self.property,
                        "Clipboard does not provide the requested format"
                    );
                    return Ok(());
                }
            }
        }
    }
    fn targets(&mut self, check: &crate::NativeCaptureCheck<'_>) -> Result<Vec<Atom>> {
        if self.owner == 0 {
            return Ok(Vec::new());
        }
        self.begin(self.targets, check)?;
        let value = self.property(CLIPBOARD_FORMATS * 4)?;
        ensure!(
            value.kind == 4
                && value.format == 32
                && value.count <= CLIPBOARD_FORMATS
                && value.after == 0,
            "Clipboard format offer exceeds its bound or is invalid"
        );
        let result = if value.count == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(value.data.cast::<c_ulong>(), value.count) }
                .to_vec()
        };
        self.delete();
        self.check(check)?;
        Ok(result)
    }
    fn name(&self, atom: Atom) -> Result<String> {
        let raw = unsafe { XGetAtomName(self.display, atom) };
        ensure!(!raw.is_null(), "Could not inspect clipboard format");
        let result = (|| {
            let length = unsafe { libc::strnlen(raw, CLIPBOARD_FORMAT_BYTES + 1) };
            ensure!(
                length <= CLIPBOARD_FORMAT_BYTES,
                "Clipboard format label exceeds128 bytes"
            );
            let bytes = unsafe { std::slice::from_raw_parts(raw.cast::<u8>(), length) };
            let name = std::str::from_utf8(bytes)?.to_owned();
            ensure!(!name.is_empty(), "Empty clipboard format label");
            Ok(name)
        })();
        unsafe {
            XFree(raw.cast());
        }
        result
    }
    fn choose(&self, targets: &[Atom], names: &[&str]) -> Result<Atom> {
        for name in names {
            let atom = self.atom(name)?;
            if targets.contains(&atom) {
                return Ok(atom);
            }
        }
        bail!("Clipboard does not provide a supported requested format")
    }
    fn bytes(
        &mut self,
        target: Atom,
        max: usize,
        check: &crate::NativeCaptureCheck<'_>,
    ) -> Result<Vec<u8>> {
        self.begin(target, check)?;
        let initial = self.property(max.min(64 * 1024))?;
        if initial.kind != self.incr {
            ensure!(
                initial.format == 8
                    && initial
                        .count
                        .checked_add(initial.after)
                        .is_some_and(|total| total <= max),
                "Clipboard offer exceeds its bound"
            );
            let total = initial.count + initial.after;
            let expected_kind = initial.kind;
            let mut bytes = Vec::with_capacity(total.min(64 * 1024));
            if initial.count != 0 {
                bytes.extend_from_slice(unsafe {
                    std::slice::from_raw_parts(initial.data, initial.count)
                });
            }
            drop(initial);
            while bytes.len() < total {
                self.check(check)?;
                ensure!(
                    bytes.len().is_multiple_of(4),
                    "Invalid segmented clipboard property"
                );
                let part =
                    self.property_at(bytes.len() / 4, (total - bytes.len()).min(64 * 1024))?;
                ensure!(
                    part.kind == expected_kind
                        && part.format == 8
                        && part.count > 0
                        && part.count.checked_add(part.after) == Some(total - bytes.len()),
                    "Clipboard property changed during transfer"
                );
                bytes.extend_from_slice(unsafe {
                    std::slice::from_raw_parts(part.data, part.count)
                });
            }
            self.delete();
            self.check(check)?;
            return Ok(bytes);
        }
        ensure!(
            initial.format == 32
                && initial.count == 1
                && initial.after == 0
                && !initial.data.is_null(),
            "Invalid clipboard INCR offer"
        );
        let hint = unsafe { *initial.data.cast::<c_ulong>() };
        ensure!(
            hint as u64 <= max as u64,
            "Clipboard INCR offer exceeds its bound"
        );
        drop(initial);
        self.delete();
        let mut output = Vec::new();
        loop {
            let event = self.event(check)?;
            if unsafe { event.kind } != 28 {
                continue;
            }
            let event = unsafe { &event.property };
            if event.window != self.window || event.atom != self.property || event.state != 0 {
                continue;
            }
            let remaining = max.saturating_sub(output.len());
            let chunk = self.property(remaining.min(64 * 1024).saturating_add(1))?;
            ensure!(
                chunk.format == 8
                    && chunk.kind != self.incr
                    && chunk.after == 0
                    && chunk.count <= remaining
                    && chunk.count <= 64 * 1024,
                "Clipboard INCR chunk or cumulative offer exceeds its bound"
            );
            if chunk.count == 0 {
                self.delete();
                self.check(check)?;
                return Ok(output);
            }
            ensure!(!chunk.data.is_null(), "Invalid clipboard chunk");
            output
                .extend_from_slice(unsafe { std::slice::from_raw_parts(chunk.data, chunk.count) });
            drop(chunk);
            self.delete();
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        if !self.display.is_null() {
            unsafe {
                if self.window != 0 {
                    XDestroyWindow(self.display, self.window);
                }
                XCloseDisplay(self.display);
            }
        }
    }
}
pub(super) fn inspect(
    kind: ClipboardInspectKind,
    backend: NativeClipboardBackend,
    deadline: Instant,
    check: &crate::NativeCaptureCheck<'_>,
    _: &mut budget::Budget,
) -> Result<ClipboardSnapshot> {
    let mut session = Session::new(backend, deadline)?;
    let targets = session.targets(check)?;
    let snapshot = match kind {
        ClipboardInspectKind::Formats => {
            let mut formats = Vec::with_capacity(targets.len());
            for atom in targets {
                session.check(check)?;
                formats.push(session.name(atom)?);
            }
            ClipboardSnapshot::Formats { formats }
        }
        ClipboardInspectKind::Text => {
            let atom = session.choose(
                &targets,
                &[
                    "UTF8_STRING",
                    "text/plain;charset=utf-8",
                    "text/plain;charset=UTF-8",
                    "text/plain",
                ],
            )?;
            let bytes = session.bytes(atom, CLIPBOARD_TEXT_BYTES, check)?;
            ClipboardSnapshot::Text {
                text: String::from_utf8(bytes)?,
            }
        }
        ClipboardInspectKind::Files => {
            let atom = session.choose(&targets, &["text/uri-list"])?;
            uri_files(&session.bytes(atom, CLIPBOARD_FILES_BYTES, check)?)?
        }
    };
    snapshot.validate()?;
    Ok(snapshot)
}
pub(super) fn image(
    backend: NativeClipboardBackend,
    deadline: Instant,
    check: &crate::NativeCaptureCheck<'_>,
    budget: &mut budget::Budget,
) -> Result<image::RgbaImage> {
    let mut session = Session::new(backend, deadline)?;
    let targets = session.targets(check)?;
    let atom = session.choose(
        &targets,
        &[
            "image/png",
            "image/jpeg",
            "image/webp",
            "image/bmp",
            "image/tiff",
        ],
    )?;
    let bytes = session.bytes(atom, CLIPBOARD_ENCODED_BYTES, check)?;
    drop(session);
    budget.decoding()?;
    check()?;
    crate::decode_editor_image_bytes(&bytes)
        .context("Clipboard image is unsupported or exceeds its image bound")
}
