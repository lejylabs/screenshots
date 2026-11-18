//! User/session-scoped Show-only ownership, with no native worker until `listen`.
use anyhow::{Context, Result, bail, ensure};
use std::{
    ffi::CString,
    fs::{File, OpenOptions},
    io::{Read, Write},
    net::Shutdown,
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        unix::{
            ffi::OsStrExt,
            fs::{DirBuilderExt, FileExt, MetadataExt, OpenOptionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

#[path = "single_instance_unix_scope.rs"]
mod scope;

const MAGIC: &[u8] = b"screenshots-reopen-v1\n";
const REQUEST: &[u8] = b"SCREENSHOT-SHOW\n";
const ACK: &[u8] = b"OK\n";
const CLIENT_DEADLINE: Duration = Duration::from_secs(2);
const STARTUP_DEADLINE: Duration = Duration::from_secs(5);

/// Scope discovery can lack a stable authority in a proxied/namespace session.
/// Such a process may keep the existing visible-window fallback, with no global services.
pub enum UnixInstanceAdmission {
    Owner(UnixSingleInstance),
    Forwarded,
    WindowedOnly(&'static str),
}

pub struct UnixSingleInstance {
    directory: Directory,
    _lock: File,
    endpoint: Record,
    socket: Option<UnixListener>,
    stop: Option<UnixStream>,
    listener: Option<JoinHandle<()>>,
}

impl UnixSingleInstance {
    /// Call before native platform/service threads. This only inspects authority and
    /// acquires resources; GPUI remains responsible for consuming startup environment.
    pub fn acquire() -> Result<UnixInstanceAdmission> {
        let Some(scope) = scope::current()? else {
            return Ok(UnixInstanceAdmission::WindowedOnly(
                "Graphical-session identity is unavailable; using a separate visible window without global desktop services",
            ));
        };
        #[cfg(target_os = "linux")]
        let temporary = Path::new("/tmp");
        #[cfg(target_os = "macos")]
        let temporary = Path::new("/private/tmp");
        Self::acquire_at(
            &temporary.join(format!("screenshots-{}-launch-v1", unsafe {
                libc::geteuid()
            })),
            &scope,
        )
    }

    fn acquire_at(path: &Path, scope: &[u8]) -> Result<UnixInstanceAdmission> {
        use sha2_10::{Digest, Sha256};
        let directory = Directory::open(path)?;
        let key = format!("owner-{:x}", Sha256::digest(scope));
        let lock = directory.lock_file(&key)?;
        let deadline = Instant::now() + STARTUP_DEADLINE;
        loop {
            if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                if let Some(previous) = read_record(&lock)? {
                    directory.remove_owned_socket(&previous)?;
                }
                // Random endpoint names avoid blocking recovery after a crash during record publication.
                let name = format!("s{}", uuid::Uuid::new_v4().simple());
                let socket_path = directory.path.join(&name);
                ensure!(
                    socket_path.as_os_str().as_bytes().len() < 100,
                    "Launch endpoint path exceeds its native limit"
                );
                let socket = UnixListener::bind(&socket_path)
                    .context("Could not bind private launch endpoint")?;
                let info = std::fs::symlink_metadata(&socket_path)?;
                let endpoint = Record {
                    name,
                    device: info.dev(),
                    inode: info.ino(),
                };
                let result = (|| {
                    let name = CString::new(endpoint.name.as_bytes())?;
                    ensure!(
                        unsafe {
                            libc::fchmodat(directory.file.as_raw_fd(), name.as_ptr(), 0o600, 0)
                        } == 0,
                        "Could not protect launch endpoint"
                    );
                    socket.set_nonblocking(true)?;
                    ensure!(
                        unsafe { libc::listen(socket.as_raw_fd(), 8) } == 0,
                        "Could not bound launch endpoint backlog"
                    );
                    write_record(&lock, &endpoint)?;
                    Ok(())
                })();
                if let Err(error) = result {
                    drop(socket);
                    let _ = directory.remove_owned_socket(&endpoint);
                    return Err(error);
                }
                return Ok(UnixInstanceAdmission::Owner(Self {
                    directory,
                    _lock: lock,
                    endpoint,
                    socket: Some(socket),
                    stop: None,
                    listener: None,
                }));
            }
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted && Instant::now() < deadline {
                continue;
            }
            ensure!(
                error.kind() == std::io::ErrorKind::WouldBlock,
                "Could not acquire launch ownership"
            );
            if let Some(record) = read_record(&lock)? {
                if directory.forward(&record, deadline)? {
                    return Ok(UnixInstanceAdmission::Forwarded);
                }
            }
            ensure!(
                Instant::now() < deadline,
                "The running app did not accept Show; another app was not started"
            );
            // Bounded startup contention only; the resident listener never polls a timer.
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Stop accepting Show before GUI quit begins; ownership stays held until Drop.
    pub fn stop_accepting(&self) {
        if let Some(stop) = &self.stop {
            let _ = stop.shutdown(Shutdown::Both);
        }
    }

    /// Call only once the native startup environment is consumed and GUI event
    /// delivery is ready. One pending Show coalesces further launches.
    pub fn listen(&mut self) -> Result<mpsc::Receiver<()>> {
        ensure!(
            self.listener.is_none(),
            "Launch listener is already running"
        );
        let socket = self
            .socket
            .take()
            .context("Launch listener is unavailable")?;
        let (read, write) = UnixStream::pair()?;
        let (sender, receiver) = mpsc::channel(1);
        let listener = std::thread::Builder::new()
            .name("launch-events".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let result = listen(&socket, &read, &sender);
                // Channel EOF must mean the native listener is already retired.
                drop(socket);
                drop(read);
                drop(sender);
                if result.is_err() {
                    tracing::error!("Launch event listener failed");
                }
            })?;
        self.stop = Some(write);
        self.listener = Some(listener);
        Ok(receiver)
    }
}

impl Drop for UnixSingleInstance {
    fn drop(&mut self) {
        self.stop_accepting();
        self.stop.take();
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
        self.socket.take();
        if self.directory.remove_owned_socket(&self.endpoint).is_err() {
            tracing::warn!("Could not remove owned launch endpoint");
        }
        // The lock file remains: unlinking would allow waiters to lock different inodes.
    }
}

struct Record {
    name: String,
    device: u64,
    inode: u64,
}
struct Directory {
    path: PathBuf,
    file: File,
}
impl Directory {
    fn open(path: &Path) -> Result<Self> {
        match std::fs::DirBuilder::new().mode(0o700).create(path) {
            Ok(()) => {
                std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o700))?
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error).context("Could not create private launch directory"),
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .context("Could not open private launch directory")?;
        let info = file.metadata()?;
        ensure!(
            info.is_dir()
                && info.uid() == unsafe { libc::geteuid() }
                && info.mode() & 0o777 == 0o700,
            "Launch directory must be private and current-user-owned"
        );
        Ok(Self {
            path: path.to_owned(),
            file,
        })
    }

    fn lock_file(&self, name: &str) -> Result<File> {
        let name = CString::new(name)?;
        let open = || unsafe {
            libc::openat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDWR | libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        let mut fd = open();
        if fd < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
            let temporary = CString::new(format!("l{}", uuid::Uuid::new_v4().simple()))?;
            let staging = unsafe {
                libc::openat(
                    self.file.as_raw_fd(),
                    temporary.as_ptr(),
                    libc::O_WRONLY
                        | libc::O_CREAT
                        | libc::O_EXCL
                        | libc::O_NOFOLLOW
                        | libc::O_CLOEXEC,
                    0o600,
                )
            };
            ensure!(staging >= 0, "Could not stage launch ownership");
            let mut staging = unsafe { File::from_raw_fd(staging) };
            let result = (|| {
                ensure!(
                    unsafe { libc::fchmod(staging.as_raw_fd(), 0o600) } == 0,
                    "Could not protect launch ownership"
                );
                staging.write_all(MAGIC)?;
                staging.sync_all()?;
                #[cfg(target_os = "linux")]
                let published = unsafe {
                    libc::renameat2(
                        self.file.as_raw_fd(),
                        temporary.as_ptr(),
                        self.file.as_raw_fd(),
                        name.as_ptr(),
                        libc::RENAME_NOREPLACE,
                    )
                };
                #[cfg(target_os = "macos")]
                let published = unsafe {
                    libc::renameatx_np(
                        self.file.as_raw_fd(),
                        temporary.as_ptr(),
                        self.file.as_raw_fd(),
                        name.as_ptr(),
                        libc::RENAME_EXCL,
                    )
                };
                ensure!(
                    published == 0
                        || std::io::Error::last_os_error().kind()
                            == std::io::ErrorKind::AlreadyExists,
                    "Could not publish launch ownership"
                );
                Ok(())
            })();
            unsafe {
                libc::unlinkat(self.file.as_raw_fd(), temporary.as_ptr(), 0);
            }
            result?;
            fd = open();
        }
        ensure!(fd >= 0, "Could not open launch ownership");
        let file = unsafe { File::from_raw_fd(fd) };
        let info = file.metadata()?;
        ensure!(
            info.is_file()
                && info.uid() == unsafe { libc::geteuid() }
                && info.mode() & 0o777 == 0o600
                && info.nlink() == 1
                && (MAGIC.len() as u64..=128).contains(&info.len()),
            "Launch ownership file conflicts with private app state"
        );
        let mut magic = vec![0; MAGIC.len()];
        file.read_exact_at(&mut magic, 0)?;
        ensure!(magic == MAGIC, "Launch ownership file has a foreign marker");
        Ok(file)
    }

    fn socket_info(&self, record: &Record) -> Result<Option<libc::stat>> {
        let name = CString::new(record.name.as_bytes())?;
        let mut info: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::fstatat(
                self.file.as_raw_fd(),
                name.as_ptr(),
                &mut info,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } != 0
        {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(error).context("Could not inspect launch endpoint");
        }
        ensure!(
            info.st_dev as u64 == record.device
                && info.st_ino as u64 == record.inode
                && info.st_uid == unsafe { libc::geteuid() }
                && info.st_mode & libc::S_IFMT == libc::S_IFSOCK
                && info.st_mode & 0o777 == 0o600
                && info.st_nlink == 1,
            "Launch endpoint conflicts with owned app state; foreign data was preserved"
        );
        Ok(Some(info))
    }

    fn remove_owned_socket(&self, record: &Record) -> Result<()> {
        if self.socket_info(record)?.is_some() {
            let name = CString::new(record.name.as_bytes())?;
            ensure!(
                unsafe { libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0) } == 0,
                "Could not remove owned launch endpoint"
            );
        }
        Ok(())
    }

    fn forward(&self, record: &Record, deadline: Instant) -> Result<bool> {
        if self.socket_info(record)?.is_none() {
            return Ok(false);
        }
        let mut stream = match connect(&self.path.join(&record.name), deadline) {
            Ok(stream) => stream,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound
                        | std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::WouldBlock
                ) =>
            {
                return Ok(false);
            }
            Err(_) => bail!("Could not connect to the running app; another app was not started"),
        };
        ensure!(
            peer_uid(stream.as_raw_fd())? == unsafe { libc::geteuid() },
            "Launch endpoint belongs to another user"
        );
        self.socket_info(record)?
            .context("Launch endpoint disappeared during forwarding")?;
        stream.set_nonblocking(true)?;
        let deadline = deadline.min(Instant::now() + CLIENT_DEADLINE);
        let mut offset = 0;
        while offset < REQUEST.len() {
            ensure!(
                wait(stream.as_raw_fd(), -1, libc::POLLOUT, Some(deadline))?,
                "Running app startup deadline elapsed"
            );
            match stream.write(&REQUEST[offset..]) {
                Ok(0) => bail!("Could not submit Show to the running app"),
                Ok(count) => offset += count,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => {
                    return Err(error).context("Could not submit Show to the running app");
                }
            }
        }
        stream.shutdown(Shutdown::Write)?;
        let mut ack = [0; 3];
        let mut offset = 0;
        while offset < ack.len() {
            ensure!(
                wait(stream.as_raw_fd(), -1, libc::POLLIN, Some(deadline))?,
                "The running app did not acknowledge Show before its deadline; another app was not started"
            );
            match stream.read(&mut ack[offset..]) {
                Ok(0) => {
                    bail!("The running app did not acknowledge Show; another app was not started")
                }
                Ok(count) => offset += count,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(error) => {
                    return Err(error).context(
                        "The running app did not acknowledge Show; another app was not started",
                    );
                }
            }
        }
        ensure!(
            &ack == ACK,
            "The running app refused Show; another app was not started"
        );
        Ok(true)
    }
}

fn read_record(file: &File) -> Result<Option<Record>> {
    let mut bytes = [0; 128];
    let length = file.read_at(&mut bytes, 0)?;
    ensure!(
        length >= MAGIC.len() && &bytes[..MAGIC.len()] == MAGIC,
        "Launch ownership marker changed"
    );
    // An interrupted owner record never authorizes deletion; a new owner can replace its tail.
    if length != MAGIC.len() + 81 {
        return Ok(None);
    }
    let body = &bytes[MAGIC.len()..MAGIC.len() + 49];
    use sha2_10::{Digest, Sha256};
    if &Sha256::digest(body)[..] != &bytes[MAGIC.len() + 49..length] {
        return Ok(None);
    }
    if body[0] != b's' || !body[1..33].iter().all(u8::is_ascii_hexdigit) {
        return Ok(None);
    }
    Ok(Some(Record {
        name: String::from_utf8(body[..33].to_vec())?,
        device: u64::from_le_bytes(body[33..41].try_into()?),
        inode: u64::from_le_bytes(body[41..49].try_into()?),
    }))
}
fn write_record(file: &File, record: &Record) -> Result<()> {
    let mut bytes = MAGIC.to_vec();
    bytes.extend_from_slice(record.name.as_bytes());
    bytes.extend_from_slice(&record.device.to_le_bytes());
    bytes.extend_from_slice(&record.inode.to_le_bytes());
    use sha2_10::{Digest, Sha256};
    let digest = Sha256::digest(&bytes[MAGIC.len()..]);
    bytes.extend_from_slice(&digest);
    file.write_all_at(&bytes, 0)?;
    file.set_len(bytes.len() as u64)?;
    file.sync_all()?;
    Ok(())
}

pub(super) fn connect(path: &Path, deadline: Instant) -> std::io::Result<UnixStream> {
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() || bytes.contains(&0) || bytes.len() >= address.sun_path.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "Local socket path is invalid",
        ));
    }
    address.sun_family = libc::AF_UNIX as _;
    for (output, input) in address.sun_path.iter_mut().zip(bytes) {
        *output = *input as _;
    }
    let length = std::mem::offset_of!(libc::sockaddr_un, sun_path) + bytes.len() + 1;
    #[cfg(target_os = "macos")]
    {
        address.sun_len = length as _;
    }
    let fd = unsafe { libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let stream = unsafe { UnixStream::from_raw_fd(fd) };
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    stream.set_nonblocking(true)?;
    if unsafe {
        libc::connect(
            fd,
            (&address as *const libc::sockaddr_un).cast(),
            length as _,
        )
    } != 0
    {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EINPROGRESS) {
            return Err(error);
        }
        if !wait(fd, -1, libc::POLLOUT, Some(deadline))? {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Local socket connect expired",
            ));
        }
        if let Some(error) = stream.take_error()? {
            return Err(error);
        }
    }
    Ok(stream)
}

pub(super) fn peer_uid(fd: RawFd) -> Result<libc::uid_t> {
    #[cfg(target_os = "linux")]
    {
        Ok(scope::peer_credentials(fd)?.uid)
    }
    #[cfg(target_os = "macos")]
    {
        let mut uid = 0;
        let mut gid = 0;
        ensure!(
            unsafe { libc::getpeereid(fd, &mut uid, &mut gid) } == 0,
            "Could not authenticate local app peer"
        );
        Ok(uid)
    }
}

fn wait(fd: RawFd, stop: RawFd, events: i16, deadline: Option<Instant>) -> std::io::Result<bool> {
    loop {
        let timeout = match deadline {
            Some(deadline) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Ok(false);
                }
                left.as_millis().saturating_add(1).min(i32::MAX as u128) as i32
            }
            None => -1,
        };
        let mut fds = [
            libc::pollfd {
                fd: stop,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd,
                events,
                revents: 0,
            },
        ];
        let found = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout) };
        if found < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if found == 0 || fds[0].revents != 0 {
            return Ok(false);
        }
        if fds[1].revents & libc::POLLNVAL != 0 {
            return Err(std::io::Error::other("Local socket closed"));
        }
        if fds[1].revents != 0 {
            return Ok(true);
        }
    }
}

fn listen(socket: &UnixListener, stop: &UnixStream, sender: &mpsc::Sender<()>) -> Result<()> {
    while wait(socket.as_raw_fd(), stop.as_raw_fd(), libc::POLLIN, None)? {
        let (mut client, _) = match socket.accept() {
            Ok(client) => client,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(error) => return Err(error).context("Could not accept launch event"),
        };
        client.set_nonblocking(true)?;
        if peer_uid(client.as_raw_fd()).ok() != Some(unsafe { libc::geteuid() }) {
            continue;
        }
        let deadline = Instant::now() + CLIENT_DEADLINE;
        let mut bytes = [0; REQUEST.len() + 1];
        let mut length = 0;
        while length < bytes.len()
            && wait(
                client.as_raw_fd(),
                stop.as_raw_fd(),
                libc::POLLIN,
                Some(deadline),
            )?
        {
            match client.read(&mut bytes[length..]) {
                Ok(0) => {
                    if &bytes[..length] == REQUEST {
                        let accepted = match sender.try_send(()) {
                            Ok(()) | Err(mpsc::error::TrySendError::Full(())) => true,
                            Err(mpsc::error::TrySendError::Closed(())) => false,
                        };
                        let ack = if accepted { ACK } else { b"NO\n" };
                        let mut offset = 0;
                        while offset < ack.len()
                            && wait(
                                client.as_raw_fd(),
                                stop.as_raw_fd(),
                                libc::POLLOUT,
                                Some(deadline),
                            )?
                        {
                            match client.write(&ack[offset..]) {
                                Ok(0) => break,
                                Ok(count) => offset += count,
                                Err(error)
                                    if matches!(
                                        error.kind(),
                                        std::io::ErrorKind::WouldBlock
                                            | std::io::ErrorKind::Interrupted
                                    ) =>
                                {
                                    ()
                                }
                                Err(_) => break,
                            }
                        }
                    }
                    break;
                }
                Ok(count) => length += count,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    ()
                }
                Err(_) => break,
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "single_instance_unix_tests.rs"]
mod tests;
