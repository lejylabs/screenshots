//! Group identity stays reserved by the unreaped leader until signaling is finished.
use std::{
    io::{self, Read},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    time::{Duration, Instant},
};
const INSPECTION_TIME: Duration = Duration::from_millis(50);
const MAX_MEMBERS: usize = 1024;

pub(super) fn exited_without_reaping(pid: i32, blocking: bool) -> io::Result<bool> {
    let mut information: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let options = libc::WEXITED | libc::WNOWAIT | if blocking { 0 } else { libc::WNOHANG };
    loop {
        if unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut information, options) } == 0 {
            return Ok(unsafe { information.si_pid() } == pid);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}
pub(super) fn signal_owned_group(pid: i32) {
    // Success, including a still-running child, proves this child remains waitable.
    if exited_without_reaping(pid, false).is_ok() {
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
}

#[cfg(target_os = "linux")]
pub(super) fn live_members(group: i32) -> io::Result<Vec<i32>> {
    let until = Instant::now() + INSPECTION_TIME;
    let mut members = Vec::new();
    let mut bytes = [0u8; 4096];
    let mut total = 0usize;
    for (index, entry) in std::fs::read_dir("/proc")?.enumerate() {
        if index >= 65536 || total > 8 * 1024 * 1024 || Instant::now() >= until {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|value| value.parse::<i32>().ok())
        else {
            continue;
        };
        if pid == group {
            continue;
        }
        // Most /proc entries cannot belong to this operation. Query the native group first,
        // then read bounded state only for matches (and verify the group again after opening).
        let actual = unsafe { libc::getpgid(pid) };
        if actual < 0 {
            if io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                continue;
            }
            return Err(io::Error::last_os_error());
        }
        if actual != group {
            continue;
        }
        let mut file = match std::fs::File::open(entry.path().join("stat")) {
            Ok(file) => file,
            Err(error)
                if matches!(error.kind(), io::ErrorKind::NotFound)
                    || error.raw_os_error() == Some(libc::ESRCH) =>
            {
                continue;
            }
            Err(error) => return Err(error),
        };
        let length = match file.read(&mut bytes) {
            Ok(length) => length,
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => continue,
            Err(error) => return Err(error),
        };
        total += length;
        if length == bytes.len() {
            return Err(io::ErrorKind::InvalidData.into());
        }
        if length == 0 {
            continue;
        }
        let end = bytes[..length]
            .iter()
            .rposition(|byte| *byte == b')')
            .ok_or(io::ErrorKind::InvalidData)?;
        let fields =
            std::str::from_utf8(&bytes[end + 1..length]).map_err(|_| io::ErrorKind::InvalidData)?;
        let mut fields = fields.split_whitespace();
        let state = fields.next().ok_or(io::ErrorKind::InvalidData)?;
        let _parent = fields.next().ok_or(io::ErrorKind::InvalidData)?;
        let actual = fields
            .next()
            .ok_or(io::ErrorKind::InvalidData)?
            .parse::<i32>()
            .map_err(|_| io::ErrorKind::InvalidData)?;
        if actual == group && !matches!(state, "Z" | "X" | "x") {
            if members.len() == MAX_MEMBERS {
                return Err(io::ErrorKind::OutOfMemory.into());
            }
            members.push(pid);
        }
    }
    Ok(members)
}

#[cfg(target_os = "macos")]
#[link(name = "proc")]
unsafe extern "C" {
    fn proc_listpgrppids(
        group: libc::pid_t,
        buffer: *mut libc::c_void,
        length: libc::c_int,
    ) -> libc::c_int;
    fn proc_pidinfo(
        pid: libc::c_int,
        flavor: libc::c_int,
        argument: u64,
        buffer: *mut libc::c_void,
        length: libc::c_int,
    ) -> libc::c_int;
}
#[cfg(target_os = "macos")]
pub(super) fn live_members(group: i32) -> io::Result<Vec<i32>> {
    let until = Instant::now() + INSPECTION_TIME;
    let mut pids = [0i32; MAX_MEMBERS + 2];
    let capacity = std::mem::size_of_val(&pids);
    let count = unsafe { proc_listpgrppids(group, pids.as_mut_ptr().cast(), capacity as i32) };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    // libproc's convenience wrapper returns a PID count, unlike proc_listpids' byte count.
    if count == 0 || count as usize >= pids.len() || !pids[..count as usize].contains(&group) {
        return Err(io::ErrorKind::InvalidData.into());
    }
    let mut members = Vec::new();
    for pid in pids
        .into_iter()
        .take(count as usize)
        .filter(|pid| *pid > 0 && *pid != group)
    {
        if Instant::now() >= until {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let expected = std::mem::size_of_val(&info);
        let count = unsafe {
            proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut info as *mut libc::proc_bsdinfo).cast(),
                expected as i32,
            )
        };
        if count != expected as i32 {
            if io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                continue;
            }
            return Err(io::ErrorKind::Other.into());
        }
        if info.pbi_pgid == group as u32 && info.pbi_status != libc::SZOMB {
            if members.len() == MAX_MEMBERS {
                return Err(io::ErrorKind::OutOfMemory.into());
            }
            members.push(pid);
        }
    }
    Ok(members)
}

#[cfg(all(test, target_os = "macos"))]
#[derive(Debug)]
pub(super) struct ProcessState {
    pub requested: i32,
    pub count: i32,
    pub expected: i32,
    pub errno: Option<i32>,
    pub pid: Option<u32>,
    pub parent: Option<u32>,
    pub group: Option<u32>,
    pub status: Option<u32>,
    pub flags: Option<u32>,
    pub started: Option<(u64, u64)>,
}
#[cfg(all(test, target_os = "macos"))]
impl ProcessState {
    pub fn live(&self) -> bool {
        if self.count == self.expected {
            self.status != Some(libc::SZOMB)
        } else {
            self.errno != Some(libc::ESRCH)
        }
    }
}
#[cfg(all(test, target_os = "macos"))]
pub(super) fn process_state(pid: i32) -> ProcessState {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let expected = std::mem::size_of_val(&info) as i32;
    let count = unsafe {
        proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            expected,
        )
    };
    let errno = if count == expected {
        None
    } else {
        std::io::Error::last_os_error().raw_os_error()
    };
    let valid = count == expected;
    ProcessState {
        requested: pid,
        count,
        expected,
        errno,
        pid: valid.then_some(info.pbi_pid),
        parent: valid.then_some(info.pbi_ppid),
        group: valid.then_some(info.pbi_pgid),
        status: valid.then_some(info.pbi_status),
        flags: valid.then_some(info.pbi_flags),
        started: valid.then_some((info.pbi_start_tvsec, info.pbi_start_tvusec)),
    }
}
#[cfg(all(test, target_os = "macos"))]
pub(super) fn process_live(pid: i32) -> bool {
    process_state(pid).live()
}

/// Block on exit readiness, never on a periodic pending-cleanup timer.
pub(super) fn wait_members(group: i32, members: &[i32]) -> bool {
    let until = Instant::now() + INSPECTION_TIME;
    #[cfg(target_os = "linux")]
    {
        let mut descriptors = Vec::with_capacity(members.len());
        for pid in members {
            if Instant::now() >= until {
                return false;
            }
            let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, *pid, 0) as i32 };
            if descriptor < 0 {
                if io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                    return true;
                }
                return false;
            }
            descriptors.push(unsafe { OwnedFd::from_raw_fd(descriptor) });
            // A member PID may exit/reuse between inspection and pidfd registration.
            // Reinspect instead of waiting on a new process outside the owned group.
            if unsafe { libc::getpgid(*pid) } != group {
                return true;
            }
        }
        let mut events = descriptors
            .iter()
            .map(|descriptor| libc::pollfd {
                fd: descriptor.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            })
            .collect::<Vec<_>>();
        if events.is_empty() {
            return true;
        }
        loop {
            let result = unsafe { libc::poll(events.as_mut_ptr(), events.len() as _, -1) };
            if result >= 0 {
                return true;
            }
            if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                return false;
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let descriptor = unsafe { libc::kqueue() };
        if descriptor < 0 {
            return false;
        }
        let owned = unsafe { OwnedFd::from_raw_fd(descriptor) };
        if unsafe { libc::fcntl(owned.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
            return false;
        }
        for pid in members {
            if Instant::now() >= until {
                return false;
            }
            let event = libc::kevent {
                ident: *pid as usize,
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
                return io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
            }
            let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
            let expected = std::mem::size_of_val(&info);
            if unsafe {
                proc_pidinfo(
                    *pid,
                    libc::PROC_PIDTBSDINFO,
                    0,
                    (&mut info as *mut libc::proc_bsdinfo).cast(),
                    expected as i32,
                )
            } != expected as i32
                || info.pbi_pgid != group as u32
            {
                return true;
            }
        }
        if members.is_empty() {
            return true;
        }
        let mut event: libc::kevent = unsafe { std::mem::zeroed() };
        loop {
            let result = unsafe {
                libc::kevent(
                    owned.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    &mut event,
                    1,
                    std::ptr::null(),
                )
            };
            if result >= 0 {
                return true;
            }
            if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
                return false;
            }
        }
    }
}

pub(super) fn terminate_own_group() -> ! {
    unsafe {
        let pid = libc::getpid();
        if libc::getpgrp() == pid {
            libc::kill(-pid, libc::SIGKILL);
        }
        libc::_exit(1);
    }
}
#[cfg(target_os = "linux")]
unsafe extern "C" fn parent_lost(_: libc::c_int) {
    terminate_own_group();
}
pub(super) fn arm_parent_death(parent: u32) -> io::Result<()> {
    if unsafe { libc::getpgrp() } != unsafe { libc::getpid() } {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    #[cfg(target_os = "linux")]
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = parent_lost as *const () as usize;
        libc::sigemptyset(&mut action.sa_mask);
        if libc::sigaction(libc::SIGUSR1, &action, std::ptr::null_mut()) != 0
            || libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGUSR1) != 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    if unsafe { libc::getppid() } != parent as i32 {
        terminate_own_group();
    }
    Ok(())
}
