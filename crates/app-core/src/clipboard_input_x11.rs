//! Isolated X11 CLIPBOARD transfer with XFixes generation observation.
use super::*;
use anyhow::{Context, bail};
use std::{
    collections::VecDeque,
    os::{fd::AsRawFd, unix::ffi::OsStrExt},
};
use x11rb::{
    COPY_DEPTH_FROM_PARENT, COPY_FROM_PARENT, CURRENT_TIME, NONE,
    connection::Connection,
    protocol::{
        Event,
        xfixes::{ConnectionExt as _, SelectionEventMask},
        xproto::{
            Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, GetPropertyReply,
            Property, Window, WindowClass,
        },
    },
    rust_connection::RustConnection,
};

struct Session {
    connection: RustConnection,
    window: Window,
    selection: Atom,
    property: Atom,
    targets: Atom,
    incr: Atom,
    owner: Window,
    generation: u64,
    baseline: u64,
    baseline_timestamp: Option<(Option<u32>, Option<u32>)>,
    selection_timestamp: Option<u32>,
    event_timestamp: Option<u32>,
    deadline: Instant,
    events: usize,
    pending: VecDeque<Event>,
}
impl Session {
    fn new(backend: NativeClipboardBackend, deadline: Instant) -> Result<Self> {
        if backend == NativeClipboardBackend::Automatic && crate::linux_wayland_session() {
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
        let display = display
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("Invalid X11 display"))?;
        let (connection, screen) = x11rb::connect(Some(display))?;
        let window = connection.generate_id()?;
        connection
            .create_window(
                COPY_DEPTH_FROM_PARENT,
                window,
                connection.setup().roots[screen].root,
                0,
                0,
                1,
                1,
                0,
                WindowClass::INPUT_OUTPUT,
                COPY_FROM_PARENT,
                &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
            )?
            .check()?;
        let mut session = Self {
            connection,
            window,
            selection: 0,
            property: 0,
            targets: 0,
            incr: 0,
            owner: 0,
            generation: 0,
            baseline: 0,
            baseline_timestamp: None,
            selection_timestamp: None,
            event_timestamp: None,
            deadline,
            events: 0,
            pending: VecDeque::new(),
        };
        session.selection = session.atom("CLIPBOARD")?;
        session.property = session.atom("_SCREENSHOTS_CLIP_INPUT")?;
        session.targets = session.atom("TARGETS")?;
        session.incr = session.atom("INCR")?;
        // Subscribe before the baseline/TARGETS. No owner-XID-only fallback is safe:
        // every subsequent assertion counts, including equal timestamps and A->B->A.
        let version = session.connection.xfixes_query_version(5, 0)?.reply()?;
        ensure!(
            version.major_version >= 1,
            "XFixes selection generations unavailable"
        );
        session
            .connection
            .xfixes_select_selection_input(
                window,
                session.selection,
                SelectionEventMask::SET_SELECTION_OWNER
                    | SelectionEventMask::SELECTION_WINDOW_DESTROY
                    | SelectionEventMask::SELECTION_CLIENT_CLOSE,
            )?
            .check()?;
        session.owner = session
            .connection
            .get_selection_owner(session.selection)?
            .reply()?
            .owner;
        session.drain_events()?;
        session.baseline = session.generation;
        session.baseline_timestamp = Some((session.selection_timestamp, session.event_timestamp));
        // A second ordered round trip closes bootstrap races before any TARGETS read.
        session.check(&|| Ok(()))?;
        Ok(session)
    }
    fn atom(&self, name: &str) -> Result<Atom> {
        Ok(self
            .connection
            .intern_atom(false, name.as_bytes())?
            .reply()?
            .atom)
    }
    fn observe(&mut self, event: Event) -> Result<()> {
        ensure!(self.events < 4096, "Clipboard event bound exceeded");
        self.events += 1;
        if let Event::XfixesSelectionNotify(ref update) = event {
            if update.window == self.window && update.selection == self.selection {
                ensure!(
                    update.response_type & 0x80 == 0,
                    "Synthetic selection generation event refused"
                );
                self.generation = self
                    .generation
                    .checked_add(1)
                    .ok_or_else(|| anyhow::anyhow!("Clipboard generation overflow"))?;
                self.selection_timestamp = Some(update.selection_timestamp);
                self.event_timestamp = Some(update.timestamp);
                return Ok(());
            }
        }
        self.pending.push_back(event);
        Ok(())
    }
    fn drain_events(&mut self) -> Result<()> {
        while let Some(event) = self.connection.poll_for_event()? {
            self.observe(event)?;
        }
        Ok(())
    }
    fn check(&mut self, check: &crate::NativeCaptureCheck<'_>) -> Result<()> {
        check()?;
        ensure!(
            Instant::now() < self.deadline,
            "Clipboard transfer deadline exceeded"
        );
        // Replies and XFixes notifications are ordered on this same connection. A
        // round-trip barrier queues all prior generation events before checking them.
        let owner = self
            .connection
            .get_selection_owner(self.selection)?
            .reply()?
            .owner;
        self.drain_events()?;
        ensure!(
            owner == self.owner
                && self.generation == self.baseline
                && self.baseline_timestamp
                    == Some((self.selection_timestamp, self.event_timestamp)),
            "Clipboard selection generation changed during transfer"
        );
        Ok(())
    }
    fn event(&mut self, check: &crate::NativeCaptureCheck<'_>) -> Result<Event> {
        loop {
            self.check(check)?;
            if let Some(event) = self.pending.pop_front() {
                return Ok(event);
            }
            let millis = self
                .deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .saturating_add(1)
                .min(i32::MAX as u128) as i32;
            let mut fd = libc::pollfd {
                fd: self.connection.stream().as_raw_fd(),
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
            self.drain_events()?;
        }
    }
    fn property(&self, max: usize) -> Result<GetPropertyReply> {
        self.property_at(0, max)
    }
    fn property_at(&self, offset: usize, max: usize) -> Result<GetPropertyReply> {
        let value = self
            .connection
            .get_property(
                false,
                self.window,
                self.property,
                AtomEnum::ANY,
                u32::try_from(offset)?,
                u32::try_from(max.div_ceil(4))?,
            )?
            .reply()?;
        ensure!(
            value.value.len() <= max.saturating_add(3).max(8),
            "Invalid clipboard property extent"
        );
        Ok(value)
    }
    fn delete(&self) -> Result<()> {
        self.connection
            .delete_property(self.window, self.property)?;
        self.connection.flush()?;
        Ok(())
    }
    fn begin(&mut self, target: Atom, check: &crate::NativeCaptureCheck<'_>) -> Result<()> {
        self.check(check)?;
        ensure!(self.owner != NONE, "Clipboard has no owner");
        self.delete()?;
        self.connection.convert_selection(
            self.window,
            self.selection,
            target,
            self.property,
            CURRENT_TIME,
        )?;
        self.connection.flush()?;
        loop {
            if let Event::SelectionNotify(event) = self.event(check)? {
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
        if self.owner == NONE {
            return Ok(Vec::new());
        }
        self.begin(self.targets, check)?;
        let value = self.property(CLIPBOARD_FORMATS * 4)?;
        ensure!(
            value.type_ == u32::from(AtomEnum::ATOM)
                && value.format == 32
                && value.value_len as usize <= CLIPBOARD_FORMATS
                && value.bytes_after == 0,
            "Clipboard format offer exceeds its bound or is invalid"
        );
        let result = value
            .value32()
            .ok_or_else(|| anyhow::anyhow!("Invalid clipboard target type"))?
            .collect();
        self.delete()?;
        self.check(check)?;
        Ok(result)
    }
    fn name(&self, atom: Atom) -> Result<String> {
        let bytes = self.connection.get_atom_name(atom)?.reply()?.name;
        ensure!(
            !bytes.is_empty() && bytes.len() <= CLIPBOARD_FORMAT_BYTES,
            "Clipboard format label exceeds128 bytes"
        );
        Ok(String::from_utf8(bytes)?)
    }
    fn choose(&self, targets: &[Atom], names: &[&str]) -> Result<Atom> {
        for name in names {
            let atom = self.atom(name)?;
            if targets.contains(&atom) {
                return Ok(atom);
            }
        }
        bail!("Clipboard does not provide the requested supported format")
    }
    fn bytes(
        &mut self,
        target: Atom,
        max: usize,
        check: &crate::NativeCaptureCheck<'_>,
    ) -> Result<Vec<u8>> {
        self.begin(target, check)?;
        let initial = self.property(max.min(64 * 1024))?;
        if initial.type_ != self.incr {
            let total = initial
                .value
                .len()
                .checked_add(initial.bytes_after as usize)
                .ok_or_else(|| anyhow::anyhow!("Clipboard size overflow"))?;
            ensure!(
                initial.format == 8 && total <= max,
                "Clipboard offer exceeds its bound"
            );
            let expected = initial.type_;
            let mut bytes = initial.value;
            while bytes.len() < total {
                self.check(check)?;
                ensure!(
                    bytes.len().is_multiple_of(4),
                    "Invalid segmented clipboard property"
                );
                let part =
                    self.property_at(bytes.len() / 4, (total - bytes.len()).min(64 * 1024))?;
                ensure!(
                    part.type_ == expected
                        && part.format == 8
                        && !part.value.is_empty()
                        && part.value.len().checked_add(part.bytes_after as usize)
                            == Some(total - bytes.len()),
                    "Clipboard property changed during transfer"
                );
                bytes.extend_from_slice(&part.value);
            }
            self.delete()?;
            self.check(check)?;
            return Ok(bytes);
        }
        ensure!(
            initial.format == 32 && initial.value_len == 1 && initial.bytes_after == 0,
            "Invalid clipboard INCR offer"
        );
        let hint = initial
            .value32()
            .and_then(|mut values| values.next())
            .ok_or_else(|| anyhow::anyhow!("Invalid clipboard INCR hint"))?;
        ensure!(
            hint as usize <= max,
            "Clipboard INCR offer exceeds its bound"
        );
        self.delete()?;
        let mut output = Vec::new();
        loop {
            if let Event::PropertyNotify(event) = self.event(check)? {
                if event.window != self.window
                    || event.atom != self.property
                    || event.state != Property::NEW_VALUE
                {
                    continue;
                }
                let remaining = max.saturating_sub(output.len());
                let chunk = self.property(remaining.min(64 * 1024).saturating_add(1))?;
                ensure!(
                    chunk.format == 8
                        && chunk.type_ != self.incr
                        && chunk.bytes_after == 0
                        && chunk.value.len() <= remaining
                        && chunk.value.len() <= 64 * 1024,
                    "Clipboard INCR chunk or cumulative offer exceeds its bound"
                );
                if chunk.value.is_empty() {
                    self.delete()?;
                    self.check(check)?;
                    return Ok(output);
                }
                output.extend_from_slice(&chunk.value);
                self.delete()?;
            }
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.connection.destroy_window(self.window);
        let _ = self.connection.flush();
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
    let value = match kind {
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
            ClipboardSnapshot::Text {
                text: String::from_utf8(session.bytes(atom, CLIPBOARD_TEXT_BYTES, check)?)?,
            }
        }
        ClipboardInspectKind::Files => {
            let atom = session.choose(&targets, &["text/uri-list"])?;
            uri_files(&session.bytes(atom, CLIPBOARD_FILES_BYTES, check)?)?
        }
    };
    session.check(check)?;
    value.validate()?;
    Ok(value)
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
