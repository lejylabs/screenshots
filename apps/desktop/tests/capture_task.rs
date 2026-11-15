use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "screenshots-capture-task-cli-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn reject(&self, args: &[&OsStr], message: &str) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_screenshots"))
            .arg("capture-task")
            .args(args)
            .env("HOME", self.path("home"))
            .env("USERPROFILE", self.path("profile"))
            .env("APPDATA", self.path("appdata"))
            .env("LOCALAPPDATA", self.path("localappdata"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_CACHE_HOME", self.path("cache"))
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let started = std::time::Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if started.elapsed() >= std::time::Duration::from_secs(5) {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("invalid capture task must not wait or capture");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains(message), "{error}");
        for name in [
            "home",
            "profile",
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

fn arguments<'a>(output: &'a Path, extra: &[&'a str]) -> Vec<&'a OsStr> {
    let mut args = vec![
        OsStr::new("display"),
        OsStr::new("fixture"),
        output.as_os_str(),
    ];
    args.extend(extra.iter().map(|value| OsStr::new(*value)));
    args
}

#[test]
fn strict_arguments_fail_without_waiting_capture_or_user_state() {
    let fixture = Fixture::new();
    let output = fixture.path("final.jpg");
    fixture.reject(&[], "Usage:");
    for (extra, error) in [
        (vec!["--delay-ms"], "Usage:"),
        (vec!["--delay-ms", "-1"], "Delay must be a whole"),
        (vec!["--delay-ms", "1.5"], "Delay must be a whole"),
        (vec!["--delay-ms", "3600001"], "Delay must be 0–3600000"),
        (
            vec!["--delay-ms", "1", "--delay-ms", "2"],
            "Duplicate --delay-ms",
        ),
        (vec!["--jpeg-quality", "0"], "JPEG quality must be 1..100"),
        (vec!["--jpeg-quality", "101"], "JPEG quality must be 1..100"),
        (
            vec!["--jpeg-quality", "256"],
            "JPEG quality must be a whole",
        ),
        (
            vec!["--jpeg-quality", "30", "--jpeg-quality", "40"],
            "Duplicate --jpeg-quality",
        ),
        (vec!["--recipe", "a", "--recipe", "b"], "Duplicate --recipe"),
        (vec!["--unknown", "x"], "Unknown capture-task option"),
    ] {
        fixture.reject(&arguments(&output, &extra), error);
    }
    let mut args = arguments(&output, &[]);
    args[0] = OsStr::new("unknown");
    fixture.reject(&args, "Usage:");
    args[0] = OsStr::new("display");
    args[1] = OsStr::new("");
    fixture.reject(&args, "Display ID must contain");
    let long_id = "x".repeat(65);
    args[1] = OsStr::new(&long_id);
    fixture.reject(&args, "Display ID must contain");
    assert!(!output.exists());
}

#[test]
fn lossy_webp_option_errors_precede_delay_and_capture() {
    let fixture = Fixture::new();
    let output = fixture.path("final.webp");
    for (extra, message) in [
        (vec!["--webp-quality", "0"], "WebP quality must be 1..100"),
        (vec!["--webp-quality", "101"], "WebP quality must be 1..100"),
        (vec!["--webp-quality", "-1"], "WebP quality must be a whole"),
        (
            vec!["--webp-quality", "1.5"],
            "WebP quality must be a whole",
        ),
        (
            vec!["--webp-quality", "20", "--webp-quality", "30"],
            "Duplicate --webp-quality",
        ),
        (
            vec!["--webp-quality", "20", "--jpeg-quality", "80"],
            "Use only one",
        ),
    ] {
        let mut options = extra;
        options.extend(["--delay-ms", "3600000"]);
        fixture.reject(&arguments(&output, &options), message);
    }
    fixture.reject(
        &arguments(&fixture.path("final.png"), &["--webp-quality", "20"]),
        "requires a WebP output",
    );
}

#[test]
fn region_bounds_and_upload_profiles_fail_before_wait_or_native_capture() {
    let fixture = Fixture::new();
    let output = fixture.path("region.png");
    for (values, message) in [
        (["-1", "0", "1", "1"], "Region coordinates must be whole"),
        (["0", "0", "0", "1"], "empty frame"),
        (["0", "0", "65536", "65536"], "256 MiB"),
        (["4294967295", "0", "2", "1"], "Region coordinates overflow"),
        (["0", "4294967295", "1", "2"], "Region coordinates overflow"),
        (["2147483648", "0", "1", "1"], "Region origin must fit"),
    ] {
        let mut args = vec![OsStr::new("region"), OsStr::new("fixture")];
        args.extend(values.iter().map(OsStr::new));
        args.push(output.as_os_str());
        args.extend([OsStr::new("--delay-ms"), OsStr::new("3600000")]);
        fixture.reject(&args, message);
    }
    let profile = fixture.path("invalid.json");
    std::fs::write(&profile, b"{}").unwrap();
    for target in ["display", "window"] {
        let args = vec![
            OsStr::new(target),
            OsStr::new("fixture"),
            output.as_os_str(),
            OsStr::new("--upload"),
            profile.as_os_str(),
            OsStr::new("--delay-ms"),
            OsStr::new("3600000"),
        ];
        fixture.reject(&args, "missing field");
    }
    let args = vec![
        OsStr::new("display"),
        OsStr::new("fixture"),
        output.as_os_str(),
        OsStr::new("--upload"),
        profile.as_os_str(),
        OsStr::new("--upload"),
        profile.as_os_str(),
    ];
    fixture.reject(&args, "Duplicate --upload");
    assert!(!output.exists());
}

#[test]
fn invalid_output_and_recipes_are_rejected_before_hour_long_delay() {
    let fixture = Fixture::new();
    let existing = fixture.path("existing.png");
    std::fs::write(&existing, b"preserve this file").unwrap();
    fixture.reject(
        &arguments(&existing, &["--delay-ms", "3600000"]),
        "Output already exists",
    );
    assert_eq!(std::fs::read(&existing).unwrap(), b"preserve this file");
    let unsupported = fixture.path("bad.gif");
    fixture.reject(
        &arguments(&unsupported, &["--delay-ms", "3600000"]),
        "Use a .png",
    );
    let png = fixture.path("final.png");
    fixture.reject(
        &arguments(&png, &["--jpeg-quality", "80"]),
        "requires a JPEG output",
    );
    for (text, message) in [
        ("{", "Invalid workflow recipe"),
        (
            r#"{"version":2,"steps":[{"op":"rotate","degrees":90}]}"#,
            "Workflow version must be 1",
        ),
        (r#"{"version":1,"steps":[]}"#, "Workflow must contain"),
        (
            r#"{"version":1,"steps":[{"op":"blur","x":0,"y":0,"width":0,"height":1}]}"#,
            "Invalid workflow step",
        ),
        (
            r#"{"version":1,"steps":[{"op":"rotate","degrees":90,"shell":"true"}]}"#,
            "Invalid workflow recipe",
        ),
    ] {
        let recipe = fixture.path("recipe.json");
        std::fs::write(&recipe, text).unwrap();
        let mut args = arguments(&png, &["--delay-ms", "3600000"]);
        args.extend([OsStr::new("--recipe"), recipe.as_os_str()]);
        fixture.reject(&args, message);
        assert!(!png.exists());
    }
    let recipe = fixture.path("huge.json");
    std::fs::write(&recipe, vec![b' '; 64 * 1024 + 1]).unwrap();
    let mut args = arguments(&png, &[]);
    args.extend([OsStr::new("--recipe"), recipe.as_os_str()]);
    fixture.reject(&args, "Workflow recipe exceeds 64 KiB");
}

#[cfg(unix)]
#[test]
fn broken_output_symlinks_and_non_utf8_ids_are_refused_before_native_capture() {
    use std::os::unix::{ffi::OsStringExt, fs::symlink};
    let fixture = Fixture::new();
    let output = fixture.path("broken.png");
    symlink(fixture.path("missing-target"), &output).unwrap();
    fixture.reject(&arguments(&output, &[]), "Output already exists");
    assert!(output.symlink_metadata().unwrap().file_type().is_symlink());
    let invalid_id = std::ffi::OsString::from_vec(vec![0xff]);
    let plain = fixture.path("plain.png");
    let mut args = arguments(&plain, &[]);
    args[1] = &invalid_id;
    fixture.reject(&args, "Display ID must be UTF-8");
    assert!(!plain.exists());
}
