use image::{Rgba, RgbaImage};
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
            "screenshots-upload-batch-{}-{}",
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
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        command.arg("upload-images");
        for (variable, name) in [
            ("HOME", "home"),
            ("USERPROFILE", "userprofile"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_CACHE_HOME", "cache"),
            ("APPDATA", "appdata"),
            ("LOCALAPPDATA", "localappdata"),
        ] {
            command.env(variable, self.path(name));
        }
        command.env_remove("DISPLAY").env_remove("WAYLAND_DISPLAY");
        command
    }
    fn run(&self, profile: &Path, paths: &[PathBuf]) -> Output {
        self.command().arg(profile).args(paths).output().unwrap()
    }
    fn profile(&self, server: &Server) -> PathBuf {
        let profile = self.path("profile.json");
        std::fs::write(
            &profile,
            serde_json::to_vec(&serde_json::json!({
                "url": server.url, "method": "PUT", "allow_http": true,
                "response": {"type": "text"}
            }))
            .unwrap(),
        )
        .unwrap();
        profile
    }
    fn image(&self, name: &str) -> PathBuf {
        let path = self.path(name);
        let image = RgbaImage::from_pixel(3, 2, Rgba([31, 87, 140, 255]));
        if path.extension().is_some_and(|extension| extension == "jpg") {
            image::DynamicImage::ImageRgba8(image)
                .into_rgb8()
                .save(&path)
                .unwrap();
        } else {
            image.save(&path).unwrap();
        }
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
    worker: Option<JoinHandle<Vec<Vec<u8>>>>,
}
impl Server {
    fn new(statuses: Vec<u16>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/image", listener.local_addr().unwrap());
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = std::thread::spawn(move || {
            let mut bodies = Vec::new();
            while !stopping.load(Ordering::Relaxed) {
                let (mut socket, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(error) => panic!("{error}"),
                };
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
                assert!(headers.starts_with("PUT /image HTTP/1.1"));
                assert!(headers.to_ascii_lowercase().contains("if-none-match: *"));
                let length = length.unwrap();
                assert!(length <= 16 * 1024 * 1024);
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                bodies.push(body);
                let index = bodies.len() - 1;
                let status = statuses.get(index).copied().unwrap_or(500);
                let link = format!("https://example.invalid/result/{}", index + 1);
                write!(socket, "HTTP/1.1 {status} Result\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{link}", link.len()).unwrap();
            }
            bodies
        });
        Self {
            url,
            stop,
            worker: Some(worker),
        }
    }
    fn finish(mut self) -> Vec<Vec<u8>> {
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
fn batch_uploads_sniffed_png_jpeg_webp_as_finalized_png_without_local_outputs() {
    let f = Fixture::new();
    let server = Server::new(vec![200, 200, 200]);
    let profile = f.profile(&server);
    let paths = vec![
        f.image("input.png"),
        f.image("input.jpg"),
        f.image("input.webp"),
    ];
    let bytes = paths
        .iter()
        .map(std::fs::read)
        .collect::<std::io::Result<Vec<_>>>()
        .unwrap();
    let before = std::fs::read_dir(&f.0).unwrap().count();
    let output = f.run(&profile, &paths);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result = lines(&output);
    assert_eq!(result.len(), 4);
    for index in 0..3 {
        assert_eq!(result[index]["type"], "uploaded");
        assert_eq!(result[index]["index"], index + 1);
        assert_eq!(result[index]["total"], 3);
        assert_eq!(
            result[index]["url"],
            format!("https://example.invalid/result/{}", index + 1)
        );
    }
    assert_eq!(
        result[3],
        serde_json::json!({"type":"complete", "uploaded":3})
    );
    let uploaded = server.finish();
    assert_eq!(uploaded.len(), 3);
    for ((body, path), original) in uploaded.iter().zip(&paths).zip(bytes) {
        assert!(body.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]));
        assert_eq!(
            image::load_from_memory(body).unwrap().into_rgba8(),
            app_core::load_editor_image(path).unwrap()
        );
        assert_eq!(std::fs::read(path).unwrap(), original);
    }
    assert_eq!(std::fs::read_dir(&f.0).unwrap().count(), before);
    f.assert_no_state();
}

#[test]
fn batch_failure_preserves_prior_links_and_never_attempts_later_images() {
    let f = Fixture::new();
    let server = Server::new(vec![200, 500, 200]);
    let profile = f.profile(&server);
    let input = f.image("input.png");
    let original = std::fs::read(&input).unwrap();
    let output = f.run(&profile, &[input.clone(), input.clone(), input.clone()]);
    assert!(!output.status.success());
    let result = lines(&output);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["url"], "https://example.invalid/result/1");
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("Image 2 of 3 failed after 1 confirmed uploads"));
    assert!(error.contains("may already exist remotely"));
    assert_eq!(server.finish().len(), 2);
    assert_eq!(std::fs::read(input).unwrap(), original);
    f.assert_no_state();
}

#[test]
fn batch_preflight_rejects_count_profile_and_missing_files_without_network() {
    let f = Fixture::new();
    let server = Server::new(vec![]);
    let profile = f.profile(&server);
    let input = f.image("input.png");
    for paths in [
        Vec::new(),
        vec![input.clone(); 21],
        vec![input.clone(), f.path("missing.png")],
        vec![f.0.clone()],
    ] {
        let output = f.run(&profile, &paths);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
    }
    let invalid = f.path("invalid.json");
    std::fs::write(&invalid, b"{}").unwrap();
    let output = f.run(&invalid, &[input]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(server.finish().is_empty());
    f.assert_no_state();
}

#[test]
fn invalid_later_image_stops_before_its_upload_and_preserves_previous_link() {
    let f = Fixture::new();
    let server = Server::new(vec![200]);
    let profile = f.profile(&server);
    let input = f.image("input.png");
    let invalid = f.path("invalid.webp");
    std::fs::write(&invalid, b"not an image").unwrap();
    let output = f.run(&profile, &[input.clone(), invalid, input]);
    assert!(!output.status.success());
    assert_eq!(lines(&output).len(), 1);
    assert_eq!(server.finish().len(), 1);
    assert!(String::from_utf8_lossy(&output.stderr).contains("Image decoding failed"));
    f.assert_no_state();
}

#[cfg(unix)]
#[test]
fn successful_remote_upload_with_failed_stdout_stops_the_batch() {
    let f = Fixture::new();
    let server = Server::new(vec![200]);
    let profile = f.profile(&server);
    let input = f.image("input.png");
    let mut child = f
        .command()
        .arg(profile)
        .arg(&input)
        .arg(input)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert_eq!(server.finish().len(), 1);
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("was uploaded but its result could not be written")
    );
    f.assert_no_state();
}
