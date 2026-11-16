//! One operation-owned native process; cancellation never forgets a live owner.
use anyhow::{Context, Result, ensure};
use app_core::{CaptureError, NativeCaptureCall, NativeCaptureCheck, NativeCaptureReply};
use std::{
    ffi::OsString,
    process::{Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
#[cfg(target_os = "linux")]
#[path = "capture_portal_linux.rs"]
mod portal;
#[path = "capture_worker_protocol.rs"]
mod protocol;
#[path = "capture_worker_transport.rs"]
mod transport;
#[cfg(unix)]
#[path = "capture_worker_unix.rs"]
mod unix;
#[cfg(target_os = "windows")]
#[path = "capture_worker_windows.rs"]
mod windows;

const CHILD_ARGUMENT: &str = "__native-capture-worker";
const CLEANUP_GRACE: Duration = Duration::from_millis(250);
static BUSY: AtomicBool = AtomicBool::new(false);
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);
static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);
static RELEASED: Condvar = Condvar::new();
static HANDOFF: AtomicBool = AtomicBool::new(false);
struct Active {
    enumeration: bool,
    cancel: Arc<AtomicBool>,
}
struct Admission;
impl Drop for Admission {
    fn drop(&mut self) {
        let mut active = lock(&ACTIVE);
        active.take();
        BUSY.store(false, Ordering::Release);
        RELEASED.notify_all();
    }
}
struct Handoff;
impl Drop for Handoff {
    fn drop(&mut self) {
        HANDOFF.store(false, Ordering::Release);
    }
}
fn admit(
    call: &NativeCaptureCall,
    cancel: Arc<AtomicBool>,
    check: &NativeCaptureCheck<'_>,
    deadline: Instant,
) -> Result<Admission, CaptureError> {
    refresh_pending()?;
    let mut active = lock(&ACTIVE);
    if !BUSY.load(Ordering::Acquire) {
        if HANDOFF.load(Ordering::Acquire) {
            return Err(CaptureError::NativeBusy);
        }
        *active = Some(Active {
            enumeration: call.is_enumeration(),
            cancel,
        });
        BUSY.store(true, Ordering::Release);
        return Ok(Admission);
    }
    if call.is_enumeration()
        || active.as_ref().is_none_or(|active| !active.enumeration)
        || HANDOFF
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
    {
        return Err(CaptureError::NativeBusy);
    }
    let _handoff = Handoff;
    active
        .as_ref()
        .unwrap()
        .cancel
        .store(true, Ordering::Release);
    drop(active);
    let until = (Instant::now() + Duration::from_millis(500)).min(deadline);
    loop {
        check()?;
        refresh_pending()?;
        if Instant::now() >= until {
            return Err(CaptureError::NativeBusy);
        }
        let mut active = lock(&ACTIVE);
        if !BUSY.load(Ordering::Acquire) {
            *active = Some(Active {
                enumeration: false,
                cancel,
            });
            BUSY.store(true, Ordering::Release);
            return Ok(Admission);
        }
        let (guard, _) = RELEASED
            .wait_timeout(
                active,
                until
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(20)),
            )
            .unwrap_or_else(|poison| poison.into_inner());
        drop(guard);
    }
}
type SharedJob = Arc<Mutex<Option<Job>>>;
struct Pending {
    thread: JoinHandle<()>,
    job: SharedJob,
    _admission: Admission,
}
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

struct Job {
    #[cfg(unix)]
    child: std::process::Child,
    #[cfg(unix)]
    group: i32,
    #[cfg(unix)]
    completion: Option<std::os::fd::OwnedFd>,
    #[cfg(unix)]
    live_members: Option<Vec<i32>>,
    #[cfg(target_os = "windows")]
    child: windows::Process,
    #[cfg(target_os = "windows")]
    container: windows::Container,
    status: Option<bool>,
}
impl Job {
    fn wait_ready(&self, duration: Duration) {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            if let Some(completion) = &self.completion {
                let mut event = libc::pollfd {
                    fd: completion.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                unsafe {
                    libc::poll(
                        &mut event,
                        1,
                        duration.as_millis().min(i32::MAX as u128) as i32,
                    );
                }
                return;
            }
            // Only older kernels without pidfd support use a finite active fallback.
            std::thread::sleep(duration.min(Duration::from_millis(1)));
        }
        #[cfg(target_os = "macos")]
        {
            use std::os::fd::AsRawFd;
            if let Some(completion) = &self.completion {
                let timeout = libc::timespec {
                    tv_sec: duration.as_secs() as _,
                    tv_nsec: duration.subsec_nanos() as _,
                };
                let mut event: libc::kevent = unsafe { std::mem::zeroed() };
                unsafe {
                    libc::kevent(
                        completion.as_raw_fd(),
                        std::ptr::null(),
                        0,
                        &mut event,
                        1,
                        &timeout,
                    );
                }
                return;
            }
            std::thread::sleep(duration.min(Duration::from_millis(1)));
        }
        #[cfg(target_os = "windows")]
        self.child.wait_ready(duration);
    }
    fn stop(&mut self) {
        #[cfg(unix)]
        {
            // This leader remains waitable (possibly a zombie) until every group signal is done.
            if self.status.is_none() {
                unix::signal_owned_group(self.group);
            }
        }
        #[cfg(target_os = "windows")]
        {
            self.container.stop();
            self.child.kill();
        }
    }
    fn reaped(&mut self) -> bool {
        if self.status.is_none() {
            #[cfg(unix)]
            {
                if !matches!(unix::exited_without_reaping(self.group, false), Ok(true)) {
                    return false;
                }
                unix::signal_owned_group(self.group);
                match unix::live_members(self.group) {
                    Ok(members) if members.is_empty() => {
                        self.live_members = Some(members);
                        if let Ok(Some(status)) = self.child.try_wait() {
                            self.status = Some(status.success());
                        }
                    }
                    Ok(members) => {
                        self.live_members = Some(members);
                        return false;
                    }
                    Err(_) => {
                        self.live_members = None;
                        return false;
                    }
                }
            }
            #[cfg(target_os = "windows")]
            {
                if let Ok(Some(status)) = self.child.try_wait() {
                    self.status = Some(status);
                }
            }
        }
        self.status.is_some()
    }
    fn family_done(&mut self) -> bool {
        if !self.reaped() {
            return false;
        }
        #[cfg(unix)]
        {
            true
        }
        #[cfg(target_os = "windows")]
        {
            self.container.empty().unwrap_or(false)
        }
    }
    fn wait(&mut self) {
        #[cfg(unix)]
        {
            if self.status.is_some() {
                return;
            }
            self.stop();
            if !matches!(unix::exited_without_reaping(self.group, true), Ok(true)) {
                return;
            }
            loop {
                if self.family_done() {
                    return;
                }
                let Some(members) = self
                    .live_members
                    .take()
                    .filter(|members| !members.is_empty())
                else {
                    return;
                };
                if !unix::wait_members(self.group, &members) {
                    return;
                }
            }
        }
        #[cfg(target_os = "windows")]
        {
            if let Ok(status) = self.child.wait() {
                self.status = Some(status);
            }
        }
    }
}
fn refresh_pending() -> Result<(), CaptureError> {
    let mut pending = lock(&PENDING);
    let Some(value) = pending.as_ref() else {
        return Ok(());
    };
    if !value.thread.is_finished() {
        return Err(CaptureError::NativeCleanupPending);
    }
    let complete = {
        let mut job = lock(&value.job);
        if let Some(job) = job.as_mut() {
            job.stop();
            job.family_done()
        } else {
            true
        }
    };
    if !complete {
        return Err(CaptureError::NativeCleanupPending);
    }
    let value = pending.take().unwrap();
    drop(pending);
    let _ = value.thread.join();
    lock(&value.job).take();
    drop(value._admission);
    Ok(())
}
fn command() -> Result<Command> {
    #[cfg(target_os = "linux")]
    let executable = std::path::PathBuf::from("/proc/self/exe");
    #[cfg(not(target_os = "linux"))]
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .arg(CHILD_ARGUMENT)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.env_remove(crate::task_clipboard::ENV);
    Ok(command)
}
fn private_environment(command: &mut Command) {
    let overrides = command
        .get_envs()
        .map(|(key, value)| (key.to_owned(), value.map(std::ffi::OsStr::to_owned)))
        .collect::<Vec<_>>();
    let allowed = |key: &std::ffi::OsStr| {
        let key = key.to_string_lossy().to_uppercase();
        key.starts_with("LC_")
            || matches!(
                key.as_str(),
                "PATH"
                    | "PATHEXT"
                    | "HOME"
                    | "USER"
                    | "LOGNAME"
                    | "LANG"
                    | "TZ"
                    | "TMPDIR"
                    | "TMP"
                    | "TEMP"
                    | "DISPLAY"
                    | "XAUTHORITY"
                    | "WAYLAND_DISPLAY"
                    | "DBUS_SESSION_BUS_ADDRESS"
                    | "XDG_RUNTIME_DIR"
                    | "XDG_SESSION_TYPE"
                    | "XDG_SESSION_DESKTOP"
                    | "XDG_CURRENT_DESKTOP"
                    | "XDG_CONFIG_HOME"
                    | "XDG_CONFIG_DIRS"
                    | "XDG_DATA_HOME"
                    | "XDG_DATA_DIRS"
                    | "XDG_CACHE_HOME"
                    | "LD_LIBRARY_PATH"
                    | "DYLD_LIBRARY_PATH"
                    | "DYLD_FRAMEWORK_PATH"
                    | "PIPEWIRE_REMOTE"
                    | "PIPEWIRE_RUNTIME_DIR"
                    | "SPA_PLUGIN_DIR"
                    | "PIPEWIRE_MODULE_DIR"
                    | "MESA_SHADER_CACHE_DISABLE"
                    | "LIBGL_ALWAYS_SOFTWARE"
                    | "__CF_USER_TEXT_ENCODING"
                    | "SYSTEMROOT"
                    | "WINDIR"
                    | "SYSTEMDRIVE"
                    | "PROGRAMFILES"
                    | "PROGRAMFILES(X86)"
                    | "PROGRAMW6432"
                    | "USERPROFILE"
                    | "APPDATA"
                    | "LOCALAPPDATA"
                    | "HOMEDRIVE"
                    | "HOMEPATH"
                    | "NUMBER_OF_PROCESSORS"
                    | "PROCESSOR_ARCHITECTURE"
                    | "PROCESSOR_ARCHITEW6432"
                    | "OS"
            )
    };
    command.env_clear();
    for (key, value) in std::env::vars_os().filter(|(key, _)| allowed(key)) {
        command.env(key, value);
    }
    for (key, value) in overrides {
        #[cfg(test)]
        let test_metadata = key.to_string_lossy().starts_with("SCREENSHOTS_");
        #[cfg(not(test))]
        let test_metadata = false;
        if let Some(value) = value {
            if allowed(&key) || test_metadata {
                command.env(key, value);
            }
        } else {
            command.env_remove(key);
        }
    }
    command.env_remove(crate::task_clipboard::ENV);
}
fn launch(command: &mut Command, prepared: &transport::Prepared) -> Result<Job> {
    #[cfg(unix)]
    {
        let child = command.spawn()?;
        let group = i32::try_from(child.id()).context("Invalid native process identity")?;
        let completion = completion_event(group);
        let _ = prepared;
        Ok(Job {
            child,
            group,
            completion,
            live_members: None,
            status: None,
        })
    }
    #[cfg(target_os = "windows")]
    {
        let container = windows::container()?;
        let child = windows::spawn(command, &container, &prepared.child_handles())?;
        Ok(Job {
            child,
            container,
            status: None,
        })
    }
}
#[cfg(unix)]
fn completion_event(pid: i32) -> Option<std::os::fd::OwnedFd> {
    use std::os::fd::{AsRawFd, FromRawFd};
    #[cfg(target_os = "linux")]
    let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) as i32 };
    #[cfg(target_os = "macos")]
    let descriptor = unsafe { libc::kqueue() };
    if descriptor < 0 {
        return None;
    }
    let owned = unsafe { std::os::fd::OwnedFd::from_raw_fd(descriptor) };
    let flags = unsafe { libc::fcntl(owned.as_raw_fd(), libc::F_GETFD) };
    if flags < 0
        || unsafe { libc::fcntl(owned.as_raw_fd(), libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0
    {
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        let event = libc::kevent {
            ident: pid as usize,
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD | libc::EV_ONESHOT,
            fflags: libc::NOTE_EXIT,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        if unsafe {
            libc::kevent(
                owned.as_raw_fd(),
                &event,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        } < 0
        {
            return None;
        }
    }
    Some(owned)
}
fn cleanup(job: &SharedJob, force: bool) -> (bool, bool) {
    let mut forced = force;
    if force && let Some(job) = lock(job).as_mut() {
        job.stop();
    }
    let until = Instant::now() + CLEANUP_GRACE;
    let natural = Instant::now() + Duration::from_millis(50);
    loop {
        let mut owned = lock(job);
        let Some(process) = owned.as_mut() else {
            return (true, !forced);
        };
        if process.family_done() {
            let success = process.status == Some(true) && !forced;
            owned.take();
            return (true, success);
        }
        if process.status.is_some() {
            // Remaining descendants stay owned/quarantined; never spin on an exited root.
            return (false, false);
        }
        if !forced && Instant::now() >= natural {
            process.stop();
            forced = true;
        }
        if Instant::now() >= until {
            break;
        }
        process.wait_ready(
            until
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(20)),
        );
        drop(owned);
    }
    (false, false)
}
fn coordinate(
    mut command: Command,
    call: NativeCaptureCall,
    deadline: Instant,
    cancel: Arc<AtomicBool>,
    owned: SharedJob,
) -> Result<NativeCaptureReply, CaptureError> {
    let error = |_: anyhow::Error| {
        CaptureError::Backend("Native capture helper failed or returned invalid data".into())
    };
    let check = || {
        if Instant::now() >= deadline {
            Err(CaptureError::NativeTimeout)
        } else if cancel.load(Ordering::Acquire) {
            Err(CaptureError::NativeCanceled)
        } else {
            Ok(())
        }
    };
    check()?;
    private_environment(&mut command);
    let prepared = transport::Prepared::new(&mut command).map_err(error)?;
    check()?;
    let job = launch(&mut command, &prepared).map_err(error)?;
    *lock(&owned) = Some(job);
    let (transport, nonce) = prepared.parent();
    let mut io = transport::ActiveIo {
        transport,
        deadline,
        check: &check,
    };
    let result = (|| -> Result<NativeCaptureReply> {
        protocol::handshake(&mut io, protocol::HELLO, &nonce)?;
        protocol::expect_handshake(&mut io, protocol::ACK, &nonce)?;
        protocol::request(&mut io, &call)?;
        protocol::read_reply(&mut io, &call)
    })();
    drop(io);
    let mut result = match result {
        Ok(reply) => Ok(reply),
        Err(failure) => Err(check().err().unwrap_or_else(|| {
            failure
                .downcast::<CaptureError>()
                .unwrap_or_else(|_| error(anyhow::anyhow!("native operation failed")))
        })),
    };
    if let Err(canceled) = check() {
        result = Err(canceled);
    }
    let (complete, clean_success) = cleanup(&owned, result.is_err());
    if !complete {
        // Drop any completed/partial result before a kernel-pending reap can block.
        result = Err(CaptureError::NativeCleanupPending);
        let mut job = lock(&owned);
        if let Some(process) = job.as_mut() {
            process.stop();
            process.wait();
            if process.family_done() {
                job.take();
            }
        }
    } else if result.is_ok() && !clean_success {
        result = Err(CaptureError::Backend(
            "Native helper did not exit cleanly".into(),
        ));
    }
    result
}

pub(crate) fn install() -> Result<()> {
    app_core::install_native_capture_executor(execute)?;
    Ok(())
}
fn execute(
    call: NativeCaptureCall,
    check: &NativeCaptureCheck<'_>,
) -> Result<NativeCaptureReply, CaptureError> {
    let timeout = operation_timeout(&call);
    execute_factory(
        || {
            command()
                .map_err(|_| CaptureError::Backend("Native worker executable unavailable".into()))
        },
        call,
        check,
        timeout,
    )
}
fn operation_timeout(call: &NativeCaptureCall) -> Duration {
    if matches!(call, NativeCaptureCall::Portal { .. }) {
        Duration::from_secs(120)
    } else if call.is_enumeration() {
        Duration::from_secs(5)
    } else {
        Duration::from_secs(20)
    }
}
#[cfg(test)]
fn execute_command(
    command: Command,
    call: NativeCaptureCall,
    check: &NativeCaptureCheck<'_>,
    timeout: Duration,
) -> Result<NativeCaptureReply, CaptureError> {
    execute_factory(move || Ok(command), call, check, timeout)
}
fn execute_factory(
    factory: impl FnOnce() -> Result<Command, CaptureError> + Send + 'static,
    call: NativeCaptureCall,
    check: &NativeCaptureCheck<'_>,
    timeout: Duration,
) -> Result<NativeCaptureReply, CaptureError> {
    check()?;
    call.validate()?;
    let deadline = Instant::now() + timeout;
    let cancel = Arc::new(AtomicBool::new(false));
    let admission = admit(&call, cancel.clone(), check, deadline)?;
    let worker_cancel = cancel.clone();
    let job: SharedJob = Arc::new(Mutex::new(None));
    let worker_job = job.clone();
    let (sender, receiver) = mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("native-capture-job".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            let result = factory()
                .and_then(|command| coordinate(command, call, deadline, worker_cancel, worker_job));
            let _ = sender.send(result);
        })
        .map_err(|_| CaptureError::Backend("Cannot start native capture coordinator".into()))?;
    let mut outcome = None;
    let mut canceled = None;
    let mut abort_until = None;
    loop {
        if canceled.is_none() {
            if let Err(error) = check() {
                canceled = Some(error);
            } else if cancel.load(Ordering::Acquire) {
                canceled = Some(CaptureError::NativeCanceled);
            } else if Instant::now() >= deadline {
                canceled = Some(CaptureError::NativeTimeout);
            }
            if canceled.is_some() {
                cancel.store(true, Ordering::Release);
                abort_until = Some(Instant::now() + CLEANUP_GRACE);
            }
        }
        if thread.is_finished()
            && let Some(result) = outcome.take()
        {
            let complete = lock(&job).as_mut().is_none_or(|job| job.family_done());
            if complete {
                let _ = thread.join();
                lock(&job).take();
                drop(admission);
                return if let Some(error) = canceled {
                    Err(error)
                } else {
                    result
                };
            }
            *lock(&PENDING) = Some(Pending {
                thread,
                job,
                _admission: admission,
            });
            return Err(CaptureError::NativeCleanupPending);
        }
        if abort_until.is_some_and(|until| Instant::now() >= until) {
            cancel.store(true, Ordering::Release);
            if let Ok(mut slot) = job.try_lock()
                && let Some(job) = slot.as_mut()
            {
                job.stop();
            }
            *lock(&PENDING) = Some(Pending {
                thread,
                job,
                _admission: admission,
            });
            return Err(CaptureError::NativeCleanupPending);
        }
        if outcome.is_some() {
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        match receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(result) => outcome = Some(result),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                cancel.store(true, Ordering::Release);
                if let Ok(mut slot) = job.try_lock()
                    && let Some(job) = slot.as_mut()
                {
                    job.stop();
                }
                outcome = Some(Err(CaptureError::Backend(
                    "Native capture coordinator failed".into(),
                )));
            }
        }
    }
}

pub(crate) fn try_child(arguments: &[OsString]) -> Result<bool> {
    if arguments
        .first()
        .is_none_or(|value| value != CHILD_ARGUMENT)
    {
        return Ok(false);
    }
    ensure!(
        arguments.len() == 1,
        "Private native worker arguments are invalid"
    );
    serve().map_err(|_| anyhow::anyhow!("Private native capture worker failed"))?;
    Ok(true)
}
fn serve() -> Result<()> {
    let (transport, nonce, parent) = transport::Transport::inherited()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut io = transport::ActiveIo {
        transport,
        deadline,
        check: &|| Ok(()),
    };
    protocol::expect_handshake(&mut io, protocol::HELLO, &nonce)?;
    protocol::handshake(&mut io, protocol::ACK, &nonce)?;
    let call = protocol::read_request(&mut io)?;
    io.deadline = Instant::now() + operation_timeout(&call);
    #[cfg(target_os = "macos")]
    let lifeline = transport::Lifeline::new(&io.transport)?;
    let result = match call {
        #[cfg(target_os = "linux")]
        NativeCaptureCall::Portal { source, cursor } => {
            portal::capture(source, cursor, io.deadline)
                .map(|frame| NativeCaptureReply::Frames(vec![frame]))
        }
        call => call.run_inline(parent),
    };
    match result {
        Ok(reply) => protocol::reply(&mut io, reply)?,
        Err(error) => protocol::failure(&mut io, &error)?,
    }
    #[cfg(target_os = "macos")]
    drop(lifeline);
    drop(io);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    const MODE: &str = "SCREENSHOTS_CAPTURE_WORKER_FIXTURE";
    const MARKER: &str = "SCREENSHOTS_CAPTURE_WORKER_MARKER";
    static SERIAL: Mutex<()> = Mutex::new(());
    struct Folder(PathBuf);
    impl Folder {
        fn new() -> Self {
            let value = Self(
                std::env::temp_dir()
                    .join(format!("screenshots-native-job-{}", uuid::Uuid::new_v4())),
            );
            std::fs::create_dir(&value.0).unwrap();
            value
        }
    }
    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn call() -> NativeCaptureCall {
        NativeCaptureCall::Batch {
            requests: vec![app_core::CaptureRequest::Region {
                display_id: "fixture".into(),
                x: 0,
                y: 0,
                width: 2,
                height: 1,
            }],
            options: Default::default(),
        }
    }
    #[test]
    fn explicit_portal_consent_has_a_finite_separate_deadline_policy() {
        let call = NativeCaptureCall::Portal {
            source: app_core::PortalCaptureSource::Window,
            cursor: app_core::PortalCaptureCursor::Exclude,
        };
        assert_eq!(operation_timeout(&call), Duration::from_secs(120));
        assert_eq!(
            operation_timeout(&NativeCaptureCall::Displays),
            Duration::from_secs(5)
        );
        assert_eq!(operation_timeout(&self::call()), Duration::from_secs(20));
    }
    fn fake_command(mode: &str, marker: Option<&Path>) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "capture_worker::tests::worker_fixture",
                "--nocapture",
            ])
            .env(MODE, mode)
            .env_remove(crate::task_clipboard::ENV)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(marker) = marker {
            command.env(MARKER, marker);
        }
        command
    }
    fn frame() -> app_core::CapturedImage {
        app_core::CapturedImage {
            display_id: "fixture".into(),
            window_id: None,
            width: 2,
            height: 1,
            png_path: PathBuf::new(),
            pixels: Arc::new(
                image::RgbaImage::from_raw(2, 1, vec![7, 11, 13, 0, 17, 19, 23, 255]).unwrap(),
            ),
            elapsed_ms: 1,
            source_format: app_core::PixelFormat::Rgba8UnormSrgb,
            display_geometry: None,
        }
    }
    #[test]
    fn native_worker_environment_drops_unneeded_credentials_and_clipboard_capabilities() {
        let mut command = fake_command("success", None);
        command
            .env("AWS_SECRET_ACCESS_KEY", "generated-test-secret")
            .env(crate::task_clipboard::ENV, "generated-stale-descriptor");
        private_environment(&mut command);
        assert!(
            !command
                .get_envs()
                .any(|(key, value)| key == "AWS_SECRET_ACCESS_KEY" && value.is_some())
        );
        assert!(
            !command
                .get_envs()
                .any(|(key, value)| key == crate::task_clipboard::ENV && value.is_some())
        );
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == MODE && value == Some(std::ffi::OsStr::new("success")))
        );
    }
    #[test]
    fn worker_fixture() -> Result<()> {
        let Ok(mode) = std::env::var(MODE) else {
            return Ok(());
        };
        let (transport, nonce, _) = transport::Transport::inherited()?;
        let mut io = transport::ActiveIo {
            transport,
            deadline: Instant::now() + Duration::from_secs(10),
            check: &|| Ok(()),
        };
        protocol::expect_handshake(&mut io, protocol::HELLO, &nonce)?;
        protocol::handshake(&mut io, protocol::ACK, &nonce)?;
        let _call = protocol::read_request(&mut io)?;
        #[cfg(target_os = "macos")]
        let _lifeline = transport::Lifeline::new(&io.transport)?;
        if let Some(marker) = std::env::var_os(MARKER) {
            std::fs::write(marker, std::process::id().to_string())?;
        }
        match mode.as_str() {
            "hang" => loop {
                std::thread::park();
            },
            #[cfg(unix)]
            "descendant" | "exit_descendant" => {
                let descendant = unsafe { libc::fork() };
                ensure!(descendant >= 0, "Cannot create owned descendant");
                if descendant == 0 {
                    // The generated descendant needs no IO, including duplicated lifeline ends.
                    for fd in 3..4096 {
                        unsafe {
                            libc::close(fd);
                        }
                    }
                    loop {
                        unsafe {
                            libc::pause();
                        }
                    }
                }
                if let Some(marker) = std::env::var_os(MARKER) {
                    std::fs::write(
                        Path::new(&marker).with_extension("descendant"),
                        descendant.to_string(),
                    )?;
                }
                if mode == "exit_descendant" {
                    protocol::reply(&mut io, NativeCaptureReply::Frames(vec![frame()]))?;
                } else {
                    loop {
                        std::thread::park();
                    }
                }
            }
            "malformed" => {
                use std::io::Write;
                io.write_all(b"not-a-native-frame")?;
            }
            "truncated" => {
                use std::io::Write;
                let mut bytes = Vec::new();
                protocol::reply(&mut bytes, NativeCaptureReply::Frames(vec![frame()]))?;
                io.write_all(&bytes[..bytes.len() - 1])?;
            }
            "slow" => {
                std::thread::sleep(Duration::from_millis(250));
                protocol::reply(&mut io, NativeCaptureReply::Frames(vec![frame()]))?;
            }
            "success" => protocol::reply(&mut io, NativeCaptureReply::Frames(vec![frame()]))?,
            "inventory" => protocol::reply(
                &mut io,
                NativeCaptureReply::Inventory(app_core::NativeSourceInventory {
                    displays: Ok(vec![app_core::DisplayInfo {
                        id: "fixture".into(),
                        name: "private fixture".into(),
                        x: 0,
                        y: 0,
                        width: 2,
                        height: 1,
                        scale_factor: 1.0,
                        rotation: 0.0,
                        is_primary: true,
                    }]),
                    windows: Err(CaptureError::Unsupported("fixture window listing")),
                }),
            )?,
            "descriptor" => {
                let mut probe = Command::new(std::env::current_exe()?);
                probe
                    .args([
                        "--exact",
                        "capture_worker::tests::descriptor_probe",
                        "--nocapture",
                    ])
                    .env("SCREENSHOTS_NATIVE_DESCRIPTOR_PROBE", "1")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                #[cfg(unix)]
                {
                    use std::os::fd::AsRawFd;
                    let mut info: libc::stat = unsafe { std::mem::zeroed() };
                    ensure!(
                        unsafe { libc::fstat(io.transport.stream.as_raw_fd(), &mut info) } == 0,
                        "Cannot inspect owned channel"
                    );
                    probe.env(
                        "SCREENSHOTS_NATIVE_DESCRIPTOR_INODE",
                        info.st_ino.to_string(),
                    );
                }
                ensure!(
                    probe.status()?.success(),
                    "Grandchild inherited private native channel"
                );
                protocol::reply(&mut io, NativeCaptureReply::Frames(vec![frame()]))?;
            }
            _ => anyhow::bail!("Unknown owned fixture mode"),
        }
        Ok(())
    }
    fn settle() {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            match refresh_pending() {
                Ok(()) => break,
                Err(_) => {
                    assert!(Instant::now() < until, "Owned cleanup did not finish");
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }
        assert!(!BUSY.load(Ordering::Acquire));
    }
    #[test]
    fn descriptor_probe() -> Result<()> {
        if std::env::var_os("SCREENSHOTS_NATIVE_DESCRIPTOR_PROBE").is_none() {
            return Ok(());
        }
        let value = std::env::var(transport::ENV)?;
        let fields = value.split(':').collect::<Vec<_>>();
        #[cfg(unix)]
        {
            let fd: i32 = fields[1].parse()?;
            let expected: u64 = std::env::var("SCREENSHOTS_NATIVE_DESCRIPTOR_INODE")?.parse()?;
            let mut info: libc::stat = unsafe { std::mem::zeroed() };
            ensure!(
                unsafe { libc::fstat(fd, &mut info) } != 0 || info.st_ino as u64 != expected,
                "Private native socket survived exec"
            );
        }
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::{
                Storage::FileSystem::*, System::Pipes::GetNamedPipeServerProcessId,
            };
            let expected: u32 = fields[3].parse()?;
            for field in [fields[1], fields[2]] {
                let handle = field.parse::<usize>()? as windows_sys::Win32::Foundation::HANDLE;
                if unsafe { GetFileType(handle) } == FILE_TYPE_PIPE {
                    let mut owner = 0;
                    ensure!(
                        unsafe { GetNamedPipeServerProcessId(handle, &mut owner) } == 0
                            || owner != expected,
                        "Private native pipe survived exec"
                    );
                }
            }
        }
        Ok(())
    }
    #[test]
    fn owned_process_roundtrip_keeps_exact_pixels_and_releases_admission() {
        let _serial = lock(&SERIAL);
        settle();
        for _ in 0..3 {
            let reply = execute_command(
                fake_command("success", None),
                call(),
                &|| Ok(()),
                Duration::from_secs(5),
            )
            .unwrap();
            let NativeCaptureReply::Frames(frames) = reply else {
                panic!("not frames");
            };
            assert_eq!(frames[0].pixels.as_raw(), frame().pixels.as_raw());
            settle();
        }
    }
    #[test]
    fn malformed_or_truncated_owned_worker_never_returns_an_image_group() {
        let _serial = lock(&SERIAL);
        settle();
        for mode in ["malformed", "truncated"] {
            assert!(
                execute_command(
                    fake_command(mode, None),
                    call(),
                    &|| Ok(()),
                    Duration::from_secs(5)
                )
                .is_err()
            );
            settle();
        }
    }
    #[test]
    fn inherited_native_channel_is_closed_before_any_grandchild_exec() {
        let _serial = lock(&SERIAL);
        settle();
        assert!(
            execute_command(
                fake_command("descriptor", None),
                call(),
                &|| Ok(()),
                Duration::from_secs(5)
            )
            .is_ok()
        );
        settle();
    }
    #[cfg(unix)]
    #[test]
    fn cancellation_ends_the_owned_process_group_including_its_descendant() {
        let _serial = lock(&SERIAL);
        settle();
        let folder = Folder::new();
        let marker = folder.0.join("pid");
        let result = execute_command(
            fake_command("descendant", Some(&marker)),
            call(),
            &|| Ok(()),
            Duration::from_secs(1),
        );
        assert!(result.is_err());
        let descendant: i32 = std::fs::read_to_string(marker.with_extension("descendant"))
            .unwrap()
            .parse()
            .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while process_live(descendant) {
            assert!(
                Instant::now() < until,
                "Owned descendant escaped cancellation"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        settle();
    }
    #[cfg(unix)]
    #[test]
    fn normal_root_exit_kills_its_live_descendant_before_reaping_and_releases_admission() {
        let _serial = lock(&SERIAL);
        settle();
        let folder = Folder::new();
        let marker = folder.0.join("root");
        let result = execute_command(
            fake_command("exit_descendant", Some(&marker)),
            call(),
            &|| Ok(()),
            Duration::from_secs(5),
        );
        let descendant: i32 = std::fs::read_to_string(marker.with_extension("descendant"))
            .unwrap()
            .parse()
            .unwrap();
        let alive = process_live(descendant);
        if alive {
            unsafe {
                libc::kill(descendant, libc::SIGKILL);
            }
        }
        assert!(!alive, "A normal root exit left its descendant running");
        assert!(result.is_ok());
        settle();
        assert!(
            execute_command(
                fake_command("success", None),
                call(),
                &|| Ok(()),
                Duration::from_secs(5)
            )
            .is_ok()
        );
        settle();
    }
    #[cfg(unix)]
    #[test]
    fn reaped_job_never_signals_a_reused_numeric_group_identifier() {
        let _serial = lock(&SERIAL);
        settle();
        let folder = Folder::new();
        let marker = folder.0.join("victim");
        let saved = marker.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let canceled = stop.clone();
        let victim = std::thread::spawn(move || {
            execute_command(
                fake_command("hang", Some(&saved)),
                call(),
                &|| {
                    if canceled.load(Ordering::Acquire) {
                        Err(CaptureError::NativeCanceled)
                    } else {
                        Ok(())
                    }
                },
                Duration::from_secs(5),
            )
        });
        let until = Instant::now() + Duration::from_secs(5);
        let pid: i32 = loop {
            if let Ok(pid) = std::fs::read_to_string(&marker).unwrap_or_default().parse() {
                break pid;
            }
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        };
        let mut already_exited = Command::new(std::env::current_exe().unwrap())
            .arg("--list")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        assert!(already_exited.wait().unwrap().success());
        let mut retired = Job {
            child: already_exited,
            group: pid,
            completion: None,
            live_members: None,
            status: Some(true),
        };
        retired.stop();
        let alive = process_live(pid);
        stop.store(true, Ordering::Release);
        assert!(
            alive,
            "A retired job signaled another live group's numeric identifier"
        );
        assert!(victim.join().unwrap().is_err());
        settle();
    }
    #[test]
    fn stalled_owned_process_times_out_and_cancellation_terminates_it_without_replacement() {
        let _serial = lock(&SERIAL);
        settle();
        let folder = Folder::new();
        let marker = folder.0.join("pid");
        let started = Instant::now();
        let result = execute_command(
            fake_command("hang", Some(&marker)),
            call(),
            &|| Ok(()),
            Duration::from_secs(1),
        );
        assert!(matches!(
            result,
            Err(CaptureError::NativeTimeout | CaptureError::NativeCleanupPending)
        ));
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(marker.exists());
        settle();
        let started = Instant::now();
        let result = execute_command(
            fake_command("hang", Some(&marker)),
            call(),
            &|| {
                if started.elapsed() >= Duration::from_millis(250) {
                    Err(CaptureError::NativeCanceled)
                } else {
                    Ok(())
                }
            },
            Duration::from_secs(5),
        );
        assert!(matches!(
            result,
            Err(CaptureError::NativeCanceled | CaptureError::NativeCleanupPending)
        ));
        assert!(started.elapsed() < Duration::from_secs(3));
        settle();
    }
    #[test]
    fn active_native_job_refuses_concurrent_admission_without_a_backlog() {
        let _serial = lock(&SERIAL);
        settle();
        let folder = Folder::new();
        let marker = folder.0.join("ready");
        let worker_marker = marker.clone();
        let running = std::thread::spawn(move || {
            execute_command(
                fake_command("slow", Some(&worker_marker)),
                call(),
                &|| Ok(()),
                Duration::from_secs(5),
            )
        });
        let until = Instant::now() + Duration::from_secs(5);
        while !marker.exists() {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(matches!(
            execute_command(
                fake_command("success", None),
                call(),
                &|| Ok(()),
                Duration::from_secs(5)
            ),
            Err(CaptureError::NativeBusy)
        ));
        assert!(running.join().unwrap().is_ok());
        settle();
    }
    #[test]
    fn coalesced_inventory_keeps_independent_success_and_failure_results() {
        let _serial = lock(&SERIAL);
        settle();
        let reply = execute_command(
            fake_command("inventory", None),
            NativeCaptureCall::Inventory,
            &|| Ok(()),
            Duration::from_secs(5),
        )
        .unwrap();
        let NativeCaptureReply::Inventory(inventory) = reply else {
            panic!("not inventory");
        };
        assert_eq!(inventory.displays.unwrap()[0].id, "fixture");
        assert!(matches!(
            inventory.windows,
            Err(CaptureError::Unsupported(_))
        ));
        settle();
    }
    #[test]
    fn one_capture_contender_cancels_enumeration_without_queuing_a_second_capture() {
        let _serial = lock(&SERIAL);
        settle();
        let folder = Folder::new();
        let enumeration_marker = folder.0.join("enum");
        let em = enumeration_marker.clone();
        let enumeration = std::thread::spawn(move || {
            execute_command(
                fake_command("hang", Some(&em)),
                NativeCaptureCall::Displays,
                &|| Ok(()),
                Duration::from_secs(5),
            )
        });
        let until = Instant::now() + Duration::from_secs(5);
        while !enumeration_marker.exists() {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(2));
        }
        let capture_marker = folder.0.join("capture");
        let cm = capture_marker.clone();
        let capture = std::thread::spawn(move || {
            execute_command(
                fake_command("slow", Some(&cm)),
                call(),
                &|| Ok(()),
                Duration::from_secs(5),
            )
        });
        while !HANDOFF.load(Ordering::Acquire) && !capture_marker.exists() {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(matches!(
            execute_command(
                fake_command("success", None),
                call(),
                &|| Ok(()),
                Duration::from_secs(5)
            ),
            Err(CaptureError::NativeBusy)
        ));
        assert!(matches!(
            enumeration.join().unwrap(),
            Err(CaptureError::NativeCanceled)
        ));
        assert!(capture.join().unwrap().is_ok());
        settle();
    }
    #[test]
    fn released_admission_remains_reserved_until_the_handoff_owner_claims_it() {
        let _serial = lock(&SERIAL);
        settle();
        let enumeration_cancel = Arc::new(AtomicBool::new(false));
        let enumeration = admit(
            &NativeCaptureCall::Displays,
            enumeration_cancel.clone(),
            &|| Ok(()),
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        let (paused, at_release) = mpsc::sync_channel(1);
        let (resume, resumed) = mpsc::sync_channel(1);
        let (claimed, owner_claimed) = mpsc::sync_channel(1);
        let (finish, finishing) = mpsc::sync_channel(1);
        let owner = std::thread::spawn(move || {
            let once = AtomicBool::new(false);
            let admission = admit(
                &call(),
                Arc::new(AtomicBool::new(false)),
                &|| {
                    if !BUSY.load(Ordering::Acquire)
                        && HANDOFF.load(Ordering::Acquire)
                        && !once.swap(true, Ordering::AcqRel)
                    {
                        paused.send(()).unwrap();
                        resumed.recv_timeout(Duration::from_secs(5)).unwrap();
                    }
                    Ok(())
                },
                Instant::now() + Duration::from_secs(5),
            )
            .unwrap();
            claimed.send(()).unwrap();
            finishing.recv_timeout(Duration::from_secs(5)).unwrap();
            drop(admission);
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !HANDOFF.load(Ordering::Acquire) {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(enumeration_cancel.load(Ordering::Acquire));
        drop(enumeration);
        at_release.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!BUSY.load(Ordering::Acquire) && HANDOFF.load(Ordering::Acquire));
        for contender in [NativeCaptureCall::Displays, call()] {
            assert!(matches!(
                admit(
                    &contender,
                    Arc::new(AtomicBool::new(false)),
                    &|| Ok(()),
                    deadline
                ),
                Err(CaptureError::NativeBusy)
            ));
        }
        assert!(!BUSY.load(Ordering::Acquire));
        resume.send(()).unwrap();
        owner_claimed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(BUSY.load(Ordering::Acquire) && !lock(&ACTIVE).as_ref().unwrap().enumeration);
        finish.send(()).unwrap();
        owner.join().unwrap();
        settle();
    }
    #[test]
    fn owned_pending_reaper_retains_admission_until_confirmed_finished() {
        let _serial = lock(&SERIAL);
        settle();
        BUSY.store(true, Ordering::Release);
        let (stop, waiting) = mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || {
            let _ = waiting.recv();
        });
        *lock(&PENDING) = Some(Pending {
            thread,
            job: Arc::new(Mutex::new(None)),
            _admission: Admission,
        });
        assert!(matches!(
            execute_command(
                fake_command("success", None),
                call(),
                &|| Ok(()),
                Duration::from_secs(5)
            ),
            Err(CaptureError::NativeCleanupPending)
        ));
        assert!(BUSY.load(Ordering::Acquire));
        stop.send(()).unwrap();
        settle();
    }
    #[test]
    fn blocked_owned_startup_returns_pending_and_never_launches_after_its_deadline() {
        let _serial = lock(&SERIAL);
        settle();
        let folder = Folder::new();
        let marker = folder.0.join("must-not-run");
        let saved = marker.clone();
        let (ready, received) = mpsc::sync_channel(1);
        let (release, blocked) = mpsc::sync_channel(1);
        let caller = std::thread::spawn(move || {
            execute_factory(
                move || {
                    ready.send(()).unwrap();
                    let _ = blocked.recv();
                    Ok(fake_command("success", Some(&saved)))
                },
                call(),
                &|| Ok(()),
                Duration::from_millis(100),
            )
        });
        received.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(
            caller.join().unwrap(),
            Err(CaptureError::NativeCleanupPending)
        ));
        assert!(BUSY.load(Ordering::Acquire));
        assert!(matches!(
            execute_command(
                fake_command("success", None),
                call(),
                &|| Ok(()),
                Duration::from_secs(5)
            ),
            Err(CaptureError::NativeCleanupPending)
        ));
        release.send(()).unwrap();
        settle();
        assert!(!marker.exists());
    }
    #[test]
    fn owner_fixture() -> Result<()> {
        if std::env::var_os("SCREENSHOTS_CAPTURE_OWNER_FIXTURE").is_none() {
            return Ok(());
        }
        let marker = std::env::var_os(MARKER).context("Missing owned PID marker")?;
        let mode =
            std::env::var("SCREENSHOTS_CAPTURE_OWNER_MODE").unwrap_or_else(|_| "hang".into());
        let _ = execute_command(
            fake_command(&mode, Some(Path::new(&marker))),
            call(),
            &|| Ok(()),
            Duration::from_secs(20),
        );
        Ok(())
    }
    #[cfg(unix)]
    fn process_live(pid: i32) -> bool {
        #[cfg(target_os = "linux")]
        {
            if let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                return stat
                    .rsplit(')')
                    .next()
                    .is_some_and(|fields| !fields.trim_start().starts_with('Z'));
            }
            false
        }
        #[cfg(target_os = "macos")]
        {
            unix::process_live(pid)
        }
    }
    #[cfg(target_os = "windows")]
    fn process_live(pid: i32) -> bool {
        use windows_sys::Win32::{Foundation::*, System::Threading::*};
        let handle = unsafe {
            OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                pid as u32,
            )
        };
        if handle.is_null() {
            return false;
        }
        let live = unsafe { WaitForSingleObject(handle, 0) } == WAIT_TIMEOUT;
        unsafe {
            CloseHandle(handle);
        }
        live
    }
    #[test]
    fn parent_death_ends_the_native_child_without_user_session_access() {
        let _serial = lock(&SERIAL);
        settle();
        let folder = Folder::new();
        let marker = folder.0.join("pid");
        let mut owner = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "capture_worker::tests::owner_fixture",
                "--nocapture",
            ])
            .env("SCREENSHOTS_CAPTURE_OWNER_FIXTURE", "1")
            .env(MARKER, &marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while !marker.exists() {
            if Instant::now() >= until {
                let _ = owner.kill();
                let _ = owner.wait();
                panic!("Owned child did not start");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let pid: i32 = loop {
            if let Ok(pid) = std::fs::read_to_string(&marker).unwrap_or_default().parse() {
                break pid;
            }
            if Instant::now() >= until {
                let _ = owner.kill();
                let _ = owner.wait();
                panic!("Owned PID marker was not completed");
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        assert!(process_live(pid));
        owner.kill().unwrap();
        owner.wait().unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while process_live(pid) {
            assert!(Instant::now() < until, "Native child escaped its parent");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[cfg(unix)]
    #[test]
    fn parent_death_terminates_the_workers_live_process_group() {
        let _serial = lock(&SERIAL);
        settle();
        let folder = Folder::new();
        let marker = folder.0.join("pid");
        let descendant_marker = marker.with_extension("descendant");
        let mut owner = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "capture_worker::tests::owner_fixture",
                "--nocapture",
            ])
            .env("SCREENSHOTS_CAPTURE_OWNER_FIXTURE", "1")
            .env("SCREENSHOTS_CAPTURE_OWNER_MODE", "descendant")
            .env(MARKER, &marker)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        let mut read_pid = |path: &Path| loop {
            if let Ok(pid) = std::fs::read_to_string(path)
                .unwrap_or_default()
                .parse::<i32>()
            {
                break pid;
            }
            if Instant::now() >= until {
                let _ = owner.kill();
                let _ = owner.wait();
                panic!("Owned process group did not start");
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        let root = read_pid(&marker);
        let descendant = read_pid(&descendant_marker);
        assert!(process_live(root) && process_live(descendant));
        owner.kill().unwrap();
        owner.wait().unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while process_live(root) || process_live(descendant) {
            assert!(Instant::now() < until, "Native family escaped its parent");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    #[ignore = "explicit optimized private-Xvfb 4K timing/RSS comparison; never use a user display"]
    fn native_worker_4k_comparison() -> Result<()> {
        use app_core::CaptureBackend;
        let _serial = lock(&SERIAL);
        settle();
        ensure!(
            !cfg!(debug_assertions),
            "This measurement requires the optimized profile"
        );
        ensure!(
            std::env::var("SCREENSHOTS_NATIVE_BENCH_PRIVATE").as_deref() == Ok("1"),
            "Private fixture opt-in is required"
        );
        let mode = std::env::var("SCREENSHOTS_NATIVE_BENCH_MODE")?;
        ensure!(
            matches!(mode.as_str(), "inline" | "worker"),
            "Invalid benchmark mode"
        );
        let display = std::env::var("SCREENSHOTS_NATIVE_BENCH_DISPLAY")?;
        let binary = std::env::var_os("SCREENSHOTS_NATIVE_BENCH_BINARY")
            .context("Missing benchmark executable")?;
        let backend = app_core::XcapBackend::new(PathBuf::new());
        let request = NativeCaptureCall::Batch {
            requests: vec![app_core::CaptureRequest::Display(display.clone())],
            options: Default::default(),
        };
        let capture = || -> Result<app_core::CapturedImage> {
            if mode == "inline" {
                return Ok(backend.capture_display(&display)?);
            }
            let mut command = Command::new(&binary);
            command
                .arg(CHILD_ARGUMENT)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .env_remove(crate::task_clipboard::ENV);
            let NativeCaptureReply::Frames(mut frames) = execute_command(
                command,
                request.clone(),
                &|| Ok(()),
                Duration::from_secs(20),
            )?
            else {
                anyhow::bail!("Expected benchmark frame");
            };
            frames.pop().context("Missing benchmark frame")
        };
        let validate = |frame: &app_core::CapturedImage| -> Result<()> {
            ensure!(
                frame.pixels.dimensions() == (3840, 2160),
                "Benchmark source must be the private 3840×2160 output"
            );
            ensure!(
                frame
                    .pixels
                    .pixels()
                    .all(|pixel| pixel.0 == [17, 29, 71, 255]),
                "Private benchmark pixels changed"
            );
            Ok(())
        };
        for _ in 0..5 {
            validate(&capture()?)?;
        }
        let descriptors_before = std::fs::read_dir("/proc/self/fd")?.count();
        let usage = |children| -> Result<libc::rusage> {
            let mut usage = unsafe { std::mem::zeroed() };
            ensure!(
                unsafe {
                    libc::getrusage(
                        if children {
                            libc::RUSAGE_CHILDREN
                        } else {
                            libc::RUSAGE_SELF
                        },
                        &mut usage,
                    )
                } == 0,
                "Cannot read benchmark resource usage"
            );
            Ok(usage)
        };
        let baseline = usage(false)?;
        let baseline_children = usage(true)?;
        let mut times = Vec::with_capacity(100);
        for _ in 0..100 {
            let started = Instant::now();
            let frame = capture()?;
            times.push(started.elapsed().as_secs_f64() * 1000.0);
            validate(&frame)?;
            drop(frame);
        }
        settle();
        let process = usage(false)?;
        let children = usage(true)?;
        let cpu = |value: &libc::rusage| {
            (value.ru_utime.tv_sec + value.ru_stime.tv_sec) as f64 * 1000.0
                + (value.ru_utime.tv_usec + value.ru_stime.tv_usec) as f64 / 1000.0
        };
        let mut sorted = times.clone();
        sorted.sort_by(f64::total_cmp);
        let descriptors_after = std::fs::read_dir("/proc/self/fd")?.count();
        ensure!(
            descriptors_after == descriptors_before && !BUSY.load(Ordering::Acquire),
            "Benchmark retained descriptors or an active native owner"
        );
        println!(
            "{}",
            serde_json::json!({"mode":mode,"samples":times.len(),"warmups":5,"width":3840,"height":2160,
            "rgba_bytes":3840u64*2160*4,"p50_ms":sorted[49],"p95_ms":sorted[94],"max_ms":sorted[99],"samples_ms":times,
            "parent_reported_peak_rss_kib":process.ru_maxrss,"largest_child_reported_peak_rss_kib":children.ru_maxrss,
            "parent_cpu_ms":cpu(&process)-cpu(&baseline),"children_cpu_ms":cpu(&children)-cpu(&baseline_children),
            "parent_context_switches":process.ru_nvcsw+process.ru_nivcsw-baseline.ru_nvcsw-baseline.ru_nivcsw,
            "children_context_switches":children.ru_nvcsw+children.ru_nivcsw-baseline_children.ru_nvcsw-baseline_children.ru_nivcsw,
            "descriptors_before":descriptors_before,"descriptors_after":descriptors_after,"pending_owner":false,"exact_generated_pixels":true})
        );
        Ok(())
    }
}
