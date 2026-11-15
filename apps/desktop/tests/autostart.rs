#[cfg(target_os = "linux")]
mod linux {
    use serde_json::Value;
    use std::{
        os::unix::fs::{PermissionsExt, symlink},
        path::PathBuf,
        process::{Command, Output, Stdio},
    };

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir()
                .join(format!("screenshots-autostart-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }

        fn entry(&self) -> PathBuf {
            self.path("config/autostart/me.lejy.screenshots.desktop")
        }

        fn command(&self) -> Command {
            let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
            command
                .arg("autostart")
                .env("XDG_CONFIG_HOME", self.path("config"))
                .env("HOME", self.path("home"))
                .env("USERPROFILE", self.path("profile"))
                .env("APPDATA", self.path("appdata"))
                .env("LOCALAPPDATA", self.path("localappdata"))
                .env("XDG_DATA_HOME", self.path("data"))
                .env("XDG_CACHE_HOME", self.path("cache"))
                .env_remove("DISPLAY")
                .env_remove("WAYLAND_DISPLAY")
                .env_remove("DBUS_SESSION_BUS_ADDRESS");
            command
        }

        fn run(&self, action: &str) -> Output {
            let mut child = self
                .command()
                .arg(action)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let started = std::time::Instant::now();
            loop {
                if child.try_wait().unwrap().is_some() {
                    return child.wait_with_output().unwrap();
                }
                if started.elapsed() >= std::time::Duration::from_secs(5) {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("autostart command timed out");
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }

        fn status(&self, action: &str, expected: &str) -> Value {
            let output = self.run(action);
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(output.stderr.is_empty());
            let value: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(value["status"], expected);
            assert_eq!(value["scope"], "managed_user_entry");
            value
        }

        fn no_app_state(&self) {
            for name in [
                "home",
                "profile",
                "appdata",
                "localappdata",
                "data",
                "cache",
            ] {
                assert!(!self.path(name).exists(), "{name}");
            }
        }

        fn prepare_parent(&self) {
            std::fs::create_dir_all(self.entry().parent().unwrap()).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn explicit_enable_status_disable_is_idempotent_and_does_not_start_ui() {
        let fixture = Fixture::new();
        fixture.status("status", "disabled");
        fixture.status("disable", "disabled");
        assert!(!fixture.path("config").exists());
        let value = fixture.status("enable", "enabled");
        assert_eq!(value["path"], fixture.entry().to_str().unwrap());
        let original = std::fs::read(fixture.entry()).unwrap();
        let contents = String::from_utf8(original.clone()).unwrap();
        assert!(contents.starts_with("# Managed by screenshots autostart v1;"));
        assert!(contents.contains("\nExec=\""));
        assert!(contents.contains("\" --show\n"));
        assert!(contents.contains(env!("CARGO_BIN_EXE_screenshots")));
        assert_eq!(
            std::fs::metadata(fixture.entry())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fixture.status("enable", "enabled");
        fixture.status("status", "enabled");
        assert_eq!(std::fs::read(fixture.entry()).unwrap(), original);
        assert_eq!(
            std::fs::read_dir(fixture.entry().parent().unwrap())
                .unwrap()
                .count(),
            1
        );
        if Command::new("desktop-file-validate")
            .arg("--version")
            .output()
            .is_ok()
        {
            let output = Command::new("desktop-file-validate")
                .arg(fixture.entry())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        fixture.status("disable", "disabled");
        fixture.status("disable", "disabled");
        assert!(!fixture.entry().exists());
        fixture.no_app_state();
    }

    #[test]
    fn foreign_edited_oversized_and_special_entries_are_preserved() {
        for kind in [
            "foreign",
            "edited",
            "oversized",
            "directory",
            "symlink",
            "fifo",
        ] {
            let fixture = Fixture::new();
            fixture.prepare_parent();
            match kind {
                "foreign" => {
                    std::fs::write(fixture.entry(), b"[Desktop Entry]\nExec=foreign\n").unwrap()
                }
                "edited" => {
                    fixture.status("enable", "enabled");
                    let mut bytes = std::fs::read(fixture.entry()).unwrap();
                    bytes.extend_from_slice(b"Hidden=true\n");
                    std::fs::write(fixture.entry(), bytes).unwrap();
                }
                "oversized" => std::fs::write(fixture.entry(), vec![b'x'; 16 * 1024 + 1]).unwrap(),
                "directory" => std::fs::create_dir(fixture.entry()).unwrap(),
                "symlink" => {
                    std::fs::write(fixture.path("target"), b"target stays unchanged").unwrap();
                    symlink(fixture.path("target"), fixture.entry()).unwrap();
                }
                "fifo" => {
                    use std::os::unix::ffi::OsStrExt;
                    let path =
                        std::ffi::CString::new(fixture.entry().as_os_str().as_bytes()).unwrap();
                    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
                }
                _ => unreachable!(),
            }
            let before = std::fs::symlink_metadata(fixture.entry()).unwrap();
            let bytes = before
                .is_file()
                .then(|| std::fs::read(fixture.entry()).unwrap());
            fixture.status("status", "conflict");
            for action in ["enable", "disable"] {
                let output = fixture.run(action);
                assert!(!output.status.success(), "{kind} {action}");
                assert!(output.stdout.is_empty());
                assert!(String::from_utf8_lossy(&output.stderr).contains("preserved"));
            }
            let after = std::fs::symlink_metadata(fixture.entry()).unwrap();
            assert_eq!(before.file_type(), after.file_type());
            if let Some(bytes) = bytes {
                assert_eq!(std::fs::read(fixture.entry()).unwrap(), bytes);
            }
            if kind == "symlink" {
                assert_eq!(
                    std::fs::read(fixture.path("target")).unwrap(),
                    b"target stays unchanged"
                );
            }
            fixture.no_app_state();
        }
    }

    #[test]
    fn home_fallback_and_invalid_configurations_do_not_touch_unrelated_state() {
        let fixture = Fixture::new();
        let output = fixture
            .command()
            .env_remove("XDG_CONFIG_HOME")
            .arg("enable")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(
            fixture
                .path("home/.config/autostart/me.lejy.screenshots.desktop")
                .is_file()
        );
        assert!(!fixture.path("config").exists());
        for (name, value) in [("XDG_CONFIG_HOME", "relative"), ("HOME", "relative")] {
            let output = fixture
                .command()
                .env_remove("XDG_CONFIG_HOME")
                .env(name, value)
                .arg("enable")
                .output()
                .unwrap();
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            assert!(String::from_utf8_lossy(&output.stderr).contains("absolute"));
        }
        for args in [vec![], vec!["invalid"], vec!["enable", "extra"]] {
            let output = fixture.command().args(args).output().unwrap();
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
        }
        assert!(!fixture.path("config").exists());
    }

    #[test]
    fn fresh_invalid_arguments_and_configurations_create_no_paths() {
        let fixture = Fixture::new();
        for args in [vec![], vec!["invalid"], vec!["enable", "extra"]] {
            let output = fixture.command().args(args).output().unwrap();
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
        }
        for (name, value) in [("XDG_CONFIG_HOME", "relative"), ("HOME", "relative")] {
            let output = fixture
                .command()
                .env_remove("XDG_CONFIG_HOME")
                .env(name, value)
                .arg("enable")
                .output()
                .unwrap();
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
        }
        assert!(std::fs::read_dir(&fixture.0).unwrap().next().is_none());
    }
}

#[cfg(not(target_os = "linux"))]
#[test]
fn other_platforms_report_explicitly_unsupported() {
    let root = std::env::temp_dir().join(format!("screenshots-autostart-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    for action in ["status", "enable", "disable"] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_screenshots"))
            .args(["autostart", action])
            .env("HOME", root.join("home"))
            .env("USERPROFILE", root.join("profile"))
            .env("APPDATA", root.join("appdata"))
            .env("LOCALAPPDATA", root.join("localappdata"))
            .env("XDG_CONFIG_HOME", root.join("config"))
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("supported only on Linux"));
    }
    assert!(std::fs::read_dir(&root).unwrap().next().is_none());
    std::fs::remove_dir(root).unwrap();
}
