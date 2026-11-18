//! The separate task host shares the main application's fd-driven checked-grab backend.
use super::{Host, Wake};
use anyhow::{Context, Result, ensure};
use std::{
    io::Write,
    os::{fd::AsRawFd, unix::net::UnixStream},
    sync::{Arc, atomic::Ordering},
};
pub(super) struct Native {
    host: Arc<Host>,
    registrar: app_core::X11ShortcutRegistrar,
    keys: Vec<(String, u32, usize)>,
    read: UnixStream,
    write: Arc<UnixStream>,
}
impl Native {
    pub(super) fn new(host: Arc<Host>) -> Result<Self> {
        crate::region_analysis::require_x11(
            std::env::var_os("DISPLAY").as_deref(),
            std::env::var_os("XDG_SESSION_TYPE").as_deref(),
            std::env::var_os("WAYLAND_DISPLAY").as_deref(),
        )?;
        let display = std::env::var("DISPLAY").context("Native task hotkeys require DISPLAY")?;
        let (read, write) = UnixStream::pair()?;
        read.set_nonblocking(true)?;
        write.set_nonblocking(true)?;
        let write = Arc::new(write);
        let pipe = write.clone();
        *host.wake.lock().unwrap() = Some(Arc::new(move || {
            let _ = (&*pipe).write(&[1]);
        }));
        let cancellation = host.clone();
        let registrar = app_core::X11ShortcutRegistrar::open_with_control(
            &display,
            read.try_clone()?,
            Arc::new(move || cancellation.stopping.load(Ordering::Acquire)),
        )?;
        let mut native = Self {
            host,
            registrar,
            keys: Vec::new(),
            read,
            write,
        };
        for key in &native.host.config.keys {
            let shortcut = &native.host.config.bindings[key.profile].hotkey;
            let id = native.registrar.register_shortcut(shortcut)?;
            native.keys.push((shortcut.clone(), id, key.profile));
        }
        Ok(native)
    }
    pub(super) fn wake(&self) -> Wake {
        let pipe = self.write.clone();
        Arc::new(move || {
            let _ = (&*pipe).write(&[1]);
        })
    }
    pub(super) fn run(&mut self) -> Result<()> {
        while !self.host.stopping.load(Ordering::Acquire) {
            let (pressed, full_batch) = self.registrar.drain()?;
            for id in pressed {
                if let Some((_, _, profile)) = self.keys.iter().find(|(_, key, _)| *key == id) {
                    let _ = self.host.trigger(*profile);
                }
            }
            if self.host.stopping.load(Ordering::Acquire) {
                break;
            }
            if full_batch {
                continue;
            }
            let mut fds = [
                libc::pollfd {
                    fd: self.registrar.fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: self.read.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, -1) };
            if result < 0 {
                if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                anyhow::bail!("X11 task shortcut wait failed");
            }
            if fds[1].revents != 0 {
                break;
            }
            ensure!(
                fds[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) == 0,
                "X11 task shortcut connection closed"
            );
        }
        Ok(())
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        for (shortcut, _, _) in &self.keys {
            let _ = self.registrar.unregister_shortcut(shortcut);
        }
    }
}
