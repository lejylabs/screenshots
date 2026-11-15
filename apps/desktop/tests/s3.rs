use std::{
    ffi::OsStr,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("screenshots-s3-cli-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn profile(&self, value: serde_json::Value) -> PathBuf {
        let path = self.0.join("profile.json");
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        path
    }
    fn run(&self, args: &[&OsStr], input: &[u8]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DBUS_SESSION_BUS_ADDRESS");
        for variable in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            command.env(variable, self.0.join(variable));
        }
        let mut child = command.spawn().unwrap();
        child.stdin.take().unwrap().write_all(input).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("S3 rejection must not wait on native services");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        for variable in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            assert!(!self.0.join(variable).exists());
        }
        output
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn valid() -> serde_json::Value {
    serde_json::json!({"endpoint":"https://example.invalid", "region":"auto", "bucket":"test-bucket", "credential":"main"})
}

#[test]
fn credential_target_is_scoped_without_secret_store_or_user_state() {
    let f = Fixture::new();
    let path = f.profile(valid());
    let output = f.run(
        &[
            "s3-credentials".as_ref(),
            "target".as_ref(),
            path.as_os_str(),
        ],
        b"",
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result["target"],
        "Screenshots/s3/https://example.invalid/test-bucket/main"
    );
}

#[test]
fn invalid_profiles_and_credentials_fail_before_native_or_network_access() {
    let f = Fixture::new();
    for (name, value) in [
        ("endpoint", "http://example.invalid"),
        ("endpoint", "https://user:secret@example.invalid"),
        ("bucket", "192.168.1.1"),
        ("bucket", "-bucket"),
        ("key_prefix", "../private"),
        ("credential", "bad/ref"),
    ] {
        let mut data = valid();
        data[name] = value.into();
        let path = f.profile(data);
        let output = f.run(
            &["s3-upload".as_ref(), path.as_os_str(), "missing".as_ref()],
            b"",
        );
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    let path = f.profile(valid());
    for input in [
        b"not json".to_vec(),
        br#"{"access_key_id":"A","secret_access_key":"SECRET-MARKER","unexpected":true}"#.to_vec(),
        vec![b'x'; 4097],
    ] {
        let output = f.run(
            &[
                "s3-credentials".as_ref(),
                "store".as_ref(),
                path.as_os_str(),
            ],
            &input,
        );
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("SECRET-MARKER"));
    }
}
