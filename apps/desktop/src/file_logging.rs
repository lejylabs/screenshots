//! Bounded, field-free desktop diagnostics. Headless commands never initialize it.
use std::{
    fmt::{self, Write as _},
    fs::{File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant, UNIX_EPOCH},
};
use tracing::{Event, Metadata, Subscriber};
use tracing_subscriber::{Layer, filter::Targets, layer::Context, prelude::*};

#[path = "file_logging_transport.rs"]
mod transport;
use transport::{Mailbox, ReceiveError};

const QUEUE: usize = 128;
const SLOTS: usize = 4;
const SLOT_BYTES: u64 = 256 * 1024;
const RECORD_BYTES: usize = 512;
const HEADER: &[u8] = b"screenshots-static-events-v1\n";
const SHUTDOWN: Duration = Duration::from_millis(250);
static STARTED: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct State {
    accepting: AtomicBool,
    in_flight: AtomicUsize,
    stopping: AtomicBool,
    failed: AtomicBool,
    written: AtomicU64,
    dropped: AtomicU64,
    #[cfg(test)]
    waiting_for_producer: std::sync::OnceLock<SyncSender<()>>,
}
struct Record {
    metadata: &'static Metadata<'static>,
    at: Instant,
}
enum Message {
    Record(Record),
    #[cfg(test)]
    Barrier(SyncSender<()>),
}
#[derive(Clone)]
struct Events {
    sender: Arc<Mailbox>,
    state: Arc<State>,
}
struct Admission<'a>(&'a Events);
impl Drop for Admission<'_> {
    fn drop(&mut self) {
        self.0.state.in_flight.fetch_sub(1, Ordering::SeqCst);
        if self.0.state.stopping.load(Ordering::SeqCst) {
            self.0.sender.signal();
        }
    }
}
impl Events {
    fn admit(&self) -> Option<Admission<'_>> {
        if !self.state.accepting.load(Ordering::Acquire) {
            return None;
        }
        self.state.in_flight.fetch_add(1, Ordering::SeqCst);
        let admission = Admission(self);
        // Register before the second check; shutdown cannot miss an admitted sender.
        if !self.state.accepting.load(Ordering::SeqCst) {
            return None;
        }
        Some(admission)
    }
}
impl Events {
    fn record(&self, metadata: &'static Metadata<'static>) {
        let Some(_admission) = self.admit() else {
            return;
        };
        // Do not record/format fields, including the message field and errors.
        let record = Record {
            metadata,
            at: Instant::now(),
        };
        if self.sender.try_send(Message::Record(record)).is_err() {
            self.state.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}
impl<S: Subscriber> Layer<S> for Events {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        self.record(event.metadata());
    }
}

pub(crate) struct LoggingGuard {
    sender: Arc<Mailbox>,
    state: Arc<State>,
    done: Option<Receiver<()>>,
}
#[derive(Debug, PartialEq, Eq)]
enum ShutdownResult {
    Complete,
    TimedOut,
}
impl LoggingGuard {
    fn shutdown(&mut self, timeout: Duration) -> ShutdownResult {
        let Some(done) = self.done.take() else {
            return ShutdownResult::Complete;
        };
        self.state.accepting.store(false, Ordering::SeqCst);
        self.state.stopping.store(true, Ordering::SeqCst);
        // A coalesced native credit wakes an empty queue or a pending sender.
        self.sender.signal();
        match done.recv_timeout(timeout) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => ShutdownResult::Complete,
            Err(mpsc::RecvTimeoutError::Timeout) => ShutdownResult::TimedOut,
        }
    }
}
impl Drop for LoggingGuard {
    fn drop(&mut self) {
        if self.shutdown(SHUTDOWN) == ShutdownResult::TimedOut {
            // The single worker may be in native filesystem I/O; never wait or restart it.
            self.state.failed.store(true, Ordering::Release);
        }
    }
}

/// GUI startup only. All path lookup, file work and text formatting run on the worker.
pub(crate) fn init() -> Option<LoggingGuard> {
    if STARTED.swap(true, Ordering::AcqRel) {
        return None;
    }
    let directory = || {
        let database = app_core::history_database_path().map_err(|_| io::ErrorKind::NotFound)?;
        Ok(database
            .parent()
            .ok_or(io::ErrorKind::NotFound)?
            .join("logs"))
    };
    let (events, guard) = match start(directory) {
        Ok(value) => value,
        Err(_) => {
            eprintln!("screenshots: application file logging is unavailable");
            return None;
        }
    };
    // Target/level directives only; no field predicates or dynamic-field formatting.
    let targets = level_targets(std::env::var("RUST_LOG").ok().as_deref());
    if tracing_subscriber::registry()
        .with(targets)
        .with(events)
        .try_init()
        .is_err()
    {
        eprintln!("screenshots: application file logging subscriber is unavailable");
        return None;
    }
    Some(guard)
}

fn level_targets(value: Option<&str>) -> Targets {
    value
        .filter(|value| value.len() <= 2048 && !value.contains(['[', ']', '{', '}']))
        .and_then(|value| value.parse::<Targets>().ok())
        .unwrap_or_else(|| Targets::new().with_default(tracing::level_filters::LevelFilter::INFO))
}

fn start(
    directory: impl FnOnce() -> io::Result<PathBuf> + Send + 'static,
) -> io::Result<(Events, LoggingGuard)> {
    let state = Arc::new(State::default());
    state.accepting.store(true, Ordering::Release);
    let sender = Mailbox::new(QUEUE, state.clone())?;
    let receiver = sender.clone();
    let (done_sender, done) = mpsc::sync_channel(1);
    let worker_state = state.clone();
    let _worker = std::thread::Builder::new()
        .name("desktop-file-log".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            let result = directory().and_then(|path| worker(&path, receiver, &worker_state));
            worker_state.accepting.store(false, Ordering::SeqCst);
            if result.is_err() {
                worker_state.failed.store(true, Ordering::Release);
                eprintln!("screenshots: application file logging is unavailable");
            }
            let _ = done_sender.try_send(());
        })?;
    Ok((
        Events {
            sender: sender.clone(),
            state: state.clone(),
        },
        LoggingGuard {
            sender,
            state,
            done: Some(done),
        },
    ))
}

struct Line {
    bytes: [u8; RECORD_BYTES],
    len: usize,
}
impl Line {
    fn new() -> Self {
        Self {
            bytes: [0; RECORD_BYTES],
            len: 0,
        }
    }
    fn component(&mut self, value: &str) -> fmt::Result {
        if value.is_empty()
            || value.len() > 96
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_:".contains(&byte))
        {
            return self.write_str("redacted");
        }
        self.write_str(value)
    }
    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}
impl fmt::Write for Line {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        let end = self.len.checked_add(value.len()).ok_or(fmt::Error)?;
        let target = self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?;
        target.copy_from_slice(value.as_bytes());
        self.len = end;
        Ok(())
    }
}
fn record_line(record: &Record, started: Instant, dropped: u64) -> io::Result<Line> {
    let mut line = Line::new();
    let result = (|| {
        write!(
            line,
            "elapsed_ms={} level={} target=",
            record.at.saturating_duration_since(started).as_millis(),
            record.metadata.level()
        )?;
        line.component(record.metadata.target())?;
        line.write_str(" module=")?;
        line.component(record.metadata.module_path().unwrap_or("unknown"))?;
        writeln!(
            line,
            " line={} dropped_total={dropped} fields=redacted",
            record.metadata.line().unwrap_or(0)
        )
    })();
    result.map_err(|_| io::ErrorKind::InvalidData)?;
    Ok(line)
}

struct Slot(File);
impl std::ops::Deref for Slot {
    type Target = File;
    fn deref(&self) -> &File {
        &self.0
    }
}
impl std::ops::DerefMut for Slot {
    fn deref_mut(&mut self) -> &mut File {
        &mut self.0
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        // Release our lock even if another thread forked before its exec.
        let _ = self.0.unlock();
    }
}
struct Ring {
    files: Vec<Slot>,
    next: usize,
    current: Option<usize>,
    bytes: u64,
    limit: u64,
}
impl Ring {
    fn open(directory: &Path, limit: u64) -> io::Result<Self> {
        if limit < HEADER.len() as u64 + RECORD_BYTES as u64 || limit > SLOT_BYTES {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        app_core::CaptureSettings::ensure_output_directory(directory)
            .map_err(|error| io::Error::other(error.to_string()))?;
        if !std::fs::symlink_metadata(directory)?.is_dir() {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let mut files = Vec::with_capacity(SLOTS);
        let mut oldest = None;
        let mut next = 0;
        for index in 0..SLOTS {
            let path = directory.join(format!("events-{index}.log"));
            let mut options = OpenOptions::new();
            options.read(true).write(true).create_new(true);
            no_follow(&mut options);
            let (file, created) = match options.open(&path) {
                Ok(file) => (file, true),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    let mut options = OpenOptions::new();
                    options.read(true).write(true);
                    no_follow(&mut options);
                    (options.open(&path)?, false)
                }
                Err(error) => return Err(error),
            };
            let mut file = Slot(file);
            regular_private_file(&file)?;
            file.try_lock().map_err(|_| io::ErrorKind::WouldBlock)?;
            if created {
                file.write_all(HEADER)?;
            } else {
                if file.metadata()?.len() > limit {
                    return Err(io::ErrorKind::InvalidData.into());
                }
                let mut header = [0; HEADER.len()];
                file.read_exact(&mut header)?;
                if header != HEADER {
                    return Err(io::ErrorKind::InvalidData.into());
                }
            }
            let modified = file.metadata()?.modified().unwrap_or(UNIX_EPOCH);
            if oldest.is_none_or(|time| modified < time) {
                oldest = Some(modified);
                next = index;
            }
            files.push(file);
        }
        Ok(Self {
            files,
            next,
            current: None,
            bytes: 0,
            limit,
        })
    }
    fn write(&mut self, line: &[u8]) -> io::Result<()> {
        if line.len() > RECORD_BYTES {
            return Err(io::ErrorKind::InvalidData.into());
        }
        if self.current.is_none() || self.bytes + line.len() as u64 > self.limit {
            let index = self.next;
            let file = &mut self.files[index];
            regular_private_file(file)?;
            file.set_len(HEADER.len() as u64)?;
            file.seek(SeekFrom::Start(HEADER.len() as u64))?;
            self.current = Some(index);
            self.next = (index + 1) % SLOTS;
            self.bytes = HEADER.len() as u64;
        }
        let file = &mut self.files[self.current.unwrap()];
        if file.metadata()?.len() != self.bytes {
            return Err(io::ErrorKind::InvalidData.into());
        }
        file.write_all(line)?;
        self.bytes += line.len() as u64;
        Ok(())
    }
}

#[cfg(unix)]
fn no_follow(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt as _;
    options
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC);
}
#[cfg(windows)]
fn no_follow(options: &mut OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt as _;
    use windows_sys::Win32::Storage::FileSystem::*;
    options
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
}
fn regular_private_file(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(io::ErrorKind::InvalidData.into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;
        if metadata.nlink() != 1
            || metadata.mode() & 0o077 != 0
            || metadata.uid() != unsafe { libc::geteuid() }
        {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle as _;
        use windows_sys::Win32::Storage::FileSystem::*;
        let mut information = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut information) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || information.nNumberOfLinks != 1
        {
            return Err(io::ErrorKind::InvalidData.into());
        }
    }
    Ok(())
}

fn worker(directory: &Path, receiver: Arc<Mailbox>, state: &State) -> io::Result<()> {
    let started = Instant::now();
    let mut ring = Ring::open(directory, SLOT_BYTES)?;
    loop {
        if state.failed.load(Ordering::Acquire) {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        let empty = match receiver.try_recv() {
            Ok(message) => {
                handle_message(message, &mut ring, started, state)?;
                false
            }
            Err(ReceiveError::Busy | ReceiveError::Empty) => true,
        };
        if state.stopping.load(Ordering::SeqCst) {
            if state.in_flight.load(Ordering::SeqCst) != 0 {
                #[cfg(test)]
                if let Some(waiting) = state.waiting_for_producer.get() {
                    let _ = waiting.try_send(());
                }
                if empty {
                    receiver.wait()?;
                }
                continue;
            }
            // All pre-close senders finished; no later sender can enqueue a record.
            for _ in 0..QUEUE {
                match receiver.try_recv() {
                    Ok(message) => handle_message(message, &mut ring, started, state)?,
                    Err(ReceiveError::Empty) => break,
                    Err(ReceiveError::Busy) => return Err(io::ErrorKind::InvalidData.into()),
                }
            }
            let mut line = Line::new();
            writeln!(
                line,
                "shutdown written={} dropped_total={} fields=redacted",
                state.written.load(Ordering::Relaxed),
                state.dropped.load(Ordering::Relaxed)
            )
            .map_err(|_| io::ErrorKind::InvalidData)?;
            ring.write(line.as_bytes())?;
            break;
        }
        if empty {
            receiver.wait()?;
        }
    }
    Ok(())
}
fn handle_message(
    message: Message,
    ring: &mut Ring,
    started: Instant,
    state: &State,
) -> io::Result<()> {
    match message {
        Message::Record(record) => {
            let line = record_line(&record, started, state.dropped.load(Ordering::Relaxed))?;
            ring.write(line.as_bytes())?;
            state.written.fetch_add(1, Ordering::Relaxed);
        }
        #[cfg(test)]
        Message::Barrier(done) => {
            let _ = done.try_send(());
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "file_logging_tests.rs"]
mod tests;
