use std::{
    io::{BufRead, Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("screenshots-upload-files-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command.arg("upload-files");
        for (key, name) in [
            ("HOME", "home"),
            ("USERPROFILE", "profile"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CACHE_HOME", "cache"),
            ("APPDATA", "appdata"),
            ("LOCALAPPDATA", "localappdata"),
        ] {
            command.env(key, self.path(name));
        }
        command
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DBUS_SESSION_BUS_ADDRESS");
        command
    }
    fn run(&self, profile: &Path, paths: &[PathBuf]) -> Output {
        let mut child = self
            .command()
            .arg(profile)
            .args(paths)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let start = Instant::now();
        loop {
            if child.try_wait().unwrap().is_some() {
                return child.wait_with_output().unwrap();
            }
            if start.elapsed() > Duration::from_secs(10) {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("upload-files fixture timed out");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn profile(&self, server: &Server) -> PathBuf {
        let path = self.path("profile.json");
        std::fs::write(&path,serde_json::to_vec(&serde_json::json!({"url":server.url,"method":"PUT","allow_http":true,"response":{"type":"text"}})).unwrap()).unwrap();
        path
    }
    fn source(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }
    fn no_state(&self) {
        for name in [
            "home",
            "profile",
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
    fn new(statuses: Vec<u16>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/file", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = std::thread::spawn(move || {
            let start = Instant::now();
            let mut requests = Vec::new();
            while !stopping.load(Ordering::Relaxed) && start.elapsed() < Duration::from_secs(15) {
                let (mut socket, _) = match listener.accept() {
                    Ok(pair) => pair,
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
                socket
                    .set_write_timeout(Some(Duration::from_secs(5)))
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
                drop(reader);
                requests.push((headers, body));
                let number = requests.len();
                let status = statuses.get(number - 1).copied().unwrap_or(500);
                let text = format!("https://example.invalid/files/{number}");
                write!(socket,"HTTP/1.1 {status} Result\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",text.len()).unwrap();
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
fn lines(output: &Output) -> Vec<serde_json::Value> {
    std::str::from_utf8(&output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn sequential_binary_empty_and_text_files_keep_exact_bodies_order_and_sources() {
    let fixture = Fixture::new();
    let server = Server::new(vec![200, 200, 200]);
    let profile = fixture.profile(&server);
    let inputs = [
        b"binary\0\xff\x01data".as_slice(),
        b"".as_slice(),
        b"ordinary text\r\n".as_slice(),
    ];
    let paths = inputs
        .iter()
        .enumerate()
        .map(|(i, bytes)| fixture.source(&format!("private-name-{i}.bin"), bytes))
        .collect::<Vec<_>>();
    let output = fixture.run(&profile, &paths);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let messages = lines(&output);
    assert_eq!(messages.len(), 4);
    for (i, message) in messages[..3].iter().enumerate() {
        assert_eq!(
            message,
            &serde_json::json!({"type":"uploaded","index":i+1,"total":3,"url":format!("https://example.invalid/files/{}",i+1),"thumbnail":null,"deletion":null})
        );
    }
    assert_eq!(
        messages[3],
        serde_json::json!({"type":"complete","uploaded":3})
    );
    let requests = server.finish();
    assert_eq!(requests.len(), 3);
    for (i, (headers, body)) in requests.iter().enumerate() {
        assert_eq!(body, inputs[i]);
        assert!(headers.starts_with("PUT /file HTTP/1.1"));
        let headers = headers.to_ascii_lowercase();
        assert!(headers.contains("content-type: application/octet-stream"));
        assert!(headers.contains("if-none-match: *"));
        assert!(!headers.contains("private-name"));
        assert_eq!(std::fs::read(&paths[i]).unwrap(), inputs[i]);
    }
    fixture.no_state();
}

#[test]
fn second_server_failure_preserves_first_ndjson_and_never_sends_third_file() {
    let fixture = Fixture::new();
    let server = Server::new(vec![200, 500, 200]);
    let profile = fixture.profile(&server);
    let paths = (1..=3)
        .map(|index| fixture.source(&format!("file{index}"), &[index]))
        .collect::<Vec<_>>();
    let output = fixture.run(&profile, &paths);
    assert!(!output.status.success());
    let messages = lines(&output);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["index"], 1);
    assert_eq!(messages[0]["type"], "uploaded");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("2 of 3") && error.contains("1 confirmed"),
        "{error}"
    );
    let requests = server.finish();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].1, [1]);
    assert_eq!(requests[1].1, [2]);
    for (i, path) in paths.iter().enumerate() {
        assert_eq!(std::fs::read(path).unwrap(), [i as u8 + 1]);
    }
    fixture.no_state();
}

#[test]
fn bad_later_input_counts_and_image_json_fail_before_first_request() {
    let fixture = Fixture::new();
    let server = Server::new(vec![]);
    let profile = fixture.profile(&server);
    let valid = fixture.source("valid.bin", b"valid");
    let huge = fixture.path("huge.bin");
    std::fs::File::create(&huge)
        .unwrap()
        .set_len(64 * 1024 * 1024 + 1)
        .unwrap();
    for second in [fixture.path("missing.bin"), fixture.0.clone(), huge] {
        let output = fixture.run(&profile, &[valid.clone(), second]);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    for paths in [vec![], vec![valid.clone(); 21]] {
        let output = fixture.run(&profile, &paths);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("1–20"));
    }
    let json = fixture.path("image-json.json");
    std::fs::write(&json,serde_json::to_vec(&serde_json::json!({"url":server.url,"method":"POST","allow_http":true,"json":{"image":"{base64}"},"response":{"type":"text"}})).unwrap()).unwrap();
    let output = fixture.run(&json, &[valid.clone()]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert_eq!(std::fs::read(valid).unwrap(), b"valid");
    assert!(server.finish().is_empty());
    fixture.no_state();
}

#[cfg(unix)]
#[test]
fn later_fifo_is_rejected_without_opening_or_connecting() {
    use std::os::unix::ffi::OsStrExt;
    let fixture = Fixture::new();
    let server = Server::new(vec![]);
    let profile = fixture.profile(&server);
    let valid = fixture.source("first.bin", b"valid");
    let fifo = fixture.path("fifo");
    let cpath = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) }, 0);
    let output = fixture.run(&profile, &[valid, fifo]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(server.finish().is_empty());
    fixture.no_state();
}
