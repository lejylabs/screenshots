//! One explicit file reference, one operation-owned native writer. No startup clipboard access.
use anyhow::{Context, Result, ensure};
#[cfg(unix)]
use std::path::Path;
use std::{
    ffi::OsString,
    io::{Read, Write},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
#[cfg(unix)]
#[allow(dead_code)]
#[path = "../../../crates/app-core/src/clipboard_input_budget.rs"]
mod budget;
#[cfg(target_os = "linux")]
#[path = "file_clipboard_linux.rs"]
mod linux;
#[cfg(target_os = "macos")]
#[path = "file_clipboard_macos.rs"]
mod macos;
#[allow(dead_code)]
#[path = "capture_worker_transport.rs"]
mod transport;
#[cfg(unix)]
#[allow(dead_code)]
#[path = "capture_worker_unix.rs"]
mod unix;

#[cfg(target_os = "windows")]
#[path = "file_clipboard_image_windows.rs"]
mod image_windows;
#[path = "file_clipboard_output.rs"]
mod output;
#[cfg(target_os = "windows")]
#[allow(dead_code)]
#[path = "capture_worker_windows.rs"]
mod windows;
#[cfg(unix)]
const CHILD: &str = "__native-file-clipboard-worker";
#[cfg(unix)]
const MAGIC: &[u8; 8] = b"SCFILE01";
const DEADLINE: Duration = Duration::from_secs(5);
static BUSY: AtomicBool = AtomicBool::new(false);
// Root integration: capture admission is reserved first; success is followed immediately by _exit.
#[cfg(unix)]
pub(crate) fn reserve_signal_shutdown() -> bool {
    BUSY.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}
#[cfg(all(test, target_os = "linux"))]
pub(crate) fn release_signal_shutdown_for_test() {
    drop(Admission);
}

static CLOSING: AtomicBool = AtomicBool::new(false);
static COMPLETION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static COMPLETED: std::sync::Condvar = std::sync::Condvar::new();
static ACTIVE_STOP: std::sync::Mutex<Option<(Arc<AtomicBool>, mpsc::SyncSender<()>)>> =
    std::sync::Mutex::new(None);
#[cfg(target_os = "linux")]
static WAYLAND: AtomicBool = AtomicBool::new(false);
struct Admission;
impl Drop for Admission {
    fn drop(&mut self) {
        let _completion = COMPLETION_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        // Admission outlives Native, whose destructor returns only after actual family reap.
        ACTIVE_STOP
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .take();
        BUSY.store(false, Ordering::Release);
        COMPLETED.notify_all();
    }
}
// Sending retirement on this one-slot channel is nonblocking. The supervisor retains the live process.
struct Lease {
    release: Option<mpsc::SyncSender<()>>,
    completed: mpsc::Receiver<()>,
}
impl app_core::FileClipboardOwner for Lease {
    fn retire_with_cancel(
        mut self: Box<Self>,
        deadline: Instant,
        check: &app_core::NativeCaptureCheck<'_>,
    ) -> Result<()> {
        if let Some(release) = self.release.take() {
            let _ = release.try_send(());
        }
        loop {
            check()?;
            if Instant::now() >= deadline {
                return Err(app_core::CaptureError::NativeTimeout.into());
            }
            match self.completed.recv_timeout(
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(20)),
            ) {
                Ok(()) => {
                    check()?;
                    if Instant::now() >= deadline {
                        return Err(app_core::CaptureError::NativeTimeout.into());
                    }
                    return Ok(());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    anyhow::bail!("Clipboard native cleanup acknowledgement is unavailable")
                }
            }
        }
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if let Some(release) = self.release.take() {
            let _ = release.try_send(());
        }
    }
}
struct Completed(mpsc::SyncSender<()>);
impl Drop for Completed {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}
struct Native {
    #[cfg(unix)]
    child: std::process::Child,
    #[cfg(unix)]
    group: i32,
    #[cfg(target_os = "windows")]
    child: windows::Process,
    #[cfg(target_os = "windows")]
    container: Arc<windows::Container>,
    #[cfg(all(test, unix))]
    cleanup_probe: Option<tests::CleanupProbe>,
}
impl Drop for Native {
    fn drop(&mut self) {
        #[cfg(all(test, unix))]
        let mut cleanup_probe = self.cleanup_probe.take();
        // Keep the waitable leader reserved through the final family signal/inspection.
        #[cfg(unix)]
        loop {
            unix::signal_owned_group(self.group);
            #[cfg(test)]
            if let Some(probe) = cleanup_probe.as_mut() {
                probe.before_reap(self.group);
            }
            if let Ok(true) = unix::exited_without_reaping(self.group, true) {
                if let Ok(members) = unix::live_members(self.group) {
                    if members.is_empty() {
                        if self.child.wait().is_ok() {
                            #[cfg(test)]
                            if let Some(probe) = cleanup_probe.as_mut() {
                                probe.after_reap(self.group);
                            }
                            return;
                        }
                    } else if unix::wait_members(self.group, &members) {
                        continue;
                    }
                }
            }
            // Only active failed teardown retries; this never installs an idle service timer.
            std::thread::park_timeout(Duration::from_millis(100));
        }
        #[cfg(target_os = "windows")]
        loop {
            self.container.stop();
            self.child.kill();
            let waited = self.child.wait().is_ok();
            if waited && self.container.empty().is_ok_and(|empty| empty) {
                return;
            }
            std::thread::park_timeout(Duration::from_millis(100));
        }
    }
}
pub(crate) fn install() -> Result<()> {
    #[cfg(target_os = "linux")]
    WAYLAND.store(app_core::linux_wayland_session(), Ordering::Release);
    #[cfg(unix)]
    app_core::install_file_clipboard_spawner(spawn, quiescent)?;
    app_core::install_native_clipboard_spawner(spawn_output)
}
#[cfg(unix)]
fn quiescent() -> Result<()> {
    ensure!(
        !BUSY.load(Ordering::Acquire),
        "A clipboard publication or cleanup is still active; no later clipboard output was attempted"
    );
    Ok(())
}
fn command_for_owner(child: &str) -> Result<Command> {
    #[cfg(target_os = "linux")]
    let executable = std::path::PathBuf::from("/proc/self/exe");
    #[cfg(not(target_os = "linux"))]
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .arg(child)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_clear();
    for (key, value) in std::env::vars_os() {
        #[cfg(target_os = "windows")]
        let name = key.to_str().map(str::to_ascii_uppercase);
        #[cfg(target_os = "windows")]
        let name = name.as_deref();
        #[cfg(not(target_os = "windows"))]
        let name = key.to_str();
        if matches!(
            name,
            Some(
                "HOME"
                    | "PATH"
                    | "DISPLAY"
                    | "XAUTHORITY"
                    | "WAYLAND_DISPLAY"
                    | "XDG_RUNTIME_DIR"
                    | "XDG_SESSION_TYPE"
                    | "LANG"
                    | "LC_ALL"
                    | "LC_CTYPE"
                    | "TMPDIR"
                    | "SYSTEMROOT"
                    | "WINDIR"
                    | "TEMP"
                    | "TMP"
                    | "USERPROFILE"
            )
        ) {
            command.env(key, value);
        }
    }
    // An inherited WAYLAND_SOCKET belongs to the original client; never silently reuse it or XWayland.
    Ok(command)
}
#[cfg(unix)]
fn spawn(
    path: &Path,
    check: &app_core::NativeCaptureCheck<'_>,
) -> Result<Box<dyn app_core::FileClipboardOwner>> {
    app_core::validate_file_clipboard_path(path)?;
    #[cfg(target_os = "linux")]
    let wayland = WAYLAND.load(Ordering::Acquire) || app_core::linux_wayland_session();
    #[cfg(not(target_os = "linux"))]
    let wayland = false;
    spawn_with_command_checked(path, wayland, command_for_owner(CHILD)?, check)
}
#[cfg(all(test, unix))]
fn spawn_with_command(
    path: &Path,
    wayland: bool,
    command: Command,
) -> Result<Box<dyn app_core::FileClipboardOwner>> {
    spawn_with_command_checked(path, wayland, command, &|| Ok(()))
}
#[cfg(unix)]
fn spawn_with_command_checked(
    path: &Path,
    wayland: bool,
    command: Command,
    check: &app_core::NativeCaptureCheck<'_>,
) -> Result<Box<dyn app_core::FileClipboardOwner>> {
    spawn_input(Input::File(path), wayland, command, check)
}
enum Input<'a> {
    #[cfg(unix)]
    File(&'a Path),
    Output(app_core::NativeClipboardOutput<'a>),
}
fn spawn_output(
    output: app_core::NativeClipboardOutput<'_>,
    check: &app_core::NativeCaptureCheck<'_>,
) -> Result<Box<dyn app_core::FileClipboardOwner>> {
    #[cfg(target_os = "linux")]
    let wayland = WAYLAND.load(Ordering::Acquire) || app_core::linux_wayland_session();
    #[cfg(not(target_os = "linux"))]
    let wayland = false;
    spawn_input(
        Input::Output(output),
        wayland,
        command_for_owner(output::CHILD)?,
        check,
    )
}
fn spawn_input(
    input: Input<'_>,
    wayland: bool,
    mut command: Command,
    check: &app_core::NativeCaptureCheck<'_>,
) -> Result<Box<dyn app_core::FileClipboardOwner>> {
    check()?;
    #[cfg(unix)]
    if let Input::File(path) = &input {
        app_core::validate_file_clipboard_path(path)?;
    }
    ensure!(
        !CLOSING.load(Ordering::Acquire),
        "Clipboard native output is shutting down"
    );
    ensure!(
        BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok(),
        "Clipboard owner is busy or still cleaning up"
    );
    let admission = Admission;
    #[cfg(all(test, unix))]
    let cleanup_probe = tests::CLEANUP_PROBE.with(|probe| probe.borrow_mut().take());
    let deadline = Instant::now() + DEADLINE;
    enum OwnedInput {
        #[cfg(unix)]
        File(Vec<u8>),
        Output(output::Request),
    }
    let input = match input {
        #[cfg(unix)]
        Input::File(path) => {
            use std::os::unix::ffi::OsStrExt;
            OwnedInput::File(path.as_os_str().as_bytes().to_vec())
        }
        Input::Output(value) => OwnedInput::Output(output::Request::new(value)?),
    };
    check()?;
    ensure!(
        Instant::now() < deadline,
        "Clipboard output preparation timed out"
    );
    let cancelled = Arc::new(AtomicBool::new(false));
    let stop = cancelled.clone();
    let (ready, response) = mpsc::sync_channel(1);
    let image_gate = Arc::new(AtomicBool::new(false));
    #[cfg(target_os = "windows")]
    let worker_image_gate = image_gate.clone();
    #[cfg(not(target_os = "windows"))]
    let worker_image_gate = image_gate;
    let (release, retired) = mpsc::sync_channel::<()>(1);
    let (completed, completion) = mpsc::sync_channel(1);
    *ACTIVE_STOP
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = Some((cancelled.clone(), release.clone()));
    ensure!(
        !CLOSING.load(Ordering::Acquire),
        "Clipboard native output is shutting down"
    );
    #[cfg(target_os = "windows")]
    let owned_container: Arc<std::sync::OnceLock<Arc<windows::Container>>> =
        Arc::new(std::sync::OnceLock::new());
    #[cfg(target_os = "windows")]
    let coordinator_container = owned_container.clone();
    std::thread::Builder::new()
        .name("file-clipboard-owner".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            let _completed = Completed(completed);
            let _admission = admission;
            let result = (|| -> Result<_> {
                ensure!(
                    !stop.load(Ordering::Acquire) && Instant::now() < deadline,
                    "File publication canceled"
                );
                let prepared = transport::Prepared::new(&mut command)?;
                #[cfg(unix)]
                let native = {
                    let mut child = command.spawn().context("Could not start clipboard owner")?;
                    let group = match i32::try_from(child.id()) {
                        Ok(group) => group,
                        Err(error) => {
                            let _ = child.kill();
                            let _ = child.wait();
                            return Err(error.into());
                        }
                    };
                    Native {
                        child,
                        group,
                        #[cfg(test)]
                        cleanup_probe,
                    }
                };
                #[cfg(target_os = "windows")]
                let native = {
                    let container = Arc::new(windows::container()?);
                    let _ = coordinator_container.set(container.clone());
                    if stop.load(Ordering::Acquire) || Instant::now() >= deadline {
                        anyhow::bail!("Clipboard output canceled before process creation");
                    }
                    let child = windows::spawn(&command, &container, &prepared.child_handles())?;
                    Native { child, container }
                };
                let (channel, nonce) = prepared.parent();
                let check = || {
                    if stop.load(Ordering::Acquire) {
                        Err(app_core::CaptureError::NativeCanceled)
                    } else {
                        Ok(())
                    }
                };
                let mut io = transport::ActiveIo {
                    transport: channel,
                    deadline,
                    check: &check,
                };
                match input {
                    #[cfg(unix)]
                    OwnedInput::File(path) => {
                        let mut hello = [0; 24];
                        io.read_exact(&mut hello)?;
                        ensure!(
                            &hello[..8] == MAGIC && hello[8..] == nonce,
                            "File owner challenge mismatch"
                        );
                        io.write_all(MAGIC)?;
                        io.write_all(&nonce)?;
                        io.write_all(&(path.len() as u32).to_be_bytes())?;
                        io.write_all(&[u8::from(wayland)])?;
                        io.write_all(&path)?;
                        drop(path);
                        let mut ack = [0];
                        io.read_exact(&mut ack)?;
                        ensure!(ack == [1], "Native file clipboard publication failed");
                    }
                    OwnedInput::Output(request) => {
                        request.exchange(wayland, &nonce, &mut io, &worker_image_gate)?
                    }
                }
                check()?;
                ensure!(
                    Instant::now() < deadline,
                    "File clipboard publication timed out"
                );
                Ok((native, io.transport))
            })();
            match result {
                Ok((native, channel)) => {
                    #[cfg(target_os = "linux")]
                    {
                        if ready.send(Ok(())).is_ok() {
                            let _ = retired.recv(); // Zero idle wakeups until replacement/discard.
                        }
                        drop(channel);
                        drop(native);
                    }
                    #[cfg(not(target_os = "linux"))]
                    {
                        drop(channel);
                        drop(native);
                        let _ = ready.send(Ok(()));
                    }
                }
                Err(error) => {
                    let preparation = error.downcast_ref::<output::PreparationFailed>().is_some();
                    let _ = ready.send(Err(preparation));
                }
            }
            #[cfg(not(target_os = "linux"))]
            drop(retired);
        })
        .context("Could not start file clipboard supervisor")?;
    let mut preparation_failed = false;
    let published = loop {
        if check().is_err() || Instant::now() >= deadline {
            break false;
        }
        match response.recv_timeout(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(20)),
        ) {
            Ok(Ok(())) => break check().is_ok() && Instant::now() < deadline,
            Ok(Err(preparation)) => {
                preparation_failed = preparation;
                break false;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break false,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    };
    if published {
        Ok(Box::new(Lease {
            release: Some(release),
            completed: completion,
        }))
    } else {
        cancelled.store(true, Ordering::Release);
        #[cfg(target_os = "windows")]
        if let Some(container) = owned_container.get() {
            container.stop();
        }
        let _ = release.try_send(());
        drop(release);
        let _ = completion.recv_timeout(Duration::from_millis(250));
        #[cfg(target_os = "windows")]
        if image_gate.load(Ordering::Acquire) {
            anyhow::bail!(
                "Clipboard image output did not complete; PNG may already be available and DIBV5 may be missing; no retry was attempted"
            );
        }
        if preparation_failed {
            anyhow::bail!(
                "Clipboard image preparation failed or exceeded its native resource budget; clipboard publication was not attempted"
            );
        }
        #[cfg(target_os = "linux")]
        if wayland {
            anyhow::bail!(
                "Wayland clipboard publication failed (data-control and exactly one seat required); clipboard content may already have changed"
            );
        }
        anyhow::bail!(
            "Clipboard publication failed or timed out; clipboard content may already have changed"
        )
    }
}

pub(crate) fn try_child(arguments: &[OsString]) -> Result<bool> {
    if arguments.first().is_some_and(|arg| arg == output::CHILD) {
        ensure!(
            arguments.len() == 1,
            "Invalid private clipboard output arguments"
        );
        output::serve().map_err(|_| {
            anyhow::anyhow!(
                "Private native clipboard output failed; contents may already have changed"
            )
        })?;
        return Ok(true);
    }
    #[cfg(unix)]
    {
        legacy_child(arguments)
    }
    #[cfg(not(unix))]
    {
        Ok(false)
    }
}
#[cfg(unix)]
fn legacy_child(arguments: &[OsString]) -> Result<bool> {
    if arguments.first().is_none_or(|arg| arg != CHILD) {
        return Ok(false);
    }
    ensure!(
        arguments.len() == 1,
        "Invalid private file clipboard arguments"
    );
    let (channel, nonce, parent) = transport::Transport::inherited()?;
    #[cfg(target_os = "macos")]
    let _lifeline = transport::Lifeline::new(parent)?;
    #[cfg(not(target_os = "macos"))]
    let _ = parent;
    let _budget = budget::Budget::enter()?;
    let mut io = transport::ActiveIo {
        transport: channel,
        deadline: Instant::now() + DEADLINE,
        check: &|| Ok(()),
    };
    io.write_all(MAGIC)?;
    io.write_all(&nonce)?;
    let mut header = [0; 29];
    io.read_exact(&mut header)?;
    ensure!(
        &header[..8] == MAGIC && header[8..24] == nonce && header[28] <= 1,
        "Invalid private file clipboard request"
    );
    let length = u32::from_be_bytes(header[24..28].try_into()?) as usize;
    ensure!(
        (1..=app_core::CLIPBOARD_FILE_BYTES).contains(&length),
        "File reference exceeds its bound"
    );
    let mut bytes = vec![0; length];
    io.read_exact(&mut bytes)?;
    use std::os::unix::ffi::OsStrExt;
    let path = Path::new(std::ffi::OsStr::from_bytes(&bytes));
    app_core::validate_file_clipboard_path(path)?;
    let uri = app_core::prepare_file_clipboard_uri(path)?;
    drop(bytes);
    #[cfg(target_os = "linux")]
    linux::serve(&uri, header[28] == 1, &mut io)?;
    #[cfg(target_os = "macos")]
    {
        ensure!(
            header[28] == 0,
            "Wayland file clipboard policy requires Linux"
        );
        macos::publish(&uri, "com.apple.pasteboard.clipboard")?;
        io.write_all(&[1])?;
    }
    Ok(true)
}

/// Operation-scoped self-pipe: delivery on any thread remains readable across check/poll races.
#[cfg(unix)]
struct HoldWake {
    read: std::os::unix::net::UnixStream,
    handlers: [Option<signal_hook::SigId>; 2],
}
#[cfg(unix)]
impl HoldWake {
    fn new() -> Result<Self> {
        let (read, write) = std::os::unix::net::UnixStream::pair()?;
        read.set_nonblocking(true)?;
        write.set_nonblocking(true)?;
        // The signal-handler endpoint is bounded and never blocks when signals coalesce.
        use std::os::fd::AsRawFd;
        let size = 4096i32;
        ensure!(
            unsafe {
                libc::setsockopt(
                    write.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&size as *const i32).cast(),
                    std::mem::size_of_val(&size) as libc::socklen_t,
                )
            } == 0,
            "Could not bound file clipboard signal wake buffer"
        );
        let mut wake = Self {
            read,
            handlers: [None, None],
        };
        for (index, signal) in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM]
            .into_iter()
            .enumerate()
        {
            wake.handlers[index] = Some(signal_hook::low_level::pipe::register(
                signal,
                write.try_clone()?,
            )?);
        }
        Ok(wake)
    }
    fn wait(&self, cancel: &crate::record::unix::Cancellation, until: Instant) -> Result<()> {
        use std::os::fd::AsRawFd;
        while Instant::now() < until {
            if cancel.check(until).is_err() {
                return Ok(());
            }
            #[cfg(test)]
            BEFORE_HOLD_POLL.with(|hook| {
                if let Some(hook) = hook.borrow_mut().take() {
                    hook();
                }
            });
            let remaining = until
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(i32::MAX as u128) as i32;
            let mut event = libc::pollfd {
                fd: self.read.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let result = unsafe { libc::poll(&mut event, 1, remaining) };
            if result < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(std::io::Error::last_os_error().into());
            }
            ensure!(
                event.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) == 0,
                "File clipboard signal wake channel closed"
            );
            // A pending byte is sufficient even when its signal ran on another native thread.
            if event.revents & libc::POLLIN != 0 {
                return Ok(());
            }
        }
        Ok(())
    }
}
#[cfg(unix)]
impl Drop for HoldWake {
    fn drop(&mut self) {
        for handler in &mut self.handlers {
            if let Some(handler) = handler.take() {
                signal_hook::low_level::unregister(handler);
            }
        }
    }
}
#[cfg(all(test, unix))]
thread_local! {
    static BEFORE_HOLD_POLL: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

/// Standalone exit must drain the actual supervisor, including after bounded API errors.
struct StandaloneDrain;
impl Drop for StandaloneDrain {
    fn drop(&mut self) {
        let mut completion = COMPLETION_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        while BUSY.load(Ordering::Acquire) {
            completion = COMPLETED
                .wait(completion)
                .unwrap_or_else(|poison| poison.into_inner());
        }
    }
}

/// Exit authority: wait for actual reap, never just the bounded API acknowledgement.
pub(crate) fn drain_owned_workers() {
    CLOSING.store(true, Ordering::Release);
    if let Some((stop, release)) = ACTIVE_STOP
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .as_ref()
    {
        stop.store(true, Ordering::Release);
        let _ = release.try_send(());
    }
    drop(StandaloneDrain);
}

/// Explicit standalone lifetime, shared with the existing `clipboard` CLI route.
#[cfg(unix)]
pub(crate) fn command(args: &[OsString]) -> Result<()> {
    ensure!(
        args.len() == 3 && args[1] == "--hold-ms",
        "Usage: screenshots clipboard copy-file ABSOLUTE_FILE --hold-ms 1000..300000"
    );
    let hold: u64 = args[2].to_str().context("Invalid hold duration")?.parse()?;
    ensure!(
        (1000..=300000).contains(&hold),
        "File clipboard hold must be 1000..300000 ms"
    );
    let cancel = crate::record::unix::Cancellation::new()?;
    let wake = HoldWake::new()?;
    let completion = StandaloneDrain;
    let result = (|| -> Result<()> {
        let mut clipboard = app_core::ImageClipboard::default();
        let publication_deadline = Instant::now() + DEADLINE;
        clipboard.copy_file_with_cancel(Path::new(&args[0]), &|| {
            cancel
                .check(publication_deadline)
                .map_err(|_| app_core::CaptureError::NativeCanceled)
        })?;
        let until = Instant::now() + Duration::from_millis(hold);
        let held = wake.wait(&cancel, until);
        let retired = clipboard.retire_file_owner();
        held?;
        retired?;
        Ok(())
    })();
    #[cfg(test)]
    tests::standalone_result_ready(&result);
    // The clipboard/lease has dropped before this unbounded, event-driven actual-reap drain.
    // Do not enable conditional signal shutdown while native cleanup still owns a child.
    drop(completion);
    cancel.output_phase(true);
    result?;
    println!("{{\"kind\":\"file\",\"published\":true,\"ownership_released\":true}}");
    Ok(())
}

#[cfg(all(test, unix))]
#[path = "file_clipboard_tests.rs"]
mod tests;

#[cfg(all(test, target_os = "windows"))]
#[path = "file_clipboard_windows_tests.rs"]
mod windows_tests;
