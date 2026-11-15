//! Explicit native task hotkeys and authenticated loopback dispatch.

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    ffi::OsString,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};

#[path = "task_host_endpoint.rs"]
mod endpoint;
#[cfg(target_os = "macos")]
#[path = "task_host_macos.rs"]
mod macos;
#[cfg(target_os = "linux")]
#[path = "task_host_portal.rs"]
mod portal;
#[path = "task_host_process.rs"]
mod process;
#[cfg(target_os = "windows")]
#[path = "task_host_windows.rs"]
mod windows;
#[cfg(target_os = "linux")]
#[path = "task_host_x11.rs"]
mod x11;

const MAX_MESSAGE: usize = 4096;
const TIMEOUT: Duration = Duration::from_secs(2);
type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    version: u32,
    #[serde(default = "task_timeout")]
    task_timeout_ms: u64,
    bindings: Vec<Binding>,
}
fn task_timeout() -> u64 {
    300_000
}
fn valid_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    id: String,
    task: PathBuf,
    #[serde(default)]
    hotkey: String,
}
#[derive(Clone)]
pub(super) struct Key {
    profile: usize,
    #[cfg(target_os = "macos")]
    normalized: String,
    modifiers: u32,
    name: String,
}
struct ReadyConfig {
    bindings: Vec<Binding>,
    keys: Vec<Key>,
    timeout: Duration,
}

fn bounded_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    ensure!(
        path.metadata()?.is_file(),
        "Task-host input must be a regular file"
    );
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    let info = file.metadata()?;
    ensure!(
        info.is_file() && info.len() <= limit,
        "Task-host input exceeds its file limit"
    );
    let mut bytes = Vec::with_capacity(info.len() as usize);
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= limit,
        "Task-host input grew past its file limit"
    );
    Ok(bytes)
}
fn json_object<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    ensure!(
        bytes
            .iter()
            .copied()
            .find(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
            == Some(b'{'),
        "Task-host JSON must be one object"
    );
    serde_json::from_slice(bytes).map_err(|_| anyhow::anyhow!("Invalid task-host JSON"))
}
impl ReadyConfig {
    fn read(path: &Path) -> Result<Self> {
        let path = std::path::absolute(path)?;
        ensure!(
            path.as_os_str().as_encoded_bytes().len() <= 4096,
            "Task-host config path is too long"
        );
        let bytes = bounded_file(&path, 65536)?;
        let config: Config = json_object(&bytes)?;
        ensure!(
            config.version == 1 && (1..=16).contains(&config.bindings.len()),
            "Task-host version must be 1 with 1–16 profiles"
        );
        let shape: serde_json::Value = json_object(&bytes)?;
        ensure!(
            shape
                .get("bindings")
                .and_then(|value| value.as_array())
                .is_some_and(|values| values.iter().all(|value| value.is_object())),
            "Task-host bindings must be JSON objects"
        );
        ensure!(
            (100..=5_400_000).contains(&config.task_timeout_ms),
            "Task-host timeout must be 100–5400000 ms"
        );
        let base = path.parent().context("Task-host config has no parent")?;
        let mut ids = HashSet::new();
        let mut shortcuts = HashSet::new();
        let mut keys = Vec::new();
        let mut bindings = Vec::new();
        for (profile, mut binding) in config.bindings.into_iter().enumerate() {
            ensure!(
                valid_id(&binding.id) && ids.insert(binding.id.clone()),
                "Task-host profile IDs must be unique ASCII names within 64 bytes"
            );
            ensure!(
                !binding.task.as_os_str().is_empty()
                    && binding.task.as_os_str().as_encoded_bytes().len() <= 4096,
                "Task-host profile path is invalid"
            );
            binding.task = std::path::absolute(if binding.task.is_absolute() {
                binding.task
            } else {
                base.join(binding.task)
            })?;
            let bytes = bounded_file(&binding.task, 65536)?;
            let _: serde_json::Map<String, serde_json::Value> = json_object(&bytes)?;
            crate::task::validate_profile(&binding.task)?;
            let candidate = app_core::CaptureHotkeys([
                binding.hotkey.clone(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ])
            .validated()?;
            binding.hotkey = candidate.0[0].clone();
            if !binding.hotkey.is_empty() {
                ensure!(
                    shortcuts.insert(binding.hotkey.clone()),
                    "Task-host hotkeys must be unique"
                );
                let pieces = binding.hotkey.split('+').collect::<Vec<_>>();
                let name = pieces.last().unwrap().to_string();
                let modifiers = pieces[..pieces.len() - 1].iter().fold(0, |mods, name| {
                    mods | match *name {
                        "Ctrl" => 1,
                        "Alt" => 2,
                        "Shift" => 4,
                        "Super" => 8,
                        _ => 0,
                    }
                });
                keys.push(Key {
                    profile,
                    #[cfg(target_os = "macos")]
                    normalized: binding.hotkey.clone(),
                    modifiers,
                    name,
                });
            }
            bindings.push(binding);
        }
        Ok(Self {
            bindings,
            keys,
            timeout: Duration::from_millis(config.task_timeout_ms),
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EndpointInfo {
    version: u32,
    port: u16,
    pid: u32,
    token: String,
}
impl Drop for EndpointInfo {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}
impl EndpointInfo {
    fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1
                && self.port > 0
                && self.pid > 0
                && self.token.len() == 64
                && self.token.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid task-host endpoint"
        );
        Ok(())
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    version: u32,
    token: String,
    command: Command,
}
impl Drop for Request {
    fn drop(&mut self) {
        self.token.zeroize();
    }
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Command {
    Trigger { profile: String },
    Status,
    Stop,
}

fn frame_read(reader: &mut impl Read) -> Result<Zeroizing<Vec<u8>>> {
    let mut prefix = [0; 4];
    reader
        .read_exact(&mut prefix)
        .context("Task-host frame is truncated")?;
    let length = u32::from_be_bytes(prefix) as usize;
    ensure!(
        (1..=MAX_MESSAGE).contains(&length),
        "Task-host frame exceeds 4096 bytes"
    );
    let mut bytes = Zeroizing::new(vec![0; length]);
    reader
        .read_exact(&mut bytes)
        .context("Task-host frame is truncated")?;
    Ok(bytes)
}
fn frame_write(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
    let bytes = Zeroizing::new(serde_json::to_vec(value)?);
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_MESSAGE,
        "Task-host result exceeds its frame limit"
    );
    writer.write_all(&(bytes.len() as u32).to_be_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}
struct ConnectionIo<'a> {
    stream: &'a mut TcpStream,
    deadline: Instant,
}
impl Read for ConnectionIo<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        self.stream.set_read_timeout(Some(remaining))?;
        self.stream.read(bytes)
    }
}
impl Write for ConnectionIo<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        self.stream.set_write_timeout(Some(remaining))?;
        self.stream.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.stream.flush()
    }
}
fn authenticated(expected: &str, actual: &str) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    expected
        .bytes()
        .zip(actual.bytes())
        .fold(0u8, |difference, (a, b)| difference | (a ^ b))
        == 0
}

#[derive(Clone, Serialize)]
pub(super) struct JobStatus {
    job: String,
    profile: String,
    state: &'static str,
    success: bool,
    exit_code: Option<i32>,
}
pub(super) struct Job {
    id: String,
    profile: usize,
    cancel: Arc<AtomicBool>,
}
enum Work {
    Run(Job),
    Stop,
}
pub(super) struct Host {
    config: ReadyConfig,
    token: Zeroizing<String>,
    stopping: AtomicBool,
    busy: AtomicBool,
    last: Mutex<Option<JobStatus>>,
    cancellation: Mutex<Option<Arc<AtomicBool>>>,
    sender: SyncSender<Work>,
    wake: Mutex<Option<Wake>>,
    stopped: Condvar,
    stop_lock: Mutex<()>,
    address: SocketAddrV4,
    clipboard: Arc<Mutex<app_core::ImageClipboard>>,
}
impl Host {
    fn trigger(&self, profile: usize) -> serde_json::Value {
        if self.stopping.load(Ordering::SeqCst) {
            return serde_json::json!({"ok":false,"error":"stopping"});
        }
        if self
            .busy
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return serde_json::json!({"ok":false,"error":"busy"});
        }
        let id = uuid::Uuid::new_v4().to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        *self.cancellation.lock().unwrap() = Some(cancel.clone());
        *self.last.lock().unwrap() = Some(JobStatus {
            job: id.clone(),
            profile: self.config.bindings[profile].id.clone(),
            state: "running",
            success: false,
            exit_code: None,
        });
        if self
            .sender
            .try_send(Work::Run(Job {
                id: id.clone(),
                profile,
                cancel,
            }))
            .is_err()
        {
            self.busy.store(false, Ordering::SeqCst);
            *self.cancellation.lock().unwrap() = None;
            *self.last.lock().unwrap() = Some(JobStatus {
                job: id,
                profile: self.config.bindings[profile].id.clone(),
                state: "failed",
                success: false,
                exit_code: None,
            });
            return serde_json::json!({"ok":false,"error":"worker_unavailable"});
        }
        serde_json::json!({"ok":true,"job":id,"state":"accepted"})
    }
    fn status(&self) -> serde_json::Value {
        serde_json::json!({"ok":true,"stopping":self.stopping.load(Ordering::SeqCst),"busy":self.busy.load(Ordering::SeqCst),"last":self.last.lock().unwrap().clone()})
    }
    fn stop(&self) {
        if self.stopping.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(cancel) = self.cancellation.lock().unwrap().as_ref() {
            cancel.store(true, Ordering::SeqCst);
        }
        let _ = self.sender.try_send(Work::Stop);
        if let Some(wake) = self.wake.lock().unwrap().as_ref() {
            wake();
        }
        let _guard = self.stop_lock.lock().unwrap();
        self.stopped.notify_all();
        let _ = TcpStream::connect_timeout(&self.address.into(), Duration::from_millis(100));
    }
    fn request(&self, request: Request) -> serde_json::Value {
        if request.version != 1 || !authenticated(&self.token, &request.token) {
            return serde_json::json!({"ok":false,"error":"unauthorized"});
        }
        match &request.command {
            Command::Status => self.status(),
            Command::Stop => {
                self.stop();
                serde_json::json!({"ok":true,"state":"stopping"})
            }
            Command::Trigger { profile } => match self
                .config
                .bindings
                .iter()
                .position(|entry| entry.id == *profile)
            {
                Some(index) => self.trigger(index),
                None => serde_json::json!({"ok":false,"error":"unknown_profile"}),
            },
        }
    }
}

fn worker(host: Arc<Host>, receiver: mpsc::Receiver<Work>) {
    while let Ok(work) = receiver.recv() {
        let Work::Run(job) = work else {
            break;
        };
        let outcome = if host.stopping.load(Ordering::SeqCst) {
            process::Outcome::canceled()
        } else {
            process::run(
                &host.config.bindings[job.profile].task,
                host.config.timeout,
                &job.cancel,
                host.clipboard.clone(),
            )
        };
        *host.last.lock().unwrap() = Some(JobStatus {
            job: job.id,
            profile: host.config.bindings[job.profile].id.clone(),
            state: outcome.state,
            success: outcome.success,
            exit_code: outcome.exit_code,
        });
        *host.cancellation.lock().unwrap() = None;
        host.busy.store(false, Ordering::SeqCst);
        if host.stopping.load(Ordering::SeqCst) {
            break;
        }
    }
}
fn listener(host: Arc<Host>, listener: TcpListener) {
    loop {
        let (mut stream, address) = match listener.accept() {
            Ok(pair) => pair,
            Err(_) => {
                host.stop();
                break;
            }
        };
        if host.stopping.load(Ordering::SeqCst) {
            break;
        }
        if !address.ip().is_loopback() {
            continue;
        }
        let mut io = ConnectionIo {
            stream: &mut stream,
            deadline: Instant::now() + TIMEOUT,
        };
        let response = frame_read(&mut io)
            .and_then(|bytes| json_object::<Request>(&bytes))
            .map(|request| host.request(request))
            .unwrap_or_else(|_| serde_json::json!({"ok":false,"error":"invalid_request"}));
        let _ = frame_write(&mut io, &response);
        if host.stopping.load(Ordering::SeqCst) {
            break;
        }
    }
}

struct Signals {
    #[cfg(unix)]
    handle: signal_hook::iterator::Handle,
    #[cfg(unix)]
    thread: Option<JoinHandle<()>>,
    #[cfg(target_os = "windows")]
    _guard: windows::SignalGuard,
}
impl Signals {
    fn new(host: Arc<Host>) -> Result<Self> {
        #[cfg(unix)]
        {
            let mut signals = signal_hook::iterator::Signals::new([
                signal_hook::consts::SIGINT,
                signal_hook::consts::SIGTERM,
            ])?;
            let handle = signals.handle();
            let thread = std::thread::Builder::new()
                .name("task-host-signals".into())
                .stack_size(256 * 1024)
                .spawn(move || {
                    if signals.forever().next().is_some() {
                        host.stop();
                    }
                })?;
            Ok(Self {
                handle,
                thread: Some(thread),
            })
        }
        #[cfg(target_os = "windows")]
        {
            Ok(Self {
                _guard: windows::SignalGuard::new(host)?,
            })
        }
        #[cfg(not(any(unix, target_os = "windows")))]
        {
            let _ = host;
            Ok(Self {})
        }
    }
}
impl Drop for Signals {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            self.handle.close();
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }
}

fn serve(config: ReadyConfig, endpoint_path: &Path) -> Result<()> {
    ensure!(
        !endpoint_path.as_os_str().is_empty()
            && endpoint_path.as_os_str().as_encoded_bytes().len() <= 4096,
        "Task-host endpoint path is invalid"
    );
    match endpoint_path.symlink_metadata() {
        Ok(_) => bail!("Task-host endpoint already exists; inspect stale endpoints explicitly"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => bail!("Task-host endpoint path is unavailable"),
    }
    let listener_socket = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .context("Could not bind task-host loopback listener")?;
    let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, listener_socket.local_addr()?.port());
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let (sender, receiver) = mpsc::sync_channel(1);
    let host = Arc::new(Host {
        config,
        token: Zeroizing::new(token),
        stopping: AtomicBool::new(false),
        busy: AtomicBool::new(false),
        last: Mutex::new(None),
        cancellation: Mutex::new(None),
        sender,
        wake: Mutex::new(None),
        stopped: Condvar::new(),
        stop_lock: Mutex::new(()),
        address,
        clipboard: Arc::new(Mutex::new(app_core::ImageClipboard::default())),
    });
    let signals = Signals::new(host.clone())?;
    let mut native = Native::new(host.clone())?;
    *host.wake.lock().unwrap() = native.wake();
    let dispatch = host.clone();
    let worker = std::thread::Builder::new()
        .name("task-host-worker".into())
        .stack_size(256 * 1024)
        .spawn(move || worker(dispatch, receiver))?;
    let ipc = host.clone();
    let server = match std::thread::Builder::new()
        .name("task-host-ipc".into())
        .stack_size(256 * 1024)
        .spawn(move || listener(ipc, listener_socket))
    {
        Ok(thread) => thread,
        Err(error) => {
            host.stop();
            let _ = worker.join();
            return Err(error.into());
        }
    };
    let publication = endpoint::Lease::create(
        endpoint_path,
        &EndpointInfo {
            version: 1,
            port: address.port(),
            pid: std::process::id(),
            token: host.token.to_string(),
        },
    );
    let result = match publication {
        Ok(mut lease) => {
            let ready = result_line(
                serde_json::json!({"ready":true,"profiles":host.config.bindings.len(),"hotkeys":host.config.keys.len()}),
            );
            let result = ready.and_then(|_| native.run(&host));
            host.stop();
            let _ = server.join();
            let _ = worker.join();
            let cleanup = lease.cleanup();
            drop(lease);
            cleanup.context("Task host stopped but endpoint cleanup failed")?;
            result
        }
        Err(error) => {
            host.stop();
            let _ = server.join();
            let _ = worker.join();
            Err(error)
        }
    };
    drop(signals);
    result
}

enum Native {
    Idle,
    #[cfg(target_os = "linux")]
    X11(x11::Native),
    #[cfg(target_os = "linux")]
    Portal(portal::Native),
    #[cfg(target_os = "windows")]
    Windows(windows::Native),
    #[cfg(target_os = "macos")]
    Macos(macos::Native),
}
impl Native {
    fn new(host: Arc<Host>) -> Result<Self> {
        if host.config.keys.is_empty() {
            return Ok(Self::Idle);
        }
        #[cfg(target_os = "linux")]
        {
            if std::env::var_os("XDG_SESSION_TYPE").as_deref()
                == Some(std::ffi::OsStr::new("wayland"))
                || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty())
            {
                return Ok(Self::Portal(portal::Native::new(host)?));
            }
            return Ok(Self::X11(x11::Native::new(host)?));
        }
        #[cfg(target_os = "windows")]
        {
            return Ok(Self::Windows(windows::Native::new(host)?));
        }
        #[cfg(target_os = "macos")]
        {
            return Ok(Self::Macos(macos::Native::new(host)?));
        }
        #[allow(unreachable_code)]
        {
            let _ = host;
            bail!("Native task hotkeys are unavailable on this platform");
        }
    }
    fn wake(&self) -> Option<Wake> {
        match self {
            Self::Idle => None,
            #[cfg(target_os = "linux")]
            Self::X11(value) => Some(value.wake()),
            #[cfg(target_os = "linux")]
            Self::Portal(value) => Some(value.wake()),
            #[cfg(target_os = "windows")]
            Self::Windows(value) => Some(value.wake()),
            #[cfg(target_os = "macos")]
            Self::Macos(value) => Some(value.wake()),
        }
    }
    fn run(&mut self, host: &Host) -> Result<()> {
        match self {
            Self::Idle => {
                let guard = host.stop_lock.lock().unwrap();
                let _guard = host
                    .stopped
                    .wait_while(guard, |_| !host.stopping.load(Ordering::SeqCst))
                    .unwrap();
                Ok(())
            }
            #[cfg(target_os = "linux")]
            Self::X11(value) => value.run(),
            #[cfg(target_os = "linux")]
            Self::Portal(value) => value.run(),
            #[cfg(target_os = "windows")]
            Self::Windows(value) => value.run(),
            #[cfg(target_os = "macos")]
            Self::Macos(value) => value.run(),
        }
    }
}

fn result_line(value: serde_json::Value) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{value}")
        .and_then(|_| stdout.flush())
        .context("Could not write task-host result")
}
fn response_shape(value: &serde_json::Value, mode: &str) -> Result<()> {
    let object = value.as_object().context("Invalid task-host response")?;
    ensure!(
        object.get("ok").and_then(|value| value.as_bool()) == Some(true),
        "Task-host request was refused"
    );
    match mode {
        "trigger" => ensure!(
            object.len() == 3
                && object.get("state").and_then(|value| value.as_str()) == Some("accepted")
                && object
                    .get("job")
                    .and_then(|value| value.as_str())
                    .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()),
            "Invalid task-host admission response"
        ),
        "stop" => ensure!(
            object.len() == 2
                && object.get("state").and_then(|value| value.as_str()) == Some("stopping"),
            "Invalid task-host stop response"
        ),
        "status" => {
            ensure!(
                object.len() == 4
                    && object
                        .get("stopping")
                        .is_some_and(|value| value.is_boolean())
                    && object.get("busy").is_some_and(|value| value.is_boolean()),
                "Invalid task-host status response"
            );
            if let Some(last) = object.get("last").filter(|value| !value.is_null()) {
                let last = last.as_object().context("Invalid task-host job status")?;
                ensure!(
                    last.len() == 5
                        && last
                            .get("job")
                            .and_then(|value| value.as_str())
                            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
                        && last
                            .get("profile")
                            .and_then(|value| value.as_str())
                            .is_some_and(valid_id)
                        && last
                            .get("state")
                            .and_then(|value| value.as_str())
                            .is_some_and(|state| matches!(
                                state,
                                "running" | "completed" | "failed" | "canceled" | "timed_out"
                            ))
                        && last.get("success").is_some_and(|value| value.is_boolean())
                        && last.get("exit_code").is_some_and(|value| value.is_null()
                            || value
                                .as_i64()
                                .is_some_and(|code| i32::try_from(code).is_ok())),
                    "Invalid task-host job status"
                );
            } else {
                ensure!(object.contains_key("last"), "Invalid task-host job status");
            }
        }
        _ => bail!("Invalid task-host command"),
    }
    Ok(())
}
#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyCheck {
    ok: bool,
    job: Option<String>,
    state: Option<String>,
    stopping: Option<bool>,
    busy: Option<bool>,
    last: Option<LastCheck>,
    error: Option<String>,
}
#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LastCheck {
    job: String,
    profile: String,
    state: String,
    success: bool,
    exit_code: Option<i32>,
}
pub(crate) fn run(args: &[OsString]) -> Result<()> {
    const USAGE: &str = "Usage: screenshots task-host validate CONFIG; task-host serve CONFIG ENDPOINT; task-host trigger ENDPOINT PROFILE_ID; task-host status|stop ENDPOINT";
    let mode = args.first().and_then(|arg| arg.to_str()).context(USAGE)?;
    match mode {
        "validate" if args.len() == 2 => {
            let config = ReadyConfig::read(Path::new(&args[1]))?;
            result_line(
                serde_json::json!({"valid":true,"profiles":config.bindings.len(),"hotkeys":config.keys.len()}),
            )
        }
        "serve" if args.len() == 3 => {
            serve(ReadyConfig::read(Path::new(&args[1]))?, Path::new(&args[2]))
        }
        "trigger" | "status" | "stop"
            if (mode == "trigger" && args.len() == 3) || (mode != "trigger" && args.len() == 2) =>
        {
            let endpoint = endpoint::read(Path::new(&args[1]))?;
            endpoint.validate()?;
            let command = match mode {
                "trigger" => Command::Trigger {
                    profile: {
                        let id = args[2]
                            .to_str()
                            .context("Task-host profile ID must be UTF-8")?;
                        ensure!(valid_id(id), "Invalid task-host profile ID");
                        id.into()
                    },
                },
                "status" => Command::Status,
                _ => Command::Stop,
            };
            let mut stream = TcpStream::connect_timeout(
                &SocketAddrV4::new(Ipv4Addr::LOCALHOST, endpoint.port).into(),
                TIMEOUT,
            )
            .context("Task host is unavailable")?;
            let mut io = ConnectionIo {
                stream: &mut stream,
                deadline: Instant::now() + TIMEOUT,
            };
            frame_write(
                &mut io,
                &Request {
                    version: 1,
                    token: endpoint.token.clone(),
                    command,
                },
            )?;
            let bytes = frame_read(&mut io)?;
            let _: ReplyCheck = json_object(&bytes)?;
            let response: serde_json::Value = json_object(&bytes)?;
            response_shape(&response, mode)?;
            result_line(response)
        }
        _ => bail!(USAGE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    #[test]
    fn framed_requests_authenticate_without_accepting_arrays_duplicates_or_truncation() {
        let token = "a".repeat(64);
        assert!(authenticated(&token, &token));
        assert!(!authenticated(&token, &"b".repeat(64)));
        assert!(!authenticated(&token, "a"));
        let request = Request {
            version: 1,
            token: token.clone(),
            command: Command::Status,
        };
        let mut bytes = Vec::new();
        frame_write(&mut bytes, &request).unwrap();
        let value: Request = json_object(&frame_read(&mut Cursor::new(&bytes)).unwrap()).unwrap();
        assert!(authenticated(&token, &value.token));
        for bad in [
            b"[]".as_slice(),
            b"[1,\"x\",{}]",
            b"{\"version\":1,\"version\":1}",
            b"{} {}",
            b"\xef\xbb\xbf{}",
        ] {
            assert!(json_object::<Request>(bad).is_err());
        }
        for size in [0u32, 4097, u32::MAX] {
            assert!(frame_read(&mut Cursor::new(size.to_be_bytes())).is_err());
        }
        assert!(
            json_object::<ReplyCheck>(br#"{"ok":true,"state":null,"state":"stopping"}"#).is_err()
        );
        assert!(frame_read(&mut Cursor::new(&bytes[..bytes.len() - 1])).is_err());
        assert!(
            response_shape(
                &serde_json::json!({"ok":true,"state":"accepted","job":"invalid"}),
                "trigger"
            )
            .is_err()
        );
        assert!(
            response_shape(
                &serde_json::json!({"ok":true,"state":"stopping","unexpected":"DO_NOT_ECHO"}),
                "stop"
            )
            .is_err()
        );
    }
    #[test]
    fn config_resolves_profiles_and_rejects_duplicates_and_unsafe_schema() {
        let root = std::env::temp_dir().join(format!(
            "screenshots-task-host-config-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("source.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([7, 8, 9, 255]))
            .save(&source)
            .unwrap();
        std::fs::write(root.join("task.json"),br#"{"version":1,"source":{"type":"file","path":"source.png"},"actions":[{"type":"save","path":"out-{uuid}.png"}]}"#).unwrap();
        let config = root.join("host.json");
        let valid = serde_json::json!({"version":1,"bindings":[{"id":"capture","task":"task.json","hotkey":"ctrl+shift+f9"}]});
        std::fs::write(&config, serde_json::to_vec(&valid).unwrap()).unwrap();
        let ready = ReadyConfig::read(&config).unwrap();
        assert_eq!(ready.bindings[0].task, root.join("task.json"));
        assert_eq!(ready.bindings[0].hotkey, "Ctrl+Shift+F9");
        let mut duplicate = valid.clone();
        duplicate["bindings"]
            .as_array_mut()
            .unwrap()
            .push(valid["bindings"][0].clone());
        std::fs::write(&config, serde_json::to_vec(&duplicate).unwrap()).unwrap();
        assert!(ReadyConfig::read(&config).is_err());
        for invalid in [
            serde_json::json!({"version":2,"bindings":[]}),
            serde_json::json!({"version":1,"task_timeout_ms":99,"bindings":[{"id":"capture","task":"task.json"}]}),
            serde_json::json!({"version":1,"bindings":[{"id":"a/b","task":"task.json"}]}),
            serde_json::json!({"version":1,"bindings":[["capture","task.json","Ctrl+F9"]]}),
        ] {
            std::fs::write(&config, serde_json::to_vec(&invalid).unwrap()).unwrap();
            assert!(ReadyConfig::read(&config).is_err());
        }
        assert!(!root.join("out.png").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn private_endpoint_refuses_permission_changes_and_preserves_replaced_path() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let root = std::env::temp_dir().join(format!(
            "screenshots-task-host-endpoint-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("endpoint.json");
        let token = "a".repeat(64);
        let lease = endpoint::Lease::create(
            &path,
            &EndpointInfo {
                version: 1,
                port: 42,
                pid: std::process::id(),
                token: token.clone(),
            },
        )
        .unwrap();
        assert_eq!(path.metadata().unwrap().mode() & 0o777, 0o600);
        assert_eq!(endpoint::read(&path).unwrap().token, token);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(endpoint::read(&path).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::rename(&path, root.join("moved.json")).unwrap();
        std::fs::write(&path, b"replacement").unwrap();
        drop(lease);
        assert_eq!(std::fs::read(&path).unwrap(), b"replacement");
        assert!(root.join("moved.json").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
