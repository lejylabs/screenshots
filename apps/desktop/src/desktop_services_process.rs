//! Reuse the authenticated inherited channel and reserved-leader cleanup for native desktop services.
use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsString,
    process::{Command, Stdio},
    time::Duration,
};
// The desktop service exchanges metadata packets, not capture frames.
#[allow(dead_code)]
#[path = "capture_worker_transport.rs"]
mod transport;
#[allow(dead_code)]
#[path = "capture_worker_unix.rs"]
mod unix;

const CHILD: &str = "__native-desktop-services-worker";
struct Owner {
    child: Option<std::process::Child>,
    group: i32,
}
impl Owner {
    fn finish(&mut self) -> Result<()> {
        let Some(child) = self.child.as_mut() else {
            return Ok(());
        };
        unix::signal_owned_group(self.group);
        // Keep the leader unreaped until the final group signal and family inspection complete.
        ensure!(
            unix::exited_without_reaping(self.group, true)?,
            "Native service leader is not waitable"
        );
        loop {
            unix::signal_owned_group(self.group);
            let members = unix::live_members(self.group)?;
            if members.is_empty() {
                break;
            }
            ensure!(
                unix::wait_members(self.group, &members),
                "Native service family cleanup is pending"
            );
        }
        child
            .wait()
            .context("Could not reap native desktop service")?;
        self.child.take();
        Ok(())
    }
}
impl app_core::LinuxDesktopProcessOwner for Owner {
    fn terminate_and_reap(&mut self) -> Result<()> {
        self.finish()
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        // Never release a live owner after an inspection/kernel failure.
        while self.child.is_some() {
            if self.finish().is_ok() {
                break;
            }
            // Kernel/process cleanup failures remain explicit active teardown, not an idle service.
            std::thread::park_timeout(Duration::from_millis(100));
        }
    }
}
pub(crate) fn install() -> Result<()> {
    app_core::install_linux_desktop_spawner(spawn)
}
pub(crate) fn spawn() -> Result<app_core::LinuxDesktopProcess> {
    let mut command = Command::new("/proc/self/exe");
    command
        .arg(CHILD)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_clear();
    for (key, value) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if matches!(
            name.as_ref(),
            "HOME"
                | "PATH"
                | "XAUTHORITY"
                | "DISPLAY"
                | "WAYLAND_DISPLAY"
                | "XDG_SESSION_TYPE"
                | "XDG_RUNTIME_DIR"
                | "DBUS_SESSION_BUS_ADDRESS"
                | "LANG"
                | "LC_ALL"
                | "LC_CTYPE"
                | "TMPDIR"
        ) {
            command.env(key, value);
        }
    }
    let prepared = transport::Prepared::packets(&mut command)?;
    let mut child = command
        .spawn()
        .context("Could not start native desktop service")?;
    let group = match i32::try_from(child.id()) {
        Ok(group) => group,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error.into());
        }
    };
    let owner = Owner {
        child: Some(child),
        group,
    };
    let (channel, nonce) = prepared.parent();
    Ok(app_core::LinuxDesktopProcess {
        socket: channel.stream,
        nonce,
        owner: Box::new(owner),
    })
}
pub(crate) fn try_child(arguments: &[OsString]) -> Result<bool> {
    if arguments.first().is_none_or(|value| value != CHILD) {
        return Ok(false);
    }
    ensure!(
        arguments.len() == 1,
        "Invalid private desktop service arguments"
    );
    let (channel, nonce, _parent) = transport::Transport::inherited_packets()?;
    app_core::serve_linux_desktop_child(channel.stream, nonce)
        .map_err(|_| anyhow::anyhow!("Private native desktop service failed"))?;
    Ok(true)
}
