use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("screenshots-oauth-cli-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn profile(&self, value: serde_json::Value) -> PathBuf {
        let path = self.path("oauth.json");
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        path
    }
    fn run(&self, args: &[&OsStr]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
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
        for key in ["DBUS_SESSION_BUS_ADDRESS", "DISPLAY", "WAYLAND_DISPLAY"] {
            command.env_remove(key);
        }
        let mut child = command.spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().unwrap().is_some() {
                return child.wait_with_output().unwrap();
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("OAuth admission blocked");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
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
            assert!(
                !self.path(key).exists(),
                "unexpected persistent state: {key}"
            );
        }
    }
    fn destination(&self) {
        std::fs::write(self.path("upload.json"),br#"{"url":"https://example.invalid/upload","method":"POST","field":"file","json_pointer":"/url","credential":"fixture","authentication":"bearer"}"#).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn profile() -> serde_json::Value {
    serde_json::json!({"version":1,"flow":"authorization_code","client_id":"synthetic-client","token_endpoint":"https://identity.invalid/token","authorization_endpoint":"https://identity.invalid/authorize","scopes":["upload"],"credential":"fixture","upload_destination":"upload.json"})
}
fn failed(output: Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("MUST_NOT_ECHO_PLAINTEXT"));
}
#[test]
fn targets_are_admitted_without_any_native_secret_store_or_persistent_state() {
    let fixture = Fixture::new();
    fixture.destination();
    let path = fixture.profile(profile());
    let original = std::fs::read(&path).unwrap();
    let result = fixture.run(&[OsStr::new("oauth"), OsStr::new("target"), path.as_os_str()]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        value["upload_target"],
        "Screenshots/upload/https://example.invalid/fixture"
    );
    assert!(
        value["refresh_target"]
            .as_str()
            .unwrap()
            .starts_with("Screenshots/provider/oauth/")
    );
    assert_eq!(std::fs::read(path).unwrap(), original);
    fixture.no_state();
}
#[test]
fn invalid_profiles_and_plaintext_secrets_are_refused_before_keyring_or_network() {
    let fixture = Fixture::new();
    fixture.destination();
    for (field, bad) in [
        ("version", 2.into()),
        ("client_secret", "MUST_NOT_ECHO_PLAINTEXT".into()),
        ("token_endpoint", "http://identity.invalid/token".into()),
        ("credential", "wrong-reference".into()),
        (
            "token_parameters",
            serde_json::json!({"client_secret":"MUST_NOT_ECHO_PLAINTEXT"}),
        ),
    ] {
        let mut value = profile();
        value[field] = bad;
        let path = fixture.profile(value);
        failed(fixture.run(&[OsStr::new("oauth"), OsStr::new("login"), path.as_os_str()]));
    }
    fixture.no_state();
}
#[test]
fn development_http_is_explicit_literal_loopback_and_cli_arguments_are_strict() {
    let fixture = Fixture::new();
    fixture.destination();
    let mut value = profile();
    value["token_endpoint"] = "http://127.0.0.1:5678/token".into();
    let path = fixture.profile(value.clone());
    failed(fixture.run(&[OsStr::new("oauth"), OsStr::new("target"), path.as_os_str()]));
    let result = fixture.run(&[
        OsStr::new("oauth"),
        OsStr::new("target"),
        path.as_os_str(),
        OsStr::new("--allow-loopback-http"),
    ]);
    assert!(result.status.success());
    for host in ["localhost", "127.1", "192.0.2.1"] {
        value["token_endpoint"] = format!("http://{host}:5678/token").into();
        let path = fixture.profile(value.clone());
        failed(fixture.run(&[
            OsStr::new("oauth"),
            OsStr::new("target"),
            path.as_os_str(),
            OsStr::new("--allow-loopback-http"),
        ]));
    }
    for args in [
        [
            OsStr::new("oauth"),
            OsStr::new("unknown"),
            Path::new("unused").as_os_str(),
        ],
        [
            OsStr::new("oauth"),
            OsStr::new("target"),
            OsStr::new("--unexpected"),
        ],
    ] {
        failed(fixture.run(&args));
    }
    fixture.no_state();
}
