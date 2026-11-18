//! One explicit file reference, one operation-owned native writer. No startup clipboard access.
use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsString,
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
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
#[allow(dead_code)]
#[path = "capture_worker_unix.rs"]
mod unix;

const CHILD: &str = "__native-file-clipboard-worker";
const MAGIC: &[u8; 8] = b"SCFILE01";
const DEADLINE: Duration = Duration::from_secs(5);
static BUSY: AtomicBool = AtomicBool::new(false);
static COMPLETION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static COMPLETED: std::sync::Condvar = std::sync::Condvar::new();
#[cfg(target_os = "linux")]
static WAYLAND: AtomicBool = AtomicBool::new(false);
struct Admission;
impl Drop for Admission {
    fn drop(&mut self) {
        let _completion = COMPLETION_LOCK
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        // Admission outlives Native, whose destructor returns only after actual family reap.
        BUSY.store(false, Ordering::Release);
        COMPLETED.notify_all();
    }
}
// Disconnecting this one-slot channel is nonblocking. The supervisor retains the live process.
struct Lease {
    release: Option<mpsc::SyncSender<()>>,
    completed: mpsc::Receiver<()>,
}
impl app_core::FileClipboardOwner for Lease {
    fn retire(mut self: Box<Self>) -> Result<()> {
        self.release.take();
        self.completed
            .recv_timeout(DEADLINE)
            .context("File clipboard cleanup is still pending")?;
        Ok(())
    }
}
struct Completed(mpsc::SyncSender<()>);
impl Drop for Completed {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}
struct Native {
    child: std::process::Child,
    group: i32,
    #[cfg(test)]
    cleanup_probe: Option<tests::CleanupProbe>,
}
impl Drop for Native {
    fn drop(&mut self) {
        #[cfg(test)]
        let mut cleanup_probe = self.cleanup_probe.take();
        // Keep the waitable leader reserved through the final family signal/inspection.
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
    }
}
pub(crate) fn install() -> Result<()> {
    #[cfg(target_os = "linux")]
    WAYLAND.store(app_core::linux_wayland_session(), Ordering::Release);
    app_core::install_file_clipboard_spawner(spawn, quiescent)
}
fn quiescent() -> Result<()> {
    ensure!(
        !BUSY.load(Ordering::Acquire),
        "A file clipboard publication or cleanup is still active; no later clipboard output was attempted"
    );
    Ok(())
}
fn command_for_owner() -> Result<Command> {
    #[cfg(target_os = "linux")]
    let executable = std::path::PathBuf::from("/proc/self/exe");
    #[cfg(not(target_os = "linux"))]
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command
        .arg(CHILD)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_clear();
    for (key, value) in std::env::vars_os() {
        if matches!(
            key.to_str(),
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
            )
        ) {
            command.env(key, value);
        }
    }
    // An inherited WAYLAND_SOCKET belongs to the original client; never silently reuse it or XWayland.
    Ok(command)
}
fn spawn(
    path: &Path,
    check: &app_core::NativeCaptureCheck<'_>,
) -> Result<Box<dyn app_core::FileClipboardOwner>> {
    app_core::validate_file_clipboard_path(path)?;
    #[cfg(target_os = "linux")]
    let wayland = WAYLAND.load(Ordering::Acquire) || app_core::linux_wayland_session();
    #[cfg(not(target_os = "linux"))]
    let wayland = false;
    spawn_with_command_checked(path, wayland, command_for_owner()?, check)
}
#[cfg(test)]
fn spawn_with_command(
    path: &Path,
    wayland: bool,
    command: Command,
) -> Result<Box<dyn app_core::FileClipboardOwner>> {
    spawn_with_command_checked(path, wayland, command, &|| Ok(()))
}
fn spawn_with_command_checked(
    path: &Path,
    wayland: bool,
    mut command: Command,
    check: &app_core::NativeCaptureCheck<'_>,
) -> Result<Box<dyn app_core::FileClipboardOwner>> {
    check()?;
    use std::os::unix::ffi::OsStrExt;
    app_core::validate_file_clipboard_path(path)?;
    ensure!(
        BUSY.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok(),
        "File clipboard owner is busy or still cleaning up"
    );
    let admission = Admission;
    #[cfg(test)]
    let cleanup_probe = tests::CLEANUP_PROBE.with(|probe| probe.borrow_mut().take());
    let path = path.as_os_str().as_bytes().to_vec();
    let deadline = Instant::now() + DEADLINE;
    let cancelled = Arc::new(AtomicBool::new(false));
    let stop = cancelled.clone();
    let (ready, response) = mpsc::sync_channel(1);
    let (release, retired) = mpsc::sync_channel::<()>(1);
    let (completed, completion) = mpsc::sync_channel(1);
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
                let mut child = command
                    .spawn()
                    .context("Could not start file clipboard owner")?;
                let group = match i32::try_from(child.id()) {
                    Ok(group) => group,
                    Err(error) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(error.into());
                    }
                };
                let native = Native {
                    child,
                    group,
                    #[cfg(test)]
                    cleanup_probe,
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
                check()?;
                ensure!(
                    Instant::now() < deadline,
                    "File clipboard publication timed out"
                );
                Ok((native, io.transport))
            })();
            match result {
                Ok((native, channel)) => {
                    if ready.send(Ok(())).is_ok() {
                        #[cfg(target_os = "linux")]
                        let _ = retired.recv(); // Park with zero idle wakeups until replacement/discard.
                    }
                    drop(channel);
                    drop(native);
                }
                Err(_) => {
                    let _ = ready.send(Err(()));
                }
            }
            #[cfg(not(target_os = "linux"))]
            drop(retired);
        })
        .context("Could not start file clipboard supervisor")?;
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
            Ok(Err(())) | Err(mpsc::RecvTimeoutError::Disconnected) => break false,
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
        drop(release);
        let _ = completion.recv_timeout(Duration::from_millis(250));
        #[cfg(target_os = "linux")]
        if wayland {
            anyhow::bail!(
                "Wayland file publication failed (data-control and exactly one seat required); clipboard content may already have changed"
            );
        }
        anyhow::bail!(
            "File clipboard publication failed or timed out; clipboard content may already have changed"
        )
    }
}

pub(crate) fn try_child(arguments: &[OsString]) -> Result<bool> {
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
struct HoldWake {
    read: std::os::unix::net::UnixStream,
    handlers: [Option<signal_hook::SigId>; 2],
}
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
impl Drop for HoldWake {
    fn drop(&mut self) {
        for handler in &mut self.handlers {
            if let Some(handler) = handler.take() {
                signal_hook::low_level::unregister(handler);
            }
        }
    }
}
#[cfg(test)]
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

/// Explicit standalone lifetime, shared with the existing `clipboard` CLI route.
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

#[cfg(test)]
#[path = "file_clipboard_tests.rs"]
mod tests;
