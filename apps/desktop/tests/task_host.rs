use image::{Rgba, RgbaImage};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::mpsc,
    thread::JoinHandle,
    time::{Duration, Instant},
};

const STATE: [&str; 7] = [
    "HOME",
    "USERPROFILE",
    "APPDATA",
    "LOCALAPPDATA",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
];
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "screenshots-task-host-cli-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn command(&self) -> Command {
        self.command_executable(Path::new(env!("CARGO_BIN_EXE_screenshots")))
    }
    fn command_executable(&self, executable: &Path) -> Command {
        let mut command = Command::new(executable);
        command.arg("task-host");
        for key in STATE {
            command.env(key, self.path(key));
        }
        for key in ["DISPLAY", "WAYLAND_DISPLAY", "DBUS_SESSION_BUS_ADDRESS"] {
            command.env_remove(key);
        }
        command
    }
    fn setup(&self, slow: bool, timeout: u64) -> PathBuf {
        RgbaImage::from_pixel(3, 2, Rgba([17, 29, 71, 255]))
            .save(self.path("source.png"))
            .unwrap();
        let actions = if slow {
            json!([{"type":"command","program":std::env::current_exe().unwrap(),"args":["--exact","task_host_engine_fixture","--nocapture"],"timeout_ms":20000}])
        } else {
            json!([{"type":"save","path":"capture-{uuid}.png"}])
        };
        std::fs::write(self.path("task.json"),serde_json::to_vec(&json!({"version":1,"source":{"type":"file","path":"source.png"},"actions":actions})).unwrap()).unwrap();
        let config = self.path("host.json");
        std::fs::write(&config,serde_json::to_vec(&json!({"version":1,"task_timeout_ms":timeout,"bindings":[{"id":"sample","task":"task.json"}]})).unwrap()).unwrap();
        config
    }
    fn cli(&self, action: &str, extra: Option<&str>) -> Output {
        let mut command = self.command();
        command.arg(action).arg(self.path("endpoint.json"));
        if let Some(value) = extra {
            command.arg(value);
        }
        command.output().unwrap()
    }
    fn no_state(&self) {
        for key in STATE {
            assert!(!self.path(key).exists());
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn output(result: Output) -> Value {
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(result.stderr.is_empty());
    serde_json::from_slice(&result.stdout).unwrap()
}

fn framed_read(stream: &mut TcpStream) -> Value {
    let mut header = [0; 4];
    stream.read_exact(&mut header).unwrap();
    let size = u32::from_be_bytes(header) as usize;
    assert!((1..=4096).contains(&size));
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes).unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
fn raw(endpoint: &Value, bytes: &[u8]) -> Value {
    let mut stream =
        TcpStream::connect(("127.0.0.1", endpoint["port"].as_u64().unwrap() as u16)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(4)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(bytes).unwrap();
    framed_read(&mut stream)
}
fn request(endpoint: &Value, command: Value) -> Value {
    raw(
        endpoint,
        &serde_json::to_vec(&json!({"version":1,"token":endpoint["token"],"command":command}))
            .unwrap(),
    )
}

struct Server {
    child: Child,
    stdout: Option<JoinHandle<Vec<u8>>>,
    stderr: Option<JoinHandle<Vec<u8>>>,
    endpoint: Value,
}
impl Server {
    fn start(f: &Fixture, config: &Path) -> Self {
        Self::start_executable(f, config, Path::new(env!("CARGO_BIN_EXE_screenshots")))
    }
    fn start_executable(f: &Fixture, config: &Path, executable: &Path) -> Self {
        let mut child = f
            .command_executable(executable)
            .arg("serve")
            .arg(config)
            .arg(f.path("endpoint.json"))
            .env("SCREENSHOTS_TASK_HOST_ENGINE_MARKER", f.path("engine.pid"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let (sender, receiver) = mpsc::sync_channel(1);
        let stdout = std::thread::spawn(move || {
            let mut reader = stdout;
            let mut all = Vec::new();
            let mut sent = false;
            let mut chunk = [0u8; 1024];
            loop {
                let count = reader.read(&mut chunk).unwrap();
                if count == 0 {
                    break;
                }
                assert!(all.len() + count <= 65536);
                all.extend_from_slice(&chunk[..count]);
                if !sent {
                    if let Some(end) = all.iter().position(|byte| *byte == b'\n') {
                        sender.send(all[..=end].to_vec()).unwrap();
                        sent = true;
                    }
                }
            }
            all
        });
        let stderr = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.take(65537).read_to_end(&mut bytes).unwrap();
            assert!(bytes.len() <= 65536);
            bytes
        });
        let ready = match receiver.recv_timeout(Duration::from_secs(10)) {
            Ok(ready) => ready,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout.join();
                let errors = stderr.join().unwrap();
                panic!(
                    "Host did not become ready: {}",
                    String::from_utf8_lossy(&errors)
                );
            }
        };
        let ready: Value = serde_json::from_slice(&ready).unwrap();
        assert_eq!(ready, json!({"ready":true,"profiles":1,"hotkeys":0}));
        let endpoint: Value =
            serde_json::from_slice(&std::fs::read(f.path("endpoint.json")).unwrap()).unwrap();
        assert_eq!(endpoint["token"].as_str().unwrap().len(), 64);
        Self {
            child,
            stdout: Some(stdout),
            stderr: Some(stderr),
            endpoint,
        }
    }
    fn finish(mut self) -> (Vec<u8>, Vec<u8>) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.child.try_wait().unwrap().is_none() {
            assert!(Instant::now() < deadline, "Host did not stop");
            std::thread::sleep(Duration::from_millis(10));
        }
        let status = self.child.wait().unwrap();
        let stdout = self.stdout.take().unwrap().join().unwrap();
        let stderr = self.stderr.take().unwrap().join().unwrap();
        assert!(status.success(), "{}", String::from_utf8_lossy(&stderr));
        (stdout, stderr)
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for thread in [&mut self.stdout, &mut self.stderr] {
            if let Some(thread) = thread.take() {
                let _ = thread.join();
            }
        }
    }
}
fn completed(endpoint: &Value) -> Value {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let state = request(endpoint, json!({"type":"status"}));
        if state["busy"] == false {
            return state;
        }
        assert!(Instant::now() < until, "Task did not complete");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn task_host_validates_and_runs_saved_profiles_via_authenticated_typed_ipc() {
    let f = Fixture::new();
    let config = f.setup(false, 5000);
    assert_eq!(
        output(f.command().arg("validate").arg(&config).output().unwrap())["valid"],
        true
    );
    let server = Server::start(&f, &config);
    let before = std::fs::read(f.path("endpoint.json")).unwrap();
    let duplicate = f
        .command()
        .arg("serve")
        .arg(&config)
        .arg(f.path("endpoint.json"))
        .output()
        .unwrap();
    assert!(!duplicate.status.success());
    assert!(duplicate.stdout.is_empty());
    assert_eq!(std::fs::read(f.path("endpoint.json")).unwrap(), before);
    for body in [
        json!({"version":1,"token":"DO_NOT_ECHO_TOKEN","command":{"type":"trigger","profile":"sample"}}),
        json!({"version":2,"token":server.endpoint["token"],"command":{"type":"status"}}),
    ] {
        let response = raw(&server.endpoint, &serde_json::to_vec(&body).unwrap());
        assert_eq!(response, json!({"ok":false,"error":"unauthorized"}));
    }
    assert_eq!(
        raw(&server.endpoint, b"[]"),
        json!({"ok":false,"error":"invalid_request"})
    );
    assert_eq!(
        request(
            &server.endpoint,
            json!({"type":"trigger","profile":"unknown"})
        ),
        json!({"ok":false,"error":"unknown_profile"})
    );
    for _ in 0..2 {
        let admitted = output(f.cli("trigger", Some("sample")));
        assert_eq!(admitted["state"], "accepted");
        let state = completed(&server.endpoint);
        assert_eq!(state["last"]["state"], "completed");
        assert_eq!(state["last"]["success"], true);
    }
    let images =
        f.0.read_dir()
            .unwrap()
            .filter_map(|entry| {
                let path = entry.unwrap().path();
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("capture-")
                    .then_some(path)
            })
            .collect::<Vec<_>>();
    assert_eq!(images.len(), 2);
    for image in images {
        assert_eq!(
            image::open(image).unwrap().into_rgba8(),
            RgbaImage::from_pixel(3, 2, Rgba([17, 29, 71, 255]))
        );
    }
    assert_eq!(output(f.cli("status", None))["busy"], false);
    assert_eq!(output(f.cli("stop", None))["state"], "stopping");
    let (_, stderr) = server.finish();
    assert!(stderr.is_empty());
    assert!(!f.path("endpoint.json").exists());
    f.no_state();
}

#[test]
fn task_host_engine_fixture() {
    let Some(path) = std::env::var_os("SCREENSHOTS_TASK_HOST_ENGINE_MARKER") else {
        return;
    };
    std::fs::write(path, std::process::id().to_string()).unwrap();
    std::thread::sleep(Duration::from_secs(10));
}

#[test]
fn task_host_has_one_active_job_no_backlog_and_stops_owned_processes() {
    let f = Fixture::new();
    let config = f.setup(true, 10000);
    let server = Server::start(&f, &config);
    assert_eq!(
        request(
            &server.endpoint,
            json!({"type":"trigger","profile":"sample"})
        )["state"],
        "accepted"
    );
    assert_eq!(
        request(
            &server.endpoint,
            json!({"type":"trigger","profile":"sample"})
        ),
        json!({"ok":false,"error":"busy"})
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f.path("engine.pid").exists() {
        assert!(Instant::now() < deadline, "Task child did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    let pid = std::fs::read_to_string(f.path("engine.pid"))
        .unwrap()
        .parse::<u32>()
        .unwrap();
    assert_eq!(
        request(&server.endpoint, json!({"type":"stop"}))["ok"],
        true
    );
    let (_, stderr) = server.finish();
    assert!(stderr.is_empty());
    #[cfg(target_os = "linux")]
    {
        let status = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        assert!(
            status.is_empty()
                || status
                    .rsplit_once(')')
                    .unwrap()
                    .1
                    .trim_start()
                    .starts_with('Z')
        );
    }
    #[cfg(not(target_os = "linux"))]
    let _ = pid;
    assert!(!f.path("endpoint.json").exists());
    f.no_state();
}

#[test]
fn task_host_timeout_releases_admission_without_queuing_a_second_job() {
    let f = Fixture::new();
    let config = f.setup(true, 1000);
    let server = Server::start(&f, &config);
    assert_eq!(
        request(
            &server.endpoint,
            json!({"type":"trigger","profile":"sample"})
        )["ok"],
        true
    );
    let status = completed(&server.endpoint);
    assert_eq!(status["last"]["state"], "timed_out");
    assert_eq!(status["last"]["success"], false);
    assert_eq!(
        request(&server.endpoint, json!({"type":"stop"}))["ok"],
        true
    );
    let _ = server.finish();
    f.no_state();
}

#[test]
fn task_host_absolute_frame_deadline_rejects_trickled_unauthenticated_payloads() {
    let f = Fixture::new();
    let config = f.setup(false, 5000);
    let server = Server::start(&f, &config);
    let mut stream = TcpStream::connect((
        "127.0.0.1",
        server.endpoint["port"].as_u64().unwrap() as u16,
    ))
    .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_millis(100)))
        .unwrap();
    stream.write_all(&256u32.to_be_bytes()).unwrap();
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(2500) {
        if stream.write_all(b" ").is_err() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let mut byte = [0];
    match stream.read(&mut byte) {
        Ok(0) => {}
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::BrokenPipe
            ) => {}
        other => panic!("Slow unauthenticated frame remained open: {other:?}"),
    }
    assert!(started.elapsed() < Duration::from_secs(6));
    let state = request(&server.endpoint, json!({"type":"status"}));
    assert_eq!(state["busy"], false);
    assert!(state["last"].is_null());
    assert_eq!(
        request(&server.endpoint, json!({"type":"stop"}))["ok"],
        true
    );
    let _ = server.finish();
    f.no_state();
}

#[cfg(target_os = "linux")]
#[test]
fn task_host_keeps_its_exact_executable_image_after_the_binary_path_is_replaced() {
    let f = Fixture::new();
    let config = f.setup(false, 5000);
    let executable = f.path("copied-host");
    std::fs::copy(env!("CARGO_BIN_EXE_screenshots"), &executable).unwrap();
    let server = Server::start_executable(&f, &config, &executable);
    std::fs::remove_file(&executable).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_screenshots"), &executable).unwrap();
    assert_eq!(
        request(
            &server.endpoint,
            json!({"type":"trigger","profile":"sample"})
        )["state"],
        "accepted"
    );
    let state = completed(&server.endpoint);
    assert_eq!(state["last"]["success"], true);
    assert_eq!(
        f.0.read_dir()
            .unwrap()
            .filter(|entry| entry
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("capture-"))
            .count(),
        1
    );
    assert_eq!(
        request(&server.endpoint, json!({"type":"stop"}))["ok"],
        true
    );
    let _ = server.finish();
    assert!(!f.path("endpoint.json").exists());
    f.no_state();
}

#[cfg(unix)]
#[test]
fn task_host_clients_reject_public_endpoint_and_shutdown_preserves_replaced_path() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let config = f.setup(false, 5000);
    let server = Server::start(&f, &config);
    let endpoint = f.path("endpoint.json");
    std::fs::set_permissions(&endpoint, std::fs::Permissions::from_mode(0o644)).unwrap();
    let failed = f.cli("status", None);
    assert!(!failed.status.success());
    assert!(failed.stdout.is_empty());
    std::fs::set_permissions(&endpoint, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::rename(&endpoint, f.path("original.json")).unwrap();
    std::fs::write(&endpoint, b"replacement").unwrap();
    assert_eq!(
        request(&server.endpoint, json!({"type":"stop"}))["ok"],
        true
    );
    let _ = server.finish();
    assert_eq!(std::fs::read(&endpoint).unwrap(), b"replacement");
    assert!(f.path("original.json").exists());
    f.no_state();
}
