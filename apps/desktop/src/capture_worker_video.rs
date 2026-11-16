//! Finite raw-video side channel within the existing native job/admission family.
use super::{SharedJob, lock, protocol, transport};
use anyhow::{Context, Result, ensure};
use app_core::{CaptureError, MacVideoRequest, NativeCaptureCall, NativeCaptureReply};
#[cfg(any(test, target_os = "macos"))]
use std::io::Write;
use std::{
    fs::File,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{net::UnixStream, process::CommandExt},
    },
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
    },
    time::{Duration, Instant},
};
const ENV: &str = "SCREENSHOTS_NATIVE_VIDEO_PIPE";
pub(crate) type Peers = Box<
    dyn FnOnce(
            i32,
            File,
            Arc<AtomicBool>,
            Instant,
            SyncSender<Result<(), CaptureError>>,
        ) -> Result<(), CaptureError>
        + Send,
>;
pub(crate) struct Plan {
    pub request: MacVideoRequest,
    peers: Option<Peers>,
    reader: Option<UnixStream>,
    writer: Option<UnixStream>,
}
impl Plan {
    pub fn new(request: MacVideoRequest, peers: Peers) -> Result<Self, CaptureError> {
        request
            .validate()
            .map_err(|_| CaptureError::InvalidRegion)?;
        Ok(Self {
            request,
            peers: Some(peers),
            reader: None,
            writer: None,
        })
    }
    pub fn prepare(&mut self, command: &mut Command, nonce: &[u8; 16]) -> Result<()> {
        let (reader, writer) = UnixStream::pair()?;
        writer.set_nonblocking(true)?;
        for stream in [&reader, &writer] {
            for option in [libc::SO_SNDBUF, libc::SO_RCVBUF] {
                let size = 128 * 1024i32;
                ensure!(
                    unsafe {
                        libc::setsockopt(
                            stream.as_raw_fd(),
                            libc::SOL_SOCKET,
                            option,
                            (&size as *const i32).cast(),
                            std::mem::size_of_val(&size) as libc::socklen_t,
                        )
                    } == 0,
                    "Cannot bound raw video queue"
                );
                let mut actual = 0i32;
                let mut length = std::mem::size_of_val(&actual) as libc::socklen_t;
                ensure!(
                    unsafe {
                        libc::getsockopt(
                            stream.as_raw_fd(),
                            libc::SOL_SOCKET,
                            option,
                            (&mut actual as *mut i32).cast(),
                            &mut length,
                        )
                    } == 0
                        && actual > 0
                        && actual <= 256 * 1024,
                    "Raw video queue exceeds its bound"
                );
            }
        }
        let fd = writer.as_raw_fd();
        let token = uuid::Uuid::from_bytes(*nonce);
        command.env(ENV, format!("v1:{fd}:{}:{token}", std::process::id()));
        unsafe {
            command.pre_exec(move || {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        self.reader = Some(reader);
        self.writer = Some(writer);
        Ok(())
    }
    pub fn after_launch(&mut self) {
        self.writer.take();
    }
    pub fn exchange(
        mut self,
        io: &mut transport::ActiveIo<'_>,
        nonce: &[u8; 16],
        job: &SharedJob,
        cancel: Arc<AtomicBool>,
        deadline: Instant,
    ) -> Result<NativeCaptureReply> {
        let call = NativeCaptureCall::MacVideo {
            request: self.request,
        };
        protocol::handshake(io, protocol::HELLO, nonce)?;
        protocol::expect_handshake(io, protocol::ACK, nonce)?;
        protocol::request(io, &call)?;
        protocol::expect_handshake(io, protocol::VIDEO_READY, nonce)?;
        let group = lock(job)
            .as_ref()
            .context("Native video group unavailable")?
            .group;
        let reader = self.reader.take().context("Raw video reader unavailable")?;
        let input: OwnedFd = reader.into();
        let input = File::from(input);
        let peers = self.peers.take().context("Video peers already consumed")?;
        let (ready_tx, ready) = mpsc::sync_channel(1);
        let (done_tx, done) = mpsc::sync_channel(1);
        let aborted = cancel.clone();
        let peer = std::thread::Builder::new()
            .name("mac-video-peers".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let result = peers(group, input, aborted.clone(), deadline, ready_tx.clone());
                if result.is_err() {
                    aborted.store(true, Ordering::Release);
                    let _ = ready_tx.try_send(Err(CaptureError::Backend(
                        "Recording peer startup failed".into(),
                    )));
                }
                let _ = done_tx.send(result);
            })?;
        let result = (|| -> Result<_> {
            receive(&ready, &cancel, deadline)?;
            protocol::handshake(io, protocol::VIDEO_START, nonce)?;
            let reply = protocol::read_reply_with_eof(io, &call, false)?;
            call.validate_reply(&reply)?;
            receive(&done, &cancel, deadline)?;
            protocol::handshake(io, protocol::VIDEO_FINISH, nonce)?;
            protocol::expect_eof(io)?;
            Ok(reply)
        })();
        if result.is_err() {
            cancel.store(true, Ordering::Release);
            if let Some(job) = lock(job).as_mut() {
                job.stop();
            }
        }
        // This coordinator remains owned/pending if a peer is kernel-stalled.
        peer.join()
            .map_err(|_| anyhow::anyhow!("Recording peer coordinator failed"))?;
        result
    }
}
fn receive<T>(
    receiver: &Receiver<Result<T, CaptureError>>,
    cancel: &AtomicBool,
    deadline: Instant,
) -> Result<T> {
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(CaptureError::NativeCanceled.into());
        }
        if Instant::now() >= deadline {
            return Err(CaptureError::NativeTimeout.into());
        }
        match receiver.recv_timeout(
            Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
        ) {
            Ok(result) => return result.map_err(Into::into),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                anyhow::bail!("Recording peer ended without completion")
            }
        }
    }
}
pub(crate) fn execute(
    plan: Plan,
    check: &app_core::NativeCaptureCheck<'_>,
) -> Result<app_core::MacVideoSummary, CaptureError> {
    let call = NativeCaptureCall::MacVideo {
        request: plan.request,
    };
    let timeout = Duration::from_secs(u64::from(plan.request.seconds) + 10);
    let reply = super::execute_operation(
        || {
            super::command()
                .map_err(|_| CaptureError::Backend("Native video helper unavailable".into()))
        },
        call,
        check,
        timeout,
        super::Operation::Video(plan),
    )?;
    let NativeCaptureReply::MacVideo { summary } = reply else {
        return Err(CaptureError::Backend(
            "Native video returned wrong result".into(),
        ));
    };
    Ok(summary)
}

pub(super) fn inherited_sink(nonce: &[u8; 16], parent: u32) -> Result<UnixStream> {
    let value = std::env::var(ENV).context("Raw video sink capability is absent")?;
    ensure!(value.len() <= 256, "Invalid raw video sink capability");
    let fields = value.split(':').collect::<Vec<_>>();
    ensure!(
        fields.len() == 4 && fields[0] == "v1",
        "Invalid raw video sink capability"
    );
    let fd = fields[1].parse::<i32>()?;
    let owner = fields[2].parse::<u32>()?;
    let token = uuid::Uuid::parse_str(fields[3])?;
    ensure!(
        fd >= 3 && owner == parent && token.as_bytes() == nonce,
        "Raw video sink ownership mismatch"
    );
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    ensure!(
        flags >= 0 && unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } == 0,
        "Cannot protect raw video sink"
    );
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    let mut kind = 0i32;
    let mut length = std::mem::size_of_val(&kind) as libc::socklen_t;
    ensure!(
        unsafe { libc::fstat(fd, &mut stat) } == 0
            && stat.st_mode & libc::S_IFMT == libc::S_IFSOCK
            && unsafe {
                libc::getsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_TYPE,
                    (&mut kind as *mut i32).cast(),
                    &mut length,
                )
            } == 0
            && kind == libc::SOCK_STREAM,
        "Raw video sink is not a stream socket"
    );
    let mut address: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of_val(&address) as libc::socklen_t;
    ensure!(
        unsafe {
            libc::getpeername(
                fd,
                (&mut address as *mut libc::sockaddr_storage).cast(),
                &mut length,
            )
        } == 0
            && address.ss_family as i32 == libc::AF_UNIX,
        "Raw video sink is not local/connected"
    );
    let sink = unsafe { UnixStream::from_raw_fd(fd) };
    sink.set_nonblocking(true)?;
    Ok(sink)
}

#[cfg(test)]
pub(super) fn fixture(
    request: MacVideoRequest,
    io: &mut transport::ActiveIo<'_>,
    nonce: &[u8; 16],
    parent: u32,
    mode: &str,
) -> Result<()> {
    ensure!(
        request.frame_bytes()? <= 4096,
        "Private raw fixture is deliberately tiny"
    );
    let mut sink = inherited_sink(nonce, parent)?;
    protocol::handshake(io, protocol::VIDEO_READY, nonce)?;
    protocol::expect_handshake(io, protocol::VIDEO_START, nonce)?;
    if mode == "video_hang" {
        loop {
            std::thread::park();
        }
    }
    let expected = request.frames()?;
    let delivered = if mode == "video_truncated" {
        expected - 1
    } else {
        expected
    };
    for index in 0..delivered {
        let pixel = match index % 3 {
            0 => [0, 0, 255, 255],
            1 => [0, 255, 0, 255],
            _ => [255, 0, 0, 255],
        };
        let mut frame = vec![0u8; request.frame_bytes()?];
        for sample in frame.chunks_exact_mut(4) {
            sample.copy_from_slice(&pixel);
        }
        sink.write_all(&frame)?;
    }
    drop(sink);
    let summary = app_core::MacVideoSummary {
        frames: expected,
        first_pts_ns: 0,
        last_pts_ns: u64::from(expected - 1) * 1_000_000_000 / u64::from(request.fps),
    };
    protocol::reply(io, NativeCaptureReply::MacVideo { summary })?;
    protocol::expect_handshake(io, protocol::VIDEO_FINISH, nonce)?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub(super) fn serve(
    request: MacVideoRequest,
    io: &mut transport::ActiveIo<'_>,
    nonce: &[u8; 16],
    parent: u32,
) -> Result<()> {
    request.validate()?;
    let mut sink = inherited_sink(nonce, parent)?;
    let proof = match app_core::preflight_macos_video_source_in_worker(request) {
        Ok(proof) => proof,
        Err(error) => {
            let error = error
                .downcast::<CaptureError>()
                .unwrap_or_else(|_| CaptureError::Backend("Native video preflight failed".into()));
            protocol::failure(io, &error)?;
            return Ok(());
        }
    };
    protocol::handshake(io, protocol::VIDEO_READY, nonce)?;
    protocol::expect_handshake(io, protocol::VIDEO_START, nonce)?;
    let deadline = io.deadline;
    let result =
        app_core::run_macos_video_source_in_worker(request, proof, deadline, move |bytes| {
            let mut left = bytes;
            while !left.is_empty() {
                ensure!(
                    Instant::now() < deadline,
                    "Raw video write deadline exceeded"
                );
                match sink.write(&left[..left.len().min(64 * 1024)]) {
                    Ok(0) => anyhow::bail!("Raw video sink closed"),
                    Ok(count) => left = &left[count..],
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        let mut event = libc::pollfd {
                            fd: sink.as_raw_fd(),
                            events: libc::POLLOUT,
                            revents: 0,
                        };
                        let millis = deadline
                            .saturating_duration_since(Instant::now())
                            .as_millis()
                            .min(i32::MAX as u128) as i32;
                        ensure!(
                            unsafe { libc::poll(&mut event, 1, millis) } > 0,
                            "Raw video sink wait failed/timed out"
                        );
                    }
                    Err(_) => anyhow::bail!("Raw video sink failed"),
                }
            }
            Ok(())
        });
    let summary = match result {
        Ok(summary) => summary,
        Err(error) => {
            let error = error
                .downcast::<CaptureError>()
                .unwrap_or_else(|_| CaptureError::Backend("Native video source failed".into()));
            protocol::failure(io, &error)?;
            return Ok(());
        }
    };
    // The source and writer have stopped. Keep the group leader until every peer
    // flush/probe/reap phase acknowledges completion; ordinary cleanup then applies.
    protocol::reply(io, NativeCaptureReply::MacVideo { summary })?;
    protocol::expect_handshake(io, protocol::VIDEO_FINISH, nonce)?;
    Ok(())
}
