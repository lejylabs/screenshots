//! Optional finite Unix recording through installed FFmpeg/ffprobe executables.

use anyhow::{Context, Result, bail, ensure};
use std::{ffi::OsString, path::Path};

struct Options<'a> {
    mode: &'a str,
    destination: &'a Path,
    seconds: u32,
    fps: u32,
    width: u32,
    height: u32,
    x: u32,
    y: u32,
}

fn options(args: &[OsString]) -> Result<Options<'_>> {
    ensure!(
        matches!(args.len(), 10 | 14),
        "Usage: screenshots record x11|test OUTPUT.mkv --seconds N --fps N --width N --height N [--x X --y Y]"
    );
    let mode = args[0].to_str().context("Recording mode must be UTF-8")?;
    ensure!(
        matches!(mode, "x11" | "test"),
        "Choose recording mode x11 or test"
    );
    let destination = Path::new(&args[1]);
    ensure!(
        destination
            .extension()
            .and_then(|extension| extension.to_str())
            == Some("mkv"),
        "Recording output must have a .mkv extension"
    );
    ensure!(
        destination
            .symlink_metadata()
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
        "Recording output already exists or cannot be inspected"
    );
    let mut values = [None; 6];
    for pair in args[2..].chunks_exact(2) {
        let index = match pair[0].to_str() {
            Some("--seconds") => 0,
            Some("--fps") => 1,
            Some("--width") => 2,
            Some("--height") => 3,
            Some("--x") => 4,
            Some("--y") => 5,
            _ => bail!("Unknown recording option; use --help"),
        };
        ensure!(values[index].is_none(), "Duplicate recording option");
        let value = pair[1]
            .to_str()
            .context("Recording option must be UTF-8")?
            .parse::<u32>()
            .context("Recording options must be nonnegative whole numbers")?;
        values[index] = Some(value);
    }
    let [seconds, fps, width, height, x, y] = values;
    let seconds = seconds.context("Missing --seconds")?;
    let fps = fps.context("Missing --fps")?;
    let width = width.context("Missing --width")?;
    let height = height.context("Missing --height")?;
    ensure!(
        (1..=60).contains(&seconds),
        "Recording seconds must be 1–60"
    );
    ensure!((1..=30).contains(&fps), "Recording FPS must be 1–30");
    ensure!(
        (1..=1920).contains(&width),
        "Recording width must be 1–1920"
    );
    ensure!(
        (1..=1080).contains(&height),
        "Recording height must be 1–1080"
    );
    app_core::validate_image_size(width, height)?;
    ensure!(x.is_some() == y.is_some(), "Use --x and --y together");
    ensure!(
        mode == "x11" || x.is_none(),
        "Offsets apply only to x11 recording"
    );
    let (x, y) = (x.unwrap_or(0), y.unwrap_or(0));
    ensure!(
        x.checked_add(width)
            .is_some_and(|end| end <= i32::MAX as u32)
            && y.checked_add(height)
                .is_some_and(|end| end <= i32::MAX as u32),
        "Recording rectangle coordinates overflow"
    );
    Ok(Options {
        mode,
        destination,
        seconds,
        fps,
        width,
        height,
        x,
        y,
    })
}

fn local_display(value: &str) -> Result<&str> {
    let valid_number =
        |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    let rest = value
        .strip_prefix(':')
        .context("X11 DISPLAY must be local :NUMBER[.SCREEN]")?;
    let (number, screen) = rest
        .split_once('.')
        .map_or((rest, None), |(a, b)| (a, Some(b)));
    ensure!(
        value.len() <= 64 && valid_number(number) && screen.is_none_or(valid_number),
        "X11 DISPLAY must be local :NUMBER[.SCREEN]"
    );
    Ok(value)
}

pub fn run(args: &[OsString]) -> Result<()> {
    let options = options(args)?;
    #[cfg(not(unix))]
    {
        let _ = options;
        bail!("Recording is currently supported on Unix only");
    }
    #[cfg(unix)]
    unix::record(options)
}

#[cfg(unix)]
mod unix {
    use super::*;
    use serde::Deserialize;
    use std::{
        fs::{DirBuilder, File},
        io::Read,
        os::unix::{fs::DirBuilderExt, process::CommandExt},
        path::PathBuf,
        process::{Child, Command, Stdio},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc::{self, Receiver, RecvTimeoutError},
        },
        thread::JoinHandle,
        time::{Duration, Instant},
    };

    const MAX_OUTPUT_BYTES: u64 = 256 * 1024 * 1024;
    const MAX_PROBE_BYTES: usize = 8192;
    const MAX_DIAGNOSTIC_BYTES: usize = 16 * 1024;
    const CHECK_INTERVAL: Duration = Duration::from_millis(25);

    struct Cancellation {
        flag: Arc<AtomicBool>,
        finished: Arc<AtomicBool>,
        handlers: Vec<signal_hook::SigId>,
    }

    impl Cancellation {
        fn new() -> Result<Self> {
            let mut result = Self {
                flag: Arc::new(AtomicBool::new(false)),
                finished: Arc::new(AtomicBool::new(false)),
                handlers: Vec::with_capacity(2),
            };
            for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
                // Keep termination available after cleanup, including blocked result output.
                signal_hook::flag::register_conditional_shutdown(
                    signal,
                    1,
                    result.finished.clone(),
                )?;
                result
                    .handlers
                    .push(signal_hook::flag::register(signal, result.flag.clone())?);
            }
            Ok(result)
        }

        fn check(&self, deadline: Instant) -> Result<()> {
            ensure!(!self.flag.load(Ordering::Relaxed), "Recording canceled");
            ensure!(
                Instant::now() < deadline,
                "Recording exceeded its duration plus 10 second deadline"
            );
            Ok(())
        }
    }

    impl Drop for Cancellation {
        fn drop(&mut self) {
            self.finished.store(true, Ordering::SeqCst);
            for handler in self.handlers.drain(..) {
                signal_hook::low_level::unregister(handler);
            }
        }
    }

    struct Staging {
        directory: PathBuf,
        file: PathBuf,
    }

    impl Staging {
        fn new(parent: &Path) -> Result<Self> {
            let directory = parent.join(format!(".screenshots-record-{}", uuid::Uuid::new_v4()));
            DirBuilder::new().mode(0o700).create(&directory)?;
            let file = directory.join("recording.mkv");
            Ok(Self { directory, file })
        }

        fn cleanup(&self) -> Result<()> {
            match std::fs::remove_file(&self.file) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            match std::fs::remove_dir(&self.directory) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            }
        }
    }

    impl Drop for Staging {
        fn drop(&mut self) {
            let _ = self.cleanup();
        }
    }

    struct Engine {
        child: Child,
        group: i32,
        workers: Vec<JoinHandle<()>>,
        finished: bool,
    }

    impl Engine {
        fn stop(&mut self) {
            unsafe {
                libc::kill(-self.group, libc::SIGKILL);
            }
            let _ = self.child.kill();
            let _ = self.child.wait();
            for worker in self.workers.drain(..) {
                let _ = worker.join();
            }
            self.finished = true;
        }
    }

    impl Drop for Engine {
        fn drop(&mut self) {
            if !self.finished {
                self.stop();
            }
        }
    }

    fn read_pipe(mut pipe: impl Read, limit: usize, keep: bool) -> Result<Vec<u8>, &'static str> {
        let mut bytes = Vec::new();
        let mut total = 0;
        let mut buffer = [0; 4096];
        loop {
            let count = pipe
                .read(&mut buffer)
                .map_err(|_| "Recording engine pipe failed")?;
            if count == 0 {
                return Ok(bytes);
            }
            total += count;
            if total > limit {
                return Err("Recording engine output exceeded its limit");
            }
            if keep {
                bytes.extend_from_slice(&buffer[..count]);
            }
        }
    }

    type PipeResult = (bool, Result<Vec<u8>, &'static str>);

    fn wait_engine(
        job: &mut Engine,
        messages: Receiver<PipeResult>,
        expected: usize,
        cancellation: &Cancellation,
        deadline: Instant,
    ) -> Result<Vec<u8>> {
        let mut received = 0;
        let mut output = Vec::new();
        loop {
            cancellation.check(deadline)?;
            if let Some(status) = job
                .child
                .try_wait()
                .context("Could not observe recording engine")?
            {
                ensure!(
                    status.success(),
                    "Recording engine failed; check installed FFmpeg capabilities and capture access"
                );
                if received == expected {
                    job.stop();
                    return Ok(output);
                }
            }
            let pause = CHECK_INTERVAL.min(deadline.saturating_duration_since(Instant::now()));
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
                        received == expected,
                        "Recording pipe worker ended without completion"
                    );
                    std::thread::sleep(pause);
                }
            }
        }
    }

    fn engine(
        mut command: Command,
        probe: bool,
        cancellation: &Cancellation,
        deadline: Instant,
    ) -> Result<Vec<u8>> {
        cancellation.check(deadline)?;
        command
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .process_group(0);
        if probe {
            command.stdout(Stdio::piped());
        } else {
            command.stdout(Stdio::null());
        }
        command.env_remove("FFREPORT");
        // Bound file growth in the child; a cap hit cannot be mistaken for a full recording.
        unsafe {
            command.pre_exec(|| {
                #[cfg(target_os = "linux")]
                {
                    let parent = libc::getppid();
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::getppid() != parent {
                        return Err(std::io::Error::other("Recording parent exited"));
                    }
                }
                let limit = libc::rlimit {
                    rlim_cur: MAX_OUTPUT_BYTES as libc::rlim_t,
                    rlim_max: MAX_OUTPUT_BYTES as libc::rlim_t,
                };
                if libc::setrlimit(libc::RLIMIT_FSIZE, &limit) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let no_core = libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                };
                if libc::setrlimit(libc::RLIMIT_CORE, &no_core) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().map_err(|_| {
            anyhow::anyhow!(
                "Recording engine is unavailable; install FFmpeg and ffprobe separately"
            )
        })?;
        let group = match i32::try_from(child.id()) {
            Ok(group) => group,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("Recording engine process ID is invalid");
            }
        };
        let mut job = Engine {
            child,
            group,
            workers: Vec::with_capacity(2),
            finished: false,
        };
        let stderr = job
            .child
            .stderr
            .take()
            .context("Recording diagnostic pipe is unavailable")?;
        let (sender, messages) = mpsc::sync_channel(2);
        let diagnostics = sender.clone();
        job.workers.push(
            std::thread::Builder::new()
                .name("record-errors".into())
                .stack_size(256 * 1024)
                .spawn(move || {
                    let _ =
                        diagnostics.send((false, read_pipe(stderr, MAX_DIAGNOSTIC_BYTES, false)));
                })?,
        );
        if probe {
            let stdout = job
                .child
                .stdout
                .take()
                .context("Recording probe pipe is unavailable")?;
            job.workers.push(
                std::thread::Builder::new()
                    .name("record-probe".into())
                    .stack_size(256 * 1024)
                    .spawn(move || {
                        let _ = sender.send((true, read_pipe(stdout, MAX_PROBE_BYTES, true)));
                    })?,
            );
        } else {
            drop(sender);
        }
        wait_engine(
            &mut job,
            messages,
            if probe { 2 } else { 1 },
            cancellation,
            deadline,
        )
    }

    fn encoder(options: &Options<'_>, stage: &Path, display: Option<&str>) -> Command {
        let mut command = Command::new("ffmpeg");
        command.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-max_alloc",
            "67108864",
            "-nostdin",
            "-n",
            "-filter_threads",
            "1",
            "-filter_complex_threads",
            "1",
            "-threads",
            "2",
            "-thread_queue_size",
            "1",
        ]);
        if let Some(display) = display {
            command.args([
                "-f",
                "x11grab",
                "-framerate",
                &options.fps.to_string(),
                "-video_size",
                &format!("{}x{}", options.width, options.height),
                "-draw_mouse",
                "0",
                "-i",
                &format!("{display}+{},{}", options.x, options.y),
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
        command.args([
            "-map",
            "0:v:0",
            "-an",
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
            "-threads",
            "2",
            "-max_muxing_queue_size",
            "1",
            "-f",
            "matroska",
        ]);
        command.arg(stage);
        command
    }

    #[derive(Deserialize)]
    struct Probe {
        streams: Vec<Stream>,
        format: Format,
    }

    #[derive(Deserialize)]
    struct Format {
        format_name: String,
    }

    #[derive(Deserialize)]
    struct Stream {
        codec_name: String,
        codec_type: String,
        width: u32,
        height: u32,
        avg_frame_rate: String,
        nb_read_frames: String,
    }

    fn verify_probe(bytes: &[u8], options: &Options<'_>) -> Result<()> {
        let probe: Probe =
            serde_json::from_slice(bytes).context("Invalid recording verification result")?;
        ensure!(
            probe
                .format
                .format_name
                .split(',')
                .any(|name| name == "matroska"),
            "Recording container verification failed"
        );
        ensure!(
            probe.streams.len() == 1,
            "Recording must contain exactly one video stream"
        );
        let stream = &probe.streams[0];
        ensure!(
            stream.codec_name == "ffv1" && stream.codec_type == "video",
            "Recording codec verification failed"
        );
        ensure!(
            stream.width == options.width && stream.height == options.height,
            "Recording dimensions do not match the request"
        );
        ensure!(
            stream.avg_frame_rate == format!("{}/1", options.fps),
            "Recording frame rate does not match the request"
        );
        ensure!(
            stream.nb_read_frames.parse::<u32>().ok() == Some(options.seconds * options.fps),
            "Recording is incomplete: frame count does not match the request"
        );
        Ok(())
    }

    pub(super) fn record(options: Options<'_>) -> Result<()> {
        let display = if options.mode == "x11" {
            let display =
                std::env::var("DISPLAY").context("X11 recording requires local DISPLAY")?;
            Some(local_display(&display)?.to_owned())
        } else {
            None
        };
        let cancellation = Cancellation::new()?;
        let deadline = Instant::now() + Duration::from_secs(u64::from(options.seconds) + 10);
        let destination = std::path::absolute(options.destination)?;
        let parent = destination
            .parent()
            .context("Recording output has no parent directory")?;
        let staging = Staging::new(parent)?;
        let result = (|| {
            engine(
                encoder(&options, &staging.file, display.as_deref()),
                false,
                &cancellation,
                deadline,
            )?;
            let metadata = staging.file.symlink_metadata()?;
            ensure!(
                metadata.is_file() && metadata.len() >= 4 && metadata.len() <= MAX_OUTPUT_BYTES,
                "Recording output is invalid or exceeds 256 MiB"
            );
            let mut file = File::open(&staging.file)?;
            let mut header = [0; 4];
            file.read_exact(&mut header)?;
            ensure!(
                header == [0x1a, 0x45, 0xdf, 0xa3],
                "Recording is not Matroska"
            );
            let mut probe = Command::new("ffprobe");
            probe.args(["-hide_banner", "-loglevel", "error", "-max_alloc", "67108864", "-threads", "2", "-protocol_whitelist", "file", "-f", "matroska", "-max_streams", "1", "-count_frames", "-show_entries", "stream=codec_name,codec_type,width,height,avg_frame_rate,nb_read_frames:format=format_name", "-of", "json", "-i"]);
            probe.arg(&staging.file);
            verify_probe(&engine(probe, true, &cancellation, deadline)?, &options)?;
            file.sync_all()?;
            cancellation.check(deadline)?;
            std::fs::hard_link(&staging.file, &destination)
                .context("Could not publish recording without replacing an existing file")?;
            File::open(parent)?.sync_all().context(
                "Recording was published, but directory durability could not be confirmed",
            )?;
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
        drop(cancellation);
        println!(
            "{}",
            serde_json::json!({
                "mode": options.mode, "container": "matroska", "codec": "ffv1",
                "seconds": options.seconds, "fps": options.fps,
                "width": options.width, "height": options.height,
                "frames": options.seconds * options.fps, "bytes": bytes,
            })
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_admission_only_accepts_local_numeric_x11_displays() {
        for value in [":0", ":1.0", ":123.4"] {
            assert_eq!(local_display(value).unwrap(), value);
        }
        for value in [
            "",
            "host:0",
            "localhost:0",
            "unix:0",
            ":",
            ":1.",
            ":1.2.3",
            ":0+1,2",
            ":0\n",
            ":-1",
            ":a",
        ] {
            assert!(local_display(value).is_err(), "{value}");
        }
        assert!(local_display(&format!(":{}", "1".repeat(64))).is_err());
    }
}
