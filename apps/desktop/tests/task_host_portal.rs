#![cfg(target_os = "linux")]
//! Actual private D-Bus transport; the mock portal never requests desktop permissions.
use dbus::{
    Message, Path,
    arg::{PropMap, Variant},
    blocking::Connection,
    channel::{Channel, MatchingReceiver, Sender},
    message::MatchRule,
};
use std::{
    io::{BufRead, BufReader, Read},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
const SERVICE: &str = "org.freedesktop.portal.Desktop";
const OBJECT: &str = "/org/freedesktop/portal/desktop";
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Folder(PathBuf);
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn connect(address: &str) -> Connection {
    let mut channel = Channel::open_private(address).unwrap();
    channel.register().unwrap();
    Connection::from(channel)
}
fn response(c: &Connection, m: &Message, request: Path<'static>, code: u32, values: PropMap) {
    c.send(m.method_return().append1(request.clone())).unwrap();
    let mut signal = Message::new_signal(
        request.to_string(),
        "org.freedesktop.portal.Request",
        "Response",
    )
    .unwrap()
    .append2(code, values);
    signal.set_destination(m.sender());
    c.send(signal).unwrap();
}

#[test]
fn wayland_shortcut_portal_binds_and_activates_a_saved_task_over_private_dbus() {
    let root =
        Folder(std::env::temp_dir().join(format!("screenshots-portal-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir(&root.0).unwrap();
    let mut daemon = ChildGuard(
        Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut address = String::new();
    BufReader::new(daemon.0.stdout.take().unwrap())
        .read_line(&mut address)
        .unwrap();
    let address = address.trim().to_owned();
    let connection = connect(&address);
    connection
        .request_name(SERVICE, false, false, true)
        .unwrap();
    let session = Arc::new(Mutex::new(None::<(Path<'static>, String, String)>));
    let observed = session.clone();
    let calls = Arc::new(Mutex::new(Vec::<String>::new()));
    let counted = calls.clone();
    connection.start_receive(
        MatchRule::new_method_call(),
        Box::new(move |m, c| {
            let member = m.member().unwrap().to_string();
            counted.lock().unwrap().push(member.clone());
            match member.as_str() {
                "CreateSession" => {
                    let (options,): (PropMap,) = m.read1().map(|o| (o,)).unwrap();
                    let token = options["handle_token"].0.as_str().unwrap();
                    let session_token = options["session_handle_token"].0.as_str().unwrap();
                    let sender = m.sender().unwrap().to_string();
                    let unique = sender.trim_start_matches(':').replace('.', "_");
                    let path = Path::new(format!(
                        "/org/freedesktop/portal/desktop/session/{unique}/{session_token}"
                    ))
                    .unwrap();
                    *observed.lock().unwrap() = Some((path.clone(), String::new(), sender));
                    let mut result = PropMap::new();
                    result.insert("session_handle".into(), Variant(Box::new(path.to_string())));
                    response(
                        c,
                        &m,
                        Path::new(format!(
                            "/org/freedesktop/portal/desktop/request/{unique}/{token}"
                        ))
                        .unwrap(),
                        0,
                        result,
                    );
                }
                "BindShortcuts" => {
                    let (actual, shortcuts, _parent, options): (
                        Path<'static>,
                        Vec<(String, PropMap)>,
                        String,
                        PropMap,
                    ) = m.read4().unwrap();
                    assert_eq!(shortcuts.len(), 1);
                    assert_eq!(
                        shortcuts[0].1["preferred_trigger"].0.as_str(),
                        Some("CTRL+SHIFT+F9")
                    );
                    let mut current = observed.lock().unwrap();
                    let data = current.as_mut().unwrap();
                    assert_eq!(data.0, actual);
                    data.1 = shortcuts[0].0.clone();
                    let unique = data.2.trim_start_matches(':').replace('.', "_");
                    let token = options["handle_token"].0.as_str().unwrap();
                    let mut result = PropMap::new();
                    result.insert("shortcuts".into(), Variant(Box::new(shortcuts)));
                    response(
                        c,
                        &m,
                        Path::new(format!(
                            "/org/freedesktop/portal/desktop/request/{unique}/{token}"
                        ))
                        .unwrap(),
                        0,
                        result,
                    );
                }
                "ActivateFixture" => {
                    let current = observed.lock().unwrap();
                    let (session, id, destination) = current.as_ref().unwrap();
                    let mut signal = Message::new_signal(
                        OBJECT,
                        "org.freedesktop.portal.GlobalShortcuts",
                        "Activated",
                    )
                    .unwrap()
                    .append3(session.clone(), id.clone(), 0u64)
                    .append1(PropMap::new());
                    signal.set_destination(Some(
                        dbus::strings::BusName::new(destination.clone()).unwrap(),
                    ));
                    c.send(signal).unwrap();
                    c.send(m.method_return()).unwrap();
                }
                "Close" => {
                    c.send(m.method_return()).unwrap();
                }
                _ => panic!("Unexpected private portal method {member}"),
            }
            true
        }),
    );
    let stopping = Arc::new(AtomicBool::new(false));
    let stop = stopping.clone();
    let server = std::thread::spawn(move || {
        while !stop.load(Ordering::SeqCst) {
            connection.process(Duration::from_millis(20)).unwrap();
        }
    });
    let image = root.0.join("source.png");
    image::RgbaImage::from_pixel(2, 3, image::Rgba([19, 37, 57, 255]))
        .save(&image)
        .unwrap();
    let output = root.0.join("result.png");
    let task = root.0.join("task.json");
    let config = root.0.join("host.json");
    let endpoint = root.0.join("endpoint.json");
    std::fs::write(&task,serde_json::to_vec(&serde_json::json!({"version":1,"source":{"type":"file","path":image},"actions":[{"type":"save","path":output}]})).unwrap()).unwrap();
    std::fs::write(&config,serde_json::to_vec(&serde_json::json!({"version":1,"bindings":[{"id":"capture","task":task,"hotkey":"Ctrl+Shift+F9"}]})).unwrap()).unwrap();
    let env = |command: &mut Command| {
        for key in [
            "HOME",
            "USERPROFILE",
            "APPDATA",
            "LOCALAPPDATA",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
            "XDG_CACHE_HOME",
        ] {
            command.env(key, root.0.join(key));
        }
        command
            .env("DBUS_SESSION_BUS_ADDRESS", &address)
            .env("XDG_SESSION_TYPE", "wayland")
            .env("WAYLAND_DISPLAY", "private-test")
            .env_remove("DISPLAY");
    };
    let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
    command
        .args(["task-host", "serve"])
        .arg(&config)
        .arg(&endpoint)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    env(&mut command);
    let mut host = ChildGuard(command.spawn().unwrap());
    let stdout = host.0.stdout.take().unwrap();
    let stderr = host.0.stderr.take().unwrap();
    let errors = std::thread::spawn(move || {
        let mut errors = Vec::new();
        stderr.take(16384).read_to_end(&mut errors).unwrap();
        errors
    });
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut line = String::new();
        BufReader::new(stdout).read_line(&mut line).unwrap();
        tx.send(line).ok();
    });
    let ready = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("Portal host did not become ready");
    if ready.is_empty() {
        let _ = host.0.wait();
        stopping.store(true, Ordering::SeqCst);
        server.join().unwrap();
        panic!(
            "Portal host failed before readiness: {}",
            String::from_utf8_lossy(&errors.join().unwrap())
        );
    }
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&ready).unwrap()["ready"],
        true
    );
    reader.join().unwrap();
    assert!(!output.exists());
    let controller = connect(&address);
    let proxy = controller.with_proxy(SERVICE, OBJECT, Duration::from_secs(2));
    let _: () = proxy
        .method_call("org.lejy.Screenshots.Test", "ActivateFixture", ())
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !output.exists() {
        assert!(
            Instant::now() < deadline,
            "Portal activation did not execute the saved task"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        image::open(&output).unwrap().into_rgba8().get_pixel(0, 0).0,
        [19, 37, 57, 255]
    );
    let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
    command.args(["task-host", "stop"]).arg(&endpoint);
    env(&mut command);
    let stopped = command.output().unwrap();
    assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    while host.0.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "Portal host did not stop");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(host.0.wait().unwrap().success());
    assert!(errors.join().unwrap().is_empty());
    assert!(!endpoint.exists());
    stopping.store(true, Ordering::SeqCst);
    server.join().unwrap();
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|v| v.as_str() == "CreateSession")
            .count(),
        1
    );
    assert_eq!(
        calls
            .iter()
            .filter(|v| v.as_str() == "BindShortcuts")
            .count(),
        1
    );
}
