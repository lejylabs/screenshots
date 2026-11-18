//! One-seat ext/wlr data-control file owner. No content reads, temp files or runtime.
use super::*;
use std::{
    fs::File,
    os::fd::{AsFd, AsRawFd, OwnedFd},
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    protocol::{wl_registry, wl_seat},
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1 as ext_device, ext_data_control_manager_v1 as ext_manager,
    ext_data_control_offer_v1 as ext_offer, ext_data_control_source_v1 as ext_source,
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1 as wlr_device, zwlr_data_control_manager_v1 as wlr_manager,
    zwlr_data_control_offer_v1 as wlr_offer, zwlr_data_control_source_v1 as wlr_source,
};
const URI: &str = "text/uri-list";
const COPIED: &str = "x-special/gnome-copied-files";
struct Transfer {
    file: File,
    copied: bool,
    position: usize,
    deadline: Instant,
}
#[derive(Default)]
struct State {
    ext: Option<ext_manager::ExtDataControlManagerV1>,
    wlr: Option<wlr_manager::ZwlrDataControlManagerV1>,
    seat: Option<wl_seat::WlSeat>,
    seats: usize,
    globals: usize,
    transfers: Vec<Transfer>,
    stopped: bool,
    events: usize,
}
impl State {
    fn event(&mut self) -> bool {
        self.events += 1;
        if self.events > 4096 {
            self.stopped = true;
        }
        !self.stopped
    }
    fn send(&mut self, mime: String, fd: OwnedFd) {
        if !self.event() || self.transfers.len() >= 4 || !matches!(mime.as_str(), URI | COPIED) {
            return;
        }
        let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } != 0
        {
            return;
        }
        self.transfers.push(Transfer {
            file: File::from(fd),
            copied: mime == COPIED,
            position: 0,
            deadline: Instant::now() + DEADLINE,
        });
    }
}
impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if !state.event() {
            return;
        }
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            state.globals += 1;
            if state.globals > 256 || interface.len() > 128 {
                state.stopped = true;
                return;
            }
            match interface.as_str() {
                "ext_data_control_manager_v1" if state.ext.is_none() && version >= 1 => {
                    state.ext = Some(registry.bind(name, 1, qh, ()));
                }
                "zwlr_data_control_manager_v1" if state.wlr.is_none() && version >= 1 => {
                    state.wlr = Some(registry.bind(name, version.min(2), qh, ()));
                }
                "wl_seat" => {
                    state.seats += 1;
                    if state.seats == 1 {
                        state.seat = Some(registry.bind(name, version.min(7), qh, ()));
                    }
                }
                _ => {}
            }
        }
    }
}
impl State {
    fn seat_event(&mut self, event: wl_seat::Event) {
        if !self.event() {
            return;
        }
        if let wl_seat::Event::Name { name } = event {
            if name.len() > 128 {
                self.stopped = true;
            }
        }
    }
}
impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.seat_event(event);
    }
}
wayland_client::delegate_noop!(State: ext_manager::ExtDataControlManagerV1);
wayland_client::delegate_noop!(State: wlr_manager::ZwlrDataControlManagerV1);
macro_rules! source_dispatch {
    ($module:ident, $type:ident) => {
        impl Dispatch<$module::$type, ()> for State {
            fn event(
                state: &mut Self,
                _: &$module::$type,
                event: $module::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                match event {
                    $module::Event::Send { mime_type, fd } => state.send(mime_type, fd),
                    $module::Event::Cancelled => {
                        state.event();
                        state.stopped = true;
                    }
                    _ => {
                        state.event();
                    }
                }
            }
        }
    };
}
source_dispatch!(ext_source, ExtDataControlSourceV1);
source_dispatch!(wlr_source, ZwlrDataControlSourceV1);
macro_rules! device_dispatch {
    ($module:ident, $type:ident, $offer_module:ident, $offer_type:ident) => {
        impl Dispatch<$module::$type, ()> for State {
            wayland_client::event_created_child!(State, $module::$type, [
                $module::EVT_DATA_OFFER_OPCODE => ($offer_module::$offer_type, ()),
            ]);
            fn event(state: &mut Self, _: &$module::$type, event: $module::Event,
                _: &(), _: &Connection, _: &QueueHandle<Self>) {
                if !state.event() { return; }
                match event {
                    $module::Event::DataOffer { id } => id.destroy(),
                    $module::Event::Finished => state.stopped = true,
                    _ => {}
                }
            }
        }
    };
}
device_dispatch!(
    ext_device,
    ExtDataControlDeviceV1,
    ext_offer,
    ExtDataControlOfferV1
);
device_dispatch!(
    wlr_device,
    ZwlrDataControlDeviceV1,
    wlr_offer,
    ZwlrDataControlOfferV1
);
// Offers are destroyed immediately, without retaining type strings or requesting payloads.
macro_rules! offer_dispatch {
    ($module:ident, $type:ident) => {
        impl Dispatch<$module::$type, ()> for State {
            fn event(
                state: &mut Self,
                _: &$module::$type,
                _: $module::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
                state.event();
            }
        }
    };
}
offer_dispatch!(ext_offer, ExtDataControlOfferV1);
offer_dispatch!(wlr_offer, ZwlrDataControlOfferV1);
impl State {
    fn write_transfers(&mut self, uri: &[u8]) {
        self.transfers.retain_mut(|transfer| {
            if Instant::now() >= transfer.deadline {
                return false;
            }
            for _ in 0..64 {
                let prefix = if transfer.copied {
                    b"copy\n".as_slice()
                } else {
                    b"".as_slice()
                };
                let data = if transfer.position < prefix.len() {
                    &prefix[transfer.position..]
                } else {
                    &uri[transfer.position - prefix.len()..]
                };
                match transfer.file.write(data) {
                    Ok(0) => return false,
                    Ok(count) => {
                        transfer.position += count;
                        if transfer.position == prefix.len() + uri.len() {
                            return false;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return true,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => return false,
                }
            }
            true
        });
    }
}
pub(super) fn serve(uri: &str, io: &mut transport::ActiveIo<'_>) -> Result<()> {
    let connection = Connection::connect_to_env()?;
    let mut queue = connection.new_event_queue::<State>();
    let qh = queue.handle();
    let _registry = connection.display().get_registry(&qh, ());
    let mut state = State {
        transfers: Vec::with_capacity(4),
        ..Default::default()
    };
    queue.roundtrip(&mut state)?;
    ensure!(
        !state.stopped && state.seats == 1,
        "Wayland file clipboard needs exactly one seat"
    );
    let seat = state
        .seat
        .as_ref()
        .context("Wayland file clipboard seat unavailable")?;
    // Retain source/device proxies for the entire selection lifetime.
    let _ext;
    let _wlr;
    if let Some(manager) = &state.ext {
        let source = manager.create_data_source(&qh, ());
        source.offer(URI.into());
        source.offer(COPIED.into());
        let device = manager.get_data_device(seat, &qh, ());
        device.set_selection(Some(&source));
        _ext = Some((source, device));
        _wlr = None;
    } else if let Some(manager) = &state.wlr {
        let source = manager.create_data_source(&qh, ());
        source.offer(URI.into());
        source.offer(COPIED.into());
        let device = manager.get_data_device(seat, &qh, ());
        device.set_selection(Some(&source));
        _wlr = Some((source, device));
        _ext = None;
    } else {
        anyhow::bail!("Wayland data-control file clipboard is unsupported");
    }
    state.events = 0;
    queue.roundtrip(&mut state)?;
    ensure!(
        !state.stopped,
        "Wayland file clipboard publication was refused"
    );
    io.write_all(&[1])?;
    loop {
        state.events = 0;
        queue.dispatch_pending(&mut state)?;
        if state.stopped {
            return Ok(());
        }
        state.write_transfers(uri.as_bytes());
        let writing = match connection.flush() {
            Ok(()) => false,
            Err(wayland_client::backend::WaylandError::Io(error))
                if error.kind() == std::io::ErrorKind::WouldBlock =>
            {
                true
            }
            Err(error) => return Err(error.into()),
        };
        let Some(read) = queue.prepare_read() else {
            continue;
        };
        let mut fds = [libc::pollfd {
            fd: -1,
            events: 0,
            revents: 0,
        }; 6];
        fds[0] = libc::pollfd {
            fd: connection.as_fd().as_raw_fd(),
            events: libc::POLLIN | if writing { libc::POLLOUT } else { 0 },
            revents: 0,
        };
        fds[1] = libc::pollfd {
            fd: io.transport.stream.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        for (index, transfer) in state.transfers.iter().enumerate() {
            fds[index + 2] = libc::pollfd {
                fd: transfer.file.as_raw_fd(),
                events: libc::POLLOUT,
                revents: 0,
            };
        }
        let timeout = state
            .transfers
            .iter()
            .map(|transfer| {
                transfer
                    .deadline
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
            "Wayland file clipboard connection closed"
        );
        if fds[0].revents & libc::POLLIN != 0 {
            read.read()?;
        }
    }
}

#[cfg(test)]
mod bounds_tests {
    use super::*;
    use std::os::unix::net::UnixStream;
    #[test]
    fn normal_seat_events_are_admitted_and_count_toward_the_native_event_bound() {
        let mut state = State::default();
        state.seat_event(wl_seat::Event::Capabilities {
            capabilities: wayland_client::WEnum::Value(wl_seat::Capability::empty()),
        });
        state.seat_event(wl_seat::Event::Name {
            name: "owned-private-seat".into(),
        });
        assert_eq!(state.events, 2);
        assert!(!state.stopped);
        for _ in 2..4096 {
            state.seat_event(wl_seat::Event::Capabilities {
                capabilities: wayland_client::WEnum::Value(wl_seat::Capability::empty()),
            });
        }
        assert!(!state.stopped);
        state.seat_event(wl_seat::Event::Name {
            name: "one-event-too-many".into(),
        });
        assert!(state.stopped);
        let mut state = State::default();
        state.seat_event(wl_seat::Event::Name {
            name: "x".repeat(129),
        });
        assert!(state.stopped);
    }
    #[test]
    fn outbound_descriptors_are_bounded_and_expired_transfers_release_without_copying_metadata() {
        let mut state = State {
            transfers: Vec::with_capacity(4),
            ..Default::default()
        };
        let mut peers = Vec::new();
        for _ in 0..5 {
            let (peer, writer) = UnixStream::pair().unwrap();
            state.send(URI.into(), writer.into());
            peers.push(peer);
        }
        assert_eq!(state.transfers.len(), 4);
        assert_eq!(state.transfers.capacity(), 4);
        let mut byte = [0];
        assert_eq!(peers[4].read(&mut byte).unwrap(), 0);
        for transfer in &mut state.transfers {
            transfer.deadline = Instant::now();
        }
        state.write_transfers(b"file:///owned");
        assert!(state.transfers.is_empty());
        for mut peer in peers {
            assert_eq!(peer.read(&mut byte).unwrap(), 0);
        }
    }
}
