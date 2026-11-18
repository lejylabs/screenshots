//! Operation-owned release-tool processes: a deadline, a 128 KiB result bound and
//! cleanup of the whole process family (POSIX process group / Windows job object).
#[cfg(unix)]
use anyhow::Context;
use anyhow::{Result, bail, ensure};
use std::{
    io::Read,
    path::Path,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

const OUTPUT_LIMIT: usize = 128 * 1024;

fn reader(mut pipe: impl Read + Send + 'static, excess: Arc<AtomicBool>) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0u8; 4096];
        while let Ok(read) = pipe.read(&mut buffer) {
            if read == 0 {
                break;
            }
            if output.len() + read > OUTPUT_LIMIT {
                excess.store(true, Ordering::SeqCst);
                break;
            }
            output.extend_from_slice(&buffer[..read]);
        }
        output
    })
}

/// Run `command` with stdin/stderr discarded and stdout optionally captured.
/// No tool diagnostics are copied into this process's output.
#[cfg(unix)]
pub fn contained(mut command: Command, working: &Path, timeout: Duration, capture: bool) -> Result<Vec<u8>> {
    use std::os::unix::process::CommandExt;
    use std::process::Stdio;
    command
        .current_dir(working)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(if capture { Stdio::piped() } else { Stdio::null() })
        .process_group(0);
    let mut child = command.spawn().context("Could not start native release tool")?;
    let pid = child.id() as libc::pid_t;
    let excess = Arc::new(AtomicBool::new(false));
    let output = child.stdout.take().map(|pipe| reader(pipe, excess.clone()));
    // Observe exit without reaping: the unreaped leader keeps its group id reserved
    // until every owned descendant has been signalled.
    let exited = || {
        // SAFETY: siginfo_t is plain data; waitid only writes into it.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let flags = libc::WEXITED | libc::WNOHANG | libc::WNOWAIT;
        // SAFETY: valid pid of our unreaped child and a writable siginfo_t.
        let result = unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, flags) };
        // SAFETY: si_pid is valid for SIGCHLD-style results; zero means still running.
        result == 0 && unsafe { info.si_pid() } == pid
    };
    let stop_group = || {
        // SAFETY: signalling our own process group; errors are inspected below.
        if unsafe { libc::killpg(pid, libc::SIGKILL) } != 0 {
            let error = std::io::Error::last_os_error();
            // ESRCH: nothing left. EPERM: Darwin reports it for a zombie-only group.
            let tolerated = error.raw_os_error() == Some(libc::ESRCH)
                || cfg!(target_os = "macos") && error.raw_os_error() == Some(libc::EPERM);
            if !tolerated {
                return Err(error);
            }
        }
        Ok(())
    };
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    while !exited() {
        if excess.load(Ordering::SeqCst) || Instant::now() >= deadline {
            timed_out = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let stopped = stop_group();
    let status = child.wait()?;
    stopped?;
    let output = match output {
        Some(handle) => handle.join().map_err(|_| anyhow::anyhow!("Native signer result reader failed"))?,
        None => Vec::new(),
    };
    ensure!(!timed_out, "Native signing tool exceeded its output or time budget");
    ensure!(!excess.load(Ordering::SeqCst), "Native signing result exceeds 128 KiB");
    ensure!(status.success(), "Native signing/verification failed; no signed artifact was published");
    Ok(output)
}

#[cfg(windows)]
pub fn contained(mut command: Command, working: &Path, timeout: Duration, capture: bool) -> Result<Vec<u8>> {
    use app_core::windows_child::{self, Environment, Stream, Streams};
    use std::os::windows::io::AsHandle;
    command.current_dir(working);
    let explicit = command.get_envs().next().is_some();
    let job = windows_child::job(None, None)?;
    let streams = Streams { stdin: Stream::Null, stdout: if capture { Stream::Pipe } else { Stream::Null }, stderr: Stream::Null };
    let mode = if explicit { Environment::Explicit } else { Environment::Inherit };
    let deadline = Instant::now() + timeout;
    let mut child = windows_child::spawn(&command, job.as_handle(), streams, mode, &[])?;
    let excess = Arc::new(AtomicBool::new(false));
    let output = child.stdout.take().map(|pipe| reader(pipe, excess.clone()));
    child.resume(deadline)?;
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if excess.load(Ordering::SeqCst) || Instant::now() >= deadline {
            timed_out = true;
            break None;
        }
        child.wait_ready(Duration::from_millis(10));
    };
    // Closing the kill-on-close job ends every remaining descendant.
    drop(job);
    if status.is_none() {
        let _ = child.wait();
    }
    let output = match output {
        Some(handle) => handle.join().map_err(|_| anyhow::anyhow!("Release-tool result reader failed"))?,
        None => Vec::new(),
    };
    ensure!(!timed_out, "Release-tool exceeded its output or time budget");
    ensure!(!excess.load(Ordering::SeqCst), "Native signing result exceeds 128 KiB");
    match status {
        Some(status) if status.success() => Ok(output),
        _ => bail!("Native signing/verification failed"),
    }
}
