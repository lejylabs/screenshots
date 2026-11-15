//! Optional finite recording through installed FFmpeg/ffprobe executables.

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use std::{ffi::OsString, path::Path};

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum OutputFormat {
    Matroska,
    Gif,
}

impl OutputFormat {
    pub(crate) fn container(self) -> &'static str {
        match self {
            Self::Matroska => "matroska",
            Self::Gif => "gif",
        }
    }

    pub(crate) fn codec(self) -> &'static str {
        match self {
            Self::Matroska => "ffv1",
            Self::Gif => "gif",
        }
    }
}

pub(crate) struct Options<'a> {
    pub(crate) mode: &'a str,
    pub(crate) destination: &'a Path,
    pub(crate) seconds: u32,
    pub(crate) fps: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) format: OutputFormat,
    pub(crate) cursor: Option<bool>,
}

impl Options<'_> {
    pub(crate) fn draw_mouse(&self) -> &'static str {
        if self.cursor == Some(true) { "1" } else { "0" }
    }

    pub(crate) fn summary(&self, bytes: u64) -> serde_json::Value {
        let mut result = serde_json::json!({
            "mode":self.mode,"container":self.format.container(),"codec":self.format.codec(),
            "seconds":self.seconds,"fps":self.fps,"width":self.width,"height":self.height,
            "frames":self.seconds*self.fps,"bytes":bytes,
        });
        if let Some(show) = self.cursor {
            result["cursor"] = if show { "show" } else { "hide" }.into();
        }
        result
    }
}

fn options(args: &[OsString]) -> Result<Options<'_>> {
    ensure!(
        matches!(args.len(), 10 | 12 | 14 | 16),
        "Usage: screenshots record MODE OUTPUT.mkv|OUTPUT.gif --seconds N --fps N --width N --height N [--x X --y Y] [--cursor show|hide]"
    );
    let mode = args[0].to_str().context("Recording mode must be UTF-8")?;
    ensure!(
        mode == "test"
            || (cfg!(unix) && mode == "x11")
            || (cfg!(target_os = "windows") && mode == "gdigrab"),
        "Choose recording mode x11/test on Unix or gdigrab/test on Windows"
    );
    let destination = Path::new(&args[1]);
    let format = match destination
        .extension()
        .and_then(|extension| extension.to_str())
    {
        Some("mkv") => OutputFormat::Matroska,
        Some("gif") => OutputFormat::Gif,
        _ => bail!("Recording output must have a .mkv or .gif extension"),
    };
    ensure!(
        destination
            .symlink_metadata()
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound),
        "Recording output already exists or cannot be inspected"
    );
    let mut values = [None; 6];
    let mut cursor = None;
    for pair in args[2..].chunks_exact(2) {
        if pair[0] == "--cursor" {
            ensure!(cursor.is_none(), "Duplicate --cursor option");
            cursor = Some(match pair[1].to_str() {
                Some("show") => true,
                Some("hide") => false,
                _ => bail!("Cursor must be show or hide"),
            });
            continue;
        }
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
    if format == OutputFormat::Gif {
        ensure!(
            seconds <= 30 && fps <= 15 && width <= 1280 && height <= 720,
            "GIF limits are 30 seconds, 15 FPS and 1280×720 pixels"
        );
    }
    app_core::validate_image_size(width, height)?;
    ensure!(x.is_some() == y.is_some(), "Use --x and --y together");
    ensure!(
        matches!(mode, "x11" | "gdigrab") || x.is_none(),
        "Offsets apply only to desktop recording"
    );
    ensure!(
        mode != "test" || cursor.is_none(),
        "Cursor option applies only to desktop recording"
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
        format,
        cursor,
    })
}

#[cfg(any(unix, test))]
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
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        let _ = options;
        bail!("Recording is currently supported on Unix and Windows only");
    }
    #[cfg(target_os = "windows")]
    return crate::record_windows::record(options);
    #[cfg(unix)]
    unix::record(options)
}

#[cfg(unix)]
mod unix {
    use super::*;
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
        fn new(parent: &Path, format: OutputFormat) -> Result<Self> {
            let directory = parent.join(format!(".screenshots-record-{}", uuid::Uuid::new_v4()));
            DirBuilder::new().mode(0o700).create(&directory)?;
            let file = directory.join(if format == OutputFormat::Gif {
                "recording.gif"
            } else {
                "recording.mkv"
            });
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
        #[cfg(target_os = "linux")]
        let controller_pid = std::process::id() as libc::pid_t;
        // Bound file growth in the child; a cap hit cannot be mistaken for a full recording.
        unsafe {
            command.pre_exec(move || {
                #[cfg(target_os = "linux")]
                {
                    if libc::getppid() != controller_pid {
                        return Err(std::io::Error::other("Recording parent exited"));
                    }
                    if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    if libc::getppid() != controller_pid {
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

    pub(super) fn encoder(options: &Options<'_>, stage: &Path, display: Option<&str>) -> Command {
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
                options.draw_mouse(),
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
            // RGB8 uses a fixed small palette; no whole-video palette generation pass.
            command.args(["-c:v", "gif", "-pix_fmt", "rgb8", "-loop", "-1"]);
        }
        command.args([
            "-threads",
            "2",
            "-max_muxing_queue_size",
            "1",
            "-f",
            options.format.container(),
        ]);
        command.arg(stage);
        command
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
        let staging = Staging::new(parent, options.format)?;
        let result = (|| {
            engine(
                encoder(&options, &staging.file, display.as_deref()),
                false,
                &cancellation,
                deadline,
            )?;
            let metadata = staging.file.symlink_metadata()?;
            ensure!(
                metadata.is_file() && metadata.len() >= 6 && metadata.len() <= MAX_OUTPUT_BYTES,
                "Recording output is invalid or exceeds 256 MiB"
            );
            let mut file = File::open(&staging.file)?;
            let mut header = [0; 6];
            file.read_exact(&mut header)?;
            ensure!(
                match options.format {
                    OutputFormat::Matroska => header[..4] == [0x1a, 0x45, 0xdf, 0xa3],
                    OutputFormat::Gif => &header == b"GIF87a" || &header == b"GIF89a",
                },
                "Recording header does not match its output format"
            );
            let mut probe = Command::new("ffprobe");
            probe.args(["-hide_banner", "-loglevel", "error", "-max_alloc", "67108864", "-threads", "2", "-protocol_whitelist", "file", "-f", options.format.container(), "-max_streams", "1", "-count_frames", "-show_entries", "stream=codec_name,codec_type,width,height,avg_frame_rate,nb_read_frames,duration:format=format_name", "-of", "json", "-i"]);
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
        println!("{}", options.summary(bytes));
        Ok(())
    }
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
    duration: Option<String>,
}

pub(crate) fn verify_probe(bytes: &[u8], options: &Options<'_>) -> Result<()> {
    let probe: Probe =
        serde_json::from_slice(bytes).context("Invalid recording verification result")?;
    ensure!(
        probe
            .format
            .format_name
            .split(',')
            .any(|name| name == options.format.container()),
        "Recording container verification failed"
    );
    ensure!(
        probe.streams.len() == 1,
        "Recording must contain exactly one video stream"
    );
    let stream = &probe.streams[0];
    ensure!(
        stream.codec_name == options.format.codec() && stream.codec_type == "video",
        "Recording codec verification failed"
    );
    ensure!(
        stream.width == options.width && stream.height == options.height,
        "Recording dimensions do not match the request"
    );
    if options.format == OutputFormat::Matroska {
        ensure!(
            stream.avg_frame_rate == format!("{}/1", options.fps),
            "Recording frame rate does not match the request"
        );
    } else {
        let (numerator, denominator) = stream
            .avg_frame_rate
            .split_once('/')
            .context("Invalid GIF frame rate")?;
        let numerator = numerator.parse::<u32>().context("Invalid GIF frame rate")?;
        let denominator = denominator
            .parse::<u32>()
            .context("Invalid GIF frame rate")?;
        let minimum_delay = 100 / u64::from(options.fps);
        let maximum_delay = 100_u64.div_ceil(u64::from(options.fps));
        ensure!(
            numerator > 0
                && denominator > 0
                && u64::from(denominator) * 100 >= u64::from(numerator) * minimum_delay
                && u64::from(denominator) * 100 <= u64::from(numerator) * maximum_delay,
            "GIF frame timing is outside centisecond quantization bounds"
        );
        let duration = stream
            .duration
            .as_deref()
            .context("GIF duration is missing")?
            .parse::<f64>()
            .context("Invalid GIF duration")?;
        let frames = f64::from(options.seconds * options.fps);
        ensure!(
            duration.is_finite()
                && duration + 0.000001 >= frames * minimum_delay as f64 / 100.0
                && duration - 0.000001 <= frames * maximum_delay as f64 / 100.0,
            "GIF duration is outside centisecond quantization bounds"
        );
    }
    ensure!(
        stream.nb_read_frames.parse::<u32>().ok() == Some(options.seconds * options.fps),
        "Recording is incomplete: frame count does not match the request"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_option_is_explicit_desktop_only_and_defaults_preserve_json() {
        let root = std::env::temp_dir().join(format!(
            "screenshots-cursor-options-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        let output = root.join("unused.mkv");
        let mode = if cfg!(windows) { "gdigrab" } else { "x11" };
        let base = vec![
            OsString::from(mode),
            output.into_os_string(),
            "--seconds".into(),
            "1".into(),
            "--fps".into(),
            "2".into(),
            "--width".into(),
            "16".into(),
            "--height".into(),
            "12".into(),
        ];
        let parsed = options(&base).unwrap();
        assert_eq!(parsed.draw_mouse(), "0");
        assert!(parsed.summary(9).get("cursor").is_none());
        for (value, flag) in [("show", "1"), ("hide", "0")] {
            let mut args = base.clone();
            args.splice(2..2, ["--cursor".into(), value.into()]);
            let parsed = options(&args).unwrap();
            assert_eq!(parsed.draw_mouse(), flag);
            assert_eq!(parsed.summary(9)["cursor"], value);
            let mut test = args;
            test[0] = "test".into();
            assert!(options(&test).is_err());
        }
        for extra in [
            vec!["--cursor", "bad"],
            vec!["--cursor", "show", "--cursor", "hide"],
            vec!["--cursor"],
        ] {
            let mut args = base.clone();
            args.extend(extra.into_iter().map(OsString::from));
            assert!(options(&args).is_err());
        }
        assert!(std::fs::read_dir(&root).unwrap().next().is_none());
        std::fs::remove_dir(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn x11_cursor_argument_precedes_its_input_and_is_absent_for_test_source() {
        for cursor in [None, Some(false), Some(true)] {
            let options = Options {
                mode: "x11",
                destination: Path::new("unused.mkv"),
                seconds: 1,
                fps: 2,
                width: 16,
                height: 12,
                x: 0,
                y: 0,
                format: OutputFormat::Matroska,
                cursor,
            };
            let command = unix::encoder(&options, Path::new("stage.mkv"), Some(":9"));
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
            let test = unix::encoder(&options, Path::new("stage.mkv"), None);
            assert!(!test.get_args().any(|v| v == "-draw_mouse"));
        }
    }

    #[test]
    fn shared_verifier_requires_complete_selected_codec_dimensions_and_rate() {
        let options = Options {
            mode: "test",
            destination: Path::new("unused.mkv"),
            seconds: 1,
            fps: 2,
            width: 16,
            height: 12,
            x: 0,
            y: 0,
            format: OutputFormat::Matroska,
            cursor: None,
        };
        let valid = serde_json::json!({"streams":[{"codec_name":"ffv1","codec_type":"video","width":16,"height":12,"avg_frame_rate":"2/1","nb_read_frames":"2"}],"format":{"format_name":"matroska,webm"}});
        verify_probe(&serde_json::to_vec(&valid).unwrap(), &options).unwrap();
        for field in ["codec_name", "width", "avg_frame_rate", "nb_read_frames"] {
            let mut invalid = valid.clone();
            invalid["streams"][0][field] = match field {
                "codec_name" => "h264".into(),
                "width" => 15.into(),
                "avg_frame_rate" => "1/1".into(),
                _ => "1".into(),
            };
            assert!(
                verify_probe(&serde_json::to_vec(&invalid).unwrap(), &options).is_err(),
                "{field}"
            );
        }
        assert!(verify_probe(b"not json", &options).is_err());
    }

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
