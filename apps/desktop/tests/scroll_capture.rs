use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("screenshots-scroll-cli-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn command(&self, output: &Path, values: &[&str]) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        child
            .arg("scroll-capture")
            .arg("1")
            .arg(output)
            .args(values)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        for key in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            child.env(key, self.path(key));
        }
        for key in ["DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS"] {
            child.env_remove(key);
        }
        child.env("XDG_SESSION_TYPE", "x11");
        let mut child = child.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Scroll admission fixture timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.wait_with_output().unwrap()
    }
    fn no_state(&self) {
        for key in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            assert!(!self.path(key).exists());
        }
        assert!(self.0.read_dir().unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".part")
        }));
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn rejected(result: Output) {
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(!result.stderr.is_empty());
}

#[test]
fn scrolling_invalid_admission_never_touches_native_input_or_user_state() {
    let f = Fixture::new();
    let output = f.path("capture.png");
    let options = [
        vec![],
        vec!["--region", "0", "0", "0", "64"],
        vec!["--region", "0", "0", "64", "0"],
        vec!["--region", "0", "0", "4097", "64"],
        vec!["--region", "0", "0", "4096", "4096"],
        vec!["--region", "4294967295", "0", "64", "64"],
        vec!["--region", "0", "0", "64", "64", "--steps", "0"],
        vec!["--region", "0", "0", "64", "64", "--steps", "129"],
        vec!["--region", "0", "0", "64", "64", "--wheel", "11"],
        vec!["--region", "0", "0", "64", "64", "--settle-ms", "19"],
        vec!["--region", "0", "0", "64", "64", "--delay-ms", "10001"],
        vec![
            "--region",
            "0",
            "0",
            "64",
            "64",
            "--delay-ms",
            "1",
            "--steps",
            "50",
            "--settle-ms",
            "2000",
        ],
        vec![
            "--region",
            "0",
            "0",
            "64",
            "64",
            "--steps",
            "128",
            "--settle-ms",
            "2000",
        ],
        vec!["--region", "0", "0", "64", "64", "--tolerance", "17"],
        vec!["--region", "0", "0", "64", "64", "--trim-top", "63"],
        vec!["--region", "0", "0", "64", "64", "--min-overlap", "64"],
        vec![
            "--region", "0", "0", "64", "64", "--wheel", "1", "--wheel", "1",
        ],
        vec!["--region", "0", "0", "64", "64", "--shell", "ignored"],
    ];
    for values in options {
        rejected(f.command(&output, &values));
        assert!(!output.exists());
        f.no_state();
    }
    rejected(f.command(&f.path("wrong.jpg"), &["--region", "0", "0", "64", "64"]));
    f.no_state();
}

#[test]
fn scrolling_existing_outputs_and_broken_symlinks_are_never_replaced() {
    let f = Fixture::new();
    let output = f.path("capture.png");
    std::fs::write(&output, b"unchanged").unwrap();
    let values = ["--region", "0", "0", "64", "64"];
    rejected(f.command(&output, &values));
    assert_eq!(std::fs::read(&output).unwrap(), b"unchanged");
    rejected(f.command(&f.0, &values));
    #[cfg(unix)]
    {
        let broken = f.path("broken.png");
        std::os::unix::fs::symlink(f.path("missing"), &broken).unwrap();
        rejected(f.command(&broken, &values));
        assert!(broken.symlink_metadata().unwrap().file_type().is_symlink());
    }
    f.no_state();
}

#[cfg(not(target_os = "windows"))]
#[test]
fn scrolling_unavailable_native_session_returns_no_image() {
    let f = Fixture::new();
    let output = f.path("capture.png");
    rejected(f.command(&output, &["--region", "0", "0", "64", "64"]));
    assert!(!output.exists());
    f.no_state();
}

#[cfg(unix)]
#[test]
fn scrolling_non_utf8_window_id_is_rejected() {
    use std::os::unix::ffi::OsStringExt;
    let f = Fixture::new();
    let result = Command::new(env!("CARGO_BIN_EXE_screenshots"))
        .arg("scroll-capture")
        .arg(OsString::from_vec(vec![0xff]))
        .arg(f.path("capture.png"))
        .args(["--region", "0", "0", "64", "64"])
        .output()
        .unwrap();
    rejected(result);
    assert!(!f.path("capture.png").exists());
    f.no_state();
}
