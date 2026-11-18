//! Bounded file metadata owners. Incoming selection payloads are never requested.
use super::*;
use std::os::fd::AsRawFd;
#[path = "file_clipboard_wayland.rs"]
mod wayland;

pub(super) fn serve(uri: &str, is_wayland: bool, io: &mut transport::ActiveIo<'_>) -> Result<()> {
    if is_wayland {
        return wayland::serve(uri, io);
    }
    serve_x11(uri, io)
}
fn serve_x11(uri: &str, io: &mut transport::ActiveIo<'_>) -> Result<()> {
    use std::os::unix::ffi::OsStrExt;
    use x11rb::{
        connection::Connection,
        protocol::{
            Event,
            xproto::{
                AtomEnum, ConnectionExt, CreateWindowAux, EventMask, PropMode,
                SELECTION_NOTIFY_EVENT, SelectionNotifyEvent, WindowClass,
            },
        },
        wrapper::ConnectionExt as _,
    };
    let display = std::env::var_os("DISPLAY").context("X11 file clipboard is unavailable")?;
    let bytes = display.as_bytes();
    ensure!(
        bytes.len() <= 64
            && bytes.first() == Some(&b':')
            && bytes.len() > 1
            && bytes[1..]
                .iter()
                .all(|byte| byte.is_ascii_digit() || *byte == b'.'),
        "File clipboard requires a local numeric X11 display"
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
    let uri_type = atom(b"text/uri-list")?;
    let copied = atom(b"x-special/gnome-copied-files")?;
    connection
        .set_selection_owner(window, selection, x11rb::CURRENT_TIME)?
        .check()?;
    ensure!(
        connection.get_selection_owner(selection)?.reply()?.owner == window,
        "X11 file clipboard publication was refused"
    );
    io.write_all(&[1])?;
    loop {
        let mut batch = 0usize;
        while let Some(event) = connection.poll_for_event()? {
            batch += 1;
            ensure!(
                batch <= 4096,
                "X11 file clipboard event batch exceeds its bound"
            );
            match event {
                Event::SelectionClear(event) if event.selection == selection => return Ok(()),
                Event::SelectionRequest(event)
                    if event.selection == selection && event.owner == window =>
                {
                    let property = if event.property == 0 {
                        event.target
                    } else {
                        event.property
                    };
                    let copied_data = (event.target == copied).then(|| format!("copy\n{uri}"));
                    let data = if event.target == uri_type {
                        Some(uri.as_bytes())
                    } else {
                        copied_data.as_deref().map(str::as_bytes)
                    };
                    let success = if event.target == targets {
                        connection
                            .change_property32(
                                PropMode::REPLACE,
                                event.requestor,
                                property,
                                AtomEnum::ATOM,
                                &[targets, uri_type, copied],
                            )
                            .is_ok_and(|cookie| cookie.check().is_ok())
                    } else if let Some(data) = data {
                        connection
                            .change_property8(
                                PropMode::REPLACE,
                                event.requestor,
                                property,
                                event.target,
                                data,
                            )
                            .is_ok_and(|cookie| cookie.check().is_ok())
                    } else {
                        false
                    };
                    // A requestor can disappear at any point; that is not a new publication.
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
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, -1) };
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
            "X11 file clipboard connection closed"
        );
    }
}
