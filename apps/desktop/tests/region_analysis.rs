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
            "screenshots-region-analysis-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn reject(&self, name: &str, args: &[OsString], expected: &str, wayland: bool) {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .arg(name)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("XDG_SESSION_TYPE")
            .env_remove("DBUS_SESSION_BUS_ADDRESS");
        let variables = [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ];
        for key in variables {
            command.env(key, self.0.join(key));
        }
        if wayland {
            command
                .env("DISPLAY", ":9876")
                .env("WAYLAND_DISPLAY", "wayland-fixture")
                .env("XDG_SESSION_TYPE", "wayland");
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("region argument/session rejection must not wait for native capture");
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
        assert_eq!(
            std::fs::read_dir(&self.0).unwrap().count(),
            0,
            "headless rejection changed user state"
        );
    }
}

fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

#[test]
fn invalid_region_arguments_fail_before_any_native_or_engine_access() {
    let fixture = Fixture::new();
    for name in ["ocr-region", "qr-region"] {
        fixture.reject(name, &[], "Usage:", false);
        fixture.reject(
            name,
            &args(&["", "0", "0", "1", "1"]),
            "Display ID must contain",
            false,
        );
        fixture.reject(
            name,
            &args(&["fixture", "-1", "0", "1", "1"]),
            "whole nonnegative",
            false,
        );
        fixture.reject(
            name,
            &args(&["fixture", "2147483648", "0", "1", "1"]),
            "coordinates overflow",
            false,
        );
        fixture.reject(
            name,
            &args(&["fixture", "0", "0", "2049", "2048"]),
            "16 MiB",
            false,
        );
        fixture.reject(
            name,
            &args(&["fixture", "0", "0", "0", "1"]),
            "empty frame",
            false,
        );
    }
    fixture.reject(
        "qr-region",
        &args(&["fixture", "0", "0", "4097", "1"]),
        "4096 pixels",
        false,
    );
    fixture.reject(
        "ocr-region",
        &args(&["fixture", "0", "0", "1", "1", "--language", "eng;private"]),
        "OCR language",
        false,
    );
    fixture.reject(
        "qr-region",
        &args(&["fixture", "0", "0", "1", "1", "--language", "eng"]),
        "Usage:",
        false,
    );
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let mut values = args(&["fixture", "0", "0", "1", "1"]);
        values[0] = OsString::from_vec(vec![255]);
        fixture.reject("qr-region", &values, "Display ID must be UTF-8", false);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn missing_x11_and_wayland_fail_without_portal_or_user_state() {
    let fixture = Fixture::new();
    for name in ["ocr-region", "qr-region"] {
        for wayland in [false, true] {
            fixture.reject(
                name,
                &args(&["fixture", "0", "0", "100", "100"]),
                "requires an X11 session",
                wayland,
            );
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn portal_analysis_preflight_rejects_invalid_arguments_without_permission_or_state() {
    let fixture = Fixture::new();
    for name in ["ocr-portal-region", "qr-portal-region"] {
        fixture.reject(name, &[], "Usage:", true);
        fixture.reject(name, &args(&["fixture", "0", "0", "1", "1"]), "source must be", true);
        fixture.reject(name, &args(&["display", "0", "0", "2049", "2048"]), "16 MiB", true);
        fixture.reject(name, &args(&["display", "4294967295", "0", "2", "1"]), "coordinates overflow", true);
        fixture.reject(name, &args(&["window", "-1", "0", "1", "1"]), "whole nonnegative", true);
    }
    fixture.reject("ocr-portal-region", &args(&["display", "0", "0", "1", "1", "--language", "../eng"]), "OCR language", true);
    fixture.reject("qr-portal-region", &args(&["display", "0", "0", "4097", "1"]), "4096 pixels", true);
    fixture.reject("qr-portal-region", &args(&["window", "0", "0", "1", "1", "--language", "eng"]), "Usage:", true);
}

#[cfg(not(target_os = "linux"))]
#[test]
fn portal_analysis_refuses_other_platforms_after_preflight() {
    let fixture = Fixture::new();
    for name in ["ocr-portal-region", "qr-portal-region"] {
        fixture.reject(name, &args(&["display", "0", "0", "1", "1"]), "requires Linux", false);
    }
}
