//! One bounded task child; no process or pipe work exists while idle.
#[cfg(target_os = "windows")]
use app_core::windows_child::{self, Child, Environment, Streams};
#[cfg(not(target_os = "windows"))]
use std::process::Child;
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub(super) struct Outcome {
    pub(super) state: &'static str,
    pub(super) success: bool,
    pub(super) exit_code: Option<i32>,
}
impl Outcome {
    pub(super) fn canceled() -> Self {
        Self {
            state: "canceled",
            success: false,
            exit_code: None,
        }
    }
}
struct Job {
    child: Child,
    #[cfg(unix)]
    group: i32,
    #[cfg(target_os = "windows")]
    job: Option<std::os::windows::io::OwnedHandle>,
}
impl Job {
    fn stop(&mut self, graceful: bool) {
        #[cfg(unix)]
        {
            if graceful {
                unsafe {
                    libc::kill(-self.group, libc::SIGTERM);
                }
                let until = Instant::now() + Duration::from_millis(500);
                while Instant::now() < until {
                    if self.child.try_wait().ok().flatten().is_some() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            unsafe {
                libc::kill(-self.group, libc::SIGKILL);
            }
        }
        #[cfg(target_os = "windows")]
        {
            let _ = graceful;
            drop(self.job.take());
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.stop(false);
    }
}

#[cfg(unix)]
fn drain(mut pipe: impl Read + std::os::fd::AsRawFd, limit: usize, stop: &AtomicBool) -> bool {
    use std::os::fd::AsRawFd;
    let fd = pipe.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return false;
    }
    let mut count = 0;
    let mut bytes = [0u8; 8192];
    while !stop.load(Ordering::Relaxed) {
        let mut descriptor = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut descriptor, 1, 25) };
        if ready < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return false;
        }
        if ready == 0 {
            continue;
        }
        loop {
            match pipe.read(&mut bytes) {
                Ok(0) => return true,
                Ok(read) => {
                    count += read;
                    if count > limit {
                        return false;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return false,
            }
        }
    }
    false
}
#[cfg(not(unix))]
fn drain(mut pipe: impl Read, limit: usize, _stop: &AtomicBool) -> bool {
    let mut count = 0;
    let mut bytes = [0u8; 8192];
    loop {
        match pipe.read(&mut bytes) {
            Ok(0) => return true,
            Ok(read) => {
                count += read;
                if count > limit {
                    return false;
                }
            }
            Err(_) => return false,
        }
    }
}

pub(super) fn run(
    profile: &Path,
    timeout: Duration,
    cancel: &Arc<AtomicBool>,
    clipboard: Arc<std::sync::Mutex<app_core::ImageClipboard>>,
) -> Outcome {
    let failed = || Outcome {
        state: "failed",
        success: false,
        exit_code: None,
    };
    if cancel.load(Ordering::Relaxed) {
        return Outcome::canceled();
    }
    #[cfg(target_os = "linux")]
    let executable = std::path::PathBuf::from("/proc/self/exe");
    #[cfg(not(target_os = "linux"))]
    let Ok(executable) = std::env::current_exe() else {
        return failed();
    };
    let mut command = Command::new(executable);
    command
        .arg("task")
        .arg("run")
        .arg(profile)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
        #[cfg(target_os = "linux")]
        let parent = std::process::id() as libc::pid_t;
        unsafe {
            command.pre_exec(move || {
                #[cfg(target_os = "linux")]
                {
                    if libc::getppid() != parent {
                        return Err(std::io::Error::other("Task host exited"));
                    }
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::getppid() != parent {
                        return Err(std::io::Error::other("Task host exited"));
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
    }
    #[cfg(target_os = "windows")]
    let container = match crate::ocr_windows::job_object() {
        Ok(job) => job,
        Err(_) => return failed(),
    };
    let Ok(prepared) = crate::task_clipboard::Prepared::new(&mut command) else {
        return failed();
    };
    #[cfg(not(target_os = "windows"))]
    let Ok(child) = command.spawn() else {
        return failed();
    };
    #[cfg(target_os = "windows")]
    let Ok(child) = ({
        use std::os::windows::io::AsHandle;
        windows_child::spawn(
            &command,
            container.as_handle(),
            Streams::OUTPUT,
            Environment::Inherit,
            &prepared.child_handles(),
        )
    }) else {
        return failed();
    };
    #[cfg(unix)]
    let group = match i32::try_from(child.id()) {
        Ok(id) => id,
        Err(_) => {
            let mut child = child;
            let _ = child.kill();
            let _ = child.wait();
            return failed();
        }
    };
    let mut job = Job {
        child,
        #[cfg(unix)]
        group,
        #[cfg(target_os = "windows")]
        job: Some(container),
    };
    let deadline = Instant::now() + timeout;
    let mut bridge = match prepared.start(clipboard, cancel.clone()) {
        Ok(bridge) => bridge,
        Err(_) => return failed(),
    };
    #[cfg(target_os = "windows")]
    if cancel.load(Ordering::Acquire) || job.child.resume(deadline).is_err() {
        return failed();
    }
    let stop = Arc::new(AtomicBool::new(false));
    let pipe_failed = Arc::new(AtomicBool::new(false));
    let stdout = job.child.stdout.take().unwrap();
    let stderr = job.child.stderr.take().unwrap();
    let (out_done, err_done) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let spawn_pipe = |name: &str,
                      limit,
                      pipe_stop: Arc<AtomicBool>,
                      failed: Arc<AtomicBool>,
                      done: Arc<AtomicBool>,
                      pipe| {
        std::thread::Builder::new()
            .name(name.into())
            .stack_size(256 * 1024)
            .spawn(move || {
                if !drain(pipe, limit, &pipe_stop) {
                    failed.store(true, Ordering::Relaxed);
                }
                done.store(true, Ordering::Release);
            })
    };
    let out = match spawn_pipe(
        "task-host-output",
        256 * 1024,
        stop.clone(),
        pipe_failed.clone(),
        out_done.clone(),
        stdout,
    ) {
        Ok(thread) => thread,
        Err(_) => {
            job.stop(false);
            bridge.stop();
            return failed();
        }
    };
    // Child stdout/stderr have different types; give the second worker its own closure.
    let err_stop = stop.clone();
    let err_failed = pipe_failed.clone();
    let err_finished = err_done.clone();
    let err = match std::thread::Builder::new()
        .name("task-host-errors".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            if !drain(stderr, 16 * 1024, &err_stop) {
                err_failed.store(true, Ordering::Relaxed);
            }
            err_finished.store(true, Ordering::Release);
        }) {
        Ok(thread) => thread,
        Err(_) => {
            job.stop(false);
            stop.store(true, Ordering::Relaxed);
            let _ = out.join();
            return failed();
        }
    };
    let (state, status) = loop {
        if cancel.load(Ordering::Relaxed) {
            break ("canceled", None);
        }
        if Instant::now() >= deadline {
            break ("timed_out", None);
        }
        if pipe_failed.load(Ordering::Relaxed) || bridge.failed() {
            break ("failed", None);
        }
        match job.child.try_wait() {
            Ok(Some(status)) => {
                break (
                    if status.success() {
                        "completed"
                    } else {
                        "failed"
                    },
                    Some(status),
                );
            }
            Ok(None) => {}
            Err(_) => break ("failed", None),
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    job.stop(matches!(state, "canceled" | "timed_out"));
    let drain_until = Instant::now() + Duration::from_secs(2);
    while !(out_done.load(Ordering::Acquire) && err_done.load(Ordering::Acquire))
        && Instant::now() < drain_until
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    stop.store(true, Ordering::Relaxed);
    let _ = out.join();
    let _ = err.join();
    bridge.stop();
    let success = state == "completed" && !pipe_failed.load(Ordering::Relaxed) && !bridge.failed();
    Outcome {
        state: if state == "completed" && !success {
            "failed"
        } else {
            state
        },
        success,
        exit_code: status.and_then(|value| value.code()),
    }
}
