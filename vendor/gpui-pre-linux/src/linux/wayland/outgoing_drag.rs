//! One bounded, cancelable metadata writer per native drag source.
use calloop::{
    Interest, LoopHandle, Mode, PostAction, RegistrationToken,
    generic::Generic,
    timer::{TimeoutAction, Timer},
};
use gpui::FileDragPaths;
use http_client::Url;
use std::{
    cell::RefCell,
    fs::File,
    io::{self, Write},
    os::fd::{AsRawFd, OwnedFd},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

const MAX_REFERENCES: usize = 32;
const MAX_PATH_BYTES: usize = 4096;
const MAX_METADATA_BYTES: usize = 64 * 1024;
const WRITE_TIMEOUT: Duration = Duration::from_secs(4);
const WRITE_CHUNK: usize = 16 * 1024;

/// Reject the entire offer before allocating its bounded wire representation.
pub(super) fn file_uri_list(paths: &FileDragPaths) -> Option<Arc<[u8]>> {
    use std::os::unix::ffi::OsStrExt;

    if paths.entries().is_empty() || paths.entries().len() > MAX_REFERENCES {
        return None;
    }
    let mut native_bytes = 0usize;
    for (path, _) in paths.entries() {
        let bytes = path.as_os_str().as_bytes();
        native_bytes = native_bytes.checked_add(bytes.len())?;
        if !path.is_absolute()
            || bytes.len() > MAX_PATH_BYTES
            || native_bytes > MAX_METADATA_BYTES
            || bytes.contains(&0)
        {
            return None;
        }
    }
    let mut list = Vec::new();
    for (path, _) in paths.entries() {
        let url = Url::from_file_path(path).ok()?;
        // URI expansion is bounded independently of the native path bytes.
        if url.as_str().len() > 12 * 1024
            || list.len().checked_add(url.as_str().len() + 2)? > MAX_METADATA_BYTES
        {
            return None;
        }
        list.reserve_exact(url.as_str().len() + 2);
        list.extend_from_slice(url.as_str().as_bytes());
        list.extend_from_slice(b"\r\n");
    }
    Some(list.into())
}

#[derive(Default)]
struct Tokens {
    writer: Option<RegistrationToken>,
    deadline: Option<RegistrationToken>,
}

/// Removing both sources releases their FD and shared bytes even if never writable.
pub(super) struct Writer {
    handle: LoopHandle<'static, ()>,
    tokens: Rc<RefCell<Tokens>>,
}

impl Writer {
    pub(super) fn start(
        handle: &LoopHandle<'static, ()>,
        fd: OwnedFd,
        bytes: Arc<[u8]>,
    ) -> io::Result<Self> {
        Self::start_until(handle, fd, bytes, Instant::now() + WRITE_TIMEOUT)
    }

    fn start_until(
        handle: &LoopHandle<'static, ()>,
        fd: OwnedFd,
        bytes: Arc<[u8]>,
        deadline: Instant,
    ) -> io::Result<Self> {
        if bytes.is_empty() || bytes.len() > MAX_METADATA_BYTES {
            return Err(io::Error::other("Invalid native drag metadata extent"));
        }
        // The compositor owns the other pipe end. Never trust its FD flags.
        let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
        {
            return Err(io::Error::last_os_error());
        }
        let writer = Self {
            handle: handle.clone(),
            tokens: Rc::new(RefCell::new(Tokens::default())),
        };
        let tokens = writer.tokens.clone();
        let weak_handle = handle.downgrade();
        let mut written = 0usize;
        let token = handle
            .insert_source(
                Generic::new(File::from(fd), Interest::WRITE, Mode::Level),
                move |_, file, _| {
                    let result = if Instant::now() >= deadline {
                        None
                    } else {
                        let end = (written + WRITE_CHUNK).min(bytes.len());
                        // One nonblocking bounded write per readiness dispatch.
                        Some(unsafe { file.get_mut() }.write(&bytes[written..end]))
                    };
                    let finished = match result {
                        Some(Ok(0)) | None => true,
                        Some(Ok(count)) => {
                            written += count;
                            written == bytes.len()
                        }
                        Some(Err(error))
                            if matches!(
                                error.kind(),
                                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                            ) =>
                        {
                            false
                        }
                        Some(Err(_)) => true,
                    };
                    if !finished {
                        return Ok(PostAction::Continue);
                    }
                    let mut tokens = tokens.borrow_mut();
                    tokens.writer.take();
                    if let Some(token) = tokens.deadline.take()
                        && let Some(handle) = weak_handle.upgrade()
                    {
                        handle.remove(token);
                    }
                    Ok(PostAction::Remove)
                },
            )
            .map_err(|_| io::Error::other("Could not register native drag writer"))?;
        writer.tokens.borrow_mut().writer = Some(token);
        let tokens = writer.tokens.clone();
        let weak_handle = handle.downgrade();
        let token = handle
            .insert_source(Timer::from_deadline(deadline), move |_, _, _| {
                let mut tokens = tokens.borrow_mut();
                tokens.deadline.take();
                if let Some(token) = tokens.writer.take()
                    && let Some(handle) = weak_handle.upgrade()
                {
                    handle.remove(token);
                }
                TimeoutAction::Drop
            })
            .map_err(|_| io::Error::other("Could not register native drag deadline"))?;
        writer.tokens.borrow_mut().deadline = Some(token);
        Ok(writer)
    }

    pub(super) fn is_active(&self) -> bool {
        self.tokens.borrow().writer.is_some()
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let mut tokens = self.tokens.borrow_mut();
        if let Some(token) = tokens.writer.take() {
            self.handle.remove(token);
        }
        if let Some(token) = tokens.deadline.take() {
            self.handle.remove(token);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Read, os::unix::net::UnixStream, path::PathBuf};

    #[test]
    fn native_drag_metadata_is_all_or_nothing_bounded_and_byte_exact() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        let paths = FileDragPaths::new([
            (PathBuf::from("/private/not-opened/a b"), false),
            (
                PathBuf::from(OsString::from_vec(b"/private/\xff".to_vec())),
                false,
            ),
        ]);
        assert_eq!(
            &*file_uri_list(&paths).unwrap(),
            b"file:///private/not-opened/a%20b\r\nfile:///private/%FF\r\n"
        );
        for paths in [
            FileDragPaths::default(),
            FileDragPaths::new([(PathBuf::from("relative"), false)]),
            FileDragPaths::new([
                (PathBuf::from("/private/valid"), false),
                (PathBuf::from("relative"), false),
            ]),
            FileDragPaths::new([(PathBuf::from("/private/nul\0path"), false)]),
            FileDragPaths::new((0..33).map(|_| (PathBuf::from("/private/path"), false))),
            FileDragPaths::new([(PathBuf::from(format!("/{}", "x".repeat(4096))), false)]),
            FileDragPaths::new(
                (0..32).map(|_| (PathBuf::from(format!("/{}", "x".repeat(3000))), false)),
            ),
            FileDragPaths::new(
                (0..8).map(|_| (PathBuf::from(format!("/{}", " ".repeat(3000))), false)),
            ),
        ] {
            assert!(file_uri_list(&paths).is_none());
        }
    }

    #[test]
    fn owned_pipe_writer_completes_and_releases_shared_body() {
        let mut event_loop = calloop::EventLoop::<()>::try_new().unwrap();
        let (mut peer, output) = UnixStream::pair().unwrap();
        peer.set_nonblocking(true).unwrap();
        let bytes: Arc<[u8]> = Arc::from(&b"private-synthetic-metadata"[..]);
        let weak = Arc::downgrade(&bytes);
        let writer = Writer::start(&event_loop.handle(), output.into(), bytes).unwrap();
        event_loop.dispatch(Duration::ZERO, &mut ()).unwrap();
        assert!(!writer.is_active());
        assert!(weak.upgrade().is_none());
        let mut actual = Vec::new();
        peer.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, b"private-synthetic-metadata");
    }

    #[test]
    fn owned_pipe_writer_cancel_closes_fd_without_a_readiness_event() {
        let event_loop = calloop::EventLoop::<()>::try_new().unwrap();
        let (mut peer, output) = UnixStream::pair().unwrap();
        peer.set_nonblocking(true).unwrap();
        let bytes: Arc<[u8]> = Arc::from(&b"private-synthetic-metadata"[..]);
        let weak = Arc::downgrade(&bytes);
        let writer = Writer::start(&event_loop.handle(), output.into(), bytes).unwrap();
        drop(writer);
        assert!(weak.upgrade().is_none());
        assert_eq!(peer.read(&mut [0; 1]).unwrap(), 0);
    }

    #[test]
    fn owned_pipe_writer_has_an_absolute_stall_deadline_and_one_body() {
        let mut event_loop = calloop::EventLoop::<()>::try_new().unwrap();
        let (mut peer, output) = UnixStream::pair().unwrap();
        output.set_nonblocking(true).unwrap();
        peer.set_nonblocking(true).unwrap();
        let mut filler = &output;
        while filler.write(&[0; 4096]).is_ok() {}
        let bytes: Arc<[u8]> = Arc::from(&b"private-synthetic-metadata"[..]);
        let weak = Arc::downgrade(&bytes);
        let writer =
            Writer::start_until(&event_loop.handle(), output.into(), bytes, Instant::now())
                .unwrap();
        event_loop.dispatch(Duration::ZERO, &mut ()).unwrap();
        assert!(!writer.is_active());
        assert!(weak.upgrade().is_none());
        let mut received = Vec::new();
        peer.read_to_end(&mut received).unwrap();
        assert!(
            !received
                .windows(b"private-synthetic-metadata".len())
                .any(|bytes| bytes == b"private-synthetic-metadata")
        );
    }

    #[test]
    fn owned_pipe_writer_rejects_closed_peer_and_oversized_metadata() {
        let mut event_loop = calloop::EventLoop::<()>::try_new().unwrap();
        let (peer, output) = UnixStream::pair().unwrap();
        drop(peer);
        let writer = Writer::start(
            &event_loop.handle(),
            output.into(),
            Arc::from(&b"private"[..]),
        )
        .unwrap();
        event_loop.dispatch(Duration::ZERO, &mut ()).unwrap();
        assert!(!writer.is_active());
        let (_peer, output) = UnixStream::pair().unwrap();
        assert!(
            Writer::start(
                &event_loop.handle(),
                output.into(),
                vec![0; MAX_METADATA_BYTES + 1].into(),
            )
            .is_err()
        );
    }
}
