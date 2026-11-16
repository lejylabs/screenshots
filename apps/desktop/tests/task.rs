use std::{
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::Duration,
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("screenshots-task-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn p(&self, n: &str) -> PathBuf {
        self.0.join(n)
    }
    fn run(&self, profile: &serde_json::Value, mode: &str) -> Output {
        std::fs::write(self.p("task.json"), serde_json::to_vec(profile).unwrap()).unwrap();
        let mut c = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        c.args(["task", mode])
            .arg(self.p("task.json"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for name in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            c.env(name, self.p(name));
        }
        c.env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("DBUS_SESSION_BUS_ADDRESS");
        let mut child = c.spawn().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while child.try_wait().unwrap().is_none() {
            if std::time::Instant::now() > deadline {
                child.kill().unwrap();
                panic!("Task did not finish");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        child.wait_with_output().unwrap()
    }
    fn image(&self) {
        image::RgbaImage::from_pixel(3, 2, image::Rgba([11, 22, 33, 255]))
            .save(self.p("in.png"))
            .unwrap();
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
            assert!(!self.p(name).exists());
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn ok(output: Output) -> Vec<serde_json::Value> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}

#[test]
fn task_runs_finalized_save_and_bounded_command_without_changing_source() {
    let f = Fixture::new();
    f.image();
    let original = std::fs::read(f.p("in.png")).unwrap();
    std::fs::write(
        f.p("recipe.json"),
        br#"{"version":1,"steps":[{"op":"redact","x":0,"y":0,"width":1,"height":2}]}"#,
    )
    .unwrap();
    let p = serde_json::json!({"version":1,"source":{"type":"file","path":"in.png"},"recipe":"recipe.json","actions":[{"type":"save","path":"out.png"},{"type":"command","program":env!("CARGO_BIN_EXE_screenshots"),"args":["image","info","{output}"],"timeout_ms":5000}]});
    assert_eq!(ok(f.run(&p, "validate"))[0]["valid"], true);
    assert!(!f.p("out.png").exists());
    let results = ok(f.run(&p, "run"));
    assert_eq!(results.len(), 3);
    assert_eq!(results[2]["type"], "complete");
    let output = image::open(f.p("out.png")).unwrap().into_rgba8();
    assert_eq!(output.get_pixel(0, 0).0, [0, 0, 0, 255]);
    assert_eq!(output.get_pixel(1, 0).0, [11, 22, 33, 255]);
    assert_eq!(std::fs::read(f.p("in.png")).unwrap(), original);
    f.no_state();
}

#[test]
fn task_preflights_all_actions_and_preserves_previous_outputs_on_later_failure() {
    let f = Fixture::new();
    f.image();
    let mut p = serde_json::json!({"version":1,"source":{"type":"file","path":"in.png"},"actions":[{"type":"save","path":"out.png"},{"type":"copy_link"}]});
    let result = f.run(&p, "run");
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert!(!f.p("out.png").exists());
    p["actions"][1] = serde_json::json!({"type":"command","program":env!("CARGO_BIN_EXE_screenshots"),"args":["invalid-task-child-command"],"timeout_ms":5000});
    let result = f.run(&p, "run");
    assert!(!result.status.success());
    assert!(f.p("out.png").is_file());
    assert_eq!(String::from_utf8_lossy(&result.stdout).lines().count(), 1);
    assert!(String::from_utf8_lossy(&result.stderr).contains("after 1 completed actions"));
    let bytes = std::fs::read(f.p("out.png")).unwrap();
    let result = f.run(&p, "run");
    assert!(!result.status.success());
    assert!(result.stdout.is_empty());
    assert_eq!(std::fs::read(f.p("out.png")).unwrap(), bytes);
    f.no_state();
}

#[test]
fn task_upload_link_qr_chain_makes_one_request_and_preserves_input() {
    let f = Fixture::new();
    f.image();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            let mut b = [0];
            s.read_exact(&mut b).unwrap();
            header.push(b[0]);
            assert!(header.len() < 16384);
        }
        let text = String::from_utf8(header).unwrap();
        let size = text
            .lines()
            .find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|s| s.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        assert!(size < 16384);
        let mut body = vec![0; size];
        s.read_exact(&mut body).unwrap();
        assert_eq!(image::load_from_memory(&body).unwrap().dimensions(), (3, 2));
        let url = "https://example.invalid/task-result";
        write!(
            s,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{url}",
            url.len()
        )
        .unwrap();
    });
    std::fs::write(f.p("destination.json"),serde_json::to_vec(&serde_json::json!({"url":format!("http://{addr}/upload"),"allow_http":true,"method":"PUT","response":{"type":"text"}})).unwrap()).unwrap();
    let p = serde_json::json!({"version":1,"source":{"type":"file","path":"in.png"},"actions":[{"type":"upload","profile":"destination.json"},{"type":"link","format":"html"},{"type":"qr","path":"share.png"}]});
    let result = ok(f.run(&p, "run"));
    server.join().unwrap();
    assert_eq!(result.len(), 4);
    assert_eq!(
        result[1]["result"]["text"],
        "<a href=\"https://example.invalid/task-result\">Screenshot</a>"
    );
    assert!(f.p("share.png").is_file());
    f.no_state();
}

use image::GenericImageView;

#[cfg(unix)]
#[test]
fn task_cooperative_command_child() {
    use std::os::unix::process::CommandExt;
    let Some(marker) = std::env::var_os("SCREENSHOTS_TASK_COOPERATIVE_FIXTURE") else {
        return;
    };
    let marker = PathBuf::from(marker);
    let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    signal_hook::flag::register(libc::SIGTERM, flag.clone()).unwrap();
    let mut child = Command::new("/bin/sh")
        .args(["-c", "exec sleep 30"])
        .process_group(0)
        .spawn()
        .unwrap();
    struct Group(i32);
    impl Drop for Group {
        fn drop(&mut self) {
            unsafe {
                libc::kill(-self.0, libc::SIGKILL);
            }
        }
    }
    let _group = Group(child.id() as i32);
    std::fs::write(&marker, child.id().to_string()).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !flag.load(std::sync::atomic::Ordering::Relaxed) && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    child.wait().unwrap();
    std::fs::write(
        marker.with_extension("cleaned"),
        b"owned nested group stopped",
    )
    .unwrap();
}

#[cfg(unix)]
#[test]
fn task_cancellation_allows_owned_commands_to_clean_separate_child_groups() {
    let f = Fixture::new();
    let marker = f.p("nested");
    let profile = serde_json::json!({"version":1,"source":{"type":"none"},"actions":[{"type":"command","program":std::env::current_exe().unwrap(),"args":["--exact","task_cooperative_command_child","--nocapture"],"timeout_ms":10000}]});
    std::fs::write(f.p("task.json"), serde_json::to_vec(&profile).unwrap()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_screenshots"))
        .args(["task", "run"])
        .arg(f.p("task.json"))
        .env("SCREENSHOTS_TASK_COOPERATIVE_FIXTURE", &marker)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let nested = loop {
        if let Ok(value) = std::fs::read_to_string(&marker) {
            if let Ok(pid) = value.parse::<i32>() {
                break pid;
            }
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Owned command did not start");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    struct Group(i32);
    impl Drop for Group {
        fn drop(&mut self) {
            unsafe {
                libc::kill(-self.0, libc::SIGKILL);
            }
        }
    }
    let _nested = Group(nested);
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Task command did not stop");
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!child.wait().unwrap().success());
    assert!(
        marker.with_extension("cleaned").is_file(),
        "Command must receive cancellation before forceful fallback"
    );
}

#[cfg(unix)]
#[test]
fn task_signal_interrupts_an_upload_while_waiting_for_the_server() {
    let f = Fixture::new();
    std::fs::write(f.p("raw.bin"), b"explicit task upload").unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();
    let (sent, received) = std::sync::mpsc::sync_channel(1);
    let server = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut socket = loop {
            match listener.accept() {
                Ok((s, _)) => break s,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(e) => panic!("Task upload did not arrive: {e}"),
            }
        };
        socket.set_nonblocking(false).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            socket.read_exact(&mut byte).unwrap();
            header.push(byte[0]);
            assert!(header.len() < 16384);
        }
        let mut body = [0; 20];
        socket.read_exact(&mut body).unwrap();
        assert_eq!(&body, b"explicit task upload");
        sent.send(()).unwrap();
        let mut byte = [0];
        assert_eq!(socket.read(&mut byte).unwrap(), 0);
    });
    std::fs::write(f.p("destination.json"), serde_json::to_vec(&serde_json::json!({"url":format!("http://{addr}/upload"),"allow_http":true,"method":"PUT","response":{"type":"text"}})).unwrap()).unwrap();
    std::fs::write(f.p("task.json"), serde_json::to_vec(&serde_json::json!({"version":1,"source":{"type":"raw_file","path":"raw.bin"},"actions":[{"type":"upload","profile":"destination.json"},{"type":"qr","path":"never.png"}]})).unwrap()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_screenshots"))
        .args(["task", "run"])
        .arg(f.p("task.json"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    if received.recv_timeout(Duration::from_secs(5)).is_err() {
        let _ = child.kill();
        let output = child.wait_with_output().unwrap();
        panic!(
            "Task upload did not start: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Task upload ignored cancellation");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let output = child.wait_with_output().unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("canceled"));
    assert!(!f.p("never.png").exists());
    server.join().unwrap();
}

#[test]
fn saved_task_output_templates_can_run_repeatedly_without_overwriting() {
    let f = Fixture::new();
    f.image();
    let p = serde_json::json!({"version":1,"source":{"type":"file","path":"in.png"},"actions":[{"type":"save","path":"out-{timestamp}-{uuid}.png"}]});
    ok(f.run(&p, "run"));
    ok(f.run(&p, "run"));
    let outputs = std::fs::read_dir(&f.0)
        .unwrap()
        .map(|p| p.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("out-"))
        .collect::<Vec<_>>();
    assert_eq!(outputs.len(), 2);
    for p in outputs {
        assert_eq!(image::open(p).unwrap().dimensions(), (3, 2));
    }
}

#[test]
fn command_only_tasks_need_no_dummy_image_and_raw_uploads_do_not_decode_input() {
    let f = Fixture::new();
    let command = serde_json::json!({"version":1,"source":{"type":"none"},"actions":[{"type":"command","program":env!("CARGO_BIN_EXE_screenshots"),"args":["--version"],"timeout_ms":5000}]});
    assert_eq!(
        ok(f.run(&command, "run")).last().unwrap()["type"],
        "complete"
    );
    let mut invalid = command;
    invalid["actions"] = serde_json::json!([{"type":"save","path":"invalid.png"}]);
    assert!(!f.run(&invalid, "validate").status.success());
    assert!(!f.p("invalid.png").exists());
    let data = b"raw task input\0\xff no image";
    std::fs::write(f.p("raw.png"), data).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            let mut b = [0];
            socket.read_exact(&mut b).unwrap();
            header.push(b[0]);
            assert!(header.len() < 16384);
        }
        let length = String::from_utf8(header)
            .unwrap()
            .lines()
            .find_map(|l| {
                l.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().parse::<usize>().unwrap())
            })
            .unwrap();
        assert_eq!(length, data.len());
        let mut body = vec![0; length];
        socket.read_exact(&mut body).unwrap();
        assert_eq!(body, data);
        let url = "https://example.invalid/raw-task";
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{url}",
            url.len()
        )
        .unwrap();
    });
    std::fs::write(f.p("upload.json"),serde_json::to_vec(&serde_json::json!({"url":format!("http://{address}/file"),"method":"PUT","allow_http":true,"response":{"type":"text"}})).unwrap()).unwrap();
    let profile = serde_json::json!({"version":1,"source":{"type":"raw_file","path":"raw.png"},"actions":[{"type":"command","program":env!("CARGO_BIN_EXE_screenshots"),"args":["hash-file","{input}"],"timeout_ms":5000},{"type":"upload","profile":"upload.json"}]});
    assert_eq!(
        ok(f.run(&profile, "run")).last().unwrap()["type"],
        "complete"
    );
    worker.join().unwrap();
    assert_eq!(std::fs::read(f.p("raw.png")).unwrap(), data);
    f.no_state();
}

#[test]
fn task_first_command_child() {
    if let Some(path) = std::env::var_os("SCREENSHOTS_TASK_SIGNAL_FIXTURE") {
        std::fs::write(PathBuf::from(path).with_extension("first"), b"finished").unwrap();
    }
}

#[cfg(unix)]
#[test]
fn task_second_command_child() {
    let Some(path) = std::env::var_os("SCREENSHOTS_TASK_SIGNAL_FIXTURE") else {
        return;
    };
    let mut child = Command::new("/bin/sleep").arg("60").spawn().unwrap();
    std::fs::write(&path, format!("{} {}", std::process::id(), child.id())).unwrap();
    child.wait().unwrap();
}

#[cfg(unix)]
#[test]
fn task_signal_after_a_completed_command_still_cleans_second_command_group() {
    let f = Fixture::new();
    f.image();
    let profile = serde_json::json!({"version":1,"source":{"type":"file","path":"in.png"},"actions":[
        {"type":"command","program":std::env::current_exe().unwrap(),"args":["--exact","task_first_command_child","--nocapture"],"timeout_ms":10000},
        {"type":"command","program":std::env::current_exe().unwrap(),"args":["--exact","task_second_command_child","--nocapture"],"timeout_ms":10000}
    ]});
    std::fs::write(f.p("task.json"), serde_json::to_vec(&profile).unwrap()).unwrap();
    let marker = f.p("children");
    let mut child = Command::new(env!("CARGO_BIN_EXE_screenshots"))
        .args(["task", "run"])
        .arg(f.p("task.json"))
        .env("SCREENSHOTS_TASK_SIGNAL_FIXTURE", &marker)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let group = loop {
        if let Ok(text) = std::fs::read_to_string(&marker) {
            if let Some(group) = text
                .split_whitespace()
                .next()
                .and_then(|v| v.parse::<i32>().ok())
            {
                break group;
            }
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Second task command did not start");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(marker.with_extension("first").exists());
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() > deadline {
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
            let _ = child.kill();
            panic!("Task signal cleanup timed out");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!child.wait().unwrap().success());
    #[cfg(target_os = "linux")]
    for pid in std::fs::read_to_string(&marker)
        .unwrap()
        .split_whitespace()
        .map(|s| s.parse::<i32>().unwrap())
    {
        loop {
            let stopped = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                Ok(stat) => stat
                    .rsplit_once(')')
                    .is_some_and(|(_, v)| matches!(v.trim().as_bytes().first(), Some(b'Z' | b'X'))),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
                Err(error) => panic!("Could not inspect synthetic descendant: {error}"),
            };
            if stopped {
                break;
            }
            if std::time::Instant::now() > deadline {
                unsafe {
                    libc::kill(-group, libc::SIGKILL);
                }
                panic!("Task descendant survived cancellation");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    #[cfg(not(target_os = "linux"))]
    while unsafe { libc::kill(-group, 0) } != -1 {
        assert!(
            std::time::Instant::now() < deadline,
            "Task command group did not finish cleanup"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
