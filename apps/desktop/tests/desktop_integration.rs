use std::{
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("screenshots-desktop-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .arg("desktop")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in [
            "HOME",
            "USERPROFILE",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "APPDATA",
            "LOCALAPPDATA",
        ] {
            command.env(key, self.path(key));
        }
        command
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DBUS_SESSION_BUS_ADDRESS");
        command
    }
    fn run(&self, args: &[&str]) -> Output {
        wait(self.command().args(args).spawn().unwrap())
    }
    fn no_state(&self) {
        for key in [
            "HOME",
            "USERPROFILE",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
            "APPDATA",
            "LOCALAPPDATA",
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
fn wait(mut child: std::process::Child) -> Output {
    let deadline = Instant::now() + Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Desktop fixture exceeded its deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}
fn success(output: Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    serde_json::from_slice(&output.stdout).unwrap()
}
fn failure(output: Output) {
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
fn invalid_commands_and_messages_fail_before_any_desktop_side_effect() {
    let f = Fixture::new();
    for args in [
        vec![],
        vec!["associations"],
        vec!["associations", "reset-defaults"],
        vec!["notify"],
        vec!["notify", "", "body"],
        vec!["notify", "title", ""],
        vec!["notify", "bad\ntitle", "body"],
        vec!["notify", "title", "bad\u{0007}body"],
    ] {
        failure(f.run(&args));
        f.no_state();
    }
    failure(f.run(&["notify", &"a".repeat(129), "body"]));
    failure(f.run(&["notify", "title", &"b".repeat(2049)]));
    f.no_state();
}

#[cfg(target_os = "linux")]
#[test]
fn linux_association_roundtrip_is_user_scoped_and_preserves_foreign_data() {
    let f = Fixture::new();
    let path = f.path("XDG_DATA_HOME/applications/me.lejy.screenshots.desktop");
    assert_eq!(
        success(f.run(&["associations", "status"]))["status"],
        "disabled"
    );
    assert_eq!(
        success(f.run(&["associations", "enable"]))["status"],
        "enabled"
    );
    let bytes = std::fs::read(&path).unwrap();
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains(" --open %f\n"));
    assert!(text.contains("MimeType=image/png;image/jpeg;image/webp;image/bmp;image/tiff;"));
    assert_eq!(
        success(f.run(&["associations", "enable"]))["status"],
        "enabled"
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(
        success(f.run(&["associations", "status"]))["status"],
        "enabled"
    );
    let defaults = f.path("XDG_DATA_HOME/applications/mimeapps.list");
    std::fs::write(
        &defaults,
        b"[Default Applications]\nimage/png=other.desktop\n",
    )
    .unwrap();
    assert_eq!(
        success(f.run(&["associations", "disable"]))["status"],
        "disabled"
    );
    assert!(!path.exists());
    assert_eq!(
        std::fs::read(defaults).unwrap(),
        b"[Default Applications]\nimage/png=other.desktop\n"
    );
    assert_eq!(
        success(f.run(&["associations", "disable"]))["status"],
        "disabled"
    );
    std::fs::write(&path, b"foreign desktop entry").unwrap();
    assert_eq!(
        success(f.run(&["associations", "status"]))["status"],
        "conflict"
    );
    failure(f.run(&["associations", "enable"]));
    failure(f.run(&["associations", "disable"]));
    assert_eq!(std::fs::read(path).unwrap(), b"foreign desktop entry");
}

#[cfg(target_os = "linux")]
#[test]
fn linux_association_rejects_special_entries_and_invalid_data_home() {
    use std::os::unix::{ffi::OsStrExt, fs::symlink};
    let f = Fixture::new();
    let path = f.path("XDG_DATA_HOME/applications/me.lejy.screenshots.desktop");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    symlink(f.path("missing"), &path).unwrap();
    assert_eq!(
        success(f.run(&["associations", "status"]))["status"],
        "conflict"
    );
    failure(f.run(&["associations", "disable"]));
    assert!(path.symlink_metadata().unwrap().file_type().is_symlink());
    std::fs::remove_file(&path).unwrap();
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
    assert_eq!(
        success(f.run(&["associations", "status"]))["status"],
        "conflict"
    );
    failure(f.run(&["associations", "enable"]));
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, vec![b'x'; 16385]).unwrap();
    assert_eq!(
        success(f.run(&["associations", "status"]))["status"],
        "conflict"
    );
    failure(f.run(&["associations", "disable"]));
    failure(wait(
        f.command()
            .args(["associations", "status"])
            .env("XDG_DATA_HOME", "relative")
            .spawn()
            .unwrap(),
    ));
}

#[cfg(target_os = "linux")]
fn notifier(f: &Fixture, source: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let directory = f.path("helpers");
    std::fs::create_dir(&directory).unwrap();
    let path = directory.join("notify-send");
    std::fs::write(&path, format!("#!/bin/sh\n{source}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    directory
}

#[cfg(target_os = "linux")]
#[test]
fn notification_uses_literal_arguments_and_reports_only_submission() {
    let f = Fixture::new();
    let capture = f.path("arguments");
    let helpers = notifier(&f, "printf '%s\\0' \"$@\" > \"$NOTIFY_CAPTURE\"");
    let output = wait(
        f.command()
            .args(["notify", "A $`\" title", "<tag>& body\nsecond"])
            .env("PATH", helpers)
            .env("NOTIFY_CAPTURE", &capture)
            .spawn()
            .unwrap(),
    );
    assert_eq!(
        success(output),
        serde_json::json!({"status":"submitted","platform":"linux"})
    );
    let bytes = std::fs::read(capture).unwrap();
    let args = bytes
        .split(|byte| *byte == 0)
        .map(|value| String::from_utf8_lossy(value).into_owned())
        .collect::<Vec<_>>();
    assert!(args.contains(&"A $`\" title".into()));
    assert!(args.contains(&"&lt;tag&gt;&amp; body\nsecond".into()));
    assert!(!args.contains(&"<tag>& body\nsecond".into()));
    f.no_state();
}

#[cfg(target_os = "linux")]
#[test]
fn notification_failures_are_bounded_and_do_not_echo_message_content() {
    let f = Fixture::new();
    let secret = "NOTIFICATION_TEXT_MUST_NOT_APPEAR";
    let helpers = notifier(&f, &format!("printf '%s' '{secret}' >&2\nexit 3"));
    let result = wait(
        f.command()
            .args(["notify", secret, secret])
            .env("PATH", &helpers)
            .spawn()
            .unwrap(),
    );
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&result.stderr).contains(secret));
    std::fs::write(helpers.join("notify-send"), "#!/bin/sh\n/bin/sleep 20\n").unwrap();
    let started = Instant::now();
    let result = wait(
        f.command()
            .args(["notify", "title", "body"])
            .env("PATH", helpers)
            .spawn()
            .unwrap(),
    );
    failure(result);
    assert!(started.elapsed() < Duration::from_secs(9));
    f.no_state();
}
