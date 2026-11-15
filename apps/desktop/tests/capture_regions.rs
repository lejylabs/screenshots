use std::{
    ffi::OsString,
    path::PathBuf,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "screenshots-capture-regions-cli-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("out")).unwrap();
        Self(root)
    }

    fn args(&self, groups: &[&str]) -> Vec<OsString> {
        std::iter::once(self.0.join("out").into_os_string())
            .chain(groups.iter().map(OsString::from))
            .collect()
    }

    fn reject(&self, args: &[OsString], expected: &str, wayland: bool) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .arg("capture-regions")
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("XDG_SESSION_TYPE")
            .env_remove("DBUS_SESSION_BUS_ADDRESS");
        for name in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            command.env(name, self.0.join(name));
        }
        if wayland {
            command
                .env("DISPLAY", ":9876")
                .env("WAYLAND_DISPLAY", "private-fixture")
                .env("XDG_SESSION_TYPE", "wayland");
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("capture-regions rejection stalled before capture");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        for name in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            assert!(!self.0.join(name).exists());
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn malformed_groups_and_crop_budget_fail_without_native_capture() {
    let f = Fixture::new();
    f.reject(&[], "Usage:", false);
    f.reject(&f.args(&[]), "Usage:", false);
    f.reject(&f.args(&["a", "0", "0", "1"]), "Usage:", false);
    for (groups, error) in [
        (vec!["a", "-1", "0", "1", "1"], "whole nonnegative"),
        (vec!["a", "+1", "0", "1", "1"], "whole nonnegative"),
        (vec!["a", "4294967295", "0", "1", "1"], "overflow"),
        (vec!["a", "0", "0", "0", "1"], "empty frame"),
        (vec!["", "0", "0", "1", "1"], "Display ID"),
        (
            vec!["a", "0", "0", "8192", "4096", "a", "0", "0", "8192", "4097"],
            "crops exceed",
        ),
    ] {
        f.reject(&f.args(&groups), error, false);
    }
    f.reject(
        &f.args(&["a", "0", "0", "1", "1"].repeat(17)),
        "Usage:",
        false,
    );
    assert_eq!(std::fs::read_dir(f.0.join("out")).unwrap().count(), 0);
}

#[test]
fn output_directory_failures_precede_native_capture() {
    let f = Fixture::new();
    std::fs::write(f.0.join("out/keep"), b"unchanged").unwrap();
    f.reject(&f.args(&["a", "0", "0", "1", "1"]), "must be empty", false);
    assert_eq!(std::fs::read(f.0.join("out/keep")).unwrap(), b"unchanged");
    let mut args = f.args(&["a", "0", "0", "1", "1"]);
    args[0] = f.0.join("missing").into_os_string();
    f.reject(&args, "screenshots:", false);
    assert!(!f.0.join("missing").exists());
}

#[cfg(target_os = "linux")]
#[test]
fn wayland_is_rejected_before_display_or_portal_access() {
    let f = Fixture::new();
    f.reject(
        &f.args(&["a", "0", "0", "1", "1"]),
        "requires an X11 session",
        true,
    );
    assert_eq!(std::fs::read_dir(f.0.join("out")).unwrap().count(), 0);
}
