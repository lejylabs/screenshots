use image::RgbaImage;
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("screenshots-image-url-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn run(&self, args: &[std::ffi::OsString]) -> std::process::Output {
        self.invoke("image-url", args)
    }
    fn invoke(&self, name: &str, args: &[std::ffi::OsString]) -> std::process::Output {
        let stdout = self.path("stdout");
        let stderr = self.path("stderr");
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command
            .arg(name)
            .args(args)
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .env("NO_PROXY", "*")
            .env("no_proxy", "*");
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
        let mut child = command
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(&stdout).unwrap())
            .stderr(std::fs::File::create(&stderr).unwrap())
            .spawn()
            .unwrap();
        let until = Instant::now() + Duration::from_secs(25);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= until {
                let _ = child.kill();
                let _ = child.wait();
                panic!("URL import exceeded outer deadline");
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        std::process::Output {
            status,
            stdout: std::fs::read(stdout).unwrap(),
            stderr: std::fs::read(stderr).unwrap(),
        }
    }
    fn no_state(&self) {
        for name in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            assert!(!self.path(name).exists(), "{name}");
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
struct Server {
    url: String,
    count: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    fn new(reply: Vec<u8>) -> Self {
        Self::configured(reply, false)
    }
    fn configured(reply: Vec<u8>, hold: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/image", listener.local_addr().unwrap());
        let count = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let c = count.clone();
        let s = stop.clone();
        let thread = std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        c.fetch_add(1, Ordering::Relaxed);
                        // BSD accepts can inherit O_NONBLOCK from the listener.
                        socket.set_nonblocking(false).unwrap();
                        socket
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        socket
                            .set_write_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut request = Vec::new();
                        while request.len() < 16384 && !request.ends_with(b"\r\n\r\n") {
                            let mut byte = [0];
                            match socket.read(&mut byte) {
                                Ok(1) => request.push(byte[0]),
                                _ => break,
                            }
                        }
                        let _ = socket.write_all(&reply);
                        while hold && !s.load(Ordering::Relaxed) {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("{error}"),
                }
            }
        });
        Self {
            url,
            count,
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}
fn png() -> (Vec<u8>, RgbaImage) {
    let pixels = RgbaImage::from_raw(2, 1, vec![11, 22, 33, 0, 44, 55, 66, 128]).unwrap();
    let mut output = std::io::Cursor::new(Vec::new());
    pixels
        .write_to(&mut output, image::ImageFormat::Png)
        .unwrap();
    (output.into_inner(), pixels)
}
fn args(url: &str, output: &Path) -> Vec<std::ffi::OsString> {
    vec![url.into(), output.into(), "--allow-http".into()]
}
#[test]
fn actual_isolated_download_preserves_exact_pixels_without_desktop_or_user_state() {
    let f = Fixture::new();
    let (png, pixels) = png();
    let mut reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        png.len()
    )
    .into_bytes();
    reply.extend(png);
    let server = Server::new(reply);
    let output = f.path("result.png");
    let result = f.run(&args(&server.url, &output));
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(image::open(output).unwrap().into_rgba8(), pixels);
    assert_eq!(server.count.load(Ordering::Relaxed), 1);
    f.no_state();
}
#[test]
fn preflight_and_untrusted_responses_never_publish_or_follow_redirects() {
    let f = Fixture::new();
    let output = f.path("result.png");
    let server =
        Server::new(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec());
    assert!(
        !f.run(&[server.url.clone().into(), output.clone().into()])
            .status
            .success()
    );
    assert_eq!(server.count.load(Ordering::Relaxed), 0);
    for reply in [
        b"HTTP/1.1 200 OK\r\nContent-Length: 33554433\r\nConnection: close\r\n\r\n".to_vec(),
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nbad".to_vec(),
        format!(
            "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            server.url
        )
        .into_bytes(),
    ] {
        let response = Server::new(reply);
        let result = f.run(&args(&response.url, &output));
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
        assert!(!output.exists());
        assert!(!String::from_utf8_lossy(&result.stderr).contains(&response.url));
        assert_eq!(response.count.load(Ordering::Relaxed), 1);
    }
    assert_eq!(server.count.load(Ordering::Relaxed), 0);
    f.no_state();
}

#[test]
fn chunked_body_limit_and_task_source_keep_output_admission_and_exact_pixels() {
    let f = Fixture::new();
    let output = f.path("oversize.png");
    let size = 32 * 1024 * 1024 + 1;
    let mut reply = format!(
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n{size:x}\r\n"
    )
    .into_bytes();
    reply.resize(reply.len() + size, b'x');
    // Never terminate this valid chunk: removing the streaming cap would wait for the15s HTTP timeout.
    let oversized = Server::configured(reply, true);
    let started = Instant::now();
    let result = f.run(&args(&oversized.url, &output));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "Streaming size rejection waited for a timeout"
    );
    assert_eq!(oversized.count.load(Ordering::Relaxed), 1);
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(!output.exists());
    let (png, pixels) = png();
    let mut reply = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        png.len()
    )
    .into_bytes();
    reply.extend(png);
    let server = Server::new(reply);
    let output = f.path("task.png");
    let profile = f.path("task.json");
    std::fs::write(&profile,serde_json::to_vec(&serde_json::json!({"version":1,"source":{"type":"image_url","url":server.url,"allow_http":true},"actions":[{"type":"save","path":output}]})).unwrap()).unwrap();
    let result = f.invoke("task", &["run".into(), profile.clone().into()]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(image::open(&output).unwrap().into_rgba8(), pixels);
    let before = server.count.load(Ordering::Relaxed);
    assert!(
        !f.invoke("task", &["run".into(), profile.into()])
            .status
            .success()
    );
    assert_eq!(server.count.load(Ordering::Relaxed), before);
    f.no_state();
}

#[cfg(unix)]
#[test]
fn stalled_download_cancels_the_owned_worker_without_an_output() {
    let f = Fixture::new();
    let server = Server::configured(
        b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n".to_vec(),
        true,
    );
    let output = f.path("canceled.png");
    let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
    command
        .arg("image-url")
        .args(args(&server.url, &output))
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DBUS_SESSION_BUS_ADDRESS")
        .env("NO_PROXY", "*")
        .env("no_proxy", "*");
    for name in [
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
    ] {
        command.env(name, f.path(name));
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(5);
    while server.count.load(Ordering::Relaxed) == 0 {
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Download never connected");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let until = Instant::now() + Duration::from_secs(3);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Canceled URL import failed to stop");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(!status.success());
    assert!(!output.exists());
    f.no_state();
}
