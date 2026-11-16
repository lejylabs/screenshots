//! One active curl child with a private inherited config pipe and source stdin.

use super::sharing_provider::{self as provider, Process};
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    io::{PipeWriter, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};

struct Curl {
    child: Child,
    group: i32,
    config: Zeroizing<Vec<u8>>,
    sent: usize,
    writer: Option<PipeWriter>,
    stopped: bool,
}
impl Process for Curl {
    fn child(&mut self) -> &mut Child {
        &mut self.child
    }
    fn config(&mut self) -> Result<bool> {
        let Some(writer) = self.writer.as_mut() else {
            return Ok(true);
        };
        while self.sent < self.config.len() {
            match writer.write(&self.config[self.sent..]) {
                Ok(0) => anyhow::bail!("Provider configuration pipe ended early"),
                Ok(count) => self.sent += count,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(false),
                Err(_) => anyhow::bail!("Could not deliver provider configuration"),
            }
        }
        self.writer.take();
        self.config.zeroize();
        Ok(true)
    }
    fn stop(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        unsafe {
            libc::kill(-self.group, libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.writer.take();
    }
}
impl Drop for Curl {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(super) fn transfer(
    configuration: Zeroizing<Vec<u8>>,
    source: File,
    length: u64,
    check: &impl Fn() -> Result<()>,
    native_sftp: bool,
) -> Result<Vec<u8>> {
    ensure!(
        configuration.len() <= 8192,
        "Provider configuration exceeds 8 KiB"
    );
    check()?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let (reader, writer) =
        std::io::pipe().context("Could not create provider configuration pipe")?;
    let descriptor = reader.as_raw_fd();
    let flags = unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_GETFL) };
    ensure!(
        flags >= 0
            && unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                == 0,
        "Could not bound provider configuration pipe"
    );
    let mut command = if native_sftp {
        let mut command = Command::new(
            std::env::current_exe().context("Could not locate private provider engine")?,
        );
        command.arg("provider-engine");
        provider::scrub_environment(&mut command);
        command
    } else {
        provider::curl()
    };
    command
        .args(["--config", "/dev/fd/3"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    #[cfg(target_os = "linux")]
    let parent = std::process::id() as libc::pid_t;
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(descriptor, 3) < 0 || libc::fcntl(3, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            #[cfg(target_os = "linux")]
            {
                if libc::getppid() != parent {
                    return Err(std::io::Error::other("Provider parent exited"));
                }
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(std::io::Error::other("Provider parent exited"));
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
    let mut child = command.spawn().map_err(|_| {
        anyhow::anyhow!(
            "Provider engine is unavailable; install curl with the required protocol separately"
        )
    })?;
    drop(reader);
    let group = match i32::try_from(child.id()) {
        Ok(group) => group,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("Invalid provider process identifier")
        }
    };
    let process = Curl {
        child,
        group,
        config: configuration,
        sent: 0,
        writer: Some(writer),
        stopped: false,
    };
    provider::execute(process, source, length, deadline, check)
        .context("Provider upload failed; a remote object may remain and no retry was attempted")
}
