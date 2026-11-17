//! A single-owner nonblocking X11 transport: every socket wait observes stop and an operation deadline.
use std::{
    cell::{Cell, RefCell},
    fs::OpenOptions,
    io::{self, Read},
    os::{
        fd::{AsRawFd, FromRawFd, RawFd},
        unix::{fs::OpenOptionsExt, net::UnixStream},
    },
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use x11rb::{
    rust_connection::{PollMode, Stream},
    utils::RawFdContainer,
};

pub(super) const OPERATION_TIMEOUT: Duration = Duration::from_secs(2);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_PACKET: usize = 256 * 1024;
const MAX_OPERATION_BYTES: usize = 1024 * 1024;
const MAX_OPERATION_PACKETS: usize = 1024;
type Cancel = Arc<dyn Fn() -> bool + Send + Sync>;
struct Frame {
    header: [u8; 8],
    filled: usize,
    delivered: usize,
    remaining: usize,
    setup: bool,
}
impl Frame {
    fn new() -> Self {
        Self {
            header: [0; 8],
            filled: 0,
            delivered: 0,
            remaining: 0,
            setup: true,
        }
    }
    fn next(&mut self) {
        *self = Self {
            setup: false,
            ..Self::new()
        };
    }
}

pub(super) struct NativeStream {
    socket: RefCell<Option<UnixStream>>,
    control: Option<UnixStream>,
    canceled: Cancel,
    deadline: Cell<Option<Instant>>,
    failed: Cell<bool>,
    bytes: Cell<usize>,
    packets: Cell<usize>,
    frame: RefCell<Frame>,
}
pub(super) struct Operation<'a> {
    stream: &'a NativeStream,
    previous: Option<Instant>,
}
impl Drop for Operation<'_> {
    fn drop(&mut self) {
        self.stream.deadline.set(self.previous);
    }
}
impl NativeStream {
    fn new(socket: Option<UnixStream>, control: Option<UnixStream>, canceled: Cancel) -> Self {
        Self {
            socket: RefCell::new(socket),
            control,
            canceled,
            deadline: Cell::new(Some(Instant::now() + STARTUP_TIMEOUT)),
            failed: Cell::new(false),
            bytes: Cell::new(0),
            packets: Cell::new(0),
            frame: RefCell::new(Frame::new()),
        }
    }
    pub(super) fn connect(
        display: &str,
        control: Option<UnixStream>,
        canceled: Cancel,
    ) -> io::Result<(Self, usize, Vec<u8>, Vec<u8>)> {
        let (number, screen) = parse_display(display)?;
        let stream = Self::new(None, control, canceled);
        // Auth bytes are bounded, never logged and never fetched via the dependency's unbounded reader.
        let (auth_name, auth_data) = stream.auth(number)?;
        let path = format!("/tmp/.X11-unix/X{number}");
        let mut last_error = None;
        for abstract_socket in [true, false] {
            stream.check()?;
            let fd = unsafe {
                libc::socket(
                    libc::AF_UNIX,
                    libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                    0,
                )
            };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            *stream.socket.borrow_mut() = Some(unsafe { UnixStream::from_raw_fd(fd) });
            let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
            address.sun_family = libc::AF_UNIX as _;
            let offset = usize::from(abstract_socket);
            if path.len() + offset + usize::from(!abstract_socket) > address.sun_path.len() {
                return Err(stream.fail(
                    io::ErrorKind::InvalidInput,
                    "X11 socket address exceeds its limit",
                ));
            }
            for (slot, byte) in address.sun_path[offset..].iter_mut().zip(path.bytes()) {
                *slot = byte as _;
            }
            let length = std::mem::offset_of!(libc::sockaddr_un, sun_path) + path.len() + 1;
            let result = unsafe {
                libc::connect(
                    fd,
                    (&address as *const libc::sockaddr_un).cast(),
                    length as _,
                )
            };
            let connected = if result == 0 {
                Ok(())
            } else {
                let error = io::Error::last_os_error();
                if matches!(
                    error.raw_os_error(),
                    Some(libc::EINPROGRESS | libc::EALREADY | libc::EAGAIN | libc::EINTR)
                ) {
                    stream.poll(PollMode::Writable).and_then(|()| {
                        let mut code = 0i32;
                        let mut size = std::mem::size_of_val(&code) as libc::socklen_t;
                        if unsafe {
                            libc::getsockopt(
                                fd,
                                libc::SOL_SOCKET,
                                libc::SO_ERROR,
                                (&mut code as *mut i32).cast(),
                                &mut size,
                            )
                        } < 0
                        {
                            Err(io::Error::last_os_error())
                        } else if code == 0 {
                            Ok(())
                        } else {
                            Err(io::Error::from_raw_os_error(code))
                        }
                    })
                } else {
                    Err(error)
                }
            };
            match connected {
                Ok(()) => return Ok((stream, screen, auth_name, auth_data)),
                Err(error) => {
                    stream.socket.borrow_mut().take();
                    if stream.failed.get() {
                        return Err(error);
                    }
                    last_error = Some(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "X11 socket unavailable")
        }))
    }
    pub(super) fn begin(&self) -> io::Result<Operation<'_>> {
        self.check()?;
        let previous = self.deadline.get();
        self.deadline.set(Some(
            previous
                .map(|p| p.min(Instant::now() + OPERATION_TIMEOUT))
                .unwrap_or_else(|| Instant::now() + OPERATION_TIMEOUT),
        ));
        if previous.is_none() {
            self.bytes.set(0);
            self.packets.set(0);
        }
        Ok(Operation {
            stream: self,
            previous,
        })
    }
    pub(super) fn ready(&self) -> io::Result<()> {
        self.check()?;
        self.deadline.set(None);
        self.bytes.set(0);
        self.packets.set(0);
        Ok(())
    }
    pub(super) fn failed(&self) -> bool {
        self.failed.get()
    }
    fn fail(&self, kind: io::ErrorKind, message: &'static str) -> io::Error {
        self.failed.set(true);
        // Taking the sole socket owner closes it now; future Drop/cleanup cannot close a reused fd.
        self.socket.borrow_mut().take();
        io::Error::new(kind, message)
    }
    fn check(&self) -> io::Result<()> {
        if self.failed.get() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "X11 transport is closed",
            ));
        }
        if (self.canceled)() {
            return Err(self.fail(io::ErrorKind::ConnectionAborted, "X11 operation canceled"));
        }
        if self
            .deadline
            .get()
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(self.fail(io::ErrorKind::TimedOut, "X11 operation deadline exceeded"));
        }
        Ok(())
    }
    fn count(&self, bytes: usize) -> io::Result<()> {
        let total = self.bytes.get().saturating_add(bytes);
        self.bytes.set(total);
        if total > MAX_OPERATION_BYTES {
            return Err(self.fail(
                io::ErrorKind::InvalidData,
                "X11 operation byte limit exceeded",
            ));
        }
        Ok(())
    }
    fn receive(&self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            self.check()?;
            let n = unsafe {
                libc::recv(
                    self.as_raw_fd(),
                    bytes.as_mut_ptr().cast(),
                    bytes.len(),
                    libc::MSG_DONTWAIT,
                )
            };
            if n >= 0 {
                self.count(n as usize)?;
                return Ok(n as usize);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return if error.kind() == io::ErrorKind::WouldBlock {
                    Err(error)
                } else {
                    Err(self.fail(error.kind(), "X11 socket read failed"))
                };
            }
        }
    }
    fn auth(&self, display: u16) -> io::Result<(Vec<u8>, Vec<u8>)> {
        let path = std::env::var_os("XAUTHORITY")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".Xauthority")));
        let Some(path) = path else {
            return Ok((Vec::new(), Vec::new()));
        };
        self.check()?;
        let mut file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok((Vec::new(), Vec::new()));
            }
            Err(error) => return Err(error),
        };
        self.check()?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > 65536 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Xauthority must be a bounded regular file",
            ));
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        loop {
            self.check()?;
            let mut chunk = [0u8; 4096];
            match file.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if bytes.len() + n > 65536 {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Xauthority exceeds its limit",
                        ));
                    }
                    bytes.extend_from_slice(&chunk[..n]);
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
        let mut hostname = [0u8; 256];
        if unsafe { libc::gethostname(hostname.as_mut_ptr().cast(), hostname.len()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let length = hostname
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(hostname.len());
        parse_auth(&bytes, &hostname[..length], display)
    }
    #[cfg(test)]
    fn from_socket(
        socket: UnixStream,
        control: Option<UnixStream>,
        canceled: Cancel,
        timeout: Duration,
    ) -> io::Result<Self> {
        socket.set_nonblocking(true)?;
        let stream = Self::new(Some(socket), control, canceled);
        stream.deadline.set(Some(Instant::now() + timeout));
        Ok(stream)
    }
}
impl AsRawFd for NativeStream {
    fn as_raw_fd(&self) -> RawFd {
        self.socket
            .borrow()
            .as_ref()
            .map(AsRawFd::as_raw_fd)
            .unwrap_or(-1)
    }
}
impl Stream for NativeStream {
    fn poll(&self, mode: PollMode) -> io::Result<()> {
        loop {
            self.check()?;
            let deadline = self.deadline.get().ok_or_else(|| {
                self.fail(
                    io::ErrorKind::InvalidInput,
                    "Blocking X11 wait outside an admitted operation",
                )
            })?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            let timeout = remaining
                .as_millis()
                .saturating_add(u128::from(remaining.subsec_nanos() % 1_000_000 != 0))
                .min(i32::MAX as u128) as i32;
            let mut descriptors = [
                libc::pollfd {
                    fd: self.as_raw_fd(),
                    events: (if mode.readable() { libc::POLLIN } else { 0 })
                        | (if mode.writable() { libc::POLLOUT } else { 0 }),
                    revents: 0,
                },
                libc::pollfd {
                    fd: self.control.as_ref().map(AsRawFd::as_raw_fd).unwrap_or(-1),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let result =
                unsafe { libc::poll(descriptors.as_mut_ptr(), descriptors.len() as _, timeout) };
            if result < 0 {
                if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(self.fail(io::ErrorKind::ConnectionAborted, "X11 native wait failed"));
            }
            self.check()?;
            if descriptors[1].revents != 0 {
                // Main-app control wakes can carry coalesced repeat/status work, not only cancellation.
                let mut discard = [0u8; 64];
                for _ in 0..64 {
                    self.check()?;
                    let n = unsafe {
                        libc::recv(
                            descriptors[1].fd,
                            discard.as_mut_ptr().cast(),
                            discard.len(),
                            libc::MSG_DONTWAIT,
                        )
                    };
                    if n > 0 {
                        continue;
                    }
                    if n == 0 {
                        return Err(
                            self.fail(io::ErrorKind::ConnectionAborted, "X11 control owner closed")
                        );
                    }
                    break;
                }
                self.check()?;
            }
            if descriptors[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                return Err(self.fail(
                    io::ErrorKind::ConnectionAborted,
                    "X11 server connection closed",
                ));
            }
            if descriptors[0].revents != 0 {
                return Ok(());
            }
        }
    }
    fn read(&self, bytes: &mut [u8], _: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        self.check()?;
        if bytes.is_empty() {
            return Ok(0);
        }
        let mut frame = self.frame.borrow_mut();
        if frame.filled < 8 {
            let offset = frame.filled;
            let count = self.receive(&mut frame.header[offset..])?;
            if count == 0 {
                return Err(self.fail(
                    io::ErrorKind::UnexpectedEof,
                    "X11 server closed during a packet",
                ));
            }
            frame.filled += count;
            if frame.filled < 8 {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let size = if frame.setup {
                8usize + usize::from(u16::from_ne_bytes([frame.header[6], frame.header[7]])) * 4
            } else if matches!(frame.header[0] & 0x7f, 1 | 35) {
                32usize.saturating_add(
                    (u32::from_ne_bytes(frame.header[4..8].try_into().unwrap()) as usize)
                        .saturating_mul(4),
                )
            } else {
                32
            };
            if !(8..=MAX_PACKET).contains(&size) {
                return Err(self.fail(
                    io::ErrorKind::InvalidData,
                    "X11 packet exceeds its byte limit",
                ));
            }
            self.packets.set(self.packets.get().saturating_add(1));
            if self.packets.get() > MAX_OPERATION_PACKETS {
                return Err(self.fail(
                    io::ErrorKind::InvalidData,
                    "X11 operation packet limit exceeded",
                ));
            }
            frame.remaining = size - 8;
        }
        if frame.delivered < 8 {
            let count = bytes.len().min(8 - frame.delivered);
            bytes[..count].copy_from_slice(&frame.header[frame.delivered..frame.delivered + count]);
            frame.delivered += count;
            if frame.delivered == 8 && frame.remaining == 0 {
                frame.next();
            }
            return Ok(count);
        }
        let length = bytes.len().min(frame.remaining);
        let count = self.receive(&mut bytes[..length])?;
        if count == 0 {
            return Err(self.fail(
                io::ErrorKind::UnexpectedEof,
                "X11 server closed during a packet",
            ));
        }
        frame.remaining -= count;
        if frame.remaining == 0 {
            frame.next();
        }
        Ok(count)
    }
    fn write(&self, bytes: &[u8], fds: &mut Vec<RawFdContainer>) -> io::Result<usize> {
        if !fds.is_empty() {
            return Err(self.fail(
                io::ErrorKind::InvalidInput,
                "Keyboard X11 connection cannot transfer file descriptors",
            ));
        }
        loop {
            self.check()?;
            let n = unsafe {
                libc::send(
                    self.as_raw_fd(),
                    bytes.as_ptr().cast(),
                    bytes.len(),
                    libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
                )
            };
            if n >= 0 {
                self.count(n as usize)?;
                return Ok(n as usize);
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return if error.kind() == io::ErrorKind::WouldBlock {
                    Err(error)
                } else {
                    Err(self.fail(error.kind(), "X11 socket write failed"))
                };
            }
        }
    }
}
fn parse_display(value: &str) -> io::Result<(u16, usize)> {
    let fail = || {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "Global shortcuts require a local numeric X11 display",
        )
    };
    if value.len() > 65 {
        return Err(fail());
    }
    let value = value.strip_prefix(':').ok_or_else(fail)?;
    let (number, screen) = value.split_once('.').map_or((value, "0"), |v| v);
    if number.is_empty()
        || screen.is_empty()
        || !number
            .bytes()
            .chain(screen.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return Err(fail());
    }
    Ok((
        number.parse().map_err(|_| fail())?,
        screen.parse().map_err(|_| fail())?,
    ))
}
fn parse_auth(mut bytes: &[u8], hostname: &[u8], display: u16) -> io::Result<(Vec<u8>, Vec<u8>)> {
    fn field<'a>(bytes: &mut &'a [u8]) -> io::Result<&'a [u8]> {
        if bytes.len() < 2 {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let size = u16::from_be_bytes(bytes[..2].try_into().unwrap()) as usize;
        *bytes = &bytes[2..];
        if bytes.len() < size {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let result = &bytes[..size];
        *bytes = &bytes[size..];
        Ok(result)
    }
    let number = display.to_string();
    while !bytes.is_empty() {
        if bytes.len() < 2 {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let family = u16::from_be_bytes(bytes[..2].try_into().unwrap());
        bytes = &bytes[2..];
        let address = field(&mut bytes)?;
        let entry_number = field(&mut bytes)?;
        let name = field(&mut bytes)?;
        let data = field(&mut bytes)?;
        if (family == 65535 || (family == 256 && address == hostname))
            && (entry_number.is_empty() || entry_number == number.as_bytes())
            && name == b"MIT-MAGIC-COOKIE-1"
        {
            if data.len() > 256 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Xauthority cookie exceeds its limit",
                ));
            }
            return Ok((name.to_vec(), data.to_vec()));
        }
    }
    Ok((Vec::new(), Vec::new()))
}

#[cfg(test)]
#[path = "unix_desktop_services_x11_stream_tests.rs"]
mod tests;
