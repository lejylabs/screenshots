//! Operation-scoped X11 server acknowledgment, inside the existing native worker.
use crate::{CaptureError, NativeCaptureCall, NativeCaptureCheck, NativeCaptureReply};

pub fn hide_capture_window(window: u32, check: &NativeCaptureCheck<'_>) -> Result<(), CaptureError> {
    let call = NativeCaptureCall::HideCaptureWindow { window };
    call.validate()?;
    check()?;
    let executor = crate::native_capture_job::executor().ok_or(CaptureError::Unsupported(
        "Control-window hiding requires the isolated native worker",
    ))?;
    let reply = executor(call.clone(), check, crate::NativeCaptureContext::default())?;
    call.validate_reply(&reply)?;
    check()?;
    match reply {
        NativeCaptureReply::CaptureWindowHidden => Ok(()),
        _ => unreachable!("validated native visibility reply"),
    }
}

pub(crate) fn hide_x11_window(window: u32, owner: u32) -> anyhow::Result<()> {
    use anyhow::{Context, ensure};
    use std::{os::fd::AsRawFd, time::{Duration, Instant}};
    use x11rb::{connection::Connection, protocol::{Event, xproto::{self, ConnectionExt as _}}};
    ensure!(!crate::linux_wayland_session(), "X11 visibility proof requires X11 ownership");
    ensure!(window != 0 && owner != 0, "Invalid owned control identity");
    let (connection, screen) = x11rb::connect(None)?;
    let root = connection.setup().roots[screen].root;
    let pid_atom = connection.intern_atom(true, b"_NET_WM_PID")?.reply()?.atom;
    ensure!(pid_atom != x11rb::NONE, "Control process identity is unavailable");
    let owns_window = || -> anyhow::Result<()> {
        let value = connection.get_property(false, window, pid_atom, xproto::AtomEnum::CARDINAL, 0, 2)?.reply()?;
        ensure!(value.bytes_after == 0 && value.format == 32 && value.value32().is_some_and(|mut values| values.next() == Some(owner) && values.next().is_none()), "Control window no longer belongs to this application");
        Ok(())
    };
    owns_window()?;
    connection.change_window_attributes(window, &xproto::ChangeWindowAttributesAux::new().event_mask(xproto::EventMask::STRUCTURE_NOTIFY))?.check()?;
    let hidden = || -> anyhow::Result<bool> {
        owns_window()?;
        Ok(connection.get_window_attributes(window)?.reply()?.map_state == xproto::MapState::UNMAPPED)
    };
    if hidden()? { return Ok(()); }
    let state_atom = connection.intern_atom(false, b"WM_CHANGE_STATE")?.reply()?.atom;
    let event = xproto::ClientMessageEvent::new(32, window, state_atom, [3, 0, 0, 0, 0]);
    connection.send_event(false, root, xproto::EventMask::SUBSTRUCTURE_REDIRECT | xproto::EventMask::SUBSTRUCTURE_NOTIFY, event)?.check()?;
    connection.flush()?;
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        ensure!(Instant::now() < until, "Window manager did not acknowledge hiding the control window");
        while let Some(event) = connection.poll_for_event()? {
            match event {
                Event::UnmapNotify(event) if event.window == window => {
                    ensure!(hidden()?, "Control window was remapped while hiding");
                    // This is an X-server state proof, never a compositor-presented-frame fence.
                    return Ok(());
                }
                Event::DestroyNotify(event) if event.window == window => anyhow::bail!("Owned control window was destroyed"),
                _ => {}
            }
            ensure!(Instant::now() < until, "Native visibility event deadline exceeded");
        }
        let remaining = until.saturating_duration_since(Instant::now());
        ensure!(!remaining.is_zero(), "Native visibility event deadline exceeded");
        let mut fd = libc::pollfd { fd: connection.stream().as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ready = unsafe { libc::poll(&mut fd, 1, remaining.as_millis().max(1).min(i32::MAX as u128) as i32) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted { continue; }
            return Err(error).context("Native visibility event wait failed");
        }
        ensure!(ready > 0 && fd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) == 0, "Native visibility event connection failed or timed out");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hide_requires_valid_identity_and_isolated_executor_and_rejects_wrong_reply() {
        assert!(NativeCaptureCall::HideCaptureWindow { window: 0 }.validate().is_err());
        let call = NativeCaptureCall::HideCaptureWindow { window: 7 };
        assert!(call.validate_reply(&NativeCaptureReply::CaptureWindowHidden).is_ok());
        assert!(call.validate_reply(&NativeCaptureReply::Windows(vec![])).is_err());
        assert!(NativeCaptureCall::Windows.validate_reply(&NativeCaptureReply::CaptureWindowHidden).is_err());
    }

    #[test]
    #[ignore = "requires an authenticated private Xvfb owned fixture; never run on a user's display"]
    fn private_owned_x11_hidden_fixture_rejects_foreign_and_destroyed_windows() {
        use std::os::unix::fs::MetadataExt;
        use x11rb::{connection::Connection, protocol::xproto::{self, ConnectionExt as _}, wrapper::ConnectionExt as _};
        assert_eq!(std::env::var("SCREENSHOTS_PRIVATE_HIDE_FIXTURE").as_deref(), Ok("owned-native-window"));
        let display = std::env::var("DISPLAY").unwrap();
        assert!((320..380).contains(&display.strip_prefix(':').unwrap().parse::<u16>().unwrap()));
        assert!(!crate::linux_wayland_session());
        let authority = std::path::PathBuf::from(std::env::var_os("XAUTHORITY").unwrap());
        let parent = std::fs::symlink_metadata(authority.parent().unwrap()).unwrap();
        assert!(parent.is_dir() && parent.mode() & 0o077 == 0);
        assert!(authority.parent().unwrap().file_name().unwrap().to_string_lossy().starts_with("screenshots-capture-hide-"));
        let metadata = std::fs::symlink_metadata(&authority).unwrap();
        assert!(metadata.is_file() && metadata.uid() == unsafe { libc::getuid() });
        let (connection, screen) = x11rb::connect(None).unwrap();
        let root = &connection.setup().roots[screen];
        let window = connection.generate_id().unwrap();
        connection.create_window(x11rb::COPY_DEPTH_FROM_PARENT, window, root.root, 0, 0, 8, 8, 0, xproto::WindowClass::INPUT_OUTPUT, 0, &xproto::CreateWindowAux::new()).unwrap().check().unwrap();
        let pid = connection.intern_atom(false, b"_NET_WM_PID").unwrap().reply().unwrap().atom;
        connection.change_property32(xproto::PropMode::REPLACE, window, pid, xproto::AtomEnum::CARDINAL, &[std::process::id()]).unwrap().check().unwrap();
        assert!(hide_x11_window(window, std::process::id()).is_ok());
        assert!(hide_x11_window(window, std::process::id().wrapping_add(1)).is_err());
        connection.map_window(window).unwrap().check().unwrap();
        connection.flush().unwrap();
        assert!(hide_x11_window(window, std::process::id()).is_err(), "private Xvfb has no WM, so no UnmapNotify may be invented");
        connection.unmap_window(window).unwrap().check().unwrap();
        assert!(hide_x11_window(window, std::process::id()).is_ok());
        connection.destroy_window(window).unwrap().check().unwrap();
        assert!(hide_x11_window(window, std::process::id()).is_err());
    }
}
