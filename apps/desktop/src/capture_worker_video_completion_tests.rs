//! Private owned processes only: keep the parent alive to make NOTE_EXIT impossible.
use super::*;
use crate::capture_worker::{Job, launch, private_environment, unix};
use std::{
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, ExitStatus, Stdio},
};

struct Folder(PathBuf);
impl Folder {
    fn new() -> Result<Self> {
        let path =
            std::env::temp_dir().join(format!("screenshots-video-eof-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Family {
    source: Job,
    peers: Vec<Child>,
}
impl Drop for Family {
    fn drop(&mut self) {
        // The source is deliberately unreaped until all group signaling is over.
        self.source.stop();
        for peer in &mut self.peers {
            let _ = peer.kill();
            let _ = peer.wait();
        }
        self.source.wait();
    }
}
#[derive(Clone, Copy, Debug)]
enum Ending {
    BeforeStart,
    BeforeFinish,
    TypedFailure,
}
fn details(pid: i32) -> String {
    #[cfg(target_os = "macos")]
    {
        format!("{:?}", unix::process_state(pid))
    }
    #[cfg(target_os = "linux")]
    {
        format!("pid={pid}")
    }
}
fn ready(path: &Path, pid: u32, source: i32) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if std::fs::read_to_string(path)
            .ok()
            .and_then(|s| s.parse().ok())
            == Some(pid)
        {
            return Ok(());
        }
        ensure!(
            Instant::now() < deadline,
            "Private peer did not start: source={} peer={}",
            details(source),
            details(pid as i32)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn child_exit(child: &mut Child) -> Result<ExitStatus> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        ensure!(
            Instant::now() < deadline,
            "Private encoder did not finish: {}",
            details(child.id() as i32)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn control_ends_before_finish(ending: Ending) -> Result<()> {
    let folder = Folder::new()?;
    let request = MacVideoRequest {
        display: 7,
        region: app_core::PixelRegion {
            x: 0,
            y: 0,
            width: 2,
            height: 2,
        },
        seconds: 1,
        fps: 3,
        cursor: false,
        microphone: false,
    };
    let mut plan = Plan::new(request, Box::new(|_, _, _, _, _| Ok(())))?;
    let call = NativeCaptureCall::MacVideo { request };
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args([
            "--exact",
            "capture_worker::tests::worker_fixture",
            "--nocapture",
        ])
        .env(
            "SCREENSHOTS_CAPTURE_WORKER_FIXTURE",
            if matches!(ending, Ending::TypedFailure) {
                "video_failure"
            } else {
                "video"
            },
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    private_environment(&mut command);
    let prepared = transport::Prepared::new(&mut command)?;
    plan.prepare(&mut command, &prepared.nonce)?;
    let source = launch(&mut command, &prepared)?;
    let mut family = Family {
        source,
        peers: Vec::with_capacity(2),
    };
    plan.after_launch();
    let (transport, nonce) = prepared.parent();
    let mut io = transport::ActiveIo {
        transport,
        deadline: Instant::now() + Duration::from_secs(5),
        check: &|| Ok(()),
    };
    protocol::handshake(&mut io, protocol::HELLO, &nonce)?;
    protocol::expect_handshake(&mut io, protocol::ACK, &nonce)?;
    protocol::request(&mut io, &call)?;
    protocol::expect_handshake(&mut io, protocol::VIDEO_READY, &nonce)?;
    let group = family.source.group;
    let reader: OwnedFd = plan
        .reader
        .take()
        .context("Private raw input missing")?
        .into();
    let encoder_marker = folder.0.join("encoder");
    let mut encoder = Command::new("python3");
    encoder
        .args(["-c", "import os,sys,time,pathlib; assert os.getpgrp()==int(sys.argv[2]); pathlib.Path(sys.argv[1]).write_text(str(os.getpid())); data=sys.stdin.buffer.read(); time.sleep(30) if sys.argv[3]=='hold' else None; assert len(data)==48"])
        .arg(&encoder_marker)
        .arg(group.to_string())
        .arg(if matches!(ending, Ending::BeforeStart) { "hold" } else { "finish" })
        .process_group(group)
        .stdin(File::from(reader))
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    family.peers.push(encoder.spawn()?);
    ready(&encoder_marker, family.peers[0].id(), group)?;
    match ending {
        Ending::BeforeStart => {}
        Ending::BeforeFinish => {
            protocol::handshake(&mut io, protocol::VIDEO_START, &nonce)?;
            let reply = protocol::read_reply_with_eof(&mut io, &call, false)?;
            ensure!(
                matches!(reply, NativeCaptureReply::MacVideo { summary } if summary.frames == 3),
                "Wrong private source summary"
            );
            ensure!(
                child_exit(&mut family.peers[0])?.success(),
                "Private encoder failed"
            );
            let probe_marker = folder.0.join("probe");
            let mut probe = Command::new("python3");
            probe
                .args(["-c", "import os,sys,time,pathlib; assert os.getpgrp()==int(sys.argv[2]); pathlib.Path(sys.argv[1]).write_text(str(os.getpid())); time.sleep(30)"])
                .arg(&probe_marker)
                .arg(group.to_string())
                .process_group(group)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            family.peers.push(probe.spawn()?);
            ready(&probe_marker, family.peers[1].id(), group)?;
        }
        Ending::TypedFailure => {
            protocol::handshake(&mut io, protocol::VIDEO_START, &nonce)?;
            let failure = match protocol::read_reply_with_eof(&mut io, &call, false) {
                Err(error) => error,
                Ok(_) => panic!("Private video unexpectedly succeeded"),
            };
            ensure!(
                matches!(
                    failure.downcast_ref::<CaptureError>(),
                    Some(CaptureError::MacScreenPermissionDenied)
                ),
                "Private source lost its typed error: {failure}"
            );
        }
    }
    let initial_source = details(group);
    let initial_peers = family
        .peers
        .iter()
        .map(|p| details(p.id() as i32))
        .collect::<Vec<_>>();
    // Parent stays alive, and no Job::stop/reaped/wait method runs during observation.
    drop(io);
    let deadline = Instant::now() + Duration::from_secs(5);
    let ended = loop {
        let source_ended = unix::exited_without_reaping(group, false)?;
        let mut peers_ended = true;
        for peer in &mut family.peers {
            peers_ended &= peer.try_wait()?.is_some();
        }
        if source_ended && peers_ended {
            break true;
        }
        if Instant::now() >= deadline {
            break false;
        }
        family.source.wait_ready(Duration::from_millis(5));
        // Keep peer observation bounded after the source exit event has been consumed.
        if source_ended {
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let final_source = details(group);
    let final_peers = family
        .peers
        .iter()
        .map(|p| details(p.id() as i32))
        .collect::<Vec<_>>();
    // Capture evidence first, then clean even a failing fixture before panicking.
    drop(family);
    ensure!(
        ended,
        "Private video family survived unexpected control end: ending={ending:?} parent={} source_initial={initial_source} peers_initial={initial_peers:?} source_final={final_source} peers_final={final_peers:?}",
        std::process::id()
    );
    Ok(())
}
#[test]
fn eof_before_start_stops_source_and_encoder_without_parent_cleanup() -> Result<()> {
    control_ends_before_finish(Ending::BeforeStart)
}
#[test]
fn eof_before_finish_stops_source_and_probe_without_parent_cleanup() -> Result<()> {
    control_ends_before_finish(Ending::BeforeFinish)
}
#[test]
fn typed_video_failure_stops_its_family_without_losing_the_error() -> Result<()> {
    control_ends_before_finish(Ending::TypedFailure)
}
