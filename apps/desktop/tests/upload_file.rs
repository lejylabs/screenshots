use std::{
    io::{BufRead, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "screenshots-file-upload-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn command(&self, command: &str) -> Command {
        let mut process = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        process.arg(command);
        for (variable, name) in [
            ("HOME", "home"),
            ("USERPROFILE", "userprofile"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CACHE_HOME", "cache"),
            ("APPDATA", "appdata"),
            ("LOCALAPPDATA", "localappdata"),
        ] {
            process.env(variable, self.path(name));
        }
        process.env_remove("DISPLAY").env_remove("WAYLAND_DISPLAY");
        process
    }
    fn run(&self, profile: &Path, source: &Path) -> Output {
        self.command("upload-file")
            .arg(profile)
            .arg(source)
            .output()
            .unwrap()
    }
    fn profile(&self, server: &Server, method: &str) -> PathBuf {
        let path = self.path("profile.json");
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "url": server.url, "method": method, "allow_http": true,
                "field": "payload", "response": {"type":"text"}
            }))
            .unwrap(),
        )
        .unwrap();
        path
    }
    fn assert_no_state(&self) {
        for name in [
            "home",
            "userprofile",
            "config",
            "data",
            "cache",
            "appdata",
            "localappdata",
        ] {
            assert!(!self.path(name).exists());
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
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<Vec<(String, Vec<u8>)>>>,
}
impl Server {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/upload", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = std::thread::spawn(move || {
            let mut requests = Vec::new();
            while !stopping.load(Ordering::Relaxed) {
                let (mut socket, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = std::io::BufReader::new(&mut socket);
                let mut headers = String::new();
                let mut length = None;
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
                    assert!(headers.len() <= 16384);
                }
                let length = length.unwrap();
                assert!(length <= 1024 * 1024);
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                requests.push((headers, body));
                let response = b"https://example.invalid/file";
                write!(
                    socket,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                )
                .unwrap();
                socket.write_all(response).unwrap();
            }
            requests
        });
        Self {
            url,
            stop,
            worker: Some(worker),
        }
    }
    fn finish(mut self) -> Vec<(String, Vec<u8>)> {
        self.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[test]
fn binary_put_streams_exact_bytes_and_empty_files_without_local_side_effects() {
    let f = Fixture::new();
    let server = Server::new();
    let profile = f.profile(&server, "PUT");
    let source = f.path("private-filename.bin");
    let original = b"arbitrary\0binary\xff\xfe\r\ndata";
    std::fs::write(&source, original).unwrap();
    let output = f.run(&profile, &source);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["url"], "https://example.invalid/file");
    assert!(result["thumbnail"].is_null());
    let empty = f.path("empty");
    std::fs::write(&empty, []).unwrap();
    assert!(f.run(&profile, &empty).status.success());
    let requests = server.finish();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].1, original);
    assert!(requests[1].1.is_empty());
    assert!(requests[0].0.starts_with("PUT /upload HTTP/1.1"));
    assert!(
        requests[0]
            .0
            .to_ascii_lowercase()
            .contains("content-type: application/octet-stream")
    );
    assert!(
        requests[0]
            .0
            .to_ascii_lowercase()
            .contains("if-none-match: *")
    );
    assert!(!requests[0].0.contains("private-filename"));
    assert_eq!(std::fs::read(source).unwrap(), original);
    f.assert_no_state();
}

#[test]
fn multipart_post_uses_generic_filename_and_mime_without_reencoding() {
    let f = Fixture::new();
    let server = Server::new();
    let profile = f.profile(&server, "POST");
    let source = f.path("private-source.bin");
    let original = b"bytes\0\xff\nunchanged";
    std::fs::write(&source, original).unwrap();
    let output = f.run(&profile, &source);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let requests = server.finish();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].0.starts_with("POST /upload HTTP/1.1"));
    assert!(
        requests[0]
            .0
            .to_ascii_lowercase()
            .contains("content-type: multipart/form-data; boundary=")
    );
    let body = &requests[0].1;
    let body_text = String::from_utf8_lossy(body);
    assert!(body_text.contains("name=\"payload\"; filename=\"file.bin\""));
    assert!(
        body_text
            .to_ascii_lowercase()
            .contains("content-type: application/octet-stream")
    );
    assert!(
        body.windows(original.len())
            .any(|window| window == original)
    );
    assert!(!body_text.contains("private-source"));
    assert_eq!(std::fs::read(source).unwrap(), original);
    f.assert_no_state();
}

#[test]
fn file_upload_rejects_oversize_directories_and_image_json_without_connecting() {
    let f = Fixture::new();
    let server = Server::new();
    let profile = f.profile(&server, "PUT");
    let huge = f.path("oversized.bin");
    std::fs::File::create(&huge)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    for source in [&huge, &f.0, &f.path("missing.bin")] {
        let output = f.run(&profile, source);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    let source = f.path("binary.bin");
    std::fs::write(&source, b"not PNG").unwrap();
    let json_profile = f.path("json-profile.json");
    std::fs::write(
        &json_profile,
        serde_json::to_vec(&serde_json::json!({
            "url": server.url, "method":"POST", "allow_http":true,
            "json": {"image":"{base64}"}, "response":{"type":"text"}
        }))
        .unwrap(),
    )
    .unwrap();
    let output = f.run(&json_profile, &source);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not image JSON bodies"));
    assert!(
        !f.command("upload")
            .arg(&profile)
            .arg(&source)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!f.command("upload-file").output().unwrap().status.success());
    assert!(server.finish().is_empty());
    f.assert_no_state();
}

#[cfg(unix)]
#[test]
fn png_and_file_uploads_reject_named_pipes_before_opening_them() {
    use std::os::unix::ffi::OsStrExt;
    let f = Fixture::new();
    let server = Server::new();
    let profile = f.profile(&server, "PUT");
    let path = f.path("named-pipe");
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
    for command in ["upload-file", "upload"] {
        let mut child = f
            .command(command)
            .arg(&profile)
            .arg(&path)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while child.try_wait().unwrap().is_none() {
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("Nonregular input opened and stalled instead of failing metadata admission");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child.wait_with_output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("not a regular file"));
    }
    assert!(server.finish().is_empty());
    f.assert_no_state();
}
