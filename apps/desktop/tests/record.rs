#[cfg(unix)]
use std::process::Output;
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::Command,
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("screenshots-record-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command.arg("record");
        for (variable, name) in [
            ("HOME", "home"),
            ("USERPROFILE", "userprofile"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CACHE_HOME", "cache"),
            ("APPDATA", "appdata"),
            ("LOCALAPPDATA", "localappdata"),
        ] {
            command.env(variable, self.path(name));
        }
        command.env_remove("DISPLAY").env_remove("WAYLAND_DISPLAY");
        command
    }

    fn arguments<'a>(&self, output: &'a Path) -> Vec<&'a OsStr> {
        vec![
            "test".as_ref(),
            output.as_os_str(),
            "--seconds".as_ref(),
            "1".as_ref(),
            "--fps".as_ref(),
            "2".as_ref(),
            "--width".as_ref(),
            "16".as_ref(),
            "--height".as_ref(),
            "12".as_ref(),
        ]
    }

    fn no_user_state_or_staging(&self) {
        for name in [
            "home",
            "userprofile",
            "config",
            "data",
            "cache",
            "appdata",
            "localappdata",
        ] {
            assert!(!self.path(name).exists());
        }
        assert!(std::fs::read_dir(&self.0).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".screenshots-record-")
        }));
    }

    #[cfg(unix)]
    fn engines(&self, encoder: &str, probe: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = self.path("engines");
        std::fs::create_dir(&path).unwrap();
        for (name, script) in [("ffmpeg", encoder), ("ffprobe", probe)] {
            let file = path.join(name);
            std::fs::write(&file, format!("#!/bin/sh\n{script}\n")).unwrap();
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        path
    }

    #[cfg(unix)]
    fn fake_record(&self, encoder: &str, probe: &str) -> (Output, PathBuf) {
        let engines = self.engines(encoder, probe);
        let destination = self.path("output $(untouched).mkv");
        let result = self
            .command()
            .args(self.arguments(&destination))
            .env("PATH", engines)
            .output()
            .unwrap();
        (result, destination)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
const ENCODE: &str =
    "for arg do output=$arg; done\nprintf '\\032\\105\\337\\243fixture' > \"$output\"";
#[cfg(unix)]
const PROBE: &str = "printf '%s' '{\"streams\":[{\"codec_name\":\"ffv1\",\"codec_type\":\"video\",\"width\":16,\"height\":12,\"avg_frame_rate\":\"2/1\",\"nb_read_frames\":\"2\"}],\"format\":{\"format_name\":\"matroska,webm\"}}'";
#[cfg(unix)]
const GIF_ENCODE: &str = "for arg do output=$arg; done\nprintf 'GIF89afixture' > \"$output\"";
#[cfg(unix)]
const GIF_PROBE: &str = "printf '%s' '{\"streams\":[{\"codec_name\":\"gif\",\"codec_type\":\"video\",\"width\":16,\"height\":12,\"avg_frame_rate\":\"2/1\",\"nb_read_frames\":\"2\",\"duration\":\"1.000000\"}],\"format\":{\"format_name\":\"gif\"}}'";

#[test]
fn cursor_options_reject_invalid_duplicate_and_synthetic_sources_before_engines() {
    let fixture = Fixture::new();
    let output = fixture.path("unused.mkv");
    for (extra, message) in [
        (vec!["--cursor", "bad"], "Cursor must be show or hide"),
        (
            vec!["--cursor", "show", "--cursor", "hide"],
            "Duplicate --cursor",
        ),
        (vec!["--cursor"], "Usage:"),
        (vec!["--cursor", "show"], "Cursor option applies only"),
        (vec!["--cursor", "hide"], "Cursor option applies only"),
    ] {
        let mut args = fixture.arguments(&output);
        args.extend(extra.iter().map(|s| OsStr::new(*s)));
        let result = fixture
            .command()
            .args(args)
            .env("PATH", fixture.path("absent"))
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(String::from_utf8_lossy(&result.stderr).contains(message));
        assert!(!output.exists());
        fixture.no_user_state_or_staging();
    }
}

#[test]
fn gif_limits_are_stricter_and_invalid_extensions_are_rejected_before_engines() {
    let fixture = Fixture::new();
    let output = fixture.path("output.gif");
    for (index, value) in [(3, "31"), (5, "16"), (7, "1281"), (9, "721")] {
        let mut arguments = fixture.arguments(&output);
        arguments[index] = OsStr::new(value);
        let result = fixture
            .command()
            .args(arguments)
            .env("PATH", fixture.path("absent"))
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("GIF limits"));
        assert!(!output.exists());
        fixture.no_user_state_or_staging();
    }
    let invalid = fixture.path("output.mp4");
    let result = fixture
        .command()
        .args(fixture.arguments(&invalid))
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains(".mkv or .gif"));
    fixture.no_user_state_or_staging();
}

#[cfg(unix)]
#[test]
fn gif_output_selects_gif_encoder_and_publishes_verified_animation() {
    let fixture = Fixture::new();
    let encoder = format!(
        r#"
codec= format= pixel= loop= previous=
for arg do
 case "$previous" in
  -c:v) codec=$arg;; -f) format=$arg;; -pix_fmt) pixel=$arg;; -loop) loop=$arg;;
 esac
 previous=$arg
done
[ "$codec" = gif ] && [ "$format" = gif ] && [ "$pixel" = rgb8 ] && [ "$loop" = -1 ] || exit 8
{GIF_ENCODE}
"#
    );
    let probe = format!(
        r#"
previous= format=
for arg do
 [ "$previous" != -f ] || format=$arg
 previous=$arg
done
[ "$format" = gif ] || exit 9
{GIF_PROBE}
"#
    );
    let engines = fixture.engines(&encoder, &probe);
    let destination = fixture.path("animation.gif");
    let result = fixture
        .command()
        .args(fixture.arguments(&destination))
        .env("PATH", engines)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(json["codec"], "gif");
    assert_eq!(json["container"], "gif");
    assert_eq!(json["frames"], 2);
    assert_eq!(std::fs::read(&destination).unwrap(), b"GIF89afixture");
    fixture.no_user_state_or_staging();
}

#[cfg(unix)]
#[test]
fn gif_wrong_codec_container_timing_and_frame_count_leave_no_output() {
    for probe in [
        GIF_PROBE.replace("\"codec_name\":\"gif\"", "\"codec_name\":\"ffv1\""),
        GIF_PROBE.replace("\"format_name\":\"gif\"", "\"format_name\":\"matroska\""),
        GIF_PROBE.replace("\"nb_read_frames\":\"2\"", "\"nb_read_frames\":\"1\""),
        GIF_PROBE.replace("2/1", "1/1"),
        GIF_PROBE.replace("2/1", "0/0"),
        GIF_PROBE.replace("1.000000", "NaN"),
        GIF_PROBE.replace("1.000000", "4.000000"),
    ] {
        let fixture = Fixture::new();
        let engines = fixture.engines(GIF_ENCODE, &probe);
        let destination = fixture.path("invalid.gif");
        let result = fixture
            .command()
            .args(fixture.arguments(&destination))
            .env("PATH", engines)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(!destination.exists());
        fixture.no_user_state_or_staging();
    }
    let fixture = Fixture::new();
    let engines = fixture.engines(ENCODE, GIF_PROBE);
    let destination = fixture.path("bad-header.gif");
    assert!(
        !fixture
            .command()
            .args(fixture.arguments(&destination))
            .env("PATH", engines)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!destination.exists());
    fixture.no_user_state_or_staging();
}

#[test]
fn malformed_arguments_and_existing_destinations_fail_without_engines() {
    let fixture = Fixture::new();
    let output = fixture.path("output.mkv");
    let base = fixture.arguments(&output);
    for (index, value) in [
        (3, "0"),
        (3, "61"),
        (5, "0"),
        (5, "31"),
        (7, "0"),
        (7, "1921"),
        (9, "0"),
        (9, "1081"),
        (3, "-1"),
        (2, "--unknown"),
        (2, "--fps"),
    ] {
        let mut args = base.clone();
        args[index] = OsStr::new(value);
        let result = fixture
            .command()
            .args(args)
            .env("PATH", fixture.path("absent"))
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(!output.exists());
        fixture.no_user_state_or_staging();
    }
    std::fs::write(&output, b"preserved").unwrap();
    let result = fixture.command().args(base).output().unwrap();
    assert!(!result.status.success());
    assert_eq!(std::fs::read(output).unwrap(), b"preserved");
    fixture.no_user_state_or_staging();
}

#[cfg(unix)]
#[test]
fn remote_display_missing_engines_and_bad_offsets_fail_without_recording() {
    let fixture = Fixture::new();
    let destination = fixture.path("output.mkv");
    let mut args = fixture.arguments(&destination);
    args[0] = "x11".as_ref();
    for display in [
        "host:0",
        "localhost:0",
        "unix:0",
        ":0+1,2",
        ":1.2.3",
        ":0\n",
    ] {
        let output = fixture
            .command()
            .args(&args)
            .env("DISPLAY", display)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("DISPLAY must be local"));
        fixture.no_user_state_or_staging();
    }
    args.extend(["--x", "4294967295", "--y", "0"].map(OsStr::new));
    let output = fixture
        .command()
        .args(&args)
        .env("DISPLAY", ":0")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("coordinates overflow"));
    let output = fixture
        .command()
        .args(fixture.arguments(&destination))
        .env("PATH", fixture.path("absent"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("engine is unavailable"));
    assert!(!destination.exists());
    fixture.no_user_state_or_staging();
}

#[cfg(unix)]
#[test]
fn fake_tools_publish_only_verified_video_and_do_not_expose_diagnostics() {
    let fixture = Fixture::new();
    let (result, output) =
        fixture.fake_record(&format!("{ENCODE}\nprintf 'PRIVATE PIXELS' >&2"), PROBE);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stderr.is_empty());
    let json: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(json["frames"], 2);
    assert_eq!(json["codec"], "ffv1");
    assert_eq!(json["container"], "matroska");
    assert_eq!(json["mode"], "test");
    assert_eq!(std::fs::read(output).unwrap(), b"\x1aE\xdf\xa3fixture");
    fixture.no_user_state_or_staging();
}

#[cfg(unix)]
#[test]
fn wrong_frame_counts_codecs_sizes_streams_and_malformed_files_are_never_published() {
    for probe in [
        PROBE.replace("\"nb_read_frames\":\"2\"", "\"nb_read_frames\":\"1\""),
        PROBE.replace("ffv1", "h264"),
        PROBE.replace("width\":16", "width\":15"),
        PROBE.replace("matroska,webm", "avi"),
        PROBE.replace("2/1", "1/1"),
        "printf '%s' '{\"streams\":[]}'".into(),
        "printf '%s' 'not json'".into(),
        "printf 'PRIVATE ERROR' >&2; exit 1".into(),
    ] {
        let fixture = Fixture::new();
        let (result, output) = fixture.fake_record(ENCODE, &probe);
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(!output.exists());
        assert!(!String::from_utf8_lossy(&result.stderr).contains("PRIVATE ERROR"));
        fixture.no_user_state_or_staging();
    }
    for encoder in [
        "exit 1",
        "for arg do output=$arg; done; printf 'wrong header' > \"$output\"",
    ] {
        let fixture = Fixture::new();
        let (result, output) = fixture.fake_record(encoder, PROBE);
        assert!(!result.status.success());
        assert!(!output.exists());
        fixture.no_user_state_or_staging();
    }
}

#[cfg(unix)]
#[test]
fn stream_limits_and_file_limit_stop_tools_without_publishing() {
    for encoder in [
        "while :; do printf 'PRIVATE DIAGNOSTIC PRIVATE DIAGNOSTIC' >&2; done".to_owned(),
        format!("{ENCODE}\n/usr/bin/truncate -s 268435457 \"$output\""),
    ] {
        let fixture = Fixture::new();
        let (result, output) = fixture.fake_record(&encoder, PROBE);
        assert!(!result.status.success());
        assert!(!String::from_utf8_lossy(&result.stderr).contains("PRIVATE DIAGNOSTIC"));
        assert!(!output.exists());
        fixture.no_user_state_or_staging();
    }
    let fixture = Fixture::new();
    let (result, output) = fixture.fake_record(ENCODE, "while :; do printf 'PRIVATE PROBE'; done");
    assert!(!result.status.success());
    assert!(!String::from_utf8_lossy(&result.stderr).contains("PRIVATE PROBE"));
    assert!(!output.exists());
    fixture.no_user_state_or_staging();
}

#[cfg(unix)]
#[test]
fn destination_created_during_recording_is_preserved() {
    let fixture = Fixture::new();
    let encoder = format!("{ENCODE}\nprintf 'racing writer' > \"$RECORD_DESTINATION\"");
    let engines = fixture.engines(&encoder, PROBE);
    let destination = fixture.path("output.mkv");
    let result = fixture
        .command()
        .args(fixture.arguments(&destination))
        .env("PATH", engines)
        .env("RECORD_DESTINATION", &destination)
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(std::fs::read(destination).unwrap(), b"racing writer");
    fixture.no_user_state_or_staging();
}

#[cfg(unix)]
fn process_stopped(pid: i32) -> bool {
    if unsafe { libc::kill(pid, 0) } != 0 {
        return true;
    }
    #[cfg(target_os = "linux")]
    if let Ok(value) = std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        return value
            .rsplit_once(") ")
            .is_some_and(|(_, fields)| fields.starts_with("Z "));
    }
    false
}

#[cfg(unix)]
#[test]
fn cancellation_terminates_engine_group_and_cleans_unpublished_output() {
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let fixture = Fixture::new();
        let encoder = format!(
            "{ENCODE}\n/bin/sleep 60 &\nprintf '%s %s' \"$$\" \"$!\" > \"$RECORD_MARKER\"\nwait"
        );
        let engines = fixture.engines(&encoder, PROBE);
        let destination = fixture.path("output.mkv");
        let marker = fixture.path("pid");
        let mut child = fixture
            .command()
            .args(fixture.arguments(&destination))
            .env("PATH", engines)
            .env("RECORD_MARKER", &marker)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let pids = loop {
            if let Ok(text) = std::fs::read_to_string(&marker) {
                if let Ok(pids) = text
                    .split_whitespace()
                    .map(str::parse::<i32>)
                    .collect::<Result<Vec<_>, _>>()
                {
                    if pids.len() == 2 {
                        break pids;
                    }
                }
            }
            if child.try_wait().unwrap().is_some() || Instant::now() >= deadline {
                let _ = child.kill();
                let output = child.wait_with_output().unwrap();
                panic!(
                    "fake recorder did not start: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                unsafe {
                    libc::kill(-pids[0], libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                panic!("record cancellation did not finish");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let result = child.wait_with_output().unwrap();
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("Recording canceled"));
        assert!(result.stdout.is_empty());
        assert!(pids.into_iter().all(process_stopped));
        assert!(!destination.exists());
        fixture.no_user_state_or_staging();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn killing_controller_stops_the_direct_recording_engine() {
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    let fixture = Fixture::new();
    let engines = fixture.engines(
        "printf '%s' \"$$\" > \"$RECORD_MARKER\"\nexec /bin/sleep 60",
        PROBE,
    );
    let output = fixture.path("output.mkv");
    let marker = fixture.path("pid");
    let mut controller = fixture
        .command()
        .args(fixture.arguments(&output))
        .env("PATH", engines)
        .env("RECORD_MARKER", &marker)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let engine = loop {
        if let Ok(value) = std::fs::read_to_string(&marker) {
            if let Ok(pid) = value.parse::<i32>() {
                break pid;
            }
        }
        if Instant::now() > deadline {
            let _ = controller.kill();
            let _ = controller.wait();
            panic!("Engine never started");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    controller.kill().unwrap();
    controller.wait().unwrap();
    while !process_stopped(engine) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if !process_stopped(engine) {
        unsafe {
            libc::kill(engine, libc::SIGKILL);
        }
    }
    assert!(
        process_stopped(engine),
        "Direct engine survived controller death"
    );
    assert!(!output.exists());
}

#[cfg(unix)]
#[test]
#[ignore = "requires installed FFmpeg and ffprobe; synthetic pixels only"]
fn real_engine_synthetic_video_has_exact_dimensions_rate_and_frame_count() {
    let fixture = Fixture::new();
    let destination = fixture.path("synthetic.mkv");
    let result = fixture
        .command()
        .args(fixture.arguments(&destination))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(json["frames"], 2);
    assert_eq!(json["width"], 16);
    assert_eq!(json["height"], 12);
    assert!(destination.metadata().unwrap().len() <= 256 * 1024 * 1024);
    fixture.no_user_state_or_staging();
}

#[cfg(unix)]
#[test]
#[ignore = "requires installed FFmpeg and ffprobe; synthetic pixels only"]
fn real_engine_synthetic_gif_verifies_odd_dimensions_and_centisecond_timing() {
    for (width, height, fps) in [(17, 13, 7), (1, 1, 1), (17, 13, 15)] {
        let fixture = Fixture::new();
        let destination = fixture.path("synthetic.gif");
        let result = fixture
            .command()
            .arg("test")
            .arg(&destination)
            .args([
                "--seconds",
                "1",
                "--fps",
                &fps.to_string(),
                "--width",
                &width.to_string(),
                "--height",
                &height.to_string(),
            ])
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(json["frames"], fps);
        assert_eq!(json["width"], width);
        assert_eq!(json["height"], height);
        assert_eq!(json["container"], "gif");
        assert_eq!(&std::fs::read(&destination).unwrap()[..6], b"GIF89a");
        fixture.no_user_state_or_staging();
    }
}
