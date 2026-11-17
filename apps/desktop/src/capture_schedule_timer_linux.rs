//! Operation-owned absolute wall-clock timerfds. There is no polling timeout.
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    os::fd::{AsRawFd, FromRawFd},
    os::unix::net::UnixStream,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
pub(super) struct Native {
    timer: File,
    alarm: Option<File>,
    read: UnixStream,
    _write: UnixStream,
    handlers: Vec<signal_hook::SigId>,
    stop: Arc<AtomicBool>,
}
fn timer(clock: libc::clockid_t) -> Result<File> {
    let fd = unsafe { libc::timerfd_create(clock, libc::TFD_CLOEXEC | libc::TFD_NONBLOCK) };
    ensure!(
        fd >= 0,
        "Could not create native schedule timer: {}",
        std::io::Error::last_os_error()
    );
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn arm(file: &File, at: Option<i64>) -> Result<()> {
    let mut value: libc::itimerspec = unsafe { std::mem::zeroed() };
    if let Some(ms) = at {
        value.it_value.tv_sec = ms.div_euclid(1000);
        value.it_value.tv_nsec = ms.rem_euclid(1000) * 1000000;
        if ms == 0 {
            value.it_value.tv_nsec = 1;
        } // An all-zero timerfd value means disarm, not Unix epoch.
    }
    let result = unsafe {
        libc::timerfd_settime(
            file.as_raw_fd(),
            libc::TFD_TIMER_ABSTIME | libc::TFD_TIMER_CANCEL_ON_SET,
            &value,
            std::ptr::null_mut(),
        )
    };
    // ECANCELED may report the previous clock change even when this rearm succeeded.
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ECANCELED) {
            return Err(error).context("Could not arm native schedule timer");
        }
    }
    Ok(())
}
impl Native {
    pub fn new(stop: Arc<AtomicBool>) -> Result<Self> {
        let (read, write) = UnixStream::pair()?;
        read.set_nonblocking(true)?;
        write.set_nonblocking(true)?;
        let mut native = Self {
            timer: timer(libc::CLOCK_REALTIME)?,
            alarm: None,
            read,
            _write: write,
            handlers: Vec::new(),
            stop,
        };
        for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
            native
                .handlers
                .push(signal_hook::flag::register(signal, native.stop.clone())?);
            native.handlers.push(signal_hook::low_level::pipe::register(
                signal,
                native._write.try_clone()?,
            )?);
        }
        Ok(native)
    }
    pub fn arm(&mut self, next: Option<i64>, wake: Option<i64>) -> Result<()> {
        arm(&self.timer, next)?;
        if wake.is_some() && self.alarm.is_none() {
            self.alarm=Some(timer(libc::CLOCK_REALTIME_ALARM).context("Explicit schedule wake requires native CAP_WAKE_ALARM; no privilege prompt was requested")?);
        }
        if let Some(alarm) = &self.alarm {
            arm(alarm, wake)?;
        }
        Ok(())
    }
    pub fn wait(&mut self) -> Result<()> {
        if self.stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        let mut fds = [
            libc::pollfd {
                fd: self.read.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.timer.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: self.alarm.as_ref().map_or(-1, AsRawFd::as_raw_fd),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        loop {
            let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
            if result < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    if self.stop.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    continue;
                }
                return Err(error.into());
            }
            ensure!(
                fds.iter()
                    .all(|fd| fd.revents & (libc::POLLNVAL | libc::POLLHUP) == 0),
                "Native schedule timer was closed unexpectedly"
            );
            for fd in &fds[1..] {
                if fd.revents & (libc::POLLIN | libc::POLLERR) != 0 {
                    let mut count = 0u64;
                    let read = unsafe { libc::read(fd.fd, (&mut count as *mut u64).cast(), 8) };
                    if read < 0 {
                        let error = std::io::Error::last_os_error();
                        ensure!(
                            matches!(
                                error.raw_os_error(),
                                Some(libc::ECANCELED) | Some(libc::EAGAIN)
                            ),
                            "Could not read schedule timer: {error}"
                        );
                    }
                }
            }
            return Ok(());
        }
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        for id in self.handlers.drain(..) {
            signal_hook::low_level::unregister(id);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unix_epoch_deadline_is_not_accidentally_disarmed() -> Result<()> {
        let mut timer = Native::new(Arc::new(AtomicBool::new(false)))?;
        timer.arm(Some(0), None)?;
        let mut fd = libc::pollfd {
            fd: timer.timer.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        ensure!(
            unsafe { libc::poll(&mut fd, 1, 100) } == 1 && fd.revents & libc::POLLIN != 0,
            "Unix epoch deadline was interpreted as disarm"
        );
        Ok(())
    }
    #[test]
    fn real_one_shot_rearms_without_a_periodic_wakeup() -> Result<()> {
        let mut timer = Native::new(Arc::new(AtomicBool::new(false)))?;
        let before = std::time::Instant::now();
        timer.arm(Some(jiff::Timestamp::now().as_millisecond() + 40), None)?;
        timer.wait()?;
        ensure!(
            before.elapsed() >= std::time::Duration::from_millis(20),
            "Timer fired before its absolute deadline"
        );
        timer.arm(Some(jiff::Timestamp::now().as_millisecond() + 30), None)?;
        timer.wait()?;
        assert!(timer.alarm.is_none());
        Ok(())
    }
}
