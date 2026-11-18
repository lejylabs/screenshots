// Explicitly invoked private daemon only. The process environment is never changed.
// Run each ignored case --exact --test-threads=1 with an outer 20 s deadline.
// The outer owner must terminate/reap the entire private process tree on timeout;
// killing only this test process would bypass the daemon's Drop cleanup.
use dbus::{arg::Variant, blocking::Connection, channel::{MatchingReceiver, Sender}, message::MatchRule, Path};
use std::sync::atomic::AtomicUsize;

const CHILD_MODE: &str = "SCREENSHOTS_CREDENTIAL_PRIVATE_CHILD";
struct Daemon(std::process::Child);
impl Drop for Daemon {
    fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
}
struct FakeService {
    daemon: Daemon,
    thread: Option<std::thread::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    entered_search: Arc<AtomicBool>,
    mutations: Arc<AtomicUsize>,
    address: String,
}
impl FakeService {
    fn new(stall_search: bool) -> Self {
        use std::os::fd::AsRawFd;
        let mut daemon = Daemon(Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile", "--print-address=1"])
            .env_clear().stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap());
        let mut output = daemon.0.stdout.take().unwrap();
        let fd = output.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert!(flags >= 0 && unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == 0);
        let until = Instant::now() + Duration::from_secs(2);
        let mut address = Vec::new();
        loop {
            assert!(Instant::now() < until && address.len() < 4096, "Private daemon did not provide bounded readiness");
            let mut byte = [0];
            match output.read(&mut byte) {
                Ok(1) if byte[0] == b'\n' => break,
                Ok(1) => address.push(byte[0]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(1)),
                _ => panic!("Private daemon failed before readiness"),
            }
        }
        let address = String::from_utf8(address).unwrap();
        assert!(address.starts_with("unix:"));
        let stop = Arc::new(AtomicBool::new(false));
        let entered_search = Arc::new(AtomicBool::new(false));
        let mutations = Arc::new(AtomicUsize::new(0));
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = {
            let address = address.clone(); let stop = stop.clone();
            let entered_search = entered_search.clone(); let mutations = mutations.clone();
            std::thread::spawn(move || {
                let connection = Connection::new_address(&address).unwrap();
                let name = connection.request_name("org.freedesktop.secrets", false, false, true).unwrap();
                assert_eq!(name, dbus::blocking::stdintf::org_freedesktop_dbus::RequestNameReply::PrimaryOwner);
                let stored = Arc::new(Mutex::new(Zeroizing::new(b"synthetic-fixture-only".to_vec())));
                connection.start_receive(MatchRule::new_method_call(), Box::new(move |message, connection| {
                    let member = message.member().map(|value| value.to_string()).unwrap_or_default();
                    let item = || Path::new("/org/freedesktop/secrets/item/synthetic").unwrap();
                    let session = || Path::new("/org/freedesktop/secrets/session/synthetic").unwrap();
                    let reply = match member.as_str() {
                        "OpenSession" => {
                            if stall_search { std::thread::sleep(Duration::from_millis(100)); }
                            message.method_return().append2(Variant(String::new()), session())
                        },
                        "SearchItems" => {
                            entered_search.store(true, Ordering::Release);
                            if stall_search { std::thread::sleep(Duration::from_millis(650)); }
                            message.method_return().append2(vec![item()], Vec::<Path<'static>>::new())
                        }
                        "GetSecret" => message.method_return().append1((session(), Vec::<u8>::new(), stored.lock().unwrap().to_vec(), "application/octet-stream".to_string())),
                        "SetSecret" => {
                            let ((_, _, bytes, _),): ((Path<'static>, Vec<u8>, Vec<u8>, String),) = message.read_all().unwrap();
                            *stored.lock().unwrap() = Zeroizing::new(bytes);
                            mutations.fetch_add(1, Ordering::SeqCst);
                            message.method_return()
                        }
                        "Delete" => {
                            mutations.fetch_add(1, Ordering::SeqCst);
                            message.method_return().append1(Path::new("/").unwrap())
                        }
                        "Close" => message.method_return(),
                        _ => panic!("Unexpected fake Secret Service method"),
                    };
                    // A client killed by the operation deadline can no longer consume this reply.
                    let _ = connection.send(reply);
                    true
                }));
                ready_tx.send(()).unwrap();
                let deadline = Instant::now() + Duration::from_secs(8);
                while !stop.load(Ordering::Acquire) && Instant::now() < deadline {
                    connection.process(Duration::from_millis(20)).unwrap();
                }
            })
        };
        ready_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        Self { daemon, thread: Some(thread), stop, entered_search, mutations, address }
    }
    fn command(&self) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "capture_worker::credentials::tests::private_credential_child_entry", "--nocapture"])
            .env(CHILD_MODE, "1").env("DBUS_SESSION_BUS_ADDRESS", &self.address)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        command
    }
    fn finish(mut self, expected_mutations: usize) {
        // Keep serving beyond the delayed SearchItems reply so a stale client would
        // have time to issue SetSecret/Delete and be counted as a failure.
        std::thread::sleep(Duration::from_millis(800));
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() { thread.join().unwrap(); }
        assert_eq!(self.mutations.load(Ordering::SeqCst), expected_mutations, "Unexpected native mutation request count");
    }
}
impl Drop for FakeService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
        let _ = self.daemon.0.kill();
    }
}
#[test]
fn private_credential_child_entry() -> Result<()> {
    if std::env::var_os(CHILD_MODE).is_none() { return Ok(()); }
    serve()
}
fn synthetic_write() -> NativeCredentialCall {
    NativeCredentialCall::Write { target: "Screenshots/upload/https://example.invalid/private_fixture".into(), payload: Zeroizing::new(b"synthetic-private-fixture".to_vec()) }
}
#[test]
#[ignore = "owned private dbus-daemon/fake Secret Service only; run --exact serial with outer 20 s timeout"]
fn stalled_secret_service_deadline_reaps_and_prevents_late_mutation() {
    let service = FakeService::new(true);
    let command = service.command();
    let start = Instant::now();
    let result = execute_factory(synthetic_write(), &|| Ok(()), Duration::from_millis(400), move || Ok(command));
    assert!(matches!(result, Err(CaptureError::NativeTimeout)));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(service.entered_search.load(Ordering::Acquire), "Fixture must reach the real native SearchItems call");
    assert!(!super::super::BUSY.load(Ordering::Acquire));
    assert!(super::super::lock(&super::super::PENDING).is_none());
    service.finish(0);
}
#[test]
#[ignore = "owned private dbus-daemon/fake Secret Service only; run --exact serial with outer 20 s timeout"]
fn canceled_secret_service_reaps_and_prevents_late_mutation() {
    let service = FakeService::new(true);
    let command = service.command();
    let entered = service.entered_search.clone();
    let start = Instant::now();
    let result = execute_factory(synthetic_write(), &|| if entered.load(Ordering::Acquire) { Err(CaptureError::NativeCanceled) } else { Ok(()) }, Duration::from_secs(3), move || Ok(command));
    assert!(matches!(result, Err(CaptureError::NativeCanceled)));
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(service.entered_search.load(Ordering::Acquire));
    assert!(!super::super::BUSY.load(Ordering::Acquire));
    assert!(super::super::lock(&super::super::PENDING).is_none());
    service.finish(0);
}

#[test]
#[ignore = "owned private dbus-daemon/fake Secret Service only; run --exact serial with outer 20 s timeout"]
fn private_secret_service_read_write_remove_roundtrip() {
    let service = FakeService::new(false);
    let target = "Screenshots/upload/https://example.invalid/private_fixture";
    let command = service.command();
    let reply = execute_factory(synthetic_write(), &|| Ok(()), TIMEOUT, move || Ok(command)).unwrap();
    assert!(reply.is_empty());
    let command = service.command();
    let reply = execute_factory(NativeCredentialCall::Read { target: target.into() }, &|| Ok(()), TIMEOUT, move || Ok(command)).unwrap();
    assert_eq!(reply.as_slice(), b"synthetic-private-fixture");
    drop(reply);
    let command = service.command();
    let reply = execute_factory(NativeCredentialCall::Remove { target: target.into() }, &|| Ok(()), TIMEOUT, move || Ok(command)).unwrap();
    assert!(reply.is_empty());
    assert!(!super::super::BUSY.load(Ordering::Acquire));
    assert!(super::super::lock(&super::super::PENDING).is_none());
    service.finish(2);
}

const PRIORITY_CHILD: &str = "SCREENSHOTS_CREDENTIAL_PRIORITY_CAPTURE_CHILD";
const PRIORITY_MARKER: &str = "SCREENSHOTS_CREDENTIAL_PRIORITY_CAPTURE_MARKER";
fn priority_capture_call(enumeration: bool) -> NativeCaptureCall {
    if enumeration { NativeCaptureCall::Displays }
    else { NativeCaptureCall::HideCaptureWindow { window: 1 } }
}
fn priority_capture_command() -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.args(["--exact", "capture_worker::credentials::tests::priority_capture_child_entry", "--nocapture"])
        .env(PRIORITY_CHILD, "1").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    command
}
#[test]
fn priority_capture_child_entry() -> Result<()> {
    if std::env::var_os(PRIORITY_CHILD).is_none() { return Ok(()); }
    let (transport, nonce, _parent) = transport::Transport::inherited()?;
    let mut io = transport::ActiveIo { transport, deadline: Instant::now() + Duration::from_secs(3), check: &|| Ok(()) };
    protocol::expect_handshake(&mut io, protocol::HELLO, &nonce)?;
    protocol::handshake(&mut io, protocol::ACK, &nonce)?;
    if let Some(marker) = std::env::var_os(PRIORITY_MARKER) {
        std::fs::write(marker, b"synthetic child launched")?;
    }
    match protocol::read_request(&mut io)? {
        NativeCaptureCall::Displays => protocol::reply(&mut io, NativeCaptureReply::Displays(Vec::new()))?,
        NativeCaptureCall::HideCaptureWindow { .. } => protocol::reply(&mut io, NativeCaptureReply::CaptureWindowHidden)?,
        _ => anyhow::bail!("Unexpected private priority capture fixture request"),
    }
    Ok(())
}
fn settle_priority_fixture() {
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        match super::super::refresh_pending() {
            Ok(()) => break,
            Err(_) => {
                assert!(Instant::now() < until, "Owned priority fixture cleanup did not finish");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
    assert!(!super::super::BUSY.load(Ordering::Acquire));
    assert!(super::super::lock(&super::super::PENDING).is_none());
}
#[test]
#[ignore = "private fake Secret Service plus authenticated fake capture child; exact serial with outer 20 s timeout"]
fn both_capture_priorities_preempt_credentials_after_actual_cleanup() {
    for enumeration in [false, true] {
        settle_priority_fixture();
        let service = FakeService::new(true);
        let command = service.command();
        let owner = std::thread::spawn(move || execute_factory(synthetic_write(), &|| Ok(()), TIMEOUT, move || Ok(command)));
        let until = Instant::now() + Duration::from_secs(3);
        while !service.entered_search.load(Ordering::Acquire) {
            assert!(Instant::now() < until, "Credential child did not reach its native provider");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(super::super::lock(&super::super::ACTIVE).as_ref().unwrap().priority, super::super::AdmissionPriority::OptionalCredentials);
        let command = priority_capture_command();
        let started = Instant::now();
        let reply = super::super::execute_factory(move || Ok(command), priority_capture_call(enumeration), &|| Ok(()), Duration::from_secs(3)).unwrap();
        if enumeration { assert!(matches!(reply, NativeCaptureReply::Displays(_))); }
        else { assert!(matches!(reply, NativeCaptureReply::CaptureWindowHidden)); }
        assert!(started.elapsed() < Duration::from_secs(2), "Capture waited for the credential operation deadline");
        assert!(matches!(owner.join().unwrap(), Err(CaptureError::NativeCanceled)));
        settle_priority_fixture();
        service.finish(0);
    }
}
#[test]
#[ignore = "authenticated fake capture child only; exact serial with outer 20 s timeout"]
fn credentials_cannot_cancel_capture_or_capture_enumeration() {
    for enumeration in [false, true] {
        settle_priority_fixture();
        let command = priority_capture_command();
        let (started, start_seen) = mpsc::sync_channel(1);
        let (release, released) = mpsc::sync_channel(1);
        let owner = std::thread::spawn(move || super::super::execute_factory(move || {
            started.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(3)).unwrap();
            Ok(command)
        }, priority_capture_call(enumeration), &|| Ok(()), Duration::from_secs(4)));
        start_seen.recv_timeout(Duration::from_secs(2)).unwrap();
        let factory_called = Arc::new(AtomicBool::new(false));
        let flag = factory_called.clone();
        let denied = execute_factory(synthetic_write(), &|| Ok(()), TIMEOUT, move || {
            flag.store(true, Ordering::Release);
            Ok(priority_capture_command())
        });
        assert!(matches!(denied, Err(CaptureError::NativeBusy)));
        assert!(!factory_called.load(Ordering::Acquire));
        assert!(!super::super::lock(&super::super::ACTIVE).as_ref().unwrap().cancel.load(Ordering::Acquire));
        release.send(()).unwrap();
        assert!(owner.join().unwrap().is_ok());
        settle_priority_fixture();
    }
}
#[test]
#[ignore = "blocked owned startup and authenticated fake child only; exact serial with outer 20 s timeout"]
fn credential_preemption_keeps_pending_cleanup_closed_to_every_priority() {
    settle_priority_fixture();
    struct Folder(std::path::PathBuf);
    impl Drop for Folder { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    let folder = Folder(std::env::temp_dir().join(format!("screenshots-credential-priority-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir(&folder.0).unwrap();
    let marker = folder.0.join("must-not-launch");
    let mut command = priority_capture_command();
    command.env(PRIORITY_MARKER, &marker);
    let (started, start_seen) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(1);
    let owner = std::thread::spawn(move || execute_factory(synthetic_write(), &|| Ok(()), TIMEOUT, move || {
        started.send(()).unwrap();
        released.recv_timeout(Duration::from_secs(3)).unwrap();
        // Returning a harmless fake child proves canceled startup never reaches launch.
        Ok(command)
    }));
    start_seen.recv_timeout(Duration::from_secs(2)).unwrap();
    let capture_factory_called = Arc::new(AtomicBool::new(false));
    let flag = capture_factory_called.clone();
    let capture = super::super::execute_factory(move || {
        flag.store(true, Ordering::Release);
        Ok(priority_capture_command())
    }, priority_capture_call(false), &|| Ok(()), Duration::from_secs(2));
    assert!(matches!(capture, Err(CaptureError::NativeCleanupPending)));
    assert!(!capture_factory_called.load(Ordering::Acquire));
    assert!(matches!(owner.join().unwrap(), Err(CaptureError::NativeCleanupPending)));
    assert!(super::super::BUSY.load(Ordering::Acquire));
    assert!(super::super::lock(&super::super::PENDING).is_some());
    for priority in [super::super::AdmissionPriority::Capture, super::super::AdmissionPriority::CaptureEnumeration, super::super::AdmissionPriority::OptionalCredentials] {
        let factory_called = Arc::new(AtomicBool::new(false));
        let flag = factory_called.clone();
        let result: Result<(), CaptureError> = super::super::execute_owned(priority, &|| Ok(()), Duration::from_secs(1), move |_, _, _| {
            flag.store(true, Ordering::Release);
            Ok(())
        });
        assert!(matches!(result, Err(CaptureError::NativeCleanupPending)));
        assert!(!factory_called.load(Ordering::Acquire));
    }
    release.send(()).unwrap();
    settle_priority_fixture();
    assert!(!marker.exists(), "Canceled optional startup launched after its owner returned");
    let command = priority_capture_command();
    assert!(super::super::execute_factory(move || Ok(command), priority_capture_call(false), &|| Ok(()), Duration::from_secs(2)).is_ok());
    settle_priority_fixture();
}
