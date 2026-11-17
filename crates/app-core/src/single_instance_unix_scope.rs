//! Startup-only graphical authority inspection. Never consumes or mutates environment.
use super::*;

pub(super) fn current() -> Result<Option<Vec<u8>>> {
    #[cfg(target_os = "linux")]
    {
        if let Some(value) = std::env::var_os("WAYLAND_SOCKET") {
            let fd = value
                .to_str()
                .and_then(|value| value.parse::<RawFd>().ok())
                .filter(|fd| *fd >= 0)
                .context("WAYLAND_SOCKET is not a valid inherited descriptor")?;
            let mut kind = 0;
            let mut size = std::mem::size_of_val(&kind) as libc::socklen_t;
            ensure!(
                unsafe {
                    libc::getsockopt(
                        fd,
                        libc::SOL_SOCKET,
                        libc::SO_TYPE,
                        (&mut kind as *mut i32).cast(),
                        &mut size,
                    )
                } == 0
                    && kind == libc::SOCK_STREAM,
                "WAYLAND_SOCKET is not an available stream descriptor"
            );
            return wayland_peer(fd);
        }
        if let Some(display) = std::env::var_os("WAYLAND_DISPLAY").filter(|value| !value.is_empty())
        {
            let path = PathBuf::from(display);
            let path = if path.is_absolute() {
                path
            } else {
                let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                else {
                    return Ok(None);
                };
                runtime.join(path)
            };
            let Ok(stream) = connect(&path, Instant::now() + Duration::from_millis(250)) else {
                return Ok(None);
            };
            return wayland_peer(stream.as_raw_fd());
        }
        Ok(std::env::var_os("DISPLAY").and_then(|value| x11_scope(value.as_bytes())))
    }
    #[cfg(target_os = "macos")]
    {
        // Apple's security-session ID is stable for the graphical login, across processes.
        #[link(name = "Security", kind = "framework")]
        unsafe extern "C" {
            fn SessionGetInfo(session: u32, session_id: *mut u32, attributes: *mut u32) -> i32;
        }
        let mut session = 0;
        let mut attributes = 0;
        // callerSecuritySession is (SecuritySessionId)-1 in Security/AuthSession.h.
        if unsafe { SessionGetInfo(u32::MAX, &mut session, &mut attributes) } != 0
            || session == 0
            || session == u32::MAX
        {
            return Ok(None);
        }
        Ok(Some(format!("macos-audit-session:{session}").into_bytes()))
    }
}

#[cfg(target_os = "linux")]
pub(super) fn peer_credentials(fd: RawFd) -> Result<libc::ucred> {
    let mut value: libc::ucred = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of_val(&value) as libc::socklen_t;
    ensure!(
        unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut value as *mut libc::ucred).cast(),
                &mut size,
            )
        } == 0
            && size as usize == std::mem::size_of_val(&value),
        "Could not authenticate local socket peer"
    );
    Ok(value)
}

#[cfg(target_os = "linux")]
pub(super) fn wayland_peer(fd: RawFd) -> Result<Option<Vec<u8>>> {
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let mut size = std::mem::size_of_val(&address) as libc::socklen_t;
    ensure!(
        unsafe {
            libc::getpeername(
                fd,
                (&mut address as *mut libc::sockaddr_un).cast(),
                &mut size,
            )
        } == 0
            && address.sun_family as i32 == libc::AF_UNIX,
        "Wayland authority must be a connected local Unix socket"
    );
    if let Ok(peer) = peer_credentials(fd) {
        if peer.pid > 0 {
            if let Some((namespace_device, namespace_inode, start)) = process_identity(peer.pid) {
                // Bind PID to its namespace and lifetime, not the inherited FD number.
                return Ok(Some(
                    format!(
                        "wayland-peer:{}:{}:{namespace_device}:{namespace_inode}:{start}",
                        peer.uid, peer.pid
                    )
                    .into_bytes(),
                ));
            }
        }
    }
    // A second identity scheme could split one server into competing owners.
    // Missing peer/PID-namespace/lifetime evidence therefore stays visible-only.
    Ok(None)
}

#[cfg(target_os = "linux")]
fn process_identity(pid: libc::pid_t) -> Option<(u64, u64, u64)> {
    let namespace = std::fs::metadata("/proc/self/ns/pid").ok()?;
    let mut bytes = [0; 4097];
    let length = File::open(format!("/proc/{pid}/stat"))
        .ok()?
        .read(&mut bytes)
        .ok()?;
    let start = process_starttime(&bytes[..length])?;
    Some((namespace.dev(), namespace.ino(), start))
}

#[cfg(any(test, target_os = "linux"))]
pub(super) fn process_starttime(bytes: &[u8]) -> Option<u64> {
    if bytes.len() > 4096 {
        return None;
    }
    let end = bytes.iter().rposition(|byte| *byte == b')')?;
    let tail = std::str::from_utf8(bytes.get(end + 1..)?).ok()?;
    tail.split_ascii_whitespace()
        .nth(19)?
        .parse::<u64>()
        .ok()
        .filter(|start| *start != 0)
}

#[cfg(any(test, target_os = "linux"))]
pub(super) fn x11_scope(display: &[u8]) -> Option<Vec<u8>> {
    if display.is_empty()
        || display.len() > 4096
        || display.iter().any(|byte| byte.is_ascii_control())
    {
        return None;
    }
    let value = std::str::from_utf8(display).ok()?;
    let (host, number) = value.rsplit_once(':')?;
    let number = match number.split_once('.') {
        Some((number, screen))
            if !screen.is_empty() && screen.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            number
        }
        Some(_) => return None,
        None => number,
    };
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let number: u32 = number.parse().ok()?;
    let host = host.to_ascii_lowercase();
    let host = if matches!(
        host.as_str(),
        "" | "unix" | "unix/" | "localhost" | "localhost/unix"
    ) {
        ""
    } else {
        &host
    };
    Some(format!("x11-server:{host}:{number}").into_bytes())
}
