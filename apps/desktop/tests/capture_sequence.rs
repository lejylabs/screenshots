#[cfg(unix)]
use std::ffi::OsString;
use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "screenshots-sequence-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn command(&self, native: bool) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .arg("capture-sequence")
            .env("HOME", self.path("home"))
            .env("USERPROFILE", self.path("userprofile"))
            .env("APPDATA", self.path("appdata"))
            .env("LOCALAPPDATA", self.path("localappdata"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_CACHE_HOME", self.path("cache"));
        if !native {
            command.env_remove("DISPLAY").env_remove("WAYLAND_DISPLAY");
        }
        command
    }

    fn run(&self, args: &[&OsStr]) -> Output {
        let mut child = self
            .command(false)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if child.try_wait().unwrap().is_some() {
                return child.wait_with_output().unwrap();
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                let output = child.wait_with_output().unwrap();
                panic!(
                    "argument rejection must not wait for a scheduled start: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn reject(&self, args: &[&OsStr], message: &str) {
        let output = self.run(args);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(message), "{error}");
        self.no_user_state();
    }

    fn no_user_state(&self) {
        for name in [
            "home",
            "userprofile",
            "appdata",
            "localappdata",
            "config",
            "data",
            "cache",
        ] {
            assert!(!self.path(name).exists());
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn args<'a>(directory: &'a Path, options: &[&'a str]) -> Vec<&'a OsStr> {
    let mut args = vec![
        OsStr::new("display"),
        OsStr::new("fixture"),
        directory.as_os_str(),
    ];
    args.extend(options.iter().map(|value| OsStr::new(*value)));
    args
}

#[test]
fn invalid_arguments_fail_before_any_native_capture_or_output() {
    let fixture = Fixture::new();
    let directory = fixture.path("captures $(ignored)");
    std::fs::create_dir(&directory).unwrap();
    fixture.reject(&[], "Usage:");
    fixture.reject(
        &args(&directory, &["--count", "1", "--interval-ms"]),
        "Usage:",
    );
    for (options, message) in [
        (["--count", "0", "--interval-ms", "100"], "Count must be 1"),
        (
            ["--count", "1001", "--interval-ms", "100"],
            "Count must be 1",
        ),
        (
            ["--count", "-1", "--interval-ms", "100"],
            "Count must be a whole",
        ),
        (
            ["--count", "65536", "--interval-ms", "100"],
            "Count must be a whole",
        ),
        (
            ["--count", "1", "--interval-ms", "99"],
            "Interval must be 100",
        ),
        (
            ["--count", "1", "--interval-ms", "3600001"],
            "Interval must be 100",
        ),
        (
            ["--count", "1", "--interval-ms", "-1"],
            "Interval must be a whole",
        ),
        (
            ["--count", "1", "--interval-ms", "1.5"],
            "Interval must be a whole",
        ),
        (["--count", "1", "--count", "1"], "Duplicate --count"),
        (
            ["--interval-ms", "100", "--interval-ms", "100"],
            "Duplicate --interval-ms",
        ),
        (
            ["--count", "1", "--other", "100"],
            "Unknown capture-sequence option",
        ),
    ] {
        fixture.reject(&args(&directory, &options), message);
    }
    let mut invalid = args(&directory, &["--count", "1", "--interval-ms", "100"]);
    invalid[0] = OsStr::new("window");
    fixture.reject(&invalid, "Usage:");
    invalid[0] = OsStr::new("display");
    invalid[1] = OsStr::new("");
    fixture.reject(&invalid, "Display ID must contain");
    let long_id = "x".repeat(65);
    invalid[1] = OsStr::new(&long_id);
    fixture.reject(&invalid, "Display ID must contain");
    assert!(std::fs::read_dir(directory).unwrap().next().is_none());
}

#[test]
fn destination_must_exist_be_a_directory_and_be_empty() {
    let fixture = Fixture::new();
    let missing = fixture.path("missing");
    let options = ["--count", "1", "--interval-ms", "100"];
    let result = fixture.run(&args(&missing, &options));
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(!missing.exists());
    let file = fixture.path("file");
    std::fs::write(&file, b"preserve this file").unwrap();
    fixture.reject(
        &args(&file, &options),
        "must be an existing empty directory",
    );
    assert_eq!(std::fs::read(file).unwrap(), b"preserve this file");
    let occupied = fixture.path("occupied");
    std::fs::create_dir(&occupied).unwrap();
    let existing = occupied.join("existing.png");
    std::fs::write(&existing, b"existing bytes").unwrap();
    fixture.reject(&args(&occupied, &options), "destination must be empty");
    assert_eq!(std::fs::read(existing).unwrap(), b"existing bytes");
    assert_eq!(std::fs::read_dir(occupied).unwrap().count(), 1);
    fixture.no_user_state();
}

#[test]
fn invalid_schedules_fail_before_directory_checks_native_capture_or_user_state() {
    let fixture = Fixture::new();
    let missing = fixture.path("missing");
    for start in [
        "",
        "tomorrow",
        "2030-01-01",
        "2030-01-01T00:00:00",
        "2030-01-01T00:00:00+25:00",
        "2030-02-30T00:00:00Z",
        "2030-01-01T00:00:00Zjunk",
    ] {
        fixture.reject(
            &args(
                &missing,
                &["--count", "1", "--interval-ms", "100", "--start-at", start],
            ),
            "Start time must be RFC 3339 with an explicit timezone",
        );
    }
    for start in ["2000-01-01T00:00:00Z", "9999-12-31T23:59:59Z"] {
        fixture.reject(
            &args(
                &missing,
                &["--start-at", start, "--interval-ms", "100", "--count", "1"],
            ),
            "Scheduled start must be in the future and no more than 7 days away",
        );
    }
    fixture.reject(
        &args(
            &missing,
            &[
                "--count",
                "1",
                "--interval-ms",
                "100",
                "--start-at",
                "2030-01-01T00:00:00-00:00",
            ],
        ),
        "Start time must use a known timezone",
    );
    fixture.reject(
        &args(
            &missing,
            &[
                "--count",
                "1",
                "--interval-ms",
                "100",
                "--start-at",
                "2030-01-01T00:00:00.0000000001Z",
            ],
        ),
        "Start time supports at most 9 fractional second digits",
    );
    let future = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    fixture.reject(
        &args(
            &missing,
            &[
                "--start-at",
                &future,
                "--count",
                "1",
                "--start-at",
                &future,
                "--interval-ms",
                "100",
            ],
        ),
        "Duplicate --start-at",
    );
    for (options, message) in [
        (
            vec!["--start-at", &future, "--count", "1"],
            "Missing --interval-ms",
        ),
        (
            vec!["--start-at", &future, "--interval-ms", "100"],
            "Missing --count",
        ),
        (
            vec!["--count", "1", "--interval-ms", "100", "--start-at"],
            "Usage:",
        ),
        (
            vec!["--start-at", &future, "--count", "1", "--unknown", "100"],
            "Unknown capture-sequence option",
        ),
    ] {
        fixture.reject(&args(&missing, &options), message);
    }
    assert!(!missing.exists());
}

#[test]
fn scheduled_destination_errors_return_before_the_future_wait() {
    let fixture = Fixture::new();
    let future = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let options = [
        "--start-at",
        &future,
        "--count",
        "1",
        "--interval-ms",
        "100",
    ];
    let file = fixture.path("file");
    std::fs::write(&file, b"keep").unwrap();
    fixture.reject(
        &args(&file, &options),
        "must be an existing empty directory",
    );
    let occupied = fixture.path("occupied");
    std::fs::create_dir(&occupied).unwrap();
    std::fs::write(occupied.join("keep.png"), b"keep").unwrap();
    fixture.reject(&args(&occupied, &options), "destination must be empty");
    assert_eq!(std::fs::read(file).unwrap(), b"keep");
    assert_eq!(std::fs::read(occupied.join("keep.png")).unwrap(), b"keep");
}

#[cfg(unix)]
#[test]
fn non_utf8_display_id_is_rejected_before_native_capture() {
    use std::os::unix::ffi::OsStringExt;
    let fixture = Fixture::new();
    let directory = fixture.path("captures");
    std::fs::create_dir(&directory).unwrap();
    let display_id = OsString::from_vec(vec![0xff]);
    let mut arguments = args(&directory, &["--count", "1", "--interval-ms", "100"]);
    arguments[1] = display_id.as_os_str();
    fixture.reject(&arguments, "Display ID must be UTF-8");
    assert!(std::fs::read_dir(directory).unwrap().next().is_none());
}

#[test]
#[ignore = "reads real desktop pixels; requires SCREENSHOTS_NATIVE_CAPTURE=1 and SCREENSHOTS_TEST_DISPLAY_ID"]
fn native_cli_captures_two_explicit_stills_without_user_state() {
    assert_eq!(
        std::env::var("SCREENSHOTS_NATIVE_CAPTURE").as_deref(),
        Ok("1")
    );
    let display_id = std::env::var_os("SCREENSHOTS_TEST_DISPLAY_ID")
        .expect("set SCREENSHOTS_TEST_DISPLAY_ID to an ID from screenshots list displays");
    let fixture = Fixture::new();
    let directory = fixture.path("native");
    std::fs::create_dir(&directory).unwrap();
    let started = std::time::Instant::now();
    let output = fixture
        .command(true)
        .arg("display")
        .arg(display_id)
        .arg(&directory)
        .args(["--count", "2", "--interval-ms", "100"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(started.elapsed() >= std::time::Duration::from_millis(100));
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["completed"], 2);
    assert_eq!(result["interval_ms"], 100);
    assert_eq!(result["path_encoding"], "lossy-native");
    let files = result["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    for (index, file) in files.iter().enumerate() {
        let name = file.as_str().unwrap();
        assert!(name.ends_with(&format!("-{:04}.png", index + 1)));
        assert_eq!(Path::new(name).components().count(), 1);
        let pixels = image::open(directory.join(name)).unwrap();
        assert!(pixels.width() > 0 && pixels.height() > 0);
    }
    assert_eq!(std::fs::read_dir(directory).unwrap().count(), 2);
    fixture.no_user_state();
}
