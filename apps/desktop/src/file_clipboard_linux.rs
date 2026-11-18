//! One retained output representation, event-driven X11/Wayland delayed ownership.
use super::*;
use std::os::fd::AsRawFd;
#[path = "file_clipboard_wayland.rs"]
mod wayland;
const SEGMENT: usize = 64 * 1024;
const MAX_SEGMENTS: usize =
    (app_core::MAX_CAPTURE_BYTES as usize + 16 * 1024 * 1024).div_ceil(SEGMENT);
/// One encoded representation, packed into bounded segments without full-image reallocation.
pub(super) struct Bytes {
    segments: Vec<Vec<u8>>,
    length: usize,
}
impl Bytes {
    pub(super) fn new() -> Self {
        Self {
            segments: Vec::new(),
            length: 0,
        }
    }
    pub(super) fn from_slice(bytes: &[u8]) -> Result<Self> {
        let mut value = Self::new();
        value.append(bytes)?;
        Ok(value)
    }
    pub(super) fn append(&mut self, mut bytes: &[u8]) -> Result<()> {
        ensure!(
            self.length
                .checked_add(bytes.len())
                .is_some_and(|n| n <= MAX_SEGMENTS * SEGMENT),
            "Encoded clipboard representation exceeds its bound"
        );
        while !bytes.is_empty() {
            if self
                .segments
                .last()
                .is_none_or(|segment| segment.len() == SEGMENT)
            {
                ensure!(
                    self.segments.len() < MAX_SEGMENTS,
                    "Clipboard segment bound exceeded"
                );
                if self.segments.len() == self.segments.capacity() {
                    self.segments
                        .try_reserve_exact((MAX_SEGMENTS - self.segments.len()).min(64))?;
                }
                let mut segment = Vec::new();
                segment.try_reserve_exact(SEGMENT)?;
                self.segments.push(segment);
            }
            let segment = self.segments.last_mut().unwrap();
            let count = bytes.len().min(SEGMENT - segment.len());
            segment.extend_from_slice(&bytes[..count]);
            self.length += count;
            bytes = &bytes[count..];
        }
        Ok(())
    }
    pub(super) fn len(&self) -> usize {
        self.length
    }
    pub(super) fn range(&self, position: usize, maximum: usize) -> &[u8] {
        if position == self.length {
            return &[];
        }
        let segment = &self.segments[position / SEGMENT];
        let offset = position % SEGMENT;
        &segment[offset..offset + maximum.min(segment.len() - offset)]
    }
    #[cfg(test)]
    fn retained_pixel_capacity(&self) -> usize {
        self.segments.iter().map(Vec::capacity).sum()
    }
}
pub(super) fn serve(uri: &str, is_wayland: bool, io: &mut transport::ActiveIo<'_>) -> Result<()> {
    serve_output(
        Bytes::from_slice(uri.as_bytes())?,
        "text/uri-list",
        is_wayland,
        io,
    )
}
pub(super) fn serve_output(
    bytes: Bytes,
    mime: &'static str,
    is_wayland: bool,
    io: &mut transport::ActiveIo<'_>,
) -> Result<()> {
    if is_wayland {
        return wayland::serve_output(&bytes, mime, io);
    }
    serve_x11(&bytes, mime, io)
}
fn serve_x11(bytes: &Bytes, mime: &str, io: &mut transport::ActiveIo<'_>) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    use x11rb::{
        connection::{Connection, RequestConnection},
        protocol::{
            Event,
            xproto::{
                AtomEnum, ChangeWindowAttributesAux, ConnectionExt, CreateWindowAux, EventMask,
                PropMode, Property, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, WindowClass,
            },
        },
        wrapper::ConnectionExt as _,
    };
    let display = std::env::var_os("DISPLAY").context("X11 clipboard is unavailable")?;
    let name = display.as_bytes();
    ensure!(
        name.len() <= 64
            && name.first() == Some(&b':')
            && name.len() > 1
            && name[1..].iter().all(|b| b.is_ascii_digit() || *b == b'.'),
        "Clipboard output requires a local numeric X11 display"
    );
    let (connection, screen) = x11rb::connect(display.to_str())?;
    let window = connection.generate_id()?;
    connection
        .create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            connection.setup().roots[screen].root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new(),
        )?
        .check()?;
    let atom =
        |name: &[u8]| -> Result<u32> { Ok(connection.intern_atom(false, name)?.reply()?.atom) };
    let selection = atom(b"CLIPBOARD")?;
    let targets = atom(b"TARGETS")?;
    let primary = atom(mime.as_bytes())?;
    let copied = atom(b"x-special/gnome-copied-files")?;
    let utf8 = atom(b"UTF8_STRING")?;
    let text = atom(b"text/plain")?;
    let incr = atom(b"INCR")?;
    let mut formats = vec![targets, primary];
    if mime == "text/uri-list" {
        formats.push(copied);
    }
    if mime == "text/plain;charset=utf-8" {
        formats.extend([utf8, text]);
    }
    let chunk = connection
        .maximum_request_bytes()
        .saturating_sub(64)
        .min(64 * 1024);
    ensure!(chunk >= 4096, "X11 clipboard request extent is too small");
    struct Transfer {
        window: u32,
        property: u32,
        target: u32,
        position: usize,
        deadline: Instant,
    }
    let mut transfers = Vec::<Transfer>::with_capacity(4);
    let unwatch = |window: u32, transfers: &[Transfer]| {
        if !transfers.iter().any(|t| t.window == window) {
            if let Ok(cookie) = connection.change_window_attributes(
                window,
                &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT),
            ) {
                let _ = cookie.check();
            }
        }
    };
    connection
        .set_selection_owner(window, selection, x11rb::CURRENT_TIME)?
        .check()?;
    ensure!(
        connection.get_selection_owner(selection)?.reply()?.owner == window,
        "X11 clipboard publication was refused"
    );
    io.write_all(&[1])?;
    loop {
        let expired = transfers
            .iter()
            .filter(|t| Instant::now() >= t.deadline)
            .map(|t| t.window)
            .collect::<Vec<_>>();
        transfers.retain(|transfer| Instant::now() < transfer.deadline);
        for window in expired {
            unwatch(window, &transfers);
        }
        let mut batch = 0usize;
        while let Some(event) = connection.poll_for_event()? {
            batch += 1;
            ensure!(batch <= 4096, "X11 clipboard event batch exceeds its bound");
            match event {
                Event::SelectionClear(event) if event.selection == selection => return Ok(()),
                Event::DestroyNotify(event) => {
                    transfers.retain(|transfer| transfer.window != event.window)
                }
                Event::PropertyNotify(event) if event.state == Property::DELETE => {
                    if let Some(index) = transfers
                        .iter()
                        .position(|t| t.window == event.window && t.property == event.atom)
                    {
                        let transfer = &mut transfers[index];
                        let data = bytes.range(transfer.position, chunk);
                        let end = transfer.position + data.len();
                        let success = connection
                            .change_property8(
                                PropMode::REPLACE,
                                transfer.window,
                                transfer.property,
                                transfer.target,
                                data,
                            )
                            .is_ok_and(|cookie| cookie.check().is_ok());
                        let finished = transfer.position == bytes.len();
                        transfer.position = end;
                        // The final zero-length property terminates an ICCCM INCR transfer.
                        if !success || finished {
                            let removed = transfers.swap_remove(index);
                            unwatch(removed.window, &transfers);
                        }
                    }
                }
                Event::SelectionRequest(event)
                    if event.selection == selection && event.owner == window =>
                {
                    let property = if event.property == 0 {
                        event.target
                    } else {
                        event.property
                    };
                    let supported = event.target == primary
                        || (mime == "text/plain;charset=utf-8"
                            && (event.target == utf8 || event.target == text));
                    let success = if event.target == targets {
                        connection
                            .change_property32(
                                PropMode::REPLACE,
                                event.requestor,
                                property,
                                AtomEnum::ATOM,
                                &formats,
                            )
                            .is_ok_and(|cookie| cookie.check().is_ok())
                    } else if event.target == copied && mime == "text/uri-list" {
                        let mut data = Vec::with_capacity(bytes.len() + 5);
                        data.extend_from_slice(b"copy\n");
                        data.extend_from_slice(bytes.range(0, bytes.len()));
                        connection
                            .change_property8(
                                PropMode::REPLACE,
                                event.requestor,
                                property,
                                event.target,
                                &data,
                            )
                            .is_ok_and(|cookie| cookie.check().is_ok())
                    } else if supported && bytes.len() <= chunk {
                        connection
                            .change_property8(
                                PropMode::REPLACE,
                                event.requestor,
                                property,
                                event.target,
                                bytes.range(0, bytes.len()),
                            )
                            .is_ok_and(|cookie| cookie.check().is_ok())
                    } else if supported
                        && transfers.len() < 4
                        && !transfers
                            .iter()
                            .any(|t| t.window == event.requestor && t.property == property)
                    {
                        let selected = connection
                            .change_window_attributes(
                                event.requestor,
                                &ChangeWindowAttributesAux::new().event_mask(
                                    EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY,
                                ),
                            )
                            .is_ok_and(|cookie| cookie.check().is_ok());
                        let started = selected
                            && connection
                                .change_property32(
                                    PropMode::REPLACE,
                                    event.requestor,
                                    property,
                                    incr,
                                    &[bytes.len() as u32],
                                )
                                .is_ok_and(|cookie| cookie.check().is_ok());
                        if started {
                            transfers.push(Transfer {
                                window: event.requestor,
                                property,
                                target: event.target,
                                position: 0,
                                deadline: Instant::now() + DEADLINE,
                            });
                        } else {
                            unwatch(event.requestor, &transfers);
                        }
                        started
                    } else {
                        false
                    };
                    let reply = SelectionNotifyEvent {
                        response_type: SELECTION_NOTIFY_EVENT,
                        sequence: 0,
                        time: event.time,
                        requestor: event.requestor,
                        selection,
                        target: event.target,
                        property: if success { property } else { 0 },
                    };
                    if let Ok(cookie) =
                        connection.send_event(false, event.requestor, EventMask::NO_EVENT, reply)
                    {
                        let _ = cookie.check();
                    }
                }
                _ => {}
            }
        }
        connection.flush()?;
        let mut fds = [
            libc::pollfd {
                fd: connection.stream().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: io.transport.stream.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let timeout = transfers
            .iter()
            .map(|t| {
                t.deadline
                    .saturating_duration_since(Instant::now())
                    .as_millis()
                    .min(i32::MAX as u128) as i32
            })
            .min()
            .unwrap_or(-1);
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(std::io::Error::last_os_error().into());
        }
        if fds[1].revents != 0 {
            return Ok(());
        }
        ensure!(
            fds[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) == 0,
            "X11 clipboard connection closed"
        );
    }
}

#[cfg(test)]
mod segment_tests {
    use super::*;
    #[test]
    fn encoded_segments_pack_small_writes_and_never_reserve_raw_image_capacity() -> Result<()> {
        let mut bytes = Bytes::new();
        for _ in 0..70000 {
            bytes.append(&[19])?;
        }
        assert_eq!(bytes.len(), 70000);
        assert_eq!(bytes.segments.len(), 2);
        assert_eq!(bytes.retained_pixel_capacity(), 2 * SEGMENT);
        assert_eq!(bytes.range(65535, 64), &[19]);
        assert_eq!(bytes.range(65536, 64), &[19; 64]);
        assert!(bytes.range(70000, 64).is_empty());
        assert!(bytes.segments.capacity() <= MAX_SEGMENTS);
        assert_eq!(MAX_SEGMENTS, 4352);
        Ok(())
    }
}
