//! Inherited operation-owned transport. No listener, endpoint file or idle runtime.
use anyhow::{Context, Result, ensure};
use app_core::{CaptureError, NativeCaptureCheck};
#[cfg(target_os = "windows")]
use std::time::Duration;
use std::{
    io::{Read, Write},
    process::Command,
    time::Instant,
};

pub(super) const ENV: &str = "SCREENSHOTS_NATIVE_CAPTURE_PIPE";
pub(super) const CHUNK: usize = 64 * 1024;

#[cfg(unix)]
pub(super) struct Transport {
    pub stream: std::os::unix::net::UnixStream,
}
#[cfg(target_os = "windows")]
pub(super) struct Transport {
    read: std::fs::File,
    write: std::fs::File,
}

pub(super) struct Prepared {
    pub transport: Transport,
    pub nonce: [u8; 16],
    #[cfg(unix)]
    child: std::os::unix::net::UnixStream,
    #[cfg(target_os = "windows")]
    child: Vec<std::os::windows::io::OwnedHandle>,
}
impl Prepared {
    #[cfg(target_os = "windows")]
    pub fn child_handles(&self) -> Vec<windows_sys::Win32::Foundation::HANDLE> {
        use std::os::windows::io::AsRawHandle;
        self.child
            .iter()
            .map(|handle| handle.as_raw_handle())
            .collect()
    }
    pub fn new(command: &mut Command) -> Result<Self> { Self::new_kind(command, false) }
    #[cfg(target_os = "linux")]
    pub fn packets(command: &mut Command) -> Result<Self> { Self::new_kind(command, true) }
    fn new_kind(command: &mut Command, packets: bool) -> Result<Self> {
        #[cfg(not(unix))] let _ = packets;
        let nonce = uuid::Uuid::new_v4();
        let parent = std::process::id();
        #[cfg(unix)]
        {
            use std::os::{fd::AsRawFd, unix::process::CommandExt};
            #[cfg(target_os = "linux")]
            let (server, child) = if packets {
                use std::os::fd::FromRawFd;
                let mut fds = [-1; 2];
                ensure!(unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0, fds.as_mut_ptr()) } == 0, "Cannot create private native packet channel");
                unsafe { (std::os::unix::net::UnixStream::from_raw_fd(fds[0]), std::os::unix::net::UnixStream::from_raw_fd(fds[1])) }
            } else { std::os::unix::net::UnixStream::pair()? };
            #[cfg(not(target_os = "linux"))]
            let (server, child) = {
                ensure!(!packets, "Native packet transport requires Linux");
                std::os::unix::net::UnixStream::pair()?
            };
            for stream in [&server, &child] {
                let size = 128 * 1024i32;
                for option in [libc::SO_SNDBUF, libc::SO_RCVBUF] {
                    ensure!(
                        unsafe {
                            libc::setsockopt(
                                stream.as_raw_fd(),
                                libc::SOL_SOCKET,
                                option,
                                (&size as *const i32).cast(),
                                std::mem::size_of_val(&size) as libc::socklen_t,
                            )
                        } == 0,
                        "Cannot bound private native channel buffers"
                    );
                    let mut actual = 0i32;
                    let mut length = std::mem::size_of_val(&actual) as libc::socklen_t;
                    ensure!(
                        unsafe {
                            libc::getsockopt(
                                stream.as_raw_fd(),
                                libc::SOL_SOCKET,
                                option,
                                (&mut actual as *mut i32).cast(),
                                &mut length,
                            )
                        } == 0
                            && actual > 0
                            && actual <= 256 * 1024,
                        "Native socket queue exceeds its bound"
                    );
                }
            }
            server.set_nonblocking(true)?;
            let fd = child.as_raw_fd();
            command.env(ENV, format!("v1:{fd}:{parent}:{nonce}"));
            unsafe {
                command.pre_exec(move || {
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::getppid() != parent as i32 || libc::setpgid(0, 0) != 0 {
                        return Err(std::io::Error::other("Native owner unavailable"));
                    }
                    #[cfg(target_os = "linux")]
                    {
                        // Startup has no descendants. Default SIGUSR1 terminates that interval;
                        // the authenticated worker later installs its family-killing handler.
                        // Use the same signal on the spawning thread and any worker entry thread:
                        // Linux's parent-death setting belongs to a thread, not the whole process.
                        let mut action: libc::sigaction = std::mem::zeroed();
                        action.sa_sigaction = libc::SIG_DFL;
                        libc::sigemptyset(&mut action.sa_mask);
                        let mut signals: libc::sigset_t = std::mem::zeroed();
                        libc::sigemptyset(&mut signals);
                        libc::sigaddset(&mut signals, libc::SIGUSR1);
                        if libc::sigaction(libc::SIGUSR1, &action, std::ptr::null_mut()) != 0
                            || libc::sigprocmask(libc::SIG_UNBLOCK, &signals, std::ptr::null_mut())
                                != 0
                            || libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGUSR1) != 0
                            || libc::getppid() != parent as i32
                        {
                            return Err(std::io::Error::other("Native owner unavailable"));
                        }
                    }
                    let no_core = libc::rlimit {
                        rlim_cur: 0,
                        rlim_max: 0,
                    };
                    if libc::setrlimit(libc::RLIMIT_CORE, &no_core) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            Ok(Self {
                transport: Transport { stream: server },
                nonce: *nonce.as_bytes(),
                child,
            })
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::io::{AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle};
            use windows_sys::Win32::{
                Foundation::*, Security::SECURITY_ATTRIBUTES, System::Pipes::*,
            };
            fn pipe() -> Result<(OwnedHandle, OwnedHandle)> {
                let mut read = std::ptr::null_mut();
                let mut write = std::ptr::null_mut();
                let attributes = SECURITY_ATTRIBUTES {
                    nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: std::ptr::null_mut(),
                    bInheritHandle: 1,
                };
                ensure!(
                    unsafe { CreatePipe(&mut read, &mut write, &attributes, 128 * 1024) } != 0,
                    "Cannot create private native pipe"
                );
                Ok((unsafe { OwnedHandle::from_raw_handle(read) }, unsafe {
                    OwnedHandle::from_raw_handle(write)
                }))
            }
            let (child_read, parent_write) = pipe()?;
            let (parent_read, child_write) = pipe()?;
            for handle in [&parent_read, &parent_write] {
                ensure!(
                    unsafe { SetHandleInformation(handle.as_raw_handle(), HANDLE_FLAG_INHERIT, 0) }
                        != 0,
                    "Cannot protect native pipe"
                );
            }
            let mut capacity = 0;
            ensure!(
                unsafe {
                    GetNamedPipeInfo(
                        parent_write.as_raw_handle(),
                        std::ptr::null_mut(),
                        &mut capacity,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                } != 0
                    && capacity >= 65536 + 64,
                "Native request pipe is smaller than its bounded request"
            );
            command.env(
                ENV,
                format!(
                    "v1:{}:{}:{parent}:{nonce}",
                    child_read.as_raw_handle() as usize,
                    child_write.as_raw_handle() as usize
                ),
            );
            Ok(Self {
                transport: Transport {
                    read: unsafe { std::fs::File::from_raw_handle(parent_read.into_raw_handle()) },
                    write: unsafe {
                        std::fs::File::from_raw_handle(parent_write.into_raw_handle())
                    },
                },
                nonce: *nonce.as_bytes(),
                child: vec![child_read, child_write],
            })
        }
    }
    pub fn parent(self) -> (Transport, [u8; 16]) {
        let Self {
            transport,
            nonce,
            child,
        } = self;
        drop(child);
        (transport, nonce)
    }
}
impl Transport {
    pub fn inherited() -> Result<(Self, [u8; 16], u32)> { Self::inherited_kind(false) }
    #[cfg(target_os = "linux")]
    pub fn inherited_packets() -> Result<(Self, [u8; 16], u32)> { Self::inherited_kind(true) }
    fn inherited_kind(packets: bool) -> Result<(Self, [u8; 16], u32)> {
        #[cfg(not(unix))] let _ = packets;
        let value = std::env::var(ENV).context("Private native channel is absent")?;
        ensure!(value.len() <= 256, "Invalid private native channel");
        let fields = value.split(':').collect::<Vec<_>>();
        #[cfg(unix)]
        {
            use std::os::fd::{AsRawFd, FromRawFd};
            ensure!(
                fields.len() == 4 && fields[0] == "v1",
                "Invalid private native channel"
            );
            let fd: i32 = fields[1].parse()?;
            let parent: u32 = fields[2].parse()?;
            ensure!(
                fd >= 3 && parent > 0 && unsafe { libc::getppid() } == parent as i32,
                "Invalid native channel owner"
            );
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            ensure!(
                flags >= 0
                    && unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } == 0,
                "Cannot protect inherited native channel"
            );
            let mut info: libc::stat = unsafe { std::mem::zeroed() };
            ensure!(
                unsafe { libc::fstat(fd, &mut info) } == 0
                    && info.st_mode & libc::S_IFMT == libc::S_IFSOCK,
                "Native channel is not a socket"
            );
            let mut kind = 0i32;
            let mut kind_length = std::mem::size_of_val(&kind) as libc::socklen_t;
            ensure!(
                unsafe {
                    libc::getsockopt(
                        fd,
                        libc::SOL_SOCKET,
                        libc::SO_TYPE,
                        (&mut kind as *mut i32).cast(),
                        &mut kind_length,
                    )
                } == 0
                    && kind == if packets { libc::SOCK_SEQPACKET } else { libc::SOCK_STREAM },
                "Native channel has an unexpected socket kind"
            );
            let mut address: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let mut length = std::mem::size_of_val(&address) as libc::socklen_t;
            ensure!(
                unsafe {
                    libc::getpeername(
                        fd,
                        (&mut address as *mut libc::sockaddr_storage).cast(),
                        &mut length,
                    )
                } == 0
                    && address.ss_family as i32 == libc::AF_UNIX,
                "Native channel is not local"
            );
            #[cfg(target_os = "linux")]
            {
                let mut peer: libc::ucred = unsafe { std::mem::zeroed() };
                let mut length = std::mem::size_of_val(&peer) as libc::socklen_t;
                ensure!(
                    unsafe {
                        libc::getsockopt(
                            fd,
                            libc::SOL_SOCKET,
                            libc::SO_PEERCRED,
                            (&mut peer as *mut libc::ucred).cast(),
                            &mut length,
                        )
                    } == 0
                        && peer.pid == parent as i32
                        && peer.uid == unsafe { libc::getuid() },
                    "Unexpected native channel peer"
                );
            }
            let nonce = *uuid::Uuid::parse_str(fields[3])?.as_bytes();
            super::unix::arm_parent_death(parent)?;
            let stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) };
            stream.set_nonblocking(true)?;
            let _ = stream.as_raw_fd();
            Ok((Self { stream }, nonce, parent))
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::io::FromRawHandle;
            use windows_sys::Win32::{
                Foundation::*,
                Storage::FileSystem::*,
                System::{Console::*, Pipes::GetNamedPipeServerProcessId},
            };
            ensure!(
                fields.len() == 5 && fields[0] == "v1",
                "Invalid private native channel"
            );
            let read: usize = fields[1].parse()?;
            let write: usize = fields[2].parse()?;
            let parent: u32 = fields[3].parse()?;
            ensure!(
                parent > 0
                    && read > 3
                    && write > 3
                    && read != write
                    && read != usize::MAX
                    && write != usize::MAX,
                "Invalid native pipe handles"
            );
            for handle in [read, write] {
                let handle = handle as HANDLE;
                ensure!(
                    [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
                        .into_iter()
                        .all(|kind| unsafe { GetStdHandle(kind) } != handle)
                        && unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } != 0
                        && unsafe { GetFileType(handle) } == FILE_TYPE_PIPE,
                    "Native handles are not private pipes"
                );
                let mut owner = 0;
                ensure!(
                    unsafe { GetNamedPipeServerProcessId(handle, &mut owner) } != 0
                        && owner == parent,
                    "Unexpected native pipe owner"
                );
            }
            Ok((
                Self {
                    read: unsafe { std::fs::File::from_raw_handle(read as _) },
                    write: unsafe { std::fs::File::from_raw_handle(write as _) },
                },
                *uuid::Uuid::parse_str(fields[4])?.as_bytes(),
                parent,
            ))
        }
    }
    fn wait(&self, writing: bool, deadline: Instant) -> std::io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let mut event = libc::pollfd {
                fd: self.stream.as_raw_fd(),
                events: if writing { libc::POLLOUT } else { libc::POLLIN },
                revents: 0,
            };
            let milliseconds = deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(20) as i32;
            let result = unsafe { libc::poll(&mut event, 1, milliseconds) };
            if result < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        #[cfg(target_os = "windows")]
        {
            let _ = writing;
            std::thread::sleep(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(5)),
            );
        }
        Ok(())
    }
}
#[cfg(unix)]
impl Read for Transport {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.stream.read(bytes)
    }
}
#[cfg(unix)]
impl Write for Transport {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.stream.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
#[cfg(target_os = "windows")]
impl Read for Transport {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        use std::os::windows::io::AsRawHandle;
        let mut available = 0;
        if unsafe {
            windows_sys::Win32::System::Pipes::PeekNamedPipe(
                self.read.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        } == 0
        {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error()
                == Some(windows_sys::Win32::Foundation::ERROR_BROKEN_PIPE as i32)
            {
                return Ok(0);
            }
            return Err(error);
        }
        if available == 0 {
            return Err(std::io::ErrorKind::WouldBlock.into());
        }
        let count = bytes.len().min(available as usize);
        self.read.read(&mut bytes[..count])
    }
}
#[cfg(target_os = "windows")]
impl Write for Transport {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.write.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.write.flush()
    }
}

pub(super) struct ActiveIo<'a> {
    pub transport: Transport,
    pub deadline: Instant,
    pub check: &'a NativeCaptureCheck<'a>,
}
impl ActiveIo<'_> {
    fn check(&self) -> std::io::Result<()> {
        (self.check)().map_err(std::io::Error::other)?;
        if Instant::now() >= self.deadline {
            return Err(std::io::Error::other(CaptureError::NativeTimeout));
        }
        Ok(())
    }
}
impl Read for ActiveIo<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        loop {
            self.check()?;
            match self.transport.read(bytes) {
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    self.transport.wait(false, self.deadline)?
                }
                value => return value,
            }
        }
    }
}
impl Write for ActiveIo<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        loop {
            self.check()?;
            match self.transport.write(&bytes[..bytes.len().min(CHUNK)]) {
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    self.transport.wait(true, self.deadline)?
                }
                value => return value,
            }
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.check()?;
        self.transport.flush()
    }
}

#[cfg(target_os = "macos")]
pub(super) struct Lifeline {
    queue: std::os::fd::OwnedFd,
    thread: Option<std::thread::JoinHandle<()>>,
}
#[cfg(target_os = "macos")]
impl Lifeline {
    pub fn new(parent: u32) -> Result<Self> {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        ensure!(
            parent > 1 && unsafe { libc::getppid() } == parent as i32,
            "Native parent is unavailable"
        );
        let descriptor = unsafe { libc::kqueue() };
        ensure!(descriptor >= 0, "Cannot observe native parent exit");
        let queue = unsafe { OwnedFd::from_raw_fd(descriptor) };
        ensure!(
            unsafe { libc::fcntl(queue.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } == 0,
            "Cannot protect native parent observer"
        );
        let changes = [
            libc::kevent {
                ident: parent as usize,
                filter: libc::EVFILT_PROC,
                flags: libc::EV_ADD | libc::EV_ONESHOT,
                fflags: libc::NOTE_EXIT,
                data: 0,
                udata: std::ptr::null_mut(),
            },
            libc::kevent {
                ident: 1,
                filter: libc::EVFILT_USER,
                flags: libc::EV_ADD | libc::EV_CLEAR,
                fflags: 0,
                data: 0,
                udata: std::ptr::null_mut(),
            },
        ];
        ensure!(
            unsafe {
                libc::kevent(
                    descriptor,
                    changes.as_ptr(),
                    changes.len() as i32,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                )
            } == 0
                && unsafe { libc::getppid() } == parent as i32,
            "Native parent changed during observer registration"
        );
        let thread = std::thread::Builder::new()
            .name("capture-parent-lifeline".into())
            .stack_size(128 * 1024)
            .spawn(move || {
                loop {
                    let mut event: libc::kevent = unsafe { std::mem::zeroed() };
                    let ready = unsafe {
                        libc::kevent(
                            descriptor,
                            std::ptr::null(),
                            0,
                            &mut event,
                            1,
                            std::ptr::null(),
                        )
                    };
                    if ready < 0
                        && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
                    {
                        continue;
                    }
                    if ready == 1
                        && event.filter == libc::EVFILT_USER
                        && event.ident == 1
                        && event.flags & libc::EV_ERROR == 0
                    {
                        return;
                    }
                    // No protocol descriptor is observed or consumed. A kernel
                    // parent-exit event (or observer failure) ends this owned group.
                    super::unix::terminate_own_group();
                }
            })?;
        Ok(Self {
            queue,
            thread: Some(thread),
        })
    }
}
#[cfg(target_os = "macos")]
impl Drop for Lifeline {
    fn drop(&mut self) {
        use std::os::fd::AsRawFd;
        let cancel = libc::kevent {
            ident: 1,
            filter: libc::EVFILT_USER,
            flags: 0,
            fflags: libc::NOTE_TRIGGER,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        loop {
            let result = unsafe {
                libc::kevent(
                    self.queue.as_raw_fd(),
                    &cancel,
                    1,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null(),
                )
            };
            if result == 0 {
                break;
            }
            if std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
                super::unix::terminate_own_group();
            }
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
