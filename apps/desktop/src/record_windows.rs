//! Finite Windows recording with parent-enforced encoded-output bounds.

use crate::record::{
    Options, OutputFormat, audio_sample_probe, verify_audio_samples, verify_probe,
};
use anyhow::{Context, Result, ensure};
use app_core::windows_child::{self, Child, Environment, Streams};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle},
    },
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{ERROR_SUCCESS, INVALID_HANDLE_VALUE, LocalFree},
    Security::{
        ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
        },
        DACL_SECURITY_INFORMATION, GetAclInformation, GetSecurityDescriptorControl,
        GetSecurityDescriptorDacl, GetTokenInformation, PSECURITY_DESCRIPTOR, SE_DACL_PROTECTED,
        SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
    },
    Storage::FileSystem::{
        CreateDirectoryW, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, GetVolumeInformationByHandleW, OPEN_EXISTING,
        READ_CONTROL,
    },
    System::{
        SystemServices::FILE_PERSISTENT_ACLS,
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};

const MAX_OUTPUT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_PROBE_BYTES: u64 = 8192;
const MAX_DIAGNOSTIC_BYTES: u64 = 16 * 1024;

struct Engine {
    child: Child,
    job: Option<OwnedHandle>,
    workers: Vec<JoinHandle<()>>,
}
impl Engine {
    fn stop(&mut self) {
        drop(self.job.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(crate) struct Staging {
    directory: PathBuf,
    pub(crate) file: PathBuf,
}

fn private_directory(path: &Path) -> Result<()> {
    let mut token = std::ptr::null_mut();
    ensure!(
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } != 0,
        "Could not identify recording user"
    );
    let token = unsafe { OwnedHandle::from_raw_handle(token) };
    let mut size = 0;
    unsafe {
        GetTokenInformation(
            token.as_raw_handle(),
            TokenUser,
            std::ptr::null_mut(),
            0,
            &mut size,
        );
    }
    ensure!(
        size >= std::mem::size_of::<TOKEN_USER>() as u32 && size <= 65536,
        "Invalid recording user token size"
    );
    let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
    ensure!(
        unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                buffer.as_mut_ptr().cast(),
                size,
                &mut size,
            )
        } != 0,
        "Could not read recording user token"
    );
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut sid = std::ptr::null_mut();
    ensure!(
        unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid) } != 0,
        "Could not read recording user SID"
    );
    let sid_text = (|| -> Result<String> {
        let mut length = 0;
        while length < 256 && unsafe { *sid.add(length) } != 0 {
            length += 1;
        }
        ensure!(length < 256, "Invalid recording user SID");
        Ok(String::from_utf16(unsafe {
            std::slice::from_raw_parts(sid, length)
        })?)
    })();
    unsafe {
        LocalFree(sid.cast());
    }
    let sddl: Vec<u16> = format!("D:P(A;OICI;FA;;;{})", sid_text?)
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut descriptor = std::ptr::null_mut();
    ensure!(
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                std::ptr::null_mut(),
            )
        } != 0,
        "Could not prepare private recording permissions"
    );
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor,
        bInheritHandle: 0,
    };
    let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let created = unsafe { CreateDirectoryW(path.as_ptr(), &attributes) };
    let error = std::io::Error::last_os_error();
    let verification = if created != 0 {
        verify_directory(&path, descriptor)
    } else {
        Err(anyhow::anyhow!(
            "Could not create private recording directory: {error}"
        ))
    };
    unsafe {
        LocalFree(descriptor);
    }
    if verification.is_err() && created != 0 {
        let native = std::ffi::OsString::from_wide(&path[..path.len() - 1]);
        let _ = std::fs::remove_dir(Path::new(&native));
    }
    verification
}

fn acl_bytes(descriptor: PSECURITY_DESCRIPTOR) -> Result<Vec<u8>> {
    let mut present = 0;
    let mut defaulted = 0;
    let mut acl: *mut ACL = std::ptr::null_mut();
    ensure!(
        unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) }
            != 0
            && present != 0
            && !acl.is_null(),
        "Recording staging DACL is missing"
    );
    let mut size = ACL_SIZE_INFORMATION::default();
    ensure!(
        unsafe {
            GetAclInformation(
                acl,
                std::ptr::from_mut(&mut size).cast(),
                std::mem::size_of_val(&size) as u32,
                AclSizeInformation,
            )
        } != 0,
        "Could not inspect recording staging DACL"
    );
    ensure!(
        size.AceCount == 1 && size.AclBytesInUse <= 65536,
        "Recording staging DACL is not restricted to one user"
    );
    Ok(
        unsafe { std::slice::from_raw_parts(acl.cast::<u8>(), size.AclBytesInUse as usize) }
            .to_vec(),
    )
}

fn verify_directory(path: &[u16], expected: PSECURITY_DESCRIPTOR) -> Result<()> {
    let raw = unsafe {
        CreateFileW(
            path.as_ptr(),
            READ_CONTROL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    ensure!(
        raw != INVALID_HANDLE_VALUE,
        "Could not open recording staging directory for security validation"
    );
    let directory = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut flags = 0;
    ensure!(
        unsafe {
            GetVolumeInformationByHandleW(
                directory.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut flags,
                std::ptr::null_mut(),
                0,
            )
        } != 0
            && flags & FILE_PERSISTENT_ACLS != 0,
        "Recording requires a volume with persistent ACLs"
    );
    let mut actual = std::ptr::null_mut();
    ensure!(
        unsafe {
            GetSecurityInfo(
                directory.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut actual,
            )
        } == ERROR_SUCCESS,
        "Could not read recording staging permissions"
    );
    let result = (|| -> Result<()> {
        let mut control = 0;
        let mut revision = 0;
        ensure!(
            unsafe { GetSecurityDescriptorControl(actual, &mut control, &mut revision) } != 0
                && control & SE_DACL_PROTECTED != 0,
            "Recording staging permissions are not protected"
        );
        ensure!(
            acl_bytes(actual)? == acl_bytes(expected)?,
            "Recording staging permissions differ from the current-user-only DACL"
        );
        Ok(())
    })();
    unsafe {
        LocalFree(actual);
    }
    result
}

impl Staging {
    pub(crate) fn new(parent: &Path, format: OutputFormat) -> Result<Self> {
        let directory = parent.join(format!(".screenshots-record-{}", uuid::Uuid::new_v4()));
        private_directory(&directory)?;
        let file = directory.join(if format == OutputFormat::Gif {
            "recording.gif"
        } else {
            "recording.mkv"
        });
        Ok(Self { directory, file })
    }
    pub(crate) fn cleanup(&self) -> Result<()> {
        if let Err(error) = std::fs::remove_file(&self.file) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(error.into());
            }
        }
        if let Err(error) = std::fs::remove_dir(&self.directory) {
            if error.kind() != std::io::ErrorKind::NotFound {
                return Err(error.into());
            }
        }
        Ok(())
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

fn pipe(
    mut input: impl Read,
    limit: u64,
    mut file: Option<File>,
    keep: bool,
) -> Result<Vec<u8>, &'static str> {
    let mut output = Vec::new();
    let mut count = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .map_err(|_| "Recording pipe read failed")?;
        if read == 0 {
            return Ok(output);
        }
        count += read as u64;
        if count > limit {
            return Err("Recording engine output exceeded its limit");
        }
        if let Some(file) = file.as_mut() {
            file.write_all(&buffer[..read])
                .map_err(|_| "Recording staging write failed")?;
        }
        if keep {
            output.extend_from_slice(&buffer[..read]);
        }
    }
}

type Message = (bool, Result<Vec<u8>, &'static str>);

fn await_engine(
    engine: &mut Engine,
    messages: Receiver<Message>,
    deadline: Instant,
    cancel: &AtomicBool,
) -> Result<Vec<u8>> {
    let mut received = 0;
    let mut output = Vec::new();
    loop {
        ensure!(!cancel.load(Ordering::Acquire), "Recording engine canceled");
        ensure!(
            Instant::now() < deadline,
            "Recording exceeded its duration plus 10 second deadline"
        );
        if let Some(status) = engine
            .child
            .try_wait()
            .context("Could not observe recording engine")?
        {
            ensure!(
                status.success(),
                "Recording engine failed; check installed FFmpeg capabilities and capture access"
            );
            if received == 2 {
                engine.stop();
                return Ok(output);
            }
        }
        let pause =
            Duration::from_millis(25).min(deadline.saturating_duration_since(Instant::now()));
        match messages.recv_timeout(pause) {
            Ok((keep, result)) => {
                received += 1;
                let bytes = result.map_err(anyhow::Error::msg)?;
                if keep {
                    output = bytes;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                ensure!(
                    received == 2,
                    "Recording pipe worker ended without completion"
                );
                std::thread::sleep(pause);
            }
        }
    }
}

pub(crate) fn engine(command: Command, stage: Option<&Path>, deadline: Instant) -> Result<Vec<u8>> {
    engine_with_cancel(command, stage, deadline, Arc::new(AtomicBool::new(false)))
}

pub(crate) fn engine_with_cancel(
    command: Command,
    stage: Option<&Path>,
    deadline: Instant,
    cancel: Arc<AtomicBool>,
) -> Result<Vec<u8>> {
    engine_with_identity(command, stage, deadline, cancel, None)
}
pub(crate) fn engine_with_identity(
    mut command: Command,
    stage: Option<&Path>,
    deadline: Instant,
    cancel: Arc<AtomicBool>,
    identity: Option<&app_core::FileIdentity>,
) -> Result<Vec<u8>> {
    ensure!(!cancel.load(Ordering::Acquire), "Recording engine canceled");
    ensure!(
        Instant::now() < deadline,
        "Recording exceeded its duration plus 10 second deadline"
    );
    let file = stage
        .map(|path| OpenOptions::new().create_new(true).write(true).open(path))
        .transpose()?;
    let job = crate::ocr_windows::job_object().context("Could not isolate recording engine")?;
    command.env_remove("FFREPORT");
    let child = windows_child::spawn_with_identity(
        &command,
        job.as_handle(),
        Streams::OUTPUT,
        Environment::Inherit,
        &[],
        identity,
    )
    .map_err(|_| {
        anyhow::anyhow!("Recording engine is unavailable; install FFmpeg and ffprobe separately")
    })?;
    let mut engine = Engine {
        child,
        job: Some(job),
        workers: Vec::with_capacity(2),
    };
    let stdout = engine
        .child
        .stdout
        .take()
        .context("Recording output pipe is unavailable")?;
    let stderr = engine
        .child
        .stderr
        .take()
        .context("Recording diagnostic pipe is unavailable")?;
    let (sender, messages) = mpsc::sync_channel(2);
    let diagnostic_sender = sender.clone();
    engine.workers.push(
        std::thread::Builder::new()
            .name("record-errors".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let _ = diagnostic_sender
                    .send((false, pipe(stderr, MAX_DIAGNOSTIC_BYTES, None, false)));
            })?,
    );
    let probe = stage.is_none();
    engine.workers.push(
        std::thread::Builder::new()
            .name("record-output".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let _ = sender.send((
                    probe,
                    pipe(
                        stdout,
                        if probe {
                            MAX_PROBE_BYTES
                        } else {
                            MAX_OUTPUT_BYTES
                        },
                        file,
                        probe,
                    ),
                ));
            })?,
    );
    ensure!(!cancel.load(Ordering::Acquire), "Recording engine canceled");
    engine
        .child
        .resume(deadline)
        .context("Could not start isolated recording engine")?;
    await_engine(&mut engine, messages, deadline, &cancel)
}

fn encoder(options: &Options<'_>) -> Command {
    let mut command = Command::new("ffmpeg.exe");
    command.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-max_alloc",
        "67108864",
        "-nostdin",
        "-filter_threads",
        "1",
        "-filter_complex_threads",
        "1",
        "-threads",
        "2",
        "-thread_queue_size",
        "1",
    ]);
    if options.mode == "gdigrab" {
        command.args([
            "-f",
            "gdigrab",
            "-framerate",
            &options.fps.to_string(),
            "-video_size",
            &format!("{}x{}", options.width, options.height),
            "-offset_x",
            &options.x.to_string(),
            "-offset_y",
            &options.y.to_string(),
            "-draw_mouse",
            options.draw_mouse(),
            "-i",
            "desktop",
        ]);
    } else {
        command.args([
            "-re",
            "-f",
            "lavfi",
            "-i",
            &format!(
                "testsrc=size={}x{}:rate={}",
                options.width, options.height, options.fps
            ),
        ]);
    }
    if let Some(audio) = options.audio {
        audio.append_input(&mut command);
    }
    command.args(["-map", "0:v:0"]);
    if options.audio.is_some() {
        command.args(["-map", "1:a:0"]);
    } else {
        command.arg("-an");
    }
    command.args([
        "-sn",
        "-dn",
        "-map_metadata",
        "-1",
        "-map_chapters",
        "-1",
        "-frames:v",
        &(options.seconds * options.fps).to_string(),
        "-fps_mode",
        "passthrough",
    ]);
    if options.format == OutputFormat::Matroska {
        command.args([
            "-c:v",
            "ffv1",
            "-level",
            "3",
            "-g",
            "1",
            "-slicecrc",
            "1",
            "-pix_fmt",
            "bgr0",
        ]);
    } else {
        command.args(["-c:v", "gif", "-pix_fmt", "rgb8", "-loop", "-1"]);
    }
    if let Some(audio) = options.audio {
        audio.append_pcm(&mut command, options.seconds, true);
    }
    command.args([
        "-threads",
        "2",
        "-max_muxing_queue_size",
        "1",
        "-f",
        options.format.container(),
        "pipe:1",
    ]);
    command
}

pub(crate) fn record(options: Options<'_>) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(u64::from(options.seconds) + 10);
    let destination = std::path::absolute(options.destination)?;
    let parent = destination
        .parent()
        .context("Recording output has no parent directory")?;
    let staging = Staging::new(parent, options.format)?;
    let result = (|| -> Result<u64> {
        engine(encoder(&options), Some(&staging.file), deadline)?;
        let metadata = staging.file.symlink_metadata()?;
        ensure!(
            metadata.is_file() && metadata.len() >= 6 && metadata.len() <= MAX_OUTPUT_BYTES,
            "Recording output is invalid or exceeds 256 MiB"
        );
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&staging.file)?;
        let mut header = [0; 6];
        file.read_exact(&mut header)?;
        ensure!(
            match options.format {
                OutputFormat::Matroska => header[..4] == [0x1a, 0x45, 0xdf, 0xa3],
                OutputFormat::Gif => &header == b"GIF87a" || &header == b"GIF89a",
            },
            "Recording header does not match its output format"
        );
        let mut probe = Command::new("ffprobe.exe");
        probe.args(["-hide_banner","-loglevel","error","-max_alloc","67108864","-threads","2","-protocol_whitelist","file","-f",options.format.container(),"-max_streams",if options.audio.is_some() { "2" } else { "1" },"-count_frames","-show_entries",if options.audio.is_some() { "stream=codec_name,codec_type,width,height,avg_frame_rate,nb_read_frames,duration,sample_rate,channels,bits_per_sample:format=format_name" } else { "stream=codec_name,codec_type,width,height,avg_frame_rate,nb_read_frames,duration:format=format_name" },"-of","json","-i"]);
        probe.arg(&staging.file);
        verify_probe(&engine(probe, None, deadline)?, &options)?;
        if options.audio.is_some() {
            verify_audio_samples(
                &engine(audio_sample_probe(&staging.file, true), None, deadline)?,
                options.seconds,
            )?;
        }
        file.sync_all()?;
        drop(file);
        ensure!(
            Instant::now() < deadline,
            "Recording exceeded its duration plus 10 second deadline"
        );
        std::fs::hard_link(&staging.file, &destination)
            .context("Could not publish recording without replacing an existing file")?;
        Ok(metadata.len())
    })();
    let cleanup = staging.cleanup();
    let bytes = match (result, cleanup) {
        (Ok(bytes), Ok(())) => bytes,
        (Err(error), Ok(())) => return Err(error),
        (Ok(_), Err(error)) => {
            return Err(error.context("Recording was published, but staging cleanup failed"));
        }
        (Err(error), Err(_)) => {
            return Err(error.context("Recording failed and staging cleanup also failed"));
        }
    };
    let result = options.summary(bytes);
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{result}")
        .and_then(|_| stdout.flush())
        .context("Recording was published, but its result could not be written")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stream_cap_never_writes_the_overflowing_chunk() {
        let root = std::env::temp_dir().join(format!(
            "screenshots-record-stream-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("bounded");
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert!(
            pipe(
                std::io::Cursor::new(vec![1; 65_537]),
                65_536,
                Some(file),
                false
            )
            .is_err()
        );
        assert_eq!(std::fs::metadata(&path).unwrap().len(), 65_536);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn private_staging_is_exclusive_and_cleans_only_its_files() {
        let root =
            std::env::temp_dir().join(format!("screenshots-record-stage-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("preserve"), b"keep").unwrap();
        let staging = Staging::new(&root, OutputFormat::Matroska).unwrap();
        assert!(private_directory(&staging.directory).is_err());
        std::fs::write(&staging.file, b"synthetic").unwrap();
        staging.cleanup().unwrap();
        assert!(!staging.directory.exists());
        assert_eq!(std::fs::read(root.join("preserve")).unwrap(), b"keep");
        std::fs::remove_file(root.join("preserve")).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn encoder_contract_uses_only_fixed_input_and_bounded_output_pipe() {
        for (mode, format) in [
            ("gdigrab", OutputFormat::Matroska),
            ("test", OutputFormat::Gif),
        ] {
            let options = Options {
                mode,
                destination: Path::new("never-used.mkv"),
                seconds: 1,
                fps: 2,
                width: 16,
                height: 12,
                x: 3,
                y: 4,
                format,
                cursor: None,
                audio: None,
            };
            let command = encoder(&options);
            let args = command
                .get_args()
                .map(|arg| arg.to_str().unwrap())
                .collect::<Vec<_>>();
            assert_eq!(args.last(), Some(&"pipe:1"));
            assert!(args.windows(2).any(|pair| pair == ["-frames:v", "2"]));
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["-max_alloc", "67108864"])
            );
            assert!(args.windows(2).any(|pair| pair == ["-c:v", format.codec()]));
            if mode == "gdigrab" {
                assert!(args.windows(2).any(|pair| pair == ["-i", "desktop"]));
            } else {
                assert!(args.windows(2).any(|pair| pair == ["-f", "lavfi"]));
            }
            assert!(!args.iter().any(|arg| arg.contains("never-used")));
        }
    }

    #[test]
    fn gdigrab_cursor_argument_precedes_input_and_default_is_hidden() {
        for cursor in [None, Some(false), Some(true)] {
            let options = Options {
                mode: "gdigrab",
                destination: Path::new("unused.mkv"),
                seconds: 1,
                fps: 2,
                width: 16,
                height: 12,
                x: 0,
                y: 0,
                format: OutputFormat::Matroska,
                cursor,
                audio: None,
            };
            let command = encoder(&options);
            let args = command
                .get_args()
                .map(|v| v.to_str().unwrap())
                .collect::<Vec<_>>();
            let index = args.iter().position(|v| *v == "-draw_mouse").unwrap();
            assert_eq!(
                args[index + 1],
                if cursor == Some(true) { "1" } else { "0" }
            );
            assert!(index < args.iter().position(|v| *v == "-i").unwrap());
        }
    }

    fn fixture(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "record_windows::tests::engine_fixture",
                "--ignored",
                "--nocapture",
                "--quiet",
            ])
            .env("SCREENSHOTS_RECORD_FIXTURE", mode);
        command
    }

    #[test]
    #[ignore = "subprocess fixture used only by isolated recording tests"]
    fn engine_fixture() {
        let Ok(mode) = std::env::var("SCREENSHOTS_RECORD_FIXTURE") else {
            return;
        };
        match mode.as_str() {
            "output" => std::io::stdout().write_all(b"SYNTHETIC RECORDING").unwrap(),
            "probe-overflow" => std::io::stdout()
                .write_all(&vec![b'x'; MAX_PROBE_BYTES as usize + 1])
                .unwrap(),
            "diagnostic-overflow" => std::io::stderr()
                .write_all(&vec![b'x'; MAX_DIAGNOSTIC_BYTES as usize + 1])
                .unwrap(),
            "hold" => std::thread::sleep(Duration::from_secs(60)),
            "failure" => std::process::exit(9),
            "descendant" => {
                let child = fixture("hold").spawn().unwrap();
                std::fs::write(
                    std::env::var_os("SCREENSHOTS_RECORD_PID").unwrap(),
                    child.id().to_string(),
                )
                .unwrap();
                std::thread::sleep(Duration::from_secs(60));
            }
            _ => panic!("unknown fixture mode"),
        }
        std::io::stdout().flush().unwrap();
        std::io::stderr().flush().unwrap();
        std::process::exit(0);
    }

    #[test]
    fn isolated_child_streams_are_bounded_and_failures_are_reported() {
        let deadline = || Instant::now() + Duration::from_secs(5);
        let result = engine(fixture("output"), None, deadline()).unwrap();
        assert!(
            result
                .windows(b"SYNTHETIC RECORDING".len())
                .any(|p| p == b"SYNTHETIC RECORDING")
        );
        for mode in ["probe-overflow", "diagnostic-overflow", "failure"] {
            assert!(engine(fixture(mode), None, deadline()).is_err(), "{mode}");
        }
        let path = std::env::temp_dir().join(format!(
            "screenshots-record-output-{}",
            uuid::Uuid::new_v4()
        ));
        assert!(
            engine(fixture("output"), Some(&path), deadline())
                .unwrap()
                .is_empty()
        );
        assert!(
            std::fs::read(&path)
                .unwrap()
                .windows(b"SYNTHETIC RECORDING".len())
                .any(|p| p == b"SYNTHETIC RECORDING")
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn deadline_terminates_descendants_holding_pipes() {
        use windows_sys::Win32::{
            Foundation::{CloseHandle, WAIT_OBJECT_0},
            System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
        };
        let path =
            std::env::temp_dir().join(format!("screenshots-record-child-{}", uuid::Uuid::new_v4()));
        let mut command = fixture("descendant");
        command.env("SCREENSHOTS_RECORD_PID", &path);
        assert!(engine(command, None, Instant::now() + Duration::from_secs(3)).is_err());
        let pid = std::fs::read_to_string(&path).unwrap().parse().unwrap();
        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if !handle.is_null() {
            assert_eq!(unsafe { WaitForSingleObject(handle, 1000) }, WAIT_OBJECT_0);
            unsafe {
                CloseHandle(handle);
            }
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn external_cancel_terminates_an_already_running_owned_family() {
        let path = std::env::temp_dir().join(format!(
            "screenshots-record-cancel-{}",
            uuid::Uuid::new_v4()
        ));
        let flag = Arc::new(AtomicBool::new(false));
        let observed = path.clone();
        let cancel = flag.clone();
        let trigger = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(5);
            while !observed.exists() && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            cancel.store(true, Ordering::Release);
        });
        let mut command = fixture("descendant");
        command.env("SCREENSHOTS_RECORD_PID", &path);
        let started = Instant::now();
        let error = engine_with_cancel(
            command,
            None,
            Instant::now() + Duration::from_secs(10),
            flag,
        )
        .unwrap_err();
        trigger.join().unwrap();
        assert!(error.to_string().contains("canceled"));
        assert!(started.elapsed() < Duration::from_secs(7));
        let pid = std::fs::read_to_string(&path)
            .expect("descendant started before cancellation")
            .parse()
            .unwrap();
        let raw = unsafe {
            windows_sys::Win32::System::Threading::OpenProcess(
                windows_sys::Win32::System::Threading::PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if !raw.is_null() {
            let process = unsafe { OwnedHandle::from_raw_handle(raw) };
            assert_eq!(
                unsafe {
                    windows_sys::Win32::System::Threading::WaitForSingleObject(
                        process.as_raw_handle(),
                        1000,
                    )
                },
                windows_sys::Win32::Foundation::WAIT_OBJECT_0
            );
        }
        std::fs::remove_file(path).unwrap();
    }
}
