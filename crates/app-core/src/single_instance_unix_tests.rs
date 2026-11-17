//! Every endpoint and subprocess is private; these tests never connect a user desktop.
use super::*;
use std::{
    os::unix::fs::FileTypeExt,
    process::{Child, Command, Stdio},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root =
            PathBuf::from("/tmp").join(format!("ss-launch-test-{}", uuid::Uuid::new_v4().simple()));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&root)
            .unwrap();
        Self(root)
    }
    fn owner(&self, key: &[u8]) -> UnixSingleInstance {
        match UnixSingleInstance::acquire_at(&self.0, key).unwrap() {
            UnixInstanceAdmission::Owner(owner) => owner,
            _ => panic!("fixture expected a fresh owner"),
        }
    }
    fn child(&self, action: &str, id: &str) -> ChildGuard {
        ChildGuard(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "single_instance_unix::tests::private_child",
                    "--nocapture",
                ])
                .env_clear()
                .env("HOME", &self.0)
                .env("XDG_RUNTIME_DIR", &self.0)
                .env("SCREENSHOTS_PRIVATE_LAUNCH_ROOT", &self.0)
                .env("SCREENSHOTS_PRIVATE_LAUNCH_ACTION", action)
                .env("SCREENSHOTS_PRIVATE_LAUNCH_ID", id)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn until(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "private fixture timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn ready_file(path: &Path) {
    std::fs::write(path, b"ready").unwrap();
}
fn send(owner: &UnixSingleInstance, value: &[u8]) -> UnixStream {
    let mut client = connect(
        &owner.directory.path.join(&owner.endpoint.name),
        Instant::now() + CLIENT_DEADLINE,
    )
    .unwrap();
    client.set_nonblocking(false).unwrap();
    client.set_read_timeout(Some(CLIENT_DEADLINE)).unwrap();
    client.set_write_timeout(Some(CLIENT_DEADLINE)).unwrap();
    client.write_all(value).unwrap();
    client.shutdown(Shutdown::Write).unwrap();
    client
}

#[test]
fn a_trickled_ack_cannot_restart_the_connected_forwarding_deadline() {
    let fixture = Fixture::new();
    let mut owner = fixture.owner(b"slow-ack");
    let socket = owner.socket.take().unwrap();
    let peer = std::thread::spawn(move || {
        assert!(
            wait(
                socket.as_raw_fd(),
                -1,
                libc::POLLIN,
                Some(Instant::now() + STARTUP_DEADLINE)
            )
            .unwrap()
        );
        let (mut stream, _) = socket.accept().unwrap();
        stream.set_nonblocking(false).unwrap();
        stream.set_read_timeout(Some(STARTUP_DEADLINE)).unwrap();
        let mut request = [0; REQUEST.len()];
        stream.read_exact(&mut request).unwrap();
        assert_eq!(&request, REQUEST);
        let mut end = [0];
        assert_eq!(stream.read(&mut end).unwrap(), 0);
        for byte in ACK {
            std::thread::sleep(Duration::from_millis(900));
            if stream.write_all(&[*byte]).is_err() {
                break;
            }
        }
    });
    let started = Instant::now();
    let result = owner
        .directory
        .forward(&owner.endpoint, started + STARTUP_DEADLINE);
    let elapsed = started.elapsed();
    peer.join().unwrap();
    assert!(
        result.is_err(),
        "a partial ACK restarted the operation deadline"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "forwarding exceeded its absolute deadline: {elapsed:?}"
    );
}

#[test]
fn request_queued_before_listener_is_not_lost_and_restarts_release_ownership() {
    let fixture = Fixture::new();
    let mut owner = fixture.owner(b"pre-listener");
    let mut pending = send(&owner, REQUEST);
    let mut events = owner.listen().unwrap();
    let mut ack = [0; 3];
    pending.read_exact(&mut ack).unwrap();
    assert_eq!(&ack, ACK);
    assert_eq!(events.blocking_recv(), Some(()));
    assert!(owner.listen().is_err());
    drop(owner);
    drop(fixture.owner(b"pre-listener"));
}

#[test]
fn launches_coalesce_to_one_pending_event_without_starting_another_owner() {
    let fixture = Fixture::new();
    let mut owner = fixture.owner(b"coalesce");
    let mut events = owner.listen().unwrap();
    for _ in 0..16 {
        assert!(matches!(
            UnixSingleInstance::acquire_at(&fixture.0, b"coalesce").unwrap(),
            UnixInstanceAdmission::Forwarded
        ));
    }
    assert_eq!(events.try_recv().unwrap(), ());
    assert!(events.try_recv().is_err());
    drop(owner);
}

#[test]
fn exact_protocol_refuses_trailing_bytes_arbitrary_commands_and_closed_receiver() {
    let fixture = Fixture::new();
    let mut owner = fixture.owner(b"protocol");
    let mut events = owner.listen().unwrap();
    for request in [
        b"QUIT\n".as_slice(),
        b"--open /private/image\n",
        b"",
        b"SCREENSHOT-SHOW\nX",
        b"SCREENSHOT-SHOW",
    ] {
        let mut client = send(&owner, request);
        let mut ack = [0; 3];
        assert_ne!(client.read(&mut ack).unwrap_or(0), 3);
        assert!(events.try_recv().is_err());
    }
    drop(events);
    let mut client = send(&owner, REQUEST);
    let mut ack = [0; 3];
    client.read_exact(&mut ack).unwrap();
    assert_eq!(&ack, b"NO\n");
}

#[test]
fn quit_wakes_accept_and_slow_client_without_waiting_for_the_client_deadline() {
    let fixture = Fixture::new();
    for slow in [false, true] {
        let mut owner = fixture.owner(if slow { b"slow" } else { b"idle" });
        let events = owner.listen().unwrap();
        let client = slow.then(|| {
            connect(
                &owner.directory.path.join(&owner.endpoint.name),
                Instant::now() + CLIENT_DEADLINE,
            )
            .unwrap()
        });
        let start = Instant::now();
        drop(owner);
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "shutdown should interrupt the native client wait"
        );
        drop(client);
        drop(events);
    }
}

#[test]
fn replaced_endpoint_and_foreign_lock_data_are_preserved() {
    let fixture = Fixture::new();
    let owner = fixture.owner(b"replacement");
    let endpoint = fixture.0.join(&owner.endpoint.name);
    std::fs::rename(&endpoint, fixture.0.join("moved-owned-socket")).unwrap();
    std::fs::write(&endpoint, b"unrelated user file").unwrap();
    drop(owner);
    assert_eq!(std::fs::read(&endpoint).unwrap(), b"unrelated user file");
    assert!(UnixSingleInstance::acquire_at(&fixture.0, b"replacement").is_err());
    use sha2_10::{Digest, Sha256};
    let foreign = fixture
        .0
        .join(format!("owner-{:x}", Sha256::digest(b"foreign-lock")));
    std::fs::write(&foreign, b"foreign bytes").unwrap();
    std::fs::set_permissions(
        &foreign,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .unwrap();
    assert!(UnixSingleInstance::acquire_at(&fixture.0, b"foreign-lock").is_err());
    assert_eq!(std::fs::read(&foreign).unwrap(), b"foreign bytes");
}

#[test]
fn unsafe_directory_symlink_and_nonregular_lock_are_rejected() {
    let fixture = Fixture::new();
    let alias = fixture.0.join("alias");
    std::os::unix::fs::symlink(&fixture.0, &alias).unwrap();
    assert!(Directory::open(&alias).is_err());
    let directory = fixture.0.join("unsafe");
    std::fs::DirBuilder::new()
        .mode(0o755)
        .create(&directory)
        .unwrap();
    // A restrictive caller umask must not make this unsafe fixture private.
    std::fs::set_permissions(
        &directory,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )
    .unwrap();
    assert!(Directory::open(&directory).is_err());
    use sha2_10::{Digest, Sha256};
    let lock = fixture
        .0
        .join(format!("owner-{:x}", Sha256::digest(b"fifo")));
    let name = CString::new(lock.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert!(UnixSingleInstance::acquire_at(&fixture.0, b"fifo").is_err());
    assert!(
        std::fs::symlink_metadata(lock)
            .unwrap()
            .file_type()
            .is_fifo()
    );
}

#[test]
fn interrupted_record_does_not_authorize_removal_of_an_unrecorded_socket() {
    let fixture = Fixture::new();
    let owner = fixture.owner(b"partial");
    let unrecorded = fixture
        .0
        .join(format!("s{}", uuid::Uuid::new_v4().simple()));
    let socket = UnixListener::bind(&unrecorded).unwrap();
    owner
        ._lock
        .write_all_at(b"partial", MAGIC.len() as u64)
        .unwrap();
    drop(owner);
    let next = fixture.owner(b"partial");
    assert!(unrecorded.exists());
    drop(next);
    drop(socket);
}

#[test]
fn actual_process_crash_reclaims_exact_stale_socket_and_preserves_lock_inode() {
    let fixture = Fixture::new();
    let mut child = fixture.child("owner", "crash");
    until(|| fixture.0.join("ready-crash").exists());
    use sha2_10::{Digest, Sha256};
    let lock_path = fixture
        .0
        .join(format!("owner-{:x}", Sha256::digest(b"process")));
    let before = std::fs::metadata(&lock_path).unwrap();
    let record = read_record(&OpenOptions::new().read(true).open(&lock_path).unwrap())
        .unwrap()
        .unwrap();
    assert!(fixture.0.join(&record.name).exists());
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());
    let owner = fixture.owner(b"process");
    assert!(!fixture.0.join(record.name).exists());
    let after = std::fs::metadata(lock_path).unwrap();
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
    drop(owner);
}

#[test]
fn actual_concurrent_launches_choose_one_owner_and_forward_all_others() {
    let fixture = Fixture::new();
    let mut children: Vec<_> = (0..8)
        .map(|index| fixture.child("contend", &index.to_string()))
        .collect();
    ready_file(&fixture.0.join("start"));
    until(|| (0..8).all(|index| fixture.0.join(format!("result-{index}")).exists()));
    let results: Vec<_> = (0..8)
        .map(|index| std::fs::read(fixture.0.join(format!("result-{index}"))).unwrap())
        .collect();
    assert_eq!(
        results
            .iter()
            .filter(|bytes| bytes.as_slice() == b"owner")
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|bytes| bytes.as_slice() == b"forwarded")
            .count(),
        7
    );
    ready_file(&fixture.0.join("stop"));
    for child in &mut children {
        assert!(child.0.wait().unwrap().success());
    }
    drop(fixture.owner(b"process"));
}

#[test]
fn every_scope_is_independent_and_native_descriptors_are_close_on_exec() {
    let fixture = Fixture::new();
    let owner = fixture.owner(b"session-one");
    let other = fixture.owner(b"session-two");
    assert_ne!(owner.endpoint.name, other.endpoint.name);
    for fd in [
        owner._lock.as_raw_fd(),
        owner.directory.file.as_raw_fd(),
        owner.socket.as_ref().unwrap().as_raw_fd(),
    ] {
        assert_ne!(
            unsafe { libc::fcntl(fd, libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
    }
}

#[test]
fn x11_server_identity_ignores_screen_and_local_aliases_but_separates_servers() {
    assert_eq!(scope::x11_scope(b":0001.2"), scope::x11_scope(b"unix:1.0"));
    assert_eq!(scope::x11_scope(b"localhost:1"), scope::x11_scope(b":1"));
    assert_ne!(scope::x11_scope(b":1"), scope::x11_scope(b":2"));
    assert_ne!(scope::x11_scope(b"remote:1"), scope::x11_scope(b":1"));
    for value in [b"".as_slice(), b":", b":1.x", b"bad\n:1", b":4294967296"] {
        assert!(scope::x11_scope(value).is_none());
    }
}

#[cfg(target_os = "linux")]
#[test]
fn inherited_wayland_authority_is_stable_across_descriptor_renumbering() {
    use std::os::unix::process::CommandExt;
    let fixture = Fixture::new();
    let (socket, _peer) = UnixStream::pair().unwrap();
    let expected = scope::wayland_peer(socket.as_raw_fd()).unwrap().unwrap();
    for fd in [100, 101] {
        let actual = socket.as_raw_fd();
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args(["--exact", "single_instance_unix::tests::private_child"])
            .env_clear()
            .env("HOME", &fixture.0)
            .env("XDG_RUNTIME_DIR", &fixture.0)
            .env("SCREENSHOTS_PRIVATE_LAUNCH_ROOT", &fixture.0)
            .env("SCREENSHOTS_PRIVATE_LAUNCH_ACTION", "scope")
            .env("SCREENSHOTS_PRIVATE_LAUNCH_ID", fd.to_string())
            .env("WAYLAND_SOCKET", fd.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(actual, fd) < 0 || libc::fcntl(fd, libc::F_SETFD, 0) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = ChildGuard(command.spawn().unwrap());
        assert!(child.0.wait().unwrap().success());
        assert_eq!(
            std::fs::read(fixture.0.join(format!("scope-{fd}"))).unwrap(),
            expected
        );
    }
}

#[test]
fn private_child() {
    let Some(root) = std::env::var_os("SCREENSHOTS_PRIVATE_LAUNCH_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    assert!(
        root.is_absolute()
            && root
                .file_name()
                .unwrap()
                .as_bytes()
                .starts_with(b"ss-launch-test-")
    );
    let action = std::env::var("SCREENSHOTS_PRIVATE_LAUNCH_ACTION").unwrap();
    let id = std::env::var("SCREENSHOTS_PRIVATE_LAUNCH_ID").unwrap();
    assert!(!id.is_empty() && id.bytes().all(|byte| byte.is_ascii_alphanumeric()));
    if action == "scope" {
        std::fs::write(
            root.join(format!("scope-{id}")),
            scope::current().unwrap().unwrap(),
        )
        .unwrap();
        return;
    }
    if action == "contend" {
        until(|| root.join("start").exists());
    }
    match UnixSingleInstance::acquire_at(&root, b"process").unwrap() {
        UnixInstanceAdmission::Owner(mut owner) => {
            let mut events = owner.listen().unwrap();
            ready_file(&root.join(format!("ready-{id}")));
            if action == "contend" {
                std::fs::write(root.join(format!("result-{id}")), b"owner").unwrap();
                until(|| root.join("stop").exists());
                assert_eq!(events.try_recv().unwrap(), ());
            } else {
                let mut stop = [0; 1];
                let _ = std::io::stdin().read_exact(&mut stop);
            }
        }
        UnixInstanceAdmission::Forwarded => {
            std::fs::write(root.join(format!("result-{id}")), b"forwarded").unwrap();
        }
        UnixInstanceAdmission::WindowedOnly(_) => panic!("explicit test scope cannot fall back"),
    }
}

#[test]
fn compositor_lifetime_parser_handles_parentheses_in_comm_and_rejects_missing_or_oversized_data() {
    let mut fields = vec!["0"; 20];
    fields[0] = "S";
    fields[19] = "12345";
    let bytes = format!("321 (synthetic ) compositor) {}\n", fields.join(" "));
    assert_eq!(scope::process_starttime(bytes.as_bytes()), Some(12345));
    assert!(scope::process_starttime(b"321 no-close-paren").is_none());
    assert!(scope::process_starttime(&vec![b'x'; 4097]).is_none());
    fields[19] = "0";
    assert!(
        scope::process_starttime(format!("321 (test) {}", fields.join(" ")).as_bytes()).is_none()
    );
}

#[test]
fn explicit_stop_refuses_later_show_but_keeps_owner_until_teardown() {
    let fixture = Fixture::new();
    let mut owner = fixture.owner(b"stop");
    let mut events = owner.listen().unwrap();
    owner.stop_accepting();
    assert_eq!(events.blocking_recv(), None);
    assert!(
        connect(
            &owner.directory.path.join(&owner.endpoint.name),
            Instant::now() + CLIENT_DEADLINE
        )
        .is_err()
    );
    use sha2_10::{Digest, Sha256};
    let lock = owner
        .directory
        .lock_file(&format!("owner-{:x}", Sha256::digest(b"stop")))
        .unwrap();
    // The lock is retained even after the listener has retired.
    assert_ne!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    drop(lock);
    drop(owner);
    drop(fixture.owner(b"stop"));
}
