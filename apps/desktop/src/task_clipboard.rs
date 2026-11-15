//! Anonymous inherited clipboard transfer for one hosted task.
use anyhow::{Context, Result, bail, ensure};
use image::RgbaImage;
use std::{
    io::{Read, Write},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

pub(crate) const ENV: &str = "SCREENSHOTS_TASK_CLIPBOARD";
const MAGIC: &[u8; 8] = b"SCTCLIP1";
const HEADER: usize = 32;
const HELLO: u32 = 0;
const IMAGE: u32 = 1;
const TEXT: u32 = 2;
const HELLO_ACK: u32 = 100;
const SUCCESS: u32 = 101;
const FAILURE: u32 = 102;
const CHUNK: usize = 64 * 1024;
trait Duplex: Read + Write + Send {}
impl<T: Read + Write + Send> Duplex for T {}

#[cfg(unix)]
struct NativeIo {
    stream: std::os::unix::net::UnixStream,
    stop: Arc<AtomicBool>,
    timeout: Option<Duration>,
}
#[cfg(unix)]
impl NativeIo {
    fn wait(&self, event: i16, until: Option<Instant>) -> std::io::Result<()> {
        use std::os::fd::AsRawFd;
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            if until.is_some_and(|deadline| Instant::now() >= deadline) {
                return Err(std::io::ErrorKind::TimedOut.into());
            }
            let mut descriptor = libc::pollfd {
                fd: self.stream.as_raw_fd(),
                events: event,
                revents: 0,
            };
            let timeout = until
                .map(|deadline| {
                    deadline
                        .saturating_duration_since(Instant::now())
                        .as_millis()
                        .min(i32::MAX as u128) as i32
                })
                .unwrap_or(-1);
            let ready = unsafe { libc::poll(&mut descriptor, 1, timeout) };
            if ready > 0 {
                return Ok(());
            }
            if ready < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
}
#[cfg(unix)]
impl Read for NativeIo {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let until = self.timeout.map(|timeout| Instant::now() + timeout);
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            match self.stream.read(bytes) {
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    self.wait(libc::POLLIN, until)?
                }
                value => return value,
            }
        }
    }
}
#[cfg(unix)]
impl Write for NativeIo {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let until = self.timeout.map(|timeout| Instant::now() + timeout);
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            match self.stream.write(bytes) {
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    self.wait(libc::POLLOUT, until)?
                }
                value => return value,
            }
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(target_os = "windows")]
struct NativeIo {
    read: std::fs::File,
    write: std::fs::File,
    stop: Arc<AtomicBool>,
    timeout: Option<Duration>,
}
#[cfg(target_os = "windows")]
impl Read for NativeIo {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        use std::os::windows::io::AsRawHandle;
        let until = self.timeout.map(|timeout| Instant::now() + timeout);
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return Err(std::io::ErrorKind::Interrupted.into());
            }
            if until.is_some_and(|deadline| Instant::now() >= deadline) {
                return Err(std::io::ErrorKind::TimedOut.into());
            }
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
            if available != 0 {
                let count = (available as usize).min(bytes.len());
                return self.read.read(&mut bytes[..count]);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
#[cfg(target_os = "windows")]
impl Write for NativeIo {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.write.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.write.flush()
    }
}

struct Packet {
    kind: u32,
    width: u32,
    height: u32,
    length: u64,
}
fn header(kind: u32, width: u32, height: u32, length: u64) -> [u8; HEADER] {
    let mut value = [0; HEADER];
    value[..8].copy_from_slice(MAGIC);
    value[8..12].copy_from_slice(&kind.to_be_bytes());
    value[12..16].copy_from_slice(&width.to_be_bytes());
    value[16..20].copy_from_slice(&height.to_be_bytes());
    value[20..28].copy_from_slice(&length.to_be_bytes());
    value
}
fn read_header(input: &mut impl Read) -> Result<Option<Packet>> {
    let mut bytes = [0; HEADER];
    let count = input.read(&mut bytes[..1])?;
    if count == 0 {
        return Ok(None);
    }
    input.read_exact(&mut bytes[1..])?;
    ensure!(
        &bytes[..8] == MAGIC && bytes[28..] == [0; 4],
        "Invalid task clipboard header"
    );
    let u32_at = |offset| u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap());
    Ok(Some(Packet {
        kind: u32_at(8),
        width: u32_at(12),
        height: u32_at(16),
        length: u64::from_be_bytes(bytes[20..28].try_into().unwrap()),
    }))
}
fn expect_header(input: &mut impl Read) -> Result<Packet> {
    read_header(input)?.context("Task clipboard channel closed")
}
fn send(output: &mut impl Write, kind: u32, width: u32, height: u32, bytes: &[u8]) -> Result<()> {
    output.write_all(&header(kind, width, height, bytes.len() as u64))?;
    for chunk in bytes.chunks(CHUNK) {
        output.write_all(chunk)?;
    }
    output.flush()?;
    Ok(())
}
fn empty(packet: &Packet, kind: u32) -> bool {
    packet.kind == kind && packet.width == 0 && packet.height == 0 && packet.length == 0
}
fn valid_text(text: &str) -> bool {
    (1..=2048).contains(&text.len())
        && !text
            .chars()
            .any(|value| value.is_control() || matches!(value, '\u{2028}' | '\u{2029}'))
}

pub(crate) struct Client {
    io: Box<dyn Duplex>,
}
impl Client {
    /// Call only at task entry, before source capture or external process creation.
    pub(crate) fn from_env() -> Result<Option<Self>> {
        let Some(value) = std::env::var_os(ENV) else {
            return Ok(None);
        };
        let value = value
            .to_str()
            .filter(|value| value.len() <= 256)
            .context("Invalid inherited task clipboard descriptor")?;
        let fields = value.split(':').collect::<Vec<_>>();
        #[cfg(unix)]
        let (mut io, nonce) = {
            use std::os::fd::FromRawFd;
            ensure!(
                fields.len() == 3 && fields[0] == "v1",
                "Invalid inherited task clipboard descriptor"
            );
            let fd: i32 = fields[1]
                .parse()
                .context("Invalid task clipboard descriptor")?;
            ensure!(fd >= 3, "Task clipboard descriptor cannot be stdio");
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            ensure!(
                flags >= 0
                    && unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } == 0,
                "Could not protect inherited task clipboard descriptor"
            );
            let mut info: libc::stat = unsafe { std::mem::zeroed() };
            ensure!(
                unsafe { libc::fstat(fd, &mut info) } == 0
                    && info.st_mode & libc::S_IFMT == libc::S_IFSOCK,
                "Task clipboard descriptor must be an anonymous socket"
            );
            let mut address: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            let mut size = std::mem::size_of_val(&address) as libc::socklen_t;
            ensure!(
                unsafe {
                    libc::getpeername(
                        fd,
                        (&mut address as *mut libc::sockaddr_storage).cast(),
                        &mut size,
                    )
                } == 0
                    && address.ss_family as i32 == libc::AF_UNIX,
                "Task clipboard requires a private local socket"
            );
            let nonce = *uuid::Uuid::parse_str(fields[2])
                .context("Invalid task clipboard challenge")?
                .as_bytes();
            let stream = unsafe { std::os::unix::net::UnixStream::from_raw_fd(fd) };
            stream.set_nonblocking(true)?;
            (
                Box::new(NativeIo {
                    stream,
                    stop: Arc::new(AtomicBool::new(false)),
                    timeout: Some(Duration::from_secs(5)),
                }) as Box<dyn Duplex>,
                nonce,
            )
        };
        #[cfg(target_os = "windows")]
        let (mut io, nonce) = {
            use std::os::windows::io::FromRawHandle;
            use windows_sys::Win32::{
                Foundation::*,
                Storage::FileSystem::{FILE_TYPE_PIPE, GetFileType},
                System::Console::*,
            };
            ensure!(
                fields.len() == 4 && fields[0] == "v1",
                "Invalid inherited task clipboard descriptor"
            );
            let read: usize = fields[1]
                .parse()
                .context("Invalid task clipboard read handle")?;
            let write: usize = fields[2]
                .parse()
                .context("Invalid task clipboard write handle")?;
            ensure!(
                read >= 4
                    && write >= 4
                    && read != usize::MAX
                    && write != usize::MAX
                    && read != write,
                "Invalid task clipboard pipe handles"
            );
            for handle in [read, write] {
                let handle = handle as HANDLE;
                ensure!(
                    [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE]
                        .into_iter()
                        .all(|kind| unsafe { GetStdHandle(kind) } != handle),
                    "Task clipboard handles cannot be stdio"
                );
                ensure!(
                    unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } != 0
                        && unsafe { GetFileType(handle) } == FILE_TYPE_PIPE,
                    "Task clipboard handles must be private pipes"
                );
            }
            let nonce = *uuid::Uuid::parse_str(fields[3])
                .context("Invalid task clipboard challenge")?
                .as_bytes();
            (
                Box::new(NativeIo {
                    read: unsafe { std::fs::File::from_raw_handle(read as _) },
                    write: unsafe { std::fs::File::from_raw_handle(write as _) },
                    stop: Arc::new(AtomicBool::new(false)),
                    timeout: Some(Duration::from_secs(5)),
                }) as Box<dyn Duplex>,
                nonce,
            )
        };
        #[cfg(not(any(unix, target_os = "windows")))]
        bail!("Inherited clipboard transfer is unavailable on this platform");
        send(&mut io, HELLO, 0, 0, &nonce).context("Could not establish host clipboard channel")?;
        let acknowledgment = expect_header(&mut io)?;
        ensure!(
            acknowledgment.kind == HELLO_ACK
                && acknowledgment.width == 0
                && acknowledgment.height == 0
                && acknowledgment.length == 16,
            "Host clipboard handshake was not acknowledged"
        );
        let mut echoed = [0; 16];
        io.read_exact(&mut echoed)?;
        ensure!(echoed == nonce, "Host clipboard challenge did not match");
        Ok(Some(Self { io }))
    }
    fn ack(&mut self) -> Result<()> {
        let packet = expect_header(&mut self.io)?;
        ensure!(
            empty(&packet, SUCCESS),
            "Host clipboard copy failed; native clipboard content may already have changed"
        );
        Ok(())
    }
    pub(crate) fn copy_image(&mut self, image: &RgbaImage) -> Result<()> {
        app_core::validate_image_size(image.width(), image.height())?;
        ensure!(
            image.as_raw().len() as u64 == u64::from(image.width()) * u64::from(image.height()) * 4,
            "Task clipboard image backing does not match dimensions"
        );
        send(
            &mut self.io,
            IMAGE,
            image.width(),
            image.height(),
            image.as_raw(),
        )
        .context("Could not transfer finalized task pixels to clipboard owner")?;
        self.ack()
    }
    pub(crate) fn copy_text(&mut self, text: &str) -> Result<()> {
        ensure!(
            valid_text(text),
            "Task clipboard link exceeds its text limit"
        );
        send(&mut self.io, TEXT, 0, 0, text.as_bytes())?;
        self.ack()
    }
}

enum Transfer {
    Image(RgbaImage),
    Text(String),
}
fn serve(
    mut io: impl Read + Write,
    nonce: [u8; 16],
    mut copy: impl FnMut(Transfer) -> Result<()>,
) -> Result<()> {
    let hello = expect_header(&mut io)?;
    ensure!(
        hello.kind == HELLO && hello.width == 0 && hello.height == 0 && hello.length == 16,
        "Invalid task clipboard handshake"
    );
    let mut challenge = [0; 16];
    io.read_exact(&mut challenge)?;
    ensure!(challenge == nonce, "Task clipboard challenge did not match");
    send(&mut io, HELLO_ACK, 0, 0, &nonce)?;
    let mut transfers = 0;
    while let Some(packet) = read_header(&mut io)? {
        transfers += 1;
        ensure!(transfers <= 16, "Task clipboard action limit exceeded");
        let transfer = match packet.kind {
            IMAGE => {
                app_core::validate_image_size(packet.width, packet.height)?;
                ensure!(
                    packet.length == u64::from(packet.width) * u64::from(packet.height) * 4,
                    "Task clipboard image length is invalid"
                );
                let length = usize::try_from(packet.length)?;
                let mut bytes = Vec::new();
                bytes
                    .try_reserve_exact(length)
                    .context("Could not allocate task clipboard pixels")?;
                bytes.resize(length, 0);
                for chunk in bytes.chunks_mut(CHUNK) {
                    io.read_exact(chunk)?;
                }
                Transfer::Image(
                    RgbaImage::from_raw(packet.width, packet.height, bytes)
                        .context("Invalid task clipboard image")?,
                )
            }
            TEXT => {
                ensure!(
                    packet.width == 0 && packet.height == 0 && (1..=2048).contains(&packet.length),
                    "Task clipboard link length is invalid"
                );
                let mut bytes = vec![0; packet.length as usize];
                io.read_exact(&mut bytes)?;
                let text = String::from_utf8(bytes).context("Task clipboard text must be UTF-8")?;
                ensure!(valid_text(&text), "Invalid task clipboard text");
                Transfer::Text(text)
            }
            _ => bail!("Unknown task clipboard packet"),
        };
        match copy(transfer) {
            Ok(()) => send(&mut io, SUCCESS, 0, 0, &[])?,
            Err(_) => {
                let _ = send(&mut io, FAILURE, 0, 0, &[]);
                bail!("Native host clipboard copy failed");
            }
        }
    }
    Ok(())
}

pub(crate) struct Prepared {
    server: Option<Box<dyn Duplex>>,
    nonce: [u8; 16],
    stop: Arc<AtomicBool>,
    #[cfg(unix)]
    child: Option<std::os::unix::net::UnixStream>,
    #[cfg(unix)]
    control: Option<std::os::unix::net::UnixStream>,
    #[cfg(target_os = "windows")]
    child: Vec<std::os::windows::io::OwnedHandle>,
}
impl Prepared {
    pub(crate) fn new(command: &mut Command) -> Result<Self> {
        let challenge = uuid::Uuid::new_v4();
        let nonce = *challenge.as_bytes();
        let stop = Arc::new(AtomicBool::new(false));
        #[cfg(unix)]
        {
            use std::os::{fd::AsRawFd, unix::process::CommandExt};
            let (server, child) = std::os::unix::net::UnixStream::pair()?;
            let control = server.try_clone()?;
            server.set_nonblocking(true)?;
            let fd = child.as_raw_fd();
            command.env(ENV, format!("v1:{fd}:{challenge}"));
            unsafe {
                command.pre_exec(move || {
                    let flags = libc::fcntl(fd, libc::F_GETFD);
                    if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            Ok(Self {
                server: Some(Box::new(NativeIo {
                    stream: server,
                    stop: stop.clone(),
                    timeout: None,
                })),
                nonce,
                stop,
                child: Some(child),
                control: Some(control),
            })
        }
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::io::{AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle};
            use windows_sys::Win32::{
                Foundation::*, Security::SECURITY_ATTRIBUTES, System::Pipes::CreatePipe,
            };
            fn pipe() -> Result<(OwnedHandle, OwnedHandle)> {
                let mut read = std::ptr::null_mut();
                let mut write = std::ptr::null_mut();
                let security = SECURITY_ATTRIBUTES {
                    nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: std::ptr::null_mut(),
                    bInheritHandle: 1,
                };
                ensure!(
                    unsafe { CreatePipe(&mut read, &mut write, &security, CHUNK as u32) } != 0,
                    "Could not create private task clipboard pipe"
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
                    "Could not protect parent clipboard pipe"
                );
            }
            command.env(
                ENV,
                format!(
                    "v1:{}:{}:{challenge}",
                    child_read.as_raw_handle() as usize,
                    child_write.as_raw_handle() as usize
                ),
            );
            let read = unsafe { std::fs::File::from_raw_handle(parent_read.into_raw_handle()) };
            let write = unsafe { std::fs::File::from_raw_handle(parent_write.into_raw_handle()) };
            Ok(Self {
                server: Some(Box::new(NativeIo {
                    read,
                    write,
                    stop: stop.clone(),
                    timeout: None,
                })),
                nonce,
                stop,
                child: vec![child_read, child_write],
            })
        }
        #[cfg(not(any(unix, target_os = "windows")))]
        {
            let _ = (command, nonce, stop);
            bail!("Task clipboard bridge is unavailable")
        }
    }
    pub(crate) fn start(
        self,
        clipboard: Arc<Mutex<app_core::ImageClipboard>>,
        canceled: Arc<AtomicBool>,
    ) -> Result<Bridge> {
        self.start_sink(move |transfer| {
            ensure!(
                !canceled.load(Ordering::SeqCst),
                "Task clipboard copy canceled before native publication"
            );
            let mut clipboard = clipboard
                .lock()
                .map_err(|_| anyhow::anyhow!("Host clipboard owner is unavailable"))?;
            match transfer {
                Transfer::Image(image) => {
                    clipboard.copy_image(&image)?;
                }
                Transfer::Text(text) => clipboard.copy_text(text)?,
            }
            Ok(())
        })
    }
    fn start_sink(
        mut self,
        copy: impl FnMut(Transfer) -> Result<()> + Send + 'static,
    ) -> Result<Bridge> {
        #[cfg(unix)]
        drop(self.child.take());
        #[cfg(target_os = "windows")]
        self.child.clear();
        let io = self
            .server
            .take()
            .context("Task clipboard parent stream is unavailable")?;
        let nonce = self.nonce;
        let stop = self.stop.clone();
        let failed = Arc::new(AtomicBool::new(false));
        let error = failed.clone();
        let thread = std::thread::Builder::new()
            .name("task-clipboard".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let result = serve(io, nonce, copy);
                if result.is_err() && !stop.load(Ordering::Relaxed) {
                    error.store(true, Ordering::Relaxed);
                }
            })?;
        Ok(Bridge {
            thread: Some(thread),
            stop: self.stop,
            failed,
            #[cfg(unix)]
            control: self.control.take(),
        })
    }
}
pub(crate) struct Bridge {
    thread: Option<JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    #[cfg(unix)]
    control: Option<std::os::unix::net::UnixStream>,
}
impl Bridge {
    pub(crate) fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }
    pub(crate) fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        #[cfg(unix)]
        if let Some(control) = self.control.take() {
            let _ = control.shutdown(std::net::Shutdown::Both);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    struct MemoryIo {
        input: Cursor<Vec<u8>>,
        output: Arc<Mutex<Vec<u8>>>,
    }
    impl Read for MemoryIo {
        fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(bytes)
        }
    }
    impl Write for MemoryIo {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.output.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    fn handshake(nonce: [u8; 16]) -> Vec<u8> {
        let mut bytes = Vec::new();
        send(&mut bytes, HELLO, 0, 0, &nonce).unwrap();
        bytes
    }
    #[test]
    fn binary_transfer_preserves_alpha_and_text_without_native_clipboard_access() {
        let nonce = *uuid::Uuid::new_v4().as_bytes();
        let image = RgbaImage::from_fn(3, 2, |x, y| {
            image::Rgba([x as u8 * 30, y as u8 * 70, 91, [0, 128, 255][x as usize]])
        });
        let mut bytes = handshake(nonce);
        send(&mut bytes, IMAGE, 3, 2, image.as_raw()).unwrap();
        send(&mut bytes, TEXT, 0, 0, b"https://example.invalid/link").unwrap();
        let output = Arc::new(Mutex::new(Vec::new()));
        let mut copies = 0;
        serve(
            MemoryIo {
                input: Cursor::new(bytes),
                output: output.clone(),
            },
            nonce,
            |transfer| {
                match transfer {
                    Transfer::Image(pixels) => assert_eq!(pixels, image),
                    Transfer::Text(text) => assert_eq!(text, "https://example.invalid/link"),
                };
                copies += 1;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(copies, 2);
        let response = output.lock().unwrap().clone();
        let mut response = Cursor::new(response);
        let hello = expect_header(&mut response).unwrap();
        assert_eq!(hello.kind, HELLO_ACK);
        let mut echoed = [0; 16];
        response.read_exact(&mut echoed).unwrap();
        assert_eq!(echoed, nonce);
        assert!(empty(&expect_header(&mut response).unwrap(), SUCCESS));
        assert!(empty(&expect_header(&mut response).unwrap(), SUCCESS));
        assert!(read_header(&mut response).unwrap().is_none());
    }
    #[test]
    fn malformed_or_incomplete_packets_never_publish_a_copy() {
        let nonce = [7u8; 16];
        let mut wrong = handshake([8; 16]);
        let mut excessive = handshake(nonce);
        excessive.extend_from_slice(&header(IMAGE, u32::MAX, u32::MAX, u64::MAX));
        let mut truncated = handshake(nonce);
        truncated.extend_from_slice(&header(IMAGE, 3, 2, 24));
        truncated.extend_from_slice(&[1; 23]);
        let mut invalid_text = handshake(nonce);
        send(&mut invalid_text, TEXT, 0, 0, b"bad\ntext").unwrap();
        let mut unknown = handshake(nonce);
        unknown.extend_from_slice(&header(900, 0, 0, 0));
        wrong[0] = 0;
        for bytes in [wrong, excessive, truncated, invalid_text, unknown] {
            let output = Arc::new(Mutex::new(Vec::new()));
            let mut copies = 0;
            assert!(
                serve(
                    MemoryIo {
                        input: Cursor::new(bytes),
                        output
                    },
                    nonce,
                    |_| {
                        copies += 1;
                        Ok(())
                    }
                )
                .is_err()
            );
            assert_eq!(copies, 0);
        }
        let mut wrong_nonce = handshake([8; 16]);
        wrong_nonce.extend_from_slice(&header(IMAGE, 1, 1, 4));
        wrong_nonce.extend_from_slice(&[255; 4]);
        let output = Arc::new(Mutex::new(Vec::new()));
        assert!(
            serve(
                MemoryIo {
                    input: Cursor::new(wrong_nonce),
                    output: output.clone()
                },
                nonce,
                |_| panic!("mismatched handshake must not copy")
            )
            .is_err()
        );
        assert!(output.lock().unwrap().is_empty());
    }
    #[test]
    fn native_failure_ack_is_fixed_and_no_payload_is_echoed() {
        let nonce = [3u8; 16];
        let mut bytes = handshake(nonce);
        send(&mut bytes, TEXT, 0, 0, b"DO_NOT_ECHO_LINK").unwrap();
        let output = Arc::new(Mutex::new(Vec::new()));
        assert!(
            serve(
                MemoryIo {
                    input: Cursor::new(bytes),
                    output: output.clone()
                },
                nonce,
                |_| bail!("DO_NOT_ECHO_NATIVE_ERROR")
            )
            .is_err()
        );
        let raw = output.lock().unwrap();
        assert!(!raw.windows(4).any(|value| value == b"ECHO"));
        assert_eq!(raw.len(), 48 + HEADER);
    }
    #[test]
    fn client_rejects_invalid_backing_and_text_before_writing_payload() {
        let output = Arc::new(Mutex::new(Vec::new()));
        let mut client = Client {
            io: Box::new(MemoryIo {
                input: Cursor::new(Vec::new()),
                output: output.clone(),
            }),
        };
        let invalid = RgbaImage::from_raw(1, 1, vec![7; 8]).unwrap();
        assert!(client.copy_image(&invalid).is_err());
        assert!(client.copy_text(&"x".repeat(2049)).is_err());
        assert!(client.copy_text("bad\nlink").is_err());
        assert!(output.lock().unwrap().is_empty());
    }

    #[test]
    fn clipboard_bridge_child_fixture() {
        let mode = std::env::var("SCREENSHOTS_TEST_CLIPBOARD_BRIDGE").unwrap_or_default();
        if mode == "grandchild" {
            assert!(Client::from_env().is_err());
            return;
        }
        if mode != "child" {
            return;
        }
        let mut client = Client::from_env().unwrap().unwrap();
        let pixels = RgbaImage::from_pixel(3, 2, image::Rgba([17, 29, 71, 128]));
        client.copy_image(&pixels).unwrap();
        client.copy_text("https://example.invalid/bridge").unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "task_clipboard::tests::clipboard_bridge_child_fixture",
                "--nocapture",
            ])
            .env("SCREENSHOTS_TEST_CLIPBOARD_BRIDGE", "grandchild")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
    }
    #[test]
    fn inherited_channel_handshake_and_noninherit_restoration_work_in_a_real_child() {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "task_clipboard::tests::clipboard_bridge_child_fixture",
                "--nocapture",
            ])
            .env("SCREENSHOTS_TEST_CLIPBOARD_BRIDGE", "child")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let prepared = Prepared::new(&mut command).unwrap();
        let mut child = command.spawn().unwrap();
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed = count.clone();
        let mut bridge = prepared
            .start_sink(move |transfer| {
                match transfer {
                    Transfer::Image(image) => {
                        assert!(image.pixels().all(|pixel| pixel.0 == [17, 29, 71, 128]))
                    }
                    Transfer::Text(text) => assert_eq!(text, "https://example.invalid/bridge"),
                };
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                bridge.stop();
                panic!("Inherited clipboard fixture timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        bridge.stop();
        assert!(!bridge.failed());
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }
}
