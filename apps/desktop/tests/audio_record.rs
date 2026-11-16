use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("screenshots-audio-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn command(&self) -> Command {
        self.command_for("record-audio")
    }
    fn command_for(&self, operation: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .arg(operation)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for name in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            command.env(name, self.path(name));
        }
        command
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DBUS_SESSION_BUS_ADDRESS");
        command
    }
    fn args(&self, path: &Path) -> Vec<OsString> {
        vec!["test".into(), path.into(), "--seconds".into(), "1".into()]
    }
    fn av_args(&self, path: &Path) -> Vec<OsString> {
        vec![
            "test".into(),
            path.into(),
            "--seconds".into(),
            "1".into(),
            "--fps".into(),
            "3".into(),
            "--width".into(),
            "16".into(),
            "--height".into(),
            "12".into(),
            "--audio-mode".into(),
            "test".into(),
        ]
    }
    fn run(&self, mut command: Command) -> Output {
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Audio fixture exceeded its outer deadline");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.wait_with_output().unwrap()
    }
    fn no_state_or_staging(&self) {
        for name in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            assert!(!self.path(name).exists());
        }
        assert!(self.0.read_dir().unwrap().all(|entry| {
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
            let output = path.join(name);
            std::fs::write(&output, format!("#!/bin/sh\n{script}\n")).unwrap();
            std::fs::set_permissions(&output, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        path
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn success(output: Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}

fn failed(output: Output) -> String {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(!error.contains("SECRET_AUDIO_DEVICE"));
    assert!(!error.contains("SECRET_ENGINE_DIAGNOSTIC"));
    error
}

#[test]
fn audio_argument_admission_precedes_engines_and_publication() {
    let fixture = Fixture::new();
    let destination = fixture.path("output.wav");
    let mut cases = Vec::new();
    for seconds in ["0", "301", "-1", "1.5"] {
        let mut args = fixture.args(&destination);
        args[3] = seconds.into();
        cases.push(args);
    }
    for extra in [
        vec!["--channels", "0"],
        vec!["--channels", "3"],
        vec!["--seconds", "1"],
        vec!["--device", "SECRET_AUDIO_DEVICE"],
        vec!["--unknown", "x"],
    ] {
        let mut args = fixture.args(&destination);
        args.extend(extra.into_iter().map(OsString::from));
        cases.push(args);
    }
    for mode in ["invalid", "http://SECRET_AUDIO_DEVICE"] {
        let mut args = fixture.args(&destination);
        args[0] = mode.into();
        cases.push(args);
    }
    for args in cases {
        let mut command = fixture.command();
        command.args(args).env("PATH", fixture.path("missing"));
        failed(fixture.run(command));
        assert!(!destination.exists());
        fixture.no_state_or_staging();
    }
    let mut command = fixture.command();
    command
        .args(fixture.args(&fixture.path("invalid.mkv")))
        .env("PATH", fixture.path("missing"));
    failed(fixture.run(command));
    std::fs::write(&destination, b"keep existing audio").unwrap();
    let mut command = fixture.command();
    command
        .args(fixture.args(&destination))
        .env("PATH", fixture.path("missing"));
    failed(fixture.run(command));
    assert_eq!(std::fs::read(destination).unwrap(), b"keep existing audio");
    fixture.no_state_or_staging();
}

#[cfg(unix)]
fn wave_encoder() -> String {
    let mut header = Vec::new();
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&192036u32.to_le_bytes());
    header.extend_from_slice(b"WAVEfmt ");
    header.extend_from_slice(&16u32.to_le_bytes());
    header.extend_from_slice(&1u16.to_le_bytes());
    header.extend_from_slice(&2u16.to_le_bytes());
    header.extend_from_slice(&48000u32.to_le_bytes());
    header.extend_from_slice(&192000u32.to_le_bytes());
    header.extend_from_slice(&4u16.to_le_bytes());
    header.extend_from_slice(&16u16.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&192000u32.to_le_bytes());
    let escaped = header
        .iter()
        .map(|byte| format!("\\{byte:03o}"))
        .collect::<String>();
    format!(
        "for arg do output=$arg; done\nprintf '%b' '{escaped}' > \"$output\"\n/bin/dd if=/dev/zero bs=192000 count=1 >> \"$output\" 2>/dev/null"
    )
}

#[cfg(unix)]
const PROBE: &str = "printf '%s' '{\"streams\":[{\"codec_name\":\"pcm_s16le\",\"codec_type\":\"audio\",\"sample_rate\":\"48000\",\"channels\":2,\"bits_per_sample\":16,\"time_base\":\"1/48000\",\"duration_ts\":48000}],\"format\":{\"format_name\":\"wav\"}}'";

#[cfg(unix)]
#[test]
fn verified_audio_is_published_once_without_user_state() {
    let fixture = Fixture::new();
    let path = fixture.path("sound $(unused).WAV");
    let engines = fixture.engines(&wave_encoder(), PROBE);
    let mut command = fixture.command();
    command.args(fixture.args(&path)).env("PATH", &engines);
    let value = success(fixture.run(command));
    assert_eq!(
        value,
        serde_json::json!({"mode":"test","container":"wav","codec":"pcm_s16le","seconds":1,"sample_rate":48000,"channels":2,"samples":48000,"bytes":192044})
    );
    assert_eq!(path.metadata().unwrap().len(), 192044);
    let original = std::fs::read(&path).unwrap();
    let mut command = fixture.command();
    command.args(fixture.args(&path)).env("PATH", &engines);
    failed(fixture.run(command));
    assert_eq!(std::fs::read(path).unwrap(), original);
    fixture.no_state_or_staging();
}

#[cfg(unix)]
#[test]
fn incomplete_audio_bad_probe_and_diagnostic_overflow_never_publish() {
    for (encoder, probe) in [
        ("for arg do output=$arg; done\nprintf 'RIFFbad' > \"$output\"".to_owned(), PROBE.to_owned()),
        (wave_encoder(), PROBE.replace("48000}", "47999}")),
        (wave_encoder(), "printf '%s' 'SECRET_ENGINE_DIAGNOSTIC'".to_owned()),
        ("printf '%s' 'SECRET_ENGINE_DIAGNOSTIC' >&2\n/bin/dd if=/dev/zero bs=17000 count=1 >&2 2>/dev/null".to_owned(), PROBE.to_owned()),
    ] {
        let fixture = Fixture::new();
        let path = fixture.path("failed.wav");
        let engines = fixture.engines(&encoder, &probe);
        let mut command = fixture.command(); command.args(fixture.args(&path)).env("PATH", engines);
        failed(fixture.run(command));
        assert!(!path.exists());
        fixture.no_state_or_staging();
    }
}

#[test]
fn combined_audio_options_fail_before_devices_and_engines() {
    let fixture = Fixture::new();
    let output = fixture.path("combined.mkv");
    for extra in [
        vec![
            "--audio-mode",
            "test",
            "--audio-device",
            "SECRET_AUDIO_DEVICE",
        ],
        vec!["--audio-mode", "test", "--audio-channels", "3"],
        vec!["--audio-mode", "invalid"],
        vec!["--audio-mode", "test", "--audio-mode", "test"],
        vec!["--audio-device", "SECRET_AUDIO_DEVICE"],
        vec!["--audio-channels", "1"],
    ] {
        let mut args = fixture.av_args(&output);
        args.truncate(10);
        args.extend(extra.into_iter().map(OsString::from));
        let mut command = fixture.command_for("record");
        command.args(args).env("PATH", fixture.path("missing"));
        failed(fixture.run(command));
        assert!(!output.exists());
    }
    let gif = fixture.path("combined.gif");
    let mut command = fixture.command_for("record");
    command
        .args(fixture.av_args(&gif))
        .env("PATH", fixture.path("missing"));
    let error = failed(fixture.run(command));
    assert!(error.contains("GIF has no audio"), "{error}");
    fixture.no_state_or_staging();
}

#[cfg(unix)]
#[test]
fn combined_audio_verification_requires_one_stream_of_each_type_and_all_samples() {
    let valid = r#"{"streams":[{"codec_name":"ffv1","codec_type":"video","width":16,"height":12,"avg_frame_rate":"3/1","nb_read_frames":"3"},{"codec_name":"pcm_s16le","codec_type":"audio","sample_rate":"48000","channels":2,"bits_per_sample":16}],"format":{"format_name":"matroska,webm"}}"#;
    for (json, samples, accepted) in [
        (valid.to_owned(), "48000", true),
        (valid.replace("pcm_s16le", "aac"), "48000", false),
        (
            valid.replace("\"channels\":2", "\"channels\":1"),
            "48000",
            false,
        ),
        (
            valid.replace("\"nb_read_frames\":\"3\"", "\"nb_read_frames\":\"2\""),
            "48000",
            false,
        ),
        (valid.to_owned(), "47999", false),
        (valid.to_owned(), "48000\n1", false),
    ] {
        let fixture = Fixture::new();
        let destination = fixture.path("combined.mkv");
        let probe = format!(
            "for arg do if [ \"$arg\" = 'csv=p=0' ]; then printf '%s\\n' '{samples}'; exit; fi; done\nprintf '%s' '{json}'"
        );
        let encoder =
            "for arg do output=$arg; done\nprintf '\\032\\105\\337\\243fixture' > \"$output\"";
        let engines = fixture.engines(encoder, &probe);
        let mut command = fixture.command_for("record");
        command
            .args(fixture.av_args(&destination))
            .env("PATH", engines);
        let output = fixture.run(command);
        if accepted {
            let value = success(output);
            assert_eq!(value["frames"], 3);
            assert_eq!(
                value["audio"],
                serde_json::json!({"mode":"test","codec":"pcm_s16le","sample_rate":48000,"channels":2,"samples":48000})
            );
            assert!(destination.exists());
        } else {
            failed(output);
            assert!(!destination.exists());
        }
        fixture.no_state_or_staging();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn audio_stdout_failure_preserves_the_published_wav() {
    let fixture = Fixture::new();
    let path = fixture.path("saved.wav");
    let engines = fixture.engines(&wave_encoder(), PROBE);
    let mut command = fixture.command();
    command
        .args(fixture.args(&path))
        .env("PATH", engines)
        .stdout(
            std::fs::OpenOptions::new()
                .write(true)
                .open("/dev/full")
                .unwrap(),
        );
    let error = failed(fixture.run(command));
    assert!(error.contains("Audio was published"), "{error}");
    assert_eq!(path.metadata().unwrap().len(), 192044);
    fixture.no_state_or_staging();
}

#[cfg(unix)]
#[test]
fn signal_cancels_audio_engine_and_cleans_private_staging() {
    let fixture = Fixture::new();
    let path = fixture.path("canceled.wav");
    let marker = fixture.path("engine.pid");
    let engines = fixture.engines(
        "printf '%s' \"$$\" > \"$AUDIO_TEST_MARKER\"\nexec /bin/sleep 60",
        PROBE,
    );
    let mut command = fixture.command();
    command
        .args(fixture.args(&path))
        .env("PATH", engines)
        .env("AUDIO_TEST_MARKER", &marker);
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let group = loop {
        if let Ok(text) = std::fs::read_to_string(&marker) {
            if let Ok(pid) = text.parse::<i32>() {
                break pid;
            }
        }
        if child.try_wait().unwrap().is_some() || Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!(
                "Audio fixture did not start: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    struct Group(i32);
    impl Drop for Group {
        fn drop(&mut self) {
            unsafe {
                libc::kill(-self.0, libc::SIGKILL);
            }
        }
    }
    let _cleanup = Group(group);
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Canceled audio did not stop");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let error = failed(child.wait_with_output().unwrap());
    assert!(error.contains("Recording canceled"), "{error}");
    assert!(!path.exists());
    assert_eq!(
        unsafe { libc::kill(group, 0) },
        -1,
        "Audio engine survived cancellation"
    );
    fixture.no_state_or_staging();
}

#[test]
#[ignore = "requires installed FFmpeg/ffprobe; records only synthetic sine audio, never a device"]
fn installed_engine_synthetic_audio_has_exact_mono_stereo_pcm_frames() {
    let fixture = Fixture::new();
    for channels in [1u16, 2] {
        let path = fixture.path(&format!("synthetic-{channels}.wav"));
        let mut args = fixture.args(&path);
        args.extend(["--channels".into(), channels.to_string().into()]);
        let mut command = fixture.command();
        command.args(args);
        let value = success(fixture.run(command));
        assert_eq!(value["channels"], channels);
        assert_eq!(value["samples"], 48000);
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(bytes.len(), 44 + 48000 * usize::from(channels) * 2);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 48000);
        let samples = bytes[44..]
            .chunks_exact(2)
            .map(|bytes| i16::from_le_bytes(bytes.try_into().unwrap()))
            .collect::<Vec<_>>();
        assert!(samples.iter().any(|sample| sample.unsigned_abs() > 100));
        assert!(samples.iter().any(|sample| *sample < -100));
        if channels == 2 {
            assert!(samples.chunks_exact(2).all(|pair| pair[0] == pair[1]));
        }
    }
    fixture.no_state_or_staging();
}

#[test]
#[ignore = "requires installed FFmpeg/ffprobe; synthetic video and sine audio only, never devices"]
fn installed_engine_combined_recording_has_exact_video_frames_and_audio_samples() {
    let fixture = Fixture::new();
    for (fps, channels) in [(3, 1u16), (7, 2), (30, 2)] {
        let path = fixture.path(&format!("combined-{fps}-{channels}.mkv"));
        let mut args = fixture.av_args(&path);
        args[5] = fps.to_string().into();
        args.extend(["--audio-channels".into(), channels.to_string().into()]);
        let mut command = fixture.command_for("record");
        command.args(args);
        let value = success(fixture.run(command));
        assert_eq!(value["frames"], fps);
        assert_eq!(value["audio"]["channels"], channels);
        assert_eq!(value["audio"]["samples"], 48000);

        let decoded = fixture.path(&format!("decoded-{fps}-{channels}.pcm"));
        let mut command = Command::new(if cfg!(target_os = "windows") {
            "ffmpeg.exe"
        } else {
            "ffmpeg"
        });
        command
            .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-n", "-i"])
            .arg(&path)
            .args(["-map", "0:a:0", "-vn", "-c:a", "pcm_s16le", "-f", "s16le"])
            .arg(&decoded)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = fixture.run(command);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        let pcm = std::fs::read(decoded).unwrap();
        assert_eq!(pcm.len(), 48000 * usize::from(channels) * 2);
        assert!(
            pcm.chunks_exact(2)
                .any(|sample| i16::from_le_bytes(sample.try_into().unwrap()).unsigned_abs() > 100)
        );

        let mut command = Command::new(if cfg!(target_os = "windows") {
            "ffprobe.exe"
        } else {
            "ffprobe"
        });
        command
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-select_streams",
                "v:0",
                "-count_frames",
                "-show_entries",
                "stream=codec_name,width,height,nb_read_frames",
                "-of",
                "json",
                "-i",
            ])
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = fixture.run(command);
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
        let probe: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(probe["streams"][0]["codec_name"], "ffv1");
        assert_eq!(probe["streams"][0]["width"], 16);
        assert_eq!(probe["streams"][0]["height"], 12);
        assert_eq!(probe["streams"][0]["nb_read_frames"], fps.to_string());
    }
    fixture.no_state_or_staging();
}
