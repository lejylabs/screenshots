//! Portal registrations live in the supervised native helper, including connection/auth startup.
use super::{Host, Wake};
use anyhow::{Result, ensure};
use std::{
    io::Write,
    os::{fd::AsRawFd, unix::net::UnixStream},
    sync::{Arc, atomic::Ordering},
};

pub(super) struct Native {
    host: Arc<Host>,
    portal: app_core::LinuxTaskPortal,
    read: UnixStream,
    write: Arc<UnixStream>,
}
impl Native {
    pub(super) fn new(host: Arc<Host>) -> Result<Self> {
        let (read, write) = UnixStream::pair()?;
        read.set_nonblocking(true)?;
        write.set_nonblocking(true)?;
        let write = Arc::new(write);
        let pipe = write.clone();
        *host.wake.lock().unwrap() = Some(Arc::new(move || {
            let _ = (&*pipe).write(&[1]);
        }));
        let bindings = host
            .config
            .keys
            .iter()
            .map(|key| {
                let binding = &host.config.bindings[key.profile];
                (binding.id.clone(), binding.hotkey.clone())
            })
            .collect();
        let portal = app_core::LinuxTaskPortal::start(bindings, &read, &|| {
            host.stopping.load(Ordering::Acquire)
        })?;
        Ok(Self {
            host,
            portal,
            read,
            write,
        })
    }
    pub(super) fn wake(&self) -> Wake {
        let pipe = self.write.clone();
        Arc::new(move || {
            let _ = (&*pipe).write(&[1]);
        })
    }
    pub(super) fn run(&mut self) -> Result<()> {
        while !self.host.stopping.load(Ordering::Acquire) {
            let (events, full) = self.portal.take_activations()?;
            for id in events {
                if let Some(key) = self
                    .host
                    .config
                    .keys
                    .iter()
                    .find(|key| self.host.config.bindings[key.profile].id == id)
                {
                    let _ = self.host.trigger(key.profile);
                }
            }
            if self.host.stopping.load(Ordering::Acquire) {
                break;
            }
            if full {
                continue;
            }
            let mut fds = [
                libc::pollfd {
                    fd: self.portal.fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: self.read.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, -1) };
            if ready < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                anyhow::bail!("Task portal helper wait failed");
            }
            if fds[1].revents != 0 {
                break;
            }
            ensure!(
                fds[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) == 0,
                "Task portal helper disconnected"
            );
        }
        Ok(())
    }
}
