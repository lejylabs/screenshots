use std::{
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
const SECRET: &str = "DO_NOT_ECHO_PROVIDER_SECRET";
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("screenshots-provider-cli-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn profile(&self, name: &str, value: &serde_json::Value) -> PathBuf {
        let path = self.path(name);
        std::fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        path
    }
    fn run(&self, args: &[&std::ffi::OsStr], input: &[u8]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("PATH", self.path("no-engine"));
        for key in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            command.env(key, self.path(key));
        }
        for key in [
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "DBUS_SESSION_BUS_ADDRESS",
            "SSH_AUTH_SOCK",
        ] {
            command.env_remove(key);
        }
        let mut child = command.spawn().unwrap();
        if let Err(error) = child.stdin.take().unwrap().write_all(input) {
            assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("Provider fixture exceeded deadline")
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
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn profile() -> serde_json::Value {
    serde_json::json!({"version":1,"protocol":"ftps","host":"example.invalid","port":21,"remote_directory":"/uploads","credential":"fixture"})
}
fn failed(result: Output) -> String {
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    let text = String::from_utf8_lossy(&result.stderr).into_owned();
    assert!(!text.contains(SECRET));
    text
}

#[test]
fn profile_and_source_errors_precede_credentials_or_network() {
    let fixture = Fixture::new();
    let source = fixture.path("source.bin");
    std::fs::write(&source, b"unchanged").unwrap();
    let original = std::fs::read(&source).unwrap();
    for (field, value) in [
        ("protocol", "invalid".into()),
        ("host", format!("{SECRET}@example.invalid").into()),
        ("remote_directory", "/../private".into()),
        ("password", SECRET.into()),
        ("allow_plaintext", true.into()),
        (
            "public_base",
            format!("https://user:{SECRET}@example.invalid/").into(),
        ),
    ] {
        let mut invalid = profile();
        invalid[field] = value;
        let path = fixture.profile("invalid.json", &invalid);
        failed(fixture.run(
            &[
                "provider-upload".as_ref(),
                path.as_os_str(),
                source.as_os_str(),
            ],
            b"",
        ));
    }
    let path = fixture.profile("valid.json", &profile());
    let before = std::fs::read(&path).unwrap();
    for source in [fixture.path("missing.bin"), fixture.0.clone()] {
        failed(fixture.run(
            &[
                "provider-upload".as_ref(),
                path.as_os_str(),
                source.as_os_str(),
            ],
            b"",
        ));
    }
    let oversized = fixture.path("too-large.bin");
    std::fs::File::create(&oversized)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    let error = failed(fixture.run(
        &[
            "provider-upload".as_ref(),
            path.as_os_str(),
            oversized.as_os_str(),
        ],
        b"",
    ));
    assert!(error.contains("64 MiB"), "{error}");
    assert_eq!(std::fs::read(source).unwrap(), original);
    assert_eq!(std::fs::read(path).unwrap(), before);
    fixture.no_state();
}

#[test]
fn plaintext_ftp_and_sftp_without_a_known_host_pin_are_rejected() {
    let fixture = Fixture::new();
    let source = fixture.path("source.bin");
    std::fs::write(&source, b"data").unwrap();
    for protocol in ["ftp", "sftp"] {
        let mut value = profile();
        value["protocol"] = protocol.into();
        let path = fixture.profile("profile.json", &value);
        failed(fixture.run(
            &[
                "provider-upload".as_ref(),
                path.as_os_str(),
                source.as_os_str(),
            ],
            b"",
        ));
    }
    fixture.no_state();
}

#[test]
fn credential_targets_are_scoped_and_invalid_secrets_do_not_touch_native_storage() {
    let fixture = Fixture::new();
    let path = fixture.profile("valid.json", &profile());
    let output = fixture.run(
        &[
            "provider-credentials".as_ref(),
            "target".as_ref(),
            path.as_os_str(),
        ],
        b"",
    );
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let target: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let name = target["target"].as_str().unwrap();
    assert!(name.starts_with("Screenshots/provider/ftps/"));
    assert!(name.ends_with("/fixture"));
    assert!(!name.contains("example.invalid"));
    let mut changed = profile();
    changed["port"] = 2121.into();
    let other = fixture.profile("other.json", &changed);
    let output = fixture.run(
        &[
            "provider-credentials".as_ref(),
            "target".as_ref(),
            other.as_os_str(),
        ],
        b"",
    );
    assert!(output.status.success());
    assert_ne!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        target
    );
    for input in [
        format!(r#"{{"kind":"password","username":"user:extra","password":"{SECRET}"}}"#)
            .into_bytes(),
        format!(r#"{{"kind":"password","username":"user","password":"{SECRET}\n"}}"#).into_bytes(),
        format!(r#"{{"kind":"password","username":"user","password":"{SECRET}","extra":1}}"#)
            .into_bytes(),
        vec![b'x'; 4097],
    ] {
        let error = failed(fixture.run(
            &[
                "provider-credentials".as_ref(),
                "store".as_ref(),
                path.as_os_str(),
            ],
            &input,
        ));
        assert!(!error.contains("Secret Service"));
    }
    fixture.no_state();
}

#[cfg(unix)]
#[test]
fn provider_fifo_input_is_refused_before_opening_it() {
    use std::os::unix::ffi::OsStrExt;
    let fixture = Fixture::new();
    let path = fixture.profile("valid.json", &profile());
    let source = fixture.path("input.fifo");
    let native = std::ffi::CString::new(source.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
    let error = failed(fixture.run(
        &[
            "provider-upload".as_ref(),
            path.as_os_str(),
            source.as_os_str(),
        ],
        b"",
    ));
    assert!(error.contains("regular file"));
    fixture.no_state();
}
