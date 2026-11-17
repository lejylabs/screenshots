//! Connected peer faults only: private Unix socket pairs, no display or real grabs.
use super::*;
use std::{
    io::{Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};
use x11rb::{
    protocol::xproto::{ConnectionExt, Screen, Setup},
    rust_connection::RustConnection,
    x11_utils::Serialize,
};

fn setup() -> Vec<u8> {
    let mut setup = Setup {
        status: 1,
        protocol_major_version: 11,
        resource_id_base: 0x100000,
        resource_id_mask: 0x0fffff,
        maximum_request_length: 65535,
        min_keycode: 8,
        max_keycode: 255,
        roots: vec![Screen {
            root: 0x100,
            width_in_pixels: 800,
            height_in_pixels: 600,
            ..Screen::default()
        }],
        ..Setup::default()
    };
    setup.length = ((setup.serialize().len() - 8) / 4) as u16;
    setup.serialize()
}
fn request(peer: &mut UnixStream) -> io::Result<u8> {
    let mut header = [0u8; 4];
    peer.read_exact(&mut header)?;
    let size = usize::from(u16::from_ne_bytes([header[2], header[3]])) * 4;
    if !(4..=64).contains(&size) {
        return Err(io::ErrorKind::InvalidData.into());
    }
    peer.read_exact(&mut vec![0u8; size - 4])?;
    Ok(header[0])
}
fn peer(
    stage: u8,
) -> (
    UnixStream,
    mpsc::Receiver<()>,
    thread::JoinHandle<io::Result<()>>,
) {
    let (client, mut peer) = UnixStream::pair().unwrap();
    let (ready, wait) = mpsc::sync_channel(1);
    let thread = thread::spawn(move || {
        peer.set_read_timeout(Some(Duration::from_secs(2)))?;
        peer.set_write_timeout(Some(Duration::from_secs(2)))?;
        let mut hello = [0u8; 12];
        peer.read_exact(&mut hello)?;
        if stage > 0 {
            peer.write_all(&setup())?;
            if stage == 1 {
                assert_eq!(request(&mut peer)?, 43);
            }
            if stage == 2 {
                assert_eq!(request(&mut peer)?, 34);
                assert_eq!(request(&mut peer)?, 43);
            }
        }
        let _ = ready.send(());
        let mut byte = [0];
        assert_eq!(
            peer.read(&mut byte)?,
            0,
            "A terminal native failure must close the sole client fd"
        );
        Ok(())
    });
    (client, wait, thread)
}
#[test]
fn connected_stalled_handshake_times_out_and_releases_peer() {
    let (socket, _ready, server) = peer(0);
    let stream =
        NativeStream::from_socket(socket, None, Arc::new(|| false), Duration::from_millis(100))
            .unwrap();
    let began = Instant::now();
    assert!(RustConnection::connect_to_stream(stream, 0).is_err());
    assert!(began.elapsed() < Duration::from_secs(2));
    server.join().unwrap().unwrap();
}
#[test]
fn stop_control_interrupts_a_handshake_before_the_peer_finishes() {
    let (socket, ready, server) = peer(0);
    let (control, mut signal) = UnixStream::pair().unwrap();
    control.set_nonblocking(true).unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    let cancel = thread::spawn(move || {
        ready.recv_timeout(Duration::from_secs(1)).unwrap();
        stop.store(true, Ordering::Release);
        let _ = signal.write_all(&[1]);
    });
    let stream = NativeStream::from_socket(
        socket,
        Some(control),
        Arc::new(move || stopped.load(Ordering::Acquire)),
        Duration::from_secs(2),
    )
    .unwrap();
    assert!(RustConnection::connect_to_stream(stream, 0).is_err());
    cancel.join().unwrap();
    server.join().unwrap().unwrap();
}
#[test]
fn stalled_reply_and_checked_cleanup_are_canceled_without_a_lingering_worker() {
    for stage in [1, 2] {
        let (socket, ready, server) = peer(stage);
        let (control, mut signal) = UnixStream::pair().unwrap();
        control.set_nonblocking(true).unwrap();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let cancel = thread::spawn(move || {
            ready.recv_timeout(Duration::from_secs(1)).unwrap();
            stop.store(true, Ordering::Release);
            let _ = signal.write_all(&[1]);
        });
        let stream = NativeStream::from_socket(
            socket,
            Some(control),
            Arc::new(move || stopped.load(Ordering::Acquire)),
            Duration::from_secs(2),
        )
        .unwrap();
        let connection = RustConnection::connect_to_stream(stream, 0).unwrap();
        let result = if stage == 1 {
            connection.get_input_focus().unwrap().reply().map(|_| ())
        } else {
            connection
                .ungrab_key(42u8, 0x100u32, 4u16.into())
                .unwrap()
                .check()
        };
        assert!(result.is_err());
        assert!(connection.stream().failed());
        assert_eq!(connection.stream().as_raw_fd(), -1);
        drop(connection);
        cancel.join().unwrap();
        server.join().unwrap().unwrap();
    }
}
#[test]
fn oversized_reply_is_rejected_before_the_dependency_allocates_it() {
    let (socket, mut peer) = UnixStream::pair().unwrap();
    let stream =
        NativeStream::from_socket(socket, None, Arc::new(|| false), Duration::from_secs(1))
            .unwrap();
    stream.frame.borrow_mut().setup = false;
    let mut header = [0u8; 8];
    header[0] = 1;
    header[4..8].copy_from_slice(&u32::MAX.to_ne_bytes());
    peer.write_all(&header).unwrap();
    assert!(stream.read(&mut [0u8; 4096], &mut Vec::new()).is_err());
    assert!(stream.failed());
    assert_eq!(stream.as_raw_fd(), -1);
}
#[test]
fn event_flood_while_waiting_for_reply_has_a_packet_bound() {
    let (socket, mut peer) = UnixStream::pair().unwrap();
    let server = thread::spawn(move || {
        peer.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        peer.set_write_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        peer.read_exact(&mut [0u8; 12]).unwrap();
        peer.write_all(&setup()).unwrap();
        assert_eq!(request(&mut peer).unwrap(), 43);
        let mut event = [0u8; 32];
        event[0] = 2;
        for _ in 0..MAX_OPERATION_PACKETS + 1 {
            if peer.write_all(&event).is_err() {
                break;
            }
        }
    });
    let stream =
        NativeStream::from_socket(socket, None, Arc::new(|| false), Duration::from_secs(2))
            .unwrap();
    let connection = RustConnection::connect_to_stream(stream, 0).unwrap();
    assert!(connection.get_input_focus().unwrap().reply().is_err());
    assert!(connection.stream().failed());
    assert_eq!(connection.stream().as_raw_fd(), -1);
    assert_eq!(connection.stream().packets.get(), MAX_OPERATION_PACKETS + 1);
    drop(connection);
    server.join().unwrap();
}
#[test]
fn blocked_socket_write_observes_stop_control() {
    let (socket, _peer) = UnixStream::pair().unwrap();
    let size = 4096i32;
    assert_eq!(
        unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                (&size as *const i32).cast(),
                std::mem::size_of_val(&size) as _,
            )
        },
        0
    );
    let (control, mut signal) = UnixStream::pair().unwrap();
    control.set_nonblocking(true).unwrap();
    let stopped = Arc::new(AtomicBool::new(false));
    let stop = stopped.clone();
    let (full, wait_full) = mpsc::sync_channel(1);
    let cancel = thread::spawn(move || {
        wait_full.recv_timeout(Duration::from_secs(1)).unwrap();
        stop.store(true, Ordering::Release);
        let _ = signal.write_all(&[1]);
    });
    let stream = NativeStream::from_socket(
        socket,
        Some(control),
        Arc::new(move || stopped.load(Ordering::Acquire)),
        Duration::from_secs(2),
    )
    .unwrap();
    loop {
        match stream.write(&[0u8; 4096], &mut Vec::new()) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) => panic!("Unexpected write error: {error}"),
        }
    }
    full.send(()).unwrap();
    assert!(stream.poll(PollMode::Writable).is_err());
    assert!(stream.failed());
    assert_eq!(stream.as_raw_fd(), -1);
    cancel.join().unwrap();
}

#[test]
fn auth_records_are_bounded_and_match_the_native_host_and_display() {
    let mut record = 256u16.to_be_bytes().to_vec();
    for field in [
        b"fixture".as_slice(),
        b"7",
        b"MIT-MAGIC-COOKIE-1",
        &[17u8; 16],
    ] {
        record.extend_from_slice(&(field.len() as u16).to_be_bytes());
        record.extend_from_slice(field);
    }
    assert_eq!(
        parse_auth(&record, b"fixture", 7).unwrap(),
        (b"MIT-MAGIC-COOKIE-1".to_vec(), vec![17; 16])
    );
    assert_eq!(
        parse_auth(&record, b"other", 7).unwrap(),
        (Vec::new(), Vec::new())
    );
    assert_eq!(
        parse_auth(&record, b"fixture", 8).unwrap(),
        (Vec::new(), Vec::new())
    );
    assert!(parse_auth(&record[..record.len() - 1], b"fixture", 7).is_err());
}
