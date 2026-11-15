use image::{Rgba, RgbaImage};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "screenshots-cli-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .env("HOME", self.path("home"))
            .env("USERPROFILE", self.path("userprofile"))
            .env("APPDATA", self.path("appdata"))
            .env("LOCALAPPDATA", self.path("localappdata"))
            .env("XDG_CONFIG_HOME", self.path("config"))
            .env("XDG_DATA_HOME", self.path("data"))
            .env("XDG_CACHE_HOME", self.path("cache"))
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY");
        command
    }
    fn run(&self, args: &[&std::ffi::OsStr]) -> Output {
        self.command().args(args).output().unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn success(output: Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn convert(f: &Fixture, input: &Path, output: &Path, flags: &[&str]) -> Output {
    let mut args = vec![
        std::ffi::OsStr::new("image"),
        std::ffi::OsStr::new("convert"),
        input.as_os_str(),
        output.as_os_str(),
    ];
    args.extend(flags.iter().map(std::ffi::OsStr::new));
    f.run(&args)
}

#[test]
fn help_and_version_bypass_ui_without_display_or_configuration() {
    let f = Fixture::new();
    for flag in ["--help", "--version"] {
        let output = f.run(&[flag.as_ref()]);
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("screenshots"));
    }
    assert!(!f.path("config").exists());
    assert!(!f.path("data").exists());
    assert!(!f.path("cache").exists());
    for name in ["home", "userprofile", "appdata", "localappdata"] {
        assert!(!f.path(name).exists());
    }
    assert!(!f.run(&["nonsense".as_ref()]).status.success());
}

#[test]
fn real_executable_converts_rotates_and_preserves_sources() {
    let f = Fixture::new();
    let source = f.path("input weird $(echo nope).png");
    let original = RgbaImage::from_fn(4, 2, |x, y| Rgba([x as u8 * 40, y as u8 * 90, 50, 255]));
    original.save(&source).unwrap();
    let bytes = std::fs::read(&source).unwrap();
    let rotated = f.path("rotated.webp");
    let result = success(convert(
        &f,
        &source,
        &rotated,
        &["--rotate", "90", "--flip-horizontal"],
    ));
    assert_eq!(result["width"], 2);
    assert_eq!(result["height"], 4);
    let expected = image::imageops::flip_horizontal(&image::imageops::rotate90(&original));
    assert_eq!(image::open(&rotated).unwrap().into_rgba8(), expected);
    for extension in ["png", "jpg", "webp"] {
        let output = f.path(&format!("small.{extension}"));
        let result = success(convert(&f, &source, &output, &["--width", "2"]));
        assert_eq!(result["width"], 2);
        assert_eq!(result["height"], 1);
        assert_eq!(
            image::open(output).unwrap().into_rgba8().dimensions(),
            (2, 1)
        );
    }
    assert_eq!(std::fs::read(&source).unwrap(), bytes);
    assert!(!f.path("data").exists());
    assert!(!f.path("config").exists());
}

#[test]
fn invalid_operations_never_publish_or_overwrite_output() {
    let f = Fixture::new();
    let source = f.path("alpha.png");
    RgbaImage::from_pixel(2, 2, Rgba([12, 34, 56, 100]))
        .save(&source)
        .unwrap();
    let output = f.path("out.png");
    for flags in [
        vec!["--width", "0"],
        vec!["--width", "3"],
        vec!["--rotate", "45"],
        vec!["--width", "1", "--width", "2"],
        vec!["--jpeg-quality", "0"],
        vec!["--wat"],
    ] {
        assert!(!convert(&f, &source, &output, &flags).status.success());
        assert!(!output.exists());
    }
    assert!(
        !convert(&f, &source, &f.path("alpha.jpg"), &[])
            .status
            .success()
    );
    assert!(!f.path("alpha.jpg").exists());
    let expected = std::fs::read(&source).unwrap();
    assert!(!convert(&f, &source, &source, &[]).status.success());
    assert_eq!(std::fs::read(source).unwrap(), expected);
    let missing = f.path("missing.sqlite3");
    assert!(
        !f.run(&["history".as_ref(), missing.as_os_str()])
            .status
            .success()
    );
    assert!(!missing.exists());
    assert!(
        !f.run(&["capture".as_ref(), "region".as_ref(), "1".as_ref()])
            .status
            .success()
    );
    assert!(!f.run(&["upload".as_ref()]).status.success());
}

#[cfg(not(target_os = "windows"))]
#[test]
fn unsupported_stitched_capture_never_creates_output() {
    let fixture = Fixture::new();
    let output = fixture.path("desktop.png");
    let result = fixture.run(&["capture".as_ref(), "desktop".as_ref(), output.as_os_str()]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("unsupported"));
    assert!(!output.exists());
}

#[cfg(unix)]
#[test]
fn non_utf8_input_paths_are_supported_without_shell_interpretation() {
    use std::os::unix::ffi::OsStringExt;
    let f = Fixture::new();
    let source = f.0.join(std::ffi::OsString::from_vec(vec![
        b'a', 0xff, b'.', b'p', b'n', b'g',
    ]));
    let pixels = RgbaImage::from_pixel(2, 1, Rgba([1, 2, 3, 255]));
    pixels
        .save_with_format(&source, image::ImageFormat::Png)
        .unwrap();
    let output = f.path("copy.webp");
    success(convert(&f, &source, &output, &[]));
    assert_eq!(image::open(output).unwrap().into_rgba8(), pixels);
}

#[test]
fn executable_upload_sends_exact_png_and_reports_remote_failures() {
    use std::io::{BufRead, Read, Write};
    use std::net::TcpListener;
    let f = Fixture::new();
    let source = f.path("upload.png");
    RgbaImage::from_pixel(3, 2, Rgba([9, 8, 7, 255]))
        .save(&source)
        .unwrap();
    let bytes = std::fs::read(&source).unwrap();
    for status in [200, 500] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let profile = f.path("profile.json");
        std::fs::write(
            &profile,
            serde_json::to_vec(&serde_json::json!({
                "url": format!("http://{}/image", listener.local_addr().unwrap()),
                "method": "PUT", "allow_http": true, "response": {"type": "text"}
            }))
            .unwrap(),
        )
        .unwrap();
        let expected = bytes.clone();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "Upload never connected"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            socket
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut reader = std::io::BufReader::new(&mut socket);
            let mut length = None;
            let mut headers = String::new();
            loop {
                let mut line = String::new();
                assert!(reader.read_line(&mut line).unwrap() > 0);
                if line == "\r\n" {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = Some(value.trim().parse::<usize>().unwrap());
                }
                headers.push_str(&line);
                assert!(headers.len() < 16384);
            }
            assert!(headers.starts_with("PUT /image HTTP/1.1"));
            assert!(headers.to_lowercase().contains("if-none-match: *"));
            assert_eq!(length, Some(expected.len()));
            let mut body = vec![0; length.unwrap()];
            reader.read_exact(&mut body).unwrap();
            assert_eq!(body, expected);
            let response = "https://example.invalid/shared.png";
            write!(socket, "HTTP/1.1 {status} Result\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
        });
        let output = f.run(&["upload".as_ref(), profile.as_os_str(), source.as_os_str()]);
        server.join().unwrap();
        if status == 200 {
            assert_eq!(success(output)["url"], "https://example.invalid/shared.png");
        } else {
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
        }
        assert_eq!(std::fs::read(&source).unwrap(), bytes);
        assert!(!f.path("data").exists());
    }
}

#[test]
fn executable_storage_backup_preserves_existing_history() {
    let f = Fixture::new();
    let source = f.path("history.sqlite3");
    app_core::record_uploaded_png(
        &source,
        Path::new("synthetic.png"),
        "https://example.invalid",
        "https://example.invalid/one",
    )
    .unwrap();
    let snapshot = f.path("backup.sqlite3");
    let check = success(f.run(&["storage".as_ref(), "check".as_ref(), source.as_os_str()]));
    assert_eq!(check["quick_check"], "ok");
    let backup = success(f.run(&[
        "storage".as_ref(),
        "backup".as_ref(),
        source.as_os_str(),
        snapshot.as_os_str(),
    ]));
    assert_eq!(backup["completed"], true);
    assert_eq!(
        app_core::upload_history_page(&snapshot, None, || false)
            .unwrap()
            .len(),
        1
    );
    assert!(
        !f.run(&[
            "storage".as_ref(),
            "backup".as_ref(),
            source.as_os_str(),
            snapshot.as_os_str()
        ])
        .status
        .success()
    );
    let history = success(f.run(&["history".as_ref(), source.as_os_str()]));
    assert_eq!(history["entries"].as_array().unwrap().len(), 0);
    let candidate = f.path(".screenshots-12345678-1234-1234-1234-123456789abc.part");
    std::fs::write(&candidate, b"not automatically deletable").unwrap();
    let inspection = success(f.run(&[
        "storage".as_ref(),
        "inspect-parts".as_ref(),
        f.0.as_os_str(),
    ]));
    assert_eq!(inspection["candidates"].as_array().unwrap().len(), 1);
    assert_eq!(
        std::fs::read(candidate).unwrap(),
        b"not automatically deletable"
    );
}

#[cfg(unix)]
#[test]
fn part_inspection_handles_non_utf8_directory_names_without_panicking() {
    use std::os::unix::ffi::OsStringExt;
    let f = Fixture::new();
    let directory = f.0.join(std::ffi::OsString::from_vec(vec![b'd', 0xff]));
    std::fs::create_dir(&directory).unwrap();
    let candidate = directory.join(".screenshots-12345678-1234-1234-1234-123456789abc.part");
    std::fs::write(&candidate, b"candidate").unwrap();
    let output = success(f.run(&[
        "storage".as_ref(),
        "inspect-parts".as_ref(),
        directory.as_os_str(),
    ]));
    assert_eq!(output["path_encoding"], "lossy UTF-8 display");
    assert_eq!(output["candidates"].as_array().unwrap().len(), 1);
    assert!(
        output["candidates"][0]
            .as_str()
            .unwrap()
            .contains('\u{fffd}')
    );
    assert_eq!(std::fs::read(candidate).unwrap(), b"candidate");
}

#[cfg(unix)]
#[test]
fn interrupting_a_stalled_upload_terminates_without_local_outputs() {
    use std::{
        net::TcpListener,
        process::Stdio,
        time::{Duration, Instant},
    };
    let f = Fixture::new();
    let image = f.path("synthetic.png");
    RgbaImage::from_pixel(4, 4, Rgba([1, 2, 3, 255]))
        .save(&image)
        .unwrap();
    let original = std::fs::read(&image).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let profile = f.path("profile.json");
    std::fs::write(
        &profile,
        serde_json::to_vec(&serde_json::json!({
            "url": format!("http://{}/image", listener.local_addr().unwrap()),
            "method": "PUT", "allow_http": true, "response": {"type": "text"}
        }))
        .unwrap(),
    )
    .unwrap();
    let mut child = f
        .command()
        .arg("upload")
        .arg(profile)
        .arg(&image)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let _socket = loop {
        match listener.accept() {
            Ok((socket, _)) => break socket,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline || child.try_wait().unwrap().is_some() {
                    let _ = child.kill();
                    let output = child.wait_with_output().unwrap();
                    panic!(
                        "Upload never connected: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("{error}"),
        }
    };
    assert!(
        Command::new("kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("Interrupted upload did not terminate");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(std::fs::read(image).unwrap(), original);
    for name in [
        "config",
        "data",
        "cache",
        "home",
        "appdata",
        "localappdata",
        "userprofile",
    ] {
        assert!(!f.path(name).exists());
    }
}
