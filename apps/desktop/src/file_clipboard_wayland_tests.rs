//! Minimal private data-control server, independent of any real compositor or clipboard.
use super::*;
use std::{
    os::fd::{AsFd, AsRawFd},
    os::unix::net::{UnixListener, UnixStream},
    sync::Arc,
};
use wayland_protocols::ext::data_control::v1::server::{
    ext_data_control_device_v1 as ext_device, ext_data_control_manager_v1 as ext_manager,
    ext_data_control_source_v1 as ext_source,
};
use wayland_protocols_wlr::data_control::v1::server::{
    zwlr_data_control_device_v1 as wlr_device, zwlr_data_control_manager_v1 as wlr_manager,
    zwlr_data_control_source_v1 as wlr_source,
};
use wayland_server::{
    Client, DataInit, Dispatch, Display, DisplayHandle, GlobalDispatch, New, Resource,
    protocol::wl_seat,
};
struct Capture {
    stream: UnixStream,
    bytes: Vec<u8>,
    name: &'static str,
}
struct State {
    names: Vec<&'static str>,
    captures: Vec<Capture>,
    output: mpsc::SyncSender<(&'static str, Vec<u8>)>,
}
impl GlobalDispatch<wl_seat::WlSeat, ()> for State {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        seat: New<wl_seat::WlSeat>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        let seat = init.init(seat, ());
        seat.capabilities(wl_seat::Capability::empty());
        if seat.version() >= 2 {
            seat.name("owned-private-seat".into());
        }
    }
}
impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &wl_seat::WlSeat,
        _: wl_seat::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}
macro_rules! protocol {
    ($manager:ident, $mt:ident, $device:ident, $dt:ident, $source:ident, $st:ident) => {
        impl GlobalDispatch<$manager::$mt, ()> for State {
            fn bind(
                _: &mut Self,
                _: &DisplayHandle,
                _: &Client,
                manager: New<$manager::$mt>,
                _: &(),
                init: &mut DataInit<'_, Self>,
            ) {
                init.init(manager, ());
            }
        }
        impl Dispatch<$manager::$mt, ()> for State {
            fn request(
                _: &mut Self,
                _: &Client,
                _: &$manager::$mt,
                request: $manager::Request,
                _: &(),
                _: &DisplayHandle,
                init: &mut DataInit<'_, Self>,
            ) {
                match request {
                    $manager::Request::CreateDataSource { id } => {
                        init.init(id, ());
                    }
                    $manager::Request::GetDataDevice { id, .. } => {
                        init.init(id, ());
                    }
                    _ => {}
                }
            }
        }
        impl Dispatch<$source::$st, ()> for State {
            fn request(
                _: &mut Self,
                _: &Client,
                _: &$source::$st,
                request: $source::Request,
                _: &(),
                _: &DisplayHandle,
                _: &mut DataInit<'_, Self>,
            ) {
                if let $source::Request::Offer { mime_type } = request {
                    assert!(matches!(
                        mime_type.as_str(),
                        "text/uri-list"
                            | "x-special/gnome-copied-files"
                            | "image/png"
                            | "text/plain;charset=utf-8"
                            | "text/plain"
                    ));
                }
            }
        }
        impl Dispatch<$device::$dt, ()> for State {
            fn request(
                state: &mut Self,
                _: &Client,
                _: &$device::$dt,
                request: $device::Request,
                _: &(),
                _: &DisplayHandle,
                _: &mut DataInit<'_, Self>,
            ) {
                if let $device::Request::SetSelection {
                    source: Some(source),
                } = request
                {
                    assert!(
                        state.captures.is_empty(),
                        "Unexpected second native publication"
                    );
                    for name in state.names.clone() {
                        let (stream, writer) = UnixStream::pair().unwrap();
                        stream.set_nonblocking(true).unwrap();
                        source.send(name.into(), writer.as_fd());
                        state.captures.push(Capture {
                            stream,
                            bytes: Vec::new(),
                            name,
                        });
                    }
                }
            }
        }
    };
}
protocol!(
    ext_manager,
    ExtDataControlManagerV1,
    ext_device,
    ExtDataControlDeviceV1,
    ext_source,
    ExtDataControlSourceV1
);
protocol!(
    wlr_manager,
    ZwlrDataControlManagerV1,
    wlr_device,
    ZwlrDataControlDeviceV1,
    wlr_source,
    ZwlrDataControlSourceV1
);
struct Private {
    shutdown: Option<UnixStream>,
    thread: Option<std::thread::JoinHandle<()>>,
    output: mpsc::Receiver<(&'static str, Vec<u8>)>,
    socket: std::path::PathBuf,
}
impl Private {
    fn new(root: &Path, ext: bool, supported: bool) -> Result<Self> {
        Self::new_types(
            root,
            ext,
            supported,
            vec!["text/uri-list", "x-special/gnome-copied-files"],
        )
    }
    fn new_types(
        root: &Path,
        ext: bool,
        supported: bool,
        names: Vec<&'static str>,
    ) -> Result<Self> {
        let socket = root.join("wayland-owned");
        let listener = UnixListener::bind(&socket)?;
        listener.set_nonblocking(true)?;
        let (control, mut shutdown) = UnixStream::pair()?;
        control.set_nonblocking(true)?;
        let (output, receive) = mpsc::sync_channel(2);
        let thread = std::thread::spawn(move || {
            let mut display = Display::<State>::new().unwrap();
            let mut handle = display.handle();
            handle.create_global::<State, wl_seat::WlSeat, _>(7, ());
            if supported {
                if ext {
                    handle.create_global::<State, ext_manager::ExtDataControlManagerV1, _>(1, ());
                } else {
                    handle.create_global::<State, wlr_manager::ZwlrDataControlManagerV1, _>(2, ());
                }
            }
            let mut state = State {
                names,
                captures: Vec::new(),
                output,
            };
            loop {
                while let Ok((stream, _)) = listener.accept() {
                    handle.insert_client(stream, Arc::new(())).unwrap();
                }
                display.dispatch_clients(&mut state).unwrap();
                display.flush_clients().unwrap();
                state.captures.retain_mut(|capture| {
                    let mut bytes = [0; 16 * 1024];
                    loop {
                        match capture.stream.read(&mut bytes) {
                            Ok(0) => {
                                state
                                    .output
                                    .try_send((capture.name, std::mem::take(&mut capture.bytes)))
                                    .unwrap();
                                return false;
                            }
                            Ok(count) => {
                                assert!(capture.bytes.len() + count <= 1024 * 1024);
                                capture.bytes.extend_from_slice(&bytes[..count]);
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                return true;
                            }
                            Err(error) => panic!("Owned protocol receive failed: {error}"),
                        }
                    }
                });
                let mut fds = vec![
                    libc::pollfd {
                        fd: listener.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: display.as_fd().as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                    libc::pollfd {
                        fd: control.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    },
                ];
                for capture in &state.captures {
                    fds.push(libc::pollfd {
                        fd: capture.stream.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    });
                }
                assert!(unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, -1) } >= 0);
                if fds[2].revents != 0 {
                    break;
                }
            }
        });
        // A socket path exists before the first private client is launched; no readiness polling.
        let _ = shutdown.write(&[]);
        Ok(Self {
            shutdown: Some(shutdown),
            thread: Some(thread),
            output: receive,
            socket,
        })
    }
}
impl Drop for Private {
    fn drop(&mut self) {
        if let Some(mut shutdown) = self.shutdown.take() {
            let _ = shutdown.write(&[1]);
        }
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
#[test]
#[ignore = "private Rust data-control server and owned helper; never connects to the user's compositor"]
fn ext_and_wlr_file_offers_preserve_copy_only_metadata_without_disk_or_image_transfer() -> Result<()>
{
    let _serial = SERIAL.lock().unwrap();
    for ext in [true, false] {
        let root = Root::new()?;
        let path = root.0.join("Grüße #?% file.bin");
        std::fs::write(&path, b"unchanged source")?;
        let server = Private::new(&root.0, ext, true)?;
        let mut command = child("native");
        command
            .env("WAYLAND_DISPLAY", &server.socket)
            .env("XDG_SESSION_TYPE", "wayland");
        let owner = spawn_with_command(&path, true, command)?;
        let uri = app_core::prepare_file_clipboard_uri(&path)?;
        let mut captured = Vec::new();
        for _ in 0..2 {
            captured.push(server.output.recv_timeout(Duration::from_secs(5))?);
        }
        captured.sort_by_key(|(name, _)| *name);
        assert_eq!(captured[0], ("text/uri-list", uri.as_bytes().to_vec()));
        assert_eq!(
            captured[1],
            (
                "x-special/gnome-copied-files",
                format!("copy\n{uri}").into_bytes()
            )
        );
        assert_eq!(std::fs::read(&path)?, b"unchanged source");
        owner.retire()?;
    }
    Ok(())
}
#[test]
#[ignore = "private Rust server deliberately omits data-control; no real compositor access"]
fn missing_wayland_data_control_fails_explicitly_without_x11_or_text_fallback() -> Result<()> {
    let _serial = SERIAL.lock().unwrap();
    let root = Root::new()?;
    let path = root.0.join("file");
    std::fs::write(&path, b"source")?;
    let server = Private::new(&root.0, true, false)?;
    let mut command = child("native");
    command
        .env("WAYLAND_DISPLAY", &server.socket)
        .env("XDG_SESSION_TYPE", "wayland")
        .env("DISPLAY", ":invalid-must-not-be-used");
    assert!(spawn_with_command(&path, true, command).is_err());
    cleaned();
    assert!(server.output.try_recv().is_err());
    Ok(())
}

#[test]
#[ignore = "private ext/wlr data-control server and owned output helper; no real compositor access"]
fn image_and_text_paste_on_owned_wayland_offer_one_retained_representation() -> Result<()> {
    let _serial = SERIAL.lock().unwrap();
    for ext in [true, false] {
        for image_output in [true, false] {
            let root = Root::new()?;
            let mime = if image_output {
                "image/png"
            } else {
                "text/plain;charset=utf-8"
            };
            let server = Private::new_types(&root.0, ext, true, vec![mime])?;
            let mut command = child("output-native");
            command
                .env("WAYLAND_DISPLAY", &server.socket)
                .env("XDG_SESSION_TYPE", "wayland")
                .env("DISPLAY", ":invalid-no-xwayland-fallback");
            let image =
                image::RgbaImage::from_fn(3, 2, |x, y| image::Rgba([x as u8, y as u8, 47, 83]));
            let text = "private UTF-8 ä 🐾";
            let value = if image_output {
                app_core::NativeClipboardOutput::Image(Arc::new(image.clone()))
            } else {
                app_core::NativeClipboardOutput::Text(text)
            };
            let owner = spawn_input(Input::Output(value), true, command, &|| Ok(()))?;
            let (actual_mime, bytes) = server.output.recv_timeout(Duration::from_secs(5))?;
            assert_eq!(actual_mime, mime);
            if image_output {
                assert_eq!(image::load_from_memory(&bytes)?.into_rgba8(), image);
            } else {
                assert_eq!(bytes, text.as_bytes());
            }
            owner.retire()?;
        }
    }
    Ok(())
}
