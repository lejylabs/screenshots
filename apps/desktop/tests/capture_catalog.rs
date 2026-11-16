use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, Instant},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("screenshots-catalog-cli-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn command(&self, args: &[&std::ffi::OsStr]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .arg("capture-catalog")
            .args(args)
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
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
            command.env(name, self.path(name));
        }
        command
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn().unwrap();
        let started = Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                break;
            }
            if started.elapsed() > Duration::from_secs(5) {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("catalog fixture must not wait or access a desktop");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
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
        output
    }
    fn profile_catalog(&self) -> (PathBuf, PathBuf) {
        let catalog = self.path("catalog.json");
        let profile = self.path("task.json");
        std::fs::write(&profile, serde_json::to_vec(&serde_json::json!({"version":1,"source":{"type":"template","catalog":catalog,"name":"crop"},"actions":[{"type":"save","path":self.path("result-{uuid}.png")}]})).unwrap()).unwrap();
        std::fs::write(&catalog, serde_json::to_vec(&serde_json::json!({"version":1,"defaults":{"delay_ms":3600000},"templates":[{"name":"crop","monitor":{"kind":"connector","key":"x11:fixture"},"geometry":{"width":100,"height":80,"scale_factor":1.0,"rotation":0.0},"x":1,"y":2,"width":3,"height":4,"profile":profile,"hotkey":"Ctrl+Shift+F9"}]})).unwrap()).unwrap();
        (catalog, profile)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn args<'a>(command: &'a str, paths: &[&'a Path]) -> Vec<&'a std::ffi::OsStr> {
    let mut args = vec![std::ffi::OsStr::new(command)];
    args.extend(paths.iter().map(|path| path.as_os_str()));
    args
}
#[test]
fn validates_template_tasks_and_exports_native_host_bindings_without_capture_or_delay() {
    let f = Fixture::new();
    let (catalog, _) = f.profile_catalog();
    let output = f.command(&args("validate", &[&catalog]));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let host = f.path("host.json");
    let output = f.command(&args("host-config", &[&catalog, &host]));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = std::fs::read(&host).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["bindings"][0]["id"], "region-01");
    assert_eq!(value["bindings"][0]["hotkey"], "Ctrl+Shift+F9");
    assert_eq!(value["task_timeout_ms"], 5400000);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&host).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let output = f.command(&args("host-config", &[&catalog, &host]));
    assert!(!output.status.success());
    assert_eq!(std::fs::read(&host).unwrap(), bytes);
    assert_eq!(std::fs::read_dir(&f.0).unwrap().count(), 3);
}
#[test]
fn assigned_profile_must_capture_this_exact_template_and_catalog_before_dispatch() {
    let f = Fixture::new();
    let (catalog, profile) = f.profile_catalog();
    let mut stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&catalog).unwrap()).unwrap();
    let mut other = stored["templates"][0].clone();
    other["name"] = "wrong".into();
    other["profile"] = serde_json::Value::Null;
    other["hotkey"] = serde_json::Value::Null;
    stored["templates"].as_array_mut().unwrap().push(other);
    std::fs::write(&catalog, serde_json::to_vec(&stored).unwrap()).unwrap();
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&profile).unwrap()).unwrap();
    value["source"]["name"] = "wrong".into();
    std::fs::write(&profile, serde_json::to_vec(&value).unwrap()).unwrap();
    for command in ["validate", "host-config", "run-task"] {
        let host = f.path("host.json");
        let mut arguments = args(command, &[&catalog]);
        if command == "host-config" {
            arguments.push(host.as_os_str());
        }
        if command == "run-task" {
            arguments.push(std::ffi::OsStr::new("crop"));
        }
        let output = f.command(&arguments);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("source must reference this catalog and template")
        );
        assert!(!host.exists());
    }
    assert_eq!(std::fs::read_dir(&f.0).unwrap().count(), 2);
}
#[test]
fn malformed_options_and_existing_outputs_reject_before_native_binding() {
    let f = Fixture::new();
    let output = f.path("catalog.json");
    for extra in [
        vec!["--defaults", "[]"],
        vec!["--defaults", r#"{"delay_ms":3600001}"#],
        vec!["--hotkey", "F99"],
        vec!["--connector", "automatic"],
        vec!["--unknown", "true"],
    ] {
        let mut arguments = vec![
            std::ffi::OsStr::new("bind"),
            output.as_os_str(),
            std::ffi::OsStr::new("crop"),
            std::ffi::OsStr::new("fixture"),
            std::ffi::OsStr::new("0"),
            std::ffi::OsStr::new("0"),
            std::ffi::OsStr::new("2"),
            std::ffi::OsStr::new("3"),
        ];
        arguments.extend(extra.iter().map(std::ffi::OsStr::new));
        let result = f.command(&arguments);
        assert!(!result.status.success());
        assert!(!output.exists());
    }
    std::fs::write(&output, b"keep").unwrap();
    let result = f.command(&[
        std::ffi::OsStr::new("bind"),
        output.as_os_str(),
        std::ffi::OsStr::new("crop"),
        std::ffi::OsStr::new("fixture"),
        std::ffi::OsStr::new("0"),
        std::ffi::OsStr::new("0"),
        std::ffi::OsStr::new("2"),
        std::ffi::OsStr::new("3"),
    ]);
    assert!(!result.status.success());
    assert_eq!(std::fs::read(&output).unwrap(), b"keep");
}

#[test]
fn mismatched_source_or_catalog_never_executes_the_assigned_task() {
    let f = Fixture::new();
    let (catalog, profile) = f.profile_catalog();
    let other = f.path("other.json");
    std::fs::copy(&catalog, &other).unwrap();
    let result = f.command(&["run-task".as_ref(), other.as_os_str(), "crop".as_ref()]);
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("source must reference this catalog and template")
    );
    let source = f.path("source.png");
    image::RgbaImage::from_pixel(1, 1, image::Rgba([13, 17, 19, 255]))
        .save(&source)
        .unwrap();
    let wrong = f.path("must-not-exist.png");
    std::fs::write(&profile, serde_json::to_vec(&serde_json::json!({"version":1,"source":{"type":"file","path":source},"actions":[{"type":"save","path":wrong}]})).unwrap()).unwrap();
    let result = f.command(&["run-task".as_ref(), catalog.as_os_str(), "crop".as_ref()]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("task must use a template source"));
    assert!(!wrong.exists());
}
