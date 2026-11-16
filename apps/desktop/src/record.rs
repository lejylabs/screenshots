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
    pub(crate) audio: Option<crate::audio_record::AudioInput<'a>>,
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
        if let Some(audio) = self.audio {
            result["audio"] = audio.summary(self.seconds);
        }
        result
    }
}

fn options(args: &[OsString]) -> Result<Options<'_>> {
    ensure!(
        (10..=22).contains(&args.len()) && args.len().is_multiple_of(2),
        "Usage: screenshots record MODE OUTPUT.mkv|OUTPUT.gif --seconds N --fps N --width N --height N [--x X --y Y] [--cursor show|hide] [--audio-mode MODE --audio-device NAME --audio-channels 1|2]"
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
    let mut audio_mode = None;
    let mut audio_device = None;
    let mut audio_channels = None;
    for pair in args[2..].chunks_exact(2) {
        match pair[0].to_str() {
            Some("--audio-mode") => {
                ensure!(audio_mode.is_none(), "Duplicate --audio-mode option");
                audio_mode = Some(pair[1].to_str().context("Audio mode must be UTF-8")?);
                continue;
            }
            Some("--audio-device") => {
                ensure!(audio_device.is_none(), "Duplicate --audio-device option");
                audio_device = Some(pair[1].to_str().context("Audio device must be UTF-8")?);
                continue;
            }
            Some("--audio-channels") => {
                ensure!(
                    audio_channels.is_none(),
                    "Duplicate --audio-channels option"
                );
                audio_channels = Some(
                    pair[1]
                        .to_str()
                        .context("Audio channels must be UTF-8")?
                        .parse::<u16>()
                        .context("Audio channels must be a whole number")?,
                );
                continue;
            }
            _ => {}
        }
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
    let audio = if let Some(mode) = audio_mode {
        ensure!(
            format == OutputFormat::Matroska,
            "Audio recording requires Matroska output; GIF has no audio"
        );
        Some(crate::audio_record::AudioInput::new(
            mode,
            audio_device,
            audio_channels,
        )?)
    } else {
        ensure!(
            audio_device.is_none() && audio_channels.is_none(),
            "Audio device/channels require --audio-mode"
        );
        None
    };
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
        audio,
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
pub(crate) mod unix {
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

    pub(crate) struct Cancellation {
        flag: Arc<AtomicBool>,
        finished: Arc<AtomicBool>,
        handlers: Vec<signal_hook::SigId>,
    }

    impl Cancellation {
        pub(crate) fn new() -> Result<Self> {
            Self::create(Arc::new(AtomicBool::new(false)), false)
        }
        pub(crate) fn with_flag(flag: Arc<AtomicBool>) -> Result<Self> {
            Self::create(flag, true)
        }
        fn create(flag: Arc<AtomicBool>, scoped: bool) -> Result<Self> {
            let mut result = Self {
                flag,
                finished: Arc::new(AtomicBool::new(false)),
                handlers: Vec::with_capacity(4),
            };
            for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
                // Keep termination available after cleanup, including blocked result output.
                let shutdown = signal_hook::flag::register_conditional_shutdown(
                    signal,
                    1,
                    result.finished.clone(),
                )?;
                // Standalone commands retain termination after cleanup if result output blocks.
                // Reused scheduled tasks are scoped: retired handlers must not kill a later run.
                if scoped {
                    result.handlers.push(shutdown);
                }
                result
                    .handlers
                    .push(signal_hook::flag::register(signal, result.flag.clone())?);
            }
            Ok(result)
        }

        pub(crate) fn check(&self, deadline: Instant) -> Result<()> {
            ensure!(!self.flag.load(Ordering::Relaxed), "Recording canceled");
            ensure!(
                Instant::now() < deadline,
                "Recording exceeded its duration plus 10 second deadline"
            );
            Ok(())
        }

        pub(crate) fn output_phase(&self, active: bool) {
            self.finished.store(active, Ordering::SeqCst);
        }

        pub(crate) fn flag(&self) -> Arc<AtomicBool> {
            self.flag.clone()
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

    pub(crate) struct Staging {
        directory: PathBuf,
        pub(crate) file: PathBuf,
    }

    impl Staging {
        pub(crate) fn new(parent: &Path, format: OutputFormat) -> Result<Self> {
            let directory = parent.join(format!(".screenshots-record-{}", uuid::Uuid::new_v4()));
            DirBuilder::new().mode(0o700).create(&directory)?;
            let file = directory.join(if format == OutputFormat::Gif {
                "recording.gif"
            } else {
                "recording.mkv"
            });
            Ok(Self { directory, file })
        }

        pub(crate) fn cleanup(&self) -> Result<()> {
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
        cooperative: bool,
    }

    impl Engine {
        fn stop(&mut self) {
            if self.cooperative && self.child.try_wait().ok().flatten().is_none() {
                unsafe {
                    libc::kill(-self.group, libc::SIGTERM);
                }
                let until = Instant::now() + Duration::from_millis(250);
                while Instant::now() < until && self.child.try_wait().ok().flatten().is_none() {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
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

    pub(crate) fn engine(
        command: Command,
        probe: bool,
        cancellation: &Cancellation,
        deadline: Instant,
    ) -> Result<Vec<u8>> {
        engine_owned(command, probe, cancellation, deadline, false, None)
    }

    pub(crate) fn cooperative_engine(
        command: Command,
        cancellation: &Cancellation,
        deadline: Instant,
    ) -> Result<Vec<u8>> {
        engine_owned(command, true, cancellation, deadline, true, None)
    }

    pub(crate) fn cooperative_engine_guarded(
        command: Command,
        cancellation: &Cancellation,
        deadline: Instant,
        identity: Option<&app_core::FileIdentity>,
    ) -> Result<Vec<u8>> {
        if identity.is_none() {
            return cooperative_engine(command, cancellation, deadline);
        }
        engine_owned(command, true, cancellation, deadline, true, identity)
    }

    fn engine_owned(
        mut command: Command,
        probe: bool,
        cancellation: &Cancellation,
        deadline: Instant,
        cooperative: bool,
        identity: Option<&app_core::FileIdentity>,
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
        // Retain the checked script/executable through the native spawn. The OS still
        // resolves the pathname/shebang; this is not an atomic hostile-user executable pin.
        let _admitted_program = if let Some(identity) = identity {
            let (file, _) = app_core::FileIdentity::open(Path::new(command.get_program()))?;
            identity.verify_file(&file)?;
            Some(file)
        } else {
            None
        };
        let mut child = command.spawn().map_err(|_| {
            anyhow::anyhow!(
                "Recording engine is unavailable; install FFmpeg and ffprobe separately"
            )
        })?;
        drop(_admitted_program);
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
            cooperative,
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
            // RGB8 uses a fixed small palette; no whole-video palette generation pass.
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
            probe.args(["-hide_banner", "-loglevel", "error", "-max_alloc", "67108864", "-threads", "2", "-protocol_whitelist", "file", "-f", options.format.container(), "-max_streams", if options.audio.is_some() { "2" } else { "1" }, "-count_frames", "-show_entries", if options.audio.is_some() { "stream=codec_name,codec_type,width,height,avg_frame_rate,nb_read_frames,duration,sample_rate,channels,bits_per_sample:format=format_name" } else { "stream=codec_name,codec_type,width,height,avg_frame_rate,nb_read_frames,duration:format=format_name" }, "-of", "json", "-i"]);
            probe.arg(&staging.file);
            verify_probe(&engine(probe, true, &cancellation, deadline)?, &options)?;
            if options.audio.is_some() {
                verify_audio_samples(
                    &engine(
                        audio_sample_probe(&staging.file, false),
                        true,
                        &cancellation,
                        deadline,
                    )?,
                    options.seconds,
                )?;
            }
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
    #[serde(default)]
    width: u32,
    #[serde(default)]
    height: u32,
    #[serde(default)]
    avg_frame_rate: String,
    #[serde(default)]
    nb_read_frames: String,
    duration: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u16>,
    bits_per_sample: Option<u16>,
}

pub(crate) fn audio_sample_probe(path: &Path, windows: bool) -> std::process::Command {
    let mut command = std::process::Command::new(if windows { "ffprobe.exe" } else { "ffprobe" });
    command
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-max_alloc",
            "67108864",
            "-threads",
            "2",
            "-protocol_whitelist",
            "file",
            "-f",
            "matroska",
            "-max_streams",
            "2",
            "-select_streams",
            "a:0",
            "-show_entries",
            "frame=nb_samples",
            "-of",
            "csv=p=0",
            "-i",
        ])
        .arg(path);
    command
}

pub(crate) fn verify_audio_samples(bytes: &[u8], seconds: u32) -> Result<()> {
    ensure!(
        bytes.len() <= 8192 && (1..=60).contains(&seconds),
        "Audio sample verification exceeds its limits"
    );
    let text = std::str::from_utf8(bytes).context("Invalid audio sample verification result")?;
    let mut total = 0u32;
    let mut frames = 0u32;
    for line in text.lines().filter(|line| !line.is_empty()) {
        ensure!(
            line.bytes().all(|byte| byte.is_ascii_digit()),
            "Invalid audio sample count"
        );
        let samples = line.parse::<u32>().context("Invalid audio sample count")?;
        ensure!(
            (1..=crate::audio_record::RATE).contains(&samples),
            "Audio sample frame exceeds its limit"
        );
        frames += 1;
        ensure!(
            frames <= seconds + 1,
            "Audio contains too many sample frames"
        );
        total = total
            .checked_add(samples)
            .context("Audio sample count overflow")?;
    }
    ensure!(
        total == seconds * crate::audio_record::RATE,
        "Recording is incomplete: audio sample count does not match the request"
    );
    Ok(())
}

pub(crate) fn verify_probe(bytes: &[u8], options: &Options<'_>) -> Result<()> {
    let probe: Probe = serde_json::from_slice(bytes)
        .map_err(|_| anyhow::anyhow!("Invalid recording verification result"))?;
    ensure!(
        probe
            .format
            .format_name
            .split(',')
            .any(|name| name == options.format.container()),
        "Recording container verification failed"
    );
    ensure!(
        probe.streams.len() == if options.audio.is_some() { 2 } else { 1 },
        "Recording stream count does not match the request"
    );
    let videos = probe
        .streams
        .iter()
        .filter(|stream| stream.codec_type == "video")
        .collect::<Vec<_>>();
    ensure!(
        videos.len() == 1,
        "Recording must contain exactly one video stream"
    );
    let stream = videos[0];
    if let Some(audio) = options.audio {
        let audios = probe
            .streams
            .iter()
            .filter(|stream| stream.codec_type == "audio")
            .collect::<Vec<_>>();
        ensure!(
            audios.len() == 1,
            "Recording must contain exactly one requested audio stream"
        );
        let recorded = audios[0];
        ensure!(
            recorded.codec_name == "pcm_s16le"
                && recorded.sample_rate.as_deref() == Some("48000")
                && recorded.channels == Some(audio.channels)
                && recorded.bits_per_sample == Some(16),
            "Recording audio codec, rate or channels verification failed"
        );
    }
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
    fn optional_audio_is_explicit_matroska_only_and_preserves_silent_summary() {
        let output = std::env::temp_dir().join(format!(
            "screenshots-av-options-{}.mkv",
            uuid::Uuid::new_v4()
        ));
        let base = vec![
            "test".into(),
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
        assert!(options(&base).unwrap().summary(12).get("audio").is_none());
        let mut valid = base.clone();
        valid.extend([
            "--audio-mode".into(),
            "test".into(),
            "--audio-channels".into(),
            "1".into(),
        ]);
        let parsed = options(&valid).unwrap();
        assert_eq!(
            parsed.summary(12)["audio"],
            serde_json::json!({"mode":"test","codec":"pcm_s16le","sample_rate":48000,"channels":1,"samples":48000})
        );
        for extra in [
            vec!["--audio-device", "default"],
            vec!["--audio-channels", "1"],
            vec!["--audio-mode", "test", "--audio-mode", "test"],
            vec!["--audio-mode", "test", "--audio-device", "default"],
            vec!["--audio-mode", "test", "--audio-channels", "3"],
        ] {
            let mut invalid = base.clone();
            invalid.extend(extra.into_iter().map(OsString::from));
            assert!(options(&invalid).is_err());
        }
        let mut gif = valid;
        gif[1] = std::env::temp_dir()
            .join(format!(
                "screenshots-av-options-{}.gif",
                uuid::Uuid::new_v4()
            ))
            .into_os_string();
        assert!(options(&gif).is_err());
    }

    #[test]
    fn combined_probe_requires_both_complete_streams_and_exact_audio_samples() {
        let options = Options {
            mode: "test",
            destination: Path::new("unused.mkv"),
            seconds: 2,
            fps: 3,
            width: 16,
            height: 12,
            x: 0,
            y: 0,
            format: OutputFormat::Matroska,
            cursor: None,
            audio: Some(crate::audio_record::AudioInput::new("test", None, Some(2)).unwrap()),
        };
        let valid = serde_json::json!({"streams":[{"codec_name":"pcm_s16le","codec_type":"audio","sample_rate":"48000","channels":2,"bits_per_sample":16},{"codec_name":"ffv1","codec_type":"video","width":16,"height":12,"avg_frame_rate":"3/1","nb_read_frames":"6"}],"format":{"format_name":"matroska,webm"}});
        verify_probe(&serde_json::to_vec(&valid).unwrap(), &options).unwrap();
        for field in [
            "codec_name",
            "codec_type",
            "sample_rate",
            "channels",
            "bits_per_sample",
        ] {
            let mut invalid = valid.clone();
            invalid["streams"][0][field] = if matches!(field, "channels" | "bits_per_sample") {
                1.into()
            } else {
                "invalid".into()
            };
            assert!(verify_probe(&serde_json::to_vec(&invalid).unwrap(), &options).is_err());
        }
        let mut missing = valid;
        missing["streams"].as_array_mut().unwrap().remove(0);
        assert!(verify_probe(&serde_json::to_vec(&missing).unwrap(), &options).is_err());
        verify_audio_samples(b"48000\n48000\n", 2).unwrap();
        for invalid in [
            b"".as_slice(),
            b"0\n96000\n",
            b"48000\n47999\n",
            b"48001\n47999\n",
            b"48000\n48000\n1\n",
            b"nb_samples=96000\n",
            b"48000,48000\n",
        ] {
            assert!(verify_audio_samples(invalid, 2).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn combined_encoder_has_two_explicit_inputs_and_bounded_pcm_frames() {
        let options = Options {
            mode: "test",
            destination: Path::new("unused.mkv"),
            seconds: 2,
            fps: 3,
            width: 16,
            height: 12,
            x: 0,
            y: 0,
            format: OutputFormat::Matroska,
            cursor: None,
            audio: Some(crate::audio_record::AudioInput::new("test", None, Some(1)).unwrap()),
        };
        let command = unix::encoder(&options, Path::new("stage.mkv"), None);
        let args = command
            .get_args()
            .map(|arg| arg.to_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(args.iter().filter(|arg| **arg == "-i").count(), 2);
        assert!(!args.contains(&"-an"));
        assert!(args.windows(2).any(|pair| pair == ["-map", "1:a:0"]));
        assert!(args.windows(2).any(|pair| pair == ["-c:a", "pcm_s16le"]));
        assert!(args.windows(2).any(|pair| pair == ["-ac", "1"]));
        assert!(
            args.iter()
                .any(|arg| arg.ends_with("asetnsamples=n=48000:p=0"))
        );
    }

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
                audio: None,
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
            audio: None,
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
