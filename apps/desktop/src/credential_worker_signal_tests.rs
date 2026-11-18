// Private subprocesses only: the owner exercises real Cancellation handlers and actual
// retained supervisor authority. The leaf never opens a credential provider or user file.
const SIGNAL_OWNER_MODE: &str = "SCREENSHOTS_CREDENTIAL_SIGNAL_OWNER";
const SIGNAL_LEAF_MODE: &str = "SCREENSHOTS_CREDENTIAL_SIGNAL_LEAF";
fn signal_fixture_command(entry: &str, key: &str, mode: &str) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", entry, "--nocapture"]).env(key, mode)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    command
}
#[test]
fn signal_cleanup_leaf_entry() -> Result<()> {
    if std::env::var_os(SIGNAL_LEAF_MODE).is_none() { return Ok(()); }
    let (transport, nonce, _parent) = transport::Transport::inherited()?;
    let mut io = transport::ActiveIo { transport, deadline: Instant::now() + Duration::from_secs(8), check: &|| Ok(()) };
    protocol::expect_handshake(&mut io, protocol::HELLO, &nonce)?;
    protocol::handshake(&mut io, protocol::ACK, &nonce)?;
    io.write_all(&std::process::id().to_be_bytes())?;
    // The owner never grants this request. Actual supervisor cleanup must kill/reap us.
    let mut byte = [0];
    let _ = io.read_exact(&mut byte);
    Ok(())
}
#[test]
fn signal_cleanup_owner_entry() -> Result<()> {
    let Some(mode) = std::env::var_os(SIGNAL_OWNER_MODE) else { return Ok(()); };
    let (transport, nonce, _parent) = transport::Transport::inherited()?;
    let mut control = transport::ActiveIo { transport, deadline: Instant::now() + Duration::from_secs(9), check: &|| Ok(()) };
    protocol::expect_handshake(&mut control, protocol::HELLO, &nonce)?;
    protocol::handshake(&mut control, protocol::ACK, &nonce)?;
    let mut cancellation = Some(crate::record::unix::Cancellation::new()?);
    let (leaf_ready, ready) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(1);
    let caller = std::thread::spawn(move || -> Result<(), CaptureError> {
        super::super::execute_owned(super::super::AdmissionPriority::OptionalCredentials, &|| Ok(()), Duration::from_secs(1), move |_, _, owned| {
            let mut command = signal_fixture_command("capture_worker::credentials::tests::signal_cleanup_leaf_entry", SIGNAL_LEAF_MODE, "1");
            super::super::private_environment(&mut command);
            let prepared = transport::Prepared::new(&mut command).map_err(|_| invalid())?;
            let job = super::super::launch(&mut command, &prepared).map_err(|_| invalid())?;
            *super::super::lock(&owned) = Some(job);
            let (transport, nonce) = prepared.parent();
            let mut io = transport::ActiveIo { transport, deadline: Instant::now() + Duration::from_secs(5), check: &|| Ok(()) };
            protocol::handshake(&mut io, protocol::HELLO, &nonce).map_err(|_| invalid())?;
            protocol::expect_handshake(&mut io, protocol::ACK, &nonce).map_err(|_| invalid())?;
            let mut pid = [0; 4];
            io.read_exact(&mut pid).map_err(|_| invalid())?;
            // Holding the real Job lock deliberately keeps actual child cleanup pending.
            let authority = super::super::lock(&owned);
            leaf_ready.send((u32::from_be_bytes(pid), owned.clone())).map_err(|_| invalid())?;
            let released = released.recv_timeout(Duration::from_secs(5)).is_ok();
            drop(authority);
            drop(io);
            let outcome = if released { CaptureError::NativeCanceled } else { CaptureError::NativeTimeout };
            super::super::finish_owned(Err(outcome), &owned)
        })
    });
    let (leaf_pid, owned) = ready.recv_timeout(Duration::from_secs(3))?;
    assert!(matches!(caller.join().unwrap(), Err(CaptureError::NativeCleanupPending)));
    assert!(super::super::BUSY.load(Ordering::Acquire));
    assert!(super::super::lock(&super::super::PENDING).is_some());
    if mode == "drop" { drop(cancellation.take()); }
    else { assert_eq!(mode, "output"); cancellation.as_ref().unwrap().output_phase(true); }
    let drain = std::thread::spawn(super::super::drain_owned_workers);
    let until = Instant::now() + Duration::from_secs(2);
    while super::super::lock(&super::super::PENDING).is_some() {
        assert!(Instant::now() < until, "Actual drain never took pending ownership");
        std::thread::sleep(Duration::from_millis(1));
    }
    // Drain has taken ownership and is blocked on the deliberately held real child Job.
    assert!(super::super::BUSY.load(Ordering::Acquire));
    control.write_all(&[1])?;
    control.write_all(&leaf_pid.to_be_bytes())?;
    let mut byte = [0];
    control.read_exact(&mut byte)?;
    assert_eq!(byte, [2]);
    release.send(())?;
    drain.join().unwrap();
    assert!(!super::super::BUSY.load(Ordering::Acquire));
    assert!(super::super::lock(&owned).is_none(), "Actual leaf family was not reaped");
    control.write_all(&[3])?;
    drop(cancellation);
    Ok(())
}
#[test]
#[ignore = "actual private owner/leaf subprocesses and SIGINT/SIGTERM only; exact serial with outer 20 s timeout"]
fn pending_credential_drain_survives_signals_after_drop_and_output_phase() {
    use std::os::fd::AsRawFd;
    struct Owner(super::super::Job);
    impl Drop for Owner {
        fn drop(&mut self) { self.0.stop(); self.0.wait(); }
    }
    for mode in ["drop", "output"] {
        for signal in [libc::SIGINT, libc::SIGTERM] {
            settle_priority_fixture();
            let mut command = signal_fixture_command("capture_worker::credentials::tests::signal_cleanup_owner_entry", SIGNAL_OWNER_MODE, mode);
            super::super::private_environment(&mut command);
            let prepared = transport::Prepared::new(&mut command).unwrap();
            let mut owner = Owner(super::super::launch(&mut command, &prepared).unwrap());
            let (transport, nonce) = prepared.parent();
            let mut io = transport::ActiveIo { transport, deadline: Instant::now() + Duration::from_secs(9), check: &|| Ok(()) };
            protocol::handshake(&mut io, protocol::HELLO, &nonce).unwrap();
            protocol::expect_handshake(&mut io, protocol::ACK, &nonce).unwrap();
            let mut phase = [0]; io.read_exact(&mut phase).unwrap(); assert_eq!(phase, [1]);
            let mut pid = [0; 4]; io.read_exact(&mut pid).unwrap();
            let leaf = super::super::completion_event(i32::try_from(u32::from_be_bytes(pid)).unwrap()).expect("Private fixture requires a native pidfd");
            let mut leaf_event = libc::pollfd { fd: leaf.as_raw_fd(), events: libc::POLLIN, revents: 0 };
            assert_eq!(unsafe { libc::poll(&mut leaf_event, 1, 0) }, 0, "Leaf exited before the signal phase");
            assert_eq!(unsafe { libc::kill(owner.0.group, signal) }, 0);
            std::thread::sleep(Duration::from_millis(100));
            assert!(!owner.0.reaped(), "Signal bypassed retained native ownership drain");
            assert_eq!(unsafe { libc::poll(&mut leaf_event, 1, 0) }, 0, "Leaf was released before the cleanup gate");
            io.write_all(&[2]).unwrap();
            io.read_exact(&mut phase).unwrap(); assert_eq!(phase, [3]);
            let until = Instant::now() + Duration::from_secs(3);
            while !owner.0.family_done() {
                assert!(Instant::now() < until, "Owner failed to exit after actual family reap");
                owner.0.wait_ready(Duration::from_millis(20));
            }
            assert_eq!(owner.0.status, Some(true));
            assert_eq!(unsafe { libc::poll(&mut leaf_event, 1, 0) }, 1, "Leaf survived confirmed owner cleanup");
            settle_priority_fixture();
        }
    }
}

#[test]
#[ignore = "isolated atomic admission race fixture; exact serial with outer 20 s timeout"]
fn signal_exit_reservation_excludes_concurrent_admission() {
    for _ in 0..32 {
        settle_priority_fixture();
        let start = Arc::new(std::sync::Barrier::new(3));
        let (release, released) = mpsc::sync_channel(1);
        let factory_called = Arc::new(AtomicBool::new(false));
        let admission = {
            let start = start.clone(); let called = factory_called.clone();
            std::thread::spawn(move || {
                start.wait();
                super::super::execute_owned(super::super::AdmissionPriority::OptionalCredentials, &|| Ok(()), Duration::from_secs(2), move |_, _, _| {
                    called.store(true, Ordering::Release);
                    released.recv_timeout(Duration::from_secs(1)).unwrap();
                    Ok(())
                })
            })
        };
        let exit = {
            let start = start.clone();
            std::thread::spawn(move || { start.wait(); super::super::reserve_signal_shutdown() })
        };
        start.wait();
        let exit_reserved = exit.join().unwrap();
        if exit_reserved {
            assert!(matches!(admission.join().unwrap(), Err(CaptureError::NativeBusy)));
            assert!(!factory_called.load(Ordering::Acquire));
            assert!(super::super::lock(&super::super::ACTIVE).is_none());
            // Only this isolated test returns after reserving exit. Production immediately _exit's.
            crate::file_clipboard::release_signal_shutdown_for_test();
            super::super::BUSY.store(false, Ordering::SeqCst);
        } else {
            assert!(super::super::BUSY.load(Ordering::Acquire));
            release.send(()).unwrap();
            assert!(admission.join().unwrap().is_ok());
            assert!(factory_called.load(Ordering::Acquire));
        }
        settle_priority_fixture();
    }
}

const EMPTY_SIGNAL_MODE: &str = "SCREENSHOTS_CREDENTIAL_EMPTY_SIGNAL";
#[test]
fn empty_signal_exit_entry() -> Result<()> {
    if std::env::var_os(EMPTY_SIGNAL_MODE).is_none() { return Ok(()); }
    let (transport, nonce, _parent) = transport::Transport::inherited()?;
    let mut io = transport::ActiveIo { transport, deadline: Instant::now() + Duration::from_secs(3), check: &|| Ok(()) };
    protocol::expect_handshake(&mut io, protocol::HELLO, &nonce)?;
    protocol::handshake(&mut io, protocol::ACK, &nonce)?;
    let cancellation = crate::record::unix::Cancellation::new()?;
    drop(cancellation);
    io.write_all(&[1])?;
    let mut byte = [0];
    io.read_exact(&mut byte)?;
    anyhow::bail!("Finished standalone command ignored a termination signal without native ownership")
}
#[test]
#[ignore = "private standalone signal-exit subprocesses only; exact serial with outer 20 s timeout"]
fn finished_standalone_commands_still_exit_when_no_native_owner_exists() {
    struct Owner(super::super::Job);
    impl Drop for Owner { fn drop(&mut self) { self.0.stop(); self.0.wait(); } }
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let mut command = signal_fixture_command("capture_worker::credentials::tests::empty_signal_exit_entry", EMPTY_SIGNAL_MODE, "1");
        super::super::private_environment(&mut command);
        let prepared = transport::Prepared::new(&mut command).unwrap();
        let mut owner = Owner(super::super::launch(&mut command, &prepared).unwrap());
        let (transport, nonce) = prepared.parent();
        let mut io = transport::ActiveIo { transport, deadline: Instant::now() + Duration::from_secs(3), check: &|| Ok(()) };
        protocol::handshake(&mut io, protocol::HELLO, &nonce).unwrap();
        protocol::expect_handshake(&mut io, protocol::ACK, &nonce).unwrap();
        let mut phase = [0]; io.read_exact(&mut phase).unwrap(); assert_eq!(phase, [1]);
        assert_eq!(unsafe { libc::kill(owner.0.group, signal) }, 0);
        let until = Instant::now() + Duration::from_secs(2);
        while !owner.0.family_done() {
            assert!(Instant::now() < until);
            owner.0.wait_ready(Duration::from_millis(20));
        }
        assert_eq!(owner.0.child.try_wait().unwrap().unwrap().code(), Some(1));
    }
}
