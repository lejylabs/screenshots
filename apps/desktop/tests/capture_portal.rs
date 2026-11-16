#![cfg(target_os = "linux")]
//! Generated pixels on private D-Bus/PipeWire only; no ambient session/permissions.
use anyhow::{Result, ensure};
use dbus::{
    Message, Path as BusPath,
    arg::{OwnedFd as BusFd, PropMap, Variant},
    blocking::Connection,
    channel::{Channel, MatchingReceiver, Sender},
    message::MatchRule,
};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct Folder(PathBuf);
impl Folder {
    fn new() -> Self {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir().join(format!("screenshots-pw-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn connection(address: &str) -> Connection {
    let mut channel = Channel::open_private(address).unwrap();
    channel.register().unwrap();
    Connection::from(channel)
}
fn wait_file(path: &Path, until: Instant) -> Result<String> {
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            if !text.is_empty() {
                return Ok(text);
            }
        }
        ensure!(Instant::now() < until, "Private fixture startup timeout");
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn reply(connection: &Connection, message: &Message, token: &str, values: PropMap, code: u32) {
    let unique = message
        .sender()
        .unwrap()
        .to_string()
        .trim_start_matches(':')
        .replace('.', "_");
    let path = BusPath::new(format!(
        "/org/freedesktop/portal/desktop/request/{unique}/{token}"
    ))
    .unwrap();
    connection
        .send(message.method_return().append1(path.clone()))
        .unwrap();
    let mut signal = Message::new_signal(
        path.to_string(),
        "org.freedesktop.portal.Request",
        "Response",
    )
    .unwrap()
    .append2(code, values);
    signal.set_destination(message.sender());
    connection.send(signal).unwrap();
}
#[derive(Default)]
struct Observed {
    calls: Vec<String>,
    sessions: HashMap<String, String>,
    closes: usize,
    selected: Vec<(u32, u32)>,
    owners: Vec<String>,
}
struct Portal {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    observed: Arc<Mutex<Observed>>,
    mode: Arc<Mutex<String>>,
}
impl Drop for Portal {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
fn portal(address: &str, root: &Path, node: u32, serial: u64) -> Portal {
    let connection = connection(address);
    connection
        .request_name("org.freedesktop.portal.Desktop", false, false, true)
        .unwrap();
    let observed = Arc::new(Mutex::new(Observed::default()));
    let lost = observed.clone();
    connection
        .add_match::<(String, String, String), _>(
            MatchRule::new_signal("org.freedesktop.DBus", "NameOwnerChanged"),
            move |(name, _, owner), _, _| {
                if owner.is_empty() {
                    lost.lock()
                        .unwrap()
                        .sessions
                        .retain(|_, current| current != &name);
                }
                true
            },
        )
        .unwrap();
    let mode = Arc::new(Mutex::new(String::from("v6")));
    let seen = observed.clone();
    let behavior = mode.clone();
    let root = root.to_owned();
    connection.start_receive(
        MatchRule::new_method_call(),
        Box::new(move |message, c| {
            let member = message.member().unwrap().to_string();
            let mode = behavior.lock().unwrap().clone();
            seen.lock().unwrap().calls.push(member.clone());
            match member.as_str() {
                "GetAll" => {
                    let mut values = PropMap::new();
                    values.insert(
                        "version".into(),
                        Variant(Box::new(if mode == "v3" { 3u32 } else { 6u32 })),
                    );
                    values.insert(
                        "AvailableSourceTypes".into(),
                        Variant(Box::new(if mode == "unsupported" { 0u32 } else { 3u32 })),
                    );
                    values.insert(
                        "AvailableCursorModes".into(),
                        Variant(Box::new(if mode == "unsupported-cursor" {
                            0u32
                        } else {
                            3u32
                        })),
                    );
                    c.send(message.method_return().append1(values)).unwrap();
                }
                "Get" => {
                    let (_, property): (String, String) = message.read2().unwrap();
                    let value = match property.as_str() {
                        "version" => {
                            if mode == "v3" {
                                3u32
                            } else {
                                6u32
                            }
                        }
                        "AvailableSourceTypes" => {
                            if mode == "unsupported" {
                                0
                            } else {
                                3
                            }
                        }
                        "AvailableCursorModes" => {
                            if mode == "unsupported-cursor" {
                                0
                            } else {
                                3
                            }
                        }
                        _ => panic!("Unexpected property"),
                    };
                    c.send(message.method_return().append1(Variant(value)))
                        .unwrap();
                }
                "CreateSession" => {
                    let options: PropMap = message.read1().unwrap();
                    let token = options["handle_token"].0.as_str().unwrap();
                    let sender = message.sender().unwrap().to_string();
                    let unique = sender.trim_start_matches(':').replace('.', "_");
                    let path = format!(
                        "/org/freedesktop/portal/desktop/session/{unique}/{}",
                        options["session_handle_token"].0.as_str().unwrap()
                    );
                    {
                        let mut seen = seen.lock().unwrap();
                        seen.sessions.insert(path.clone(), sender.clone());
                        seen.owners.push(sender);
                    }
                    let mut values = PropMap::new();
                    values.insert("session_handle".into(), Variant(Box::new(path)));
                    reply(c, &message, token, values, 0);
                }
                "SelectSources" => {
                    let (_session, options): (BusPath<'static>, PropMap) = message.read2().unwrap();
                    assert_eq!(options["multiple"].0.as_i64(), Some(0));
                    assert_eq!(options["persist_mode"].0.as_u64(), Some(0));
                    assert!(!options.contains_key("restore_token"));
                    seen.lock().unwrap().selected.push((
                        options["types"].0.as_u64().unwrap() as u32,
                        options["cursor_mode"].0.as_u64().unwrap() as u32,
                    ));
                    reply(
                        c,
                        &message,
                        options["handle_token"].0.as_str().unwrap(),
                        PropMap::new(),
                        0,
                    );
                }
                "Start" => {
                    let (session, parent, options): (BusPath<'static>, String, PropMap) =
                        message.read3().unwrap();
                    assert!(parent.is_empty());
                    let mut props = PropMap::new();
                    props.insert(
                        "source_type".into(),
                        Variant(Box::new(if mode == "wrong-source" {
                            4u32
                        } else {
                            seen.lock().unwrap().selected.last().unwrap().0
                        })),
                    );
                    // This independent libdbus encoder deliberately uses the specification's hyphen.
                    if mode != "v3" && mode != "missing-serial" {
                        props.insert(
                            if mode == "underscore-serial" {
                                "pipewire_serial"
                            } else {
                                "pipewire-serial"
                            }
                            .into(),
                            Variant(Box::new(if mode == "wrong-serial" {
                                serial + 1
                            } else {
                                serial
                            })),
                        );
                    }
                    // Logical portal coordinates must not select or crop physical buffer pixels.
                    props.insert("position".into(), Variant(Box::new((-900i32, 21i32))));
                    props.insert("size".into(), Variant(Box::new((1i32, 1i32))));
                    let mut streams = vec![(node, props)];
                    if mode == "multiple" {
                        streams.push((node, PropMap::new()));
                    }
                    let mut values = PropMap::new();
                    values.insert("streams".into(), Variant(Box::new(streams)));
                    reply(
                        c,
                        &message,
                        options["handle_token"].0.as_str().unwrap(),
                        values,
                        if mode == "denied" { 1 } else { 0 },
                    );
                    if mode == "closed" {
                        let mut signal = Message::new_signal(
                            session.to_string(),
                            "org.freedesktop.portal.Session",
                            "Closed",
                        )
                        .unwrap()
                        .append1(PropMap::new());
                        signal.set_destination(message.sender());
                        c.send(signal).unwrap();
                    }
                }
                "OpenPipeWireRemote" => {
                    use std::os::fd::IntoRawFd;
                    let fd = if mode == "invalid-fd" {
                        std::fs::File::open("/dev/null").unwrap().into_raw_fd()
                    } else {
                        std::os::unix::net::UnixStream::connect(root.join("screenshots-private"))
                            .unwrap()
                            .into_raw_fd()
                    };
                    c.send(message.method_return().append1(unsafe { BusFd::new(fd) }))
                        .unwrap();
                }
                "Close" => {
                    let path = message.path().unwrap().to_string();
                    let mut seen = seen.lock().unwrap();
                    seen.sessions.remove(&path);
                    seen.closes += 1;
                    if mode == "close-error" {
                        c.send(
                            message.error(
                                &dbus::strings::ErrorName::new("org.freedesktop.DBus.Error.Failed")
                                    .unwrap(),
                                &std::ffi::CString::new("generated cleanup refusal").unwrap(),
                            ),
                        )
                        .unwrap();
                    } else {
                        c.send(message.method_return()).unwrap();
                    }
                }
                _ => {
                    c.send(
                        message.error(
                            &dbus::strings::ErrorName::new(
                                "org.freedesktop.DBus.Error.UnknownMethod",
                            )
                            .unwrap(),
                            &std::ffi::CString::new("unexpected private fixture method").unwrap(),
                        ),
                    )
                    .unwrap();
                }
            }
            true
        }),
    );
    let stop = Arc::new(AtomicBool::new(false));
    let halted = stop.clone();
    let thread = std::thread::spawn(move || {
        while !halted.load(Ordering::Acquire) {
            connection.process(Duration::from_millis(20)).unwrap();
        }
    });
    Portal {
        stop,
        thread: Some(thread),
        observed,
        mode,
    }
}
fn configuration(root: &Path) -> Result<PathBuf> {
    let path = root.join("private.conf");
    std::fs::write(
        &path,
        r#"
context.properties = { core.daemon = true core.name = screenshots-private support.dbus = false mem.warn-mlock = false }
context.spa-libs = { support.* = support/libspa-support }
context.modules = [ { name = libpipewire-module-protocol-native } { name = libpipewire-module-access args = { access.force = unrestricted } } { name = libpipewire-module-spa-node-factory } { name = libpipewire-module-client-node } { name = libpipewire-module-link-factory } { name = libpipewire-module-adapter } ]
context.objects = [ { factory = spa-node-factory args = { factory.name = support.node.driver node.name = Dummy-Driver node.group = pipewire.dummy priority.driver = 20000 } } ]
"#,
    )?;
    Ok(path)
}
fn private(command: &mut Command, root: &Path, address: &str) {
    parent_death(command);
    for name in [
        "PIPEWIRE_CONFIG_DIR",
        "PIPEWIRE_CONFIG_NAME",
        "PIPEWIRE_CONFIG_PREFIX",
        "PIPEWIRE_MODULE_DIR",
        "SPA_PLUGIN_DIR",
        "PIPEWIRE_LOG",
    ] {
        command.env_remove(name);
    }
    for name in ["HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"] {
        command.env(name, root.join(name));
    }
    command
        .env("XDG_RUNTIME_DIR", root)
        .env("PIPEWIRE_RUNTIME_DIR", root)
        .env("DBUS_SESSION_BUS_ADDRESS", address)
        .env("PIPEWIRE_REMOTE", "must-not-use-ambient-remote")
        .env("WAYLAND_DISPLAY", "private-not-a-real-compositor")
        .env("XDG_SESSION_TYPE", "wayland")
        .env_remove("DISPLAY")
        .env_remove("AT_SPI_BUS_ADDRESS")
        .env_remove("SCREENSHOTS_TASK_CLIPBOARD")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
}
fn parent_death(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    let parent = std::process::id() as i32;
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 || libc::getppid() != parent
            {
                return Err(std::io::Error::other("Private fixture owner unavailable"));
            }
            Ok(())
        });
    }
}
fn run(command: &mut Command, limit: Duration) -> Result<std::process::Output> {
    let mut child = ChildGuard(command.spawn()?);
    let until = Instant::now() + limit;
    loop {
        if child.0.try_wait()?.is_some() {
            break;
        }
        ensure!(
            Instant::now() < until,
            "Private portal command exceeded its deadline"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let stdout = child
        .0
        .stdout
        .take()
        .map(|mut file| {
            let mut data = Vec::new();
            std::io::Read::read_to_end(&mut file, &mut data).map(|_| data)
        })
        .transpose()?
        .unwrap_or_default();
    let stderr = child
        .0
        .stderr
        .take()
        .map(|mut file| {
            let mut data = Vec::new();
            std::io::Read::read_to_end(&mut file, &mut data).map(|_| data)
        })
        .transpose()?
        .unwrap_or_default();
    Ok(std::process::Output {
        status: child.0.wait()?,
        stdout,
        stderr,
    })
}
fn expected(x: u32, y: u32) -> [u8; 4] {
    [
        (x * 17 + 7) as u8,
        (y * 23 + 11) as u8,
        ((x + y) * 13 + 3) as u8,
        if x == 0 { 0 } else { (x * 71 + 19) as u8 },
    ]
}

#[test]
fn portal_cli_preflight_refuses_invalid_inputs_before_permission_access() -> Result<()> {
    let root = Folder::new();
    let existing = root.0.join("existing.png");
    std::fs::write(&existing, b"generated existing output")?;
    for (output, extra, message) in [
        (
            root.0.join("cursor.png"),
            vec!["--cursor", "metadata"],
            "Cursor must",
        ),
        (
            root.0.join("delay.png"),
            vec!["--delay-ms", "3600001"],
            "Delay must",
        ),
        (root.0.join("format.bmp"), vec![], ".png"),
        (existing.clone(), vec![], "already exists"),
        (
            root.0.join("frame.png"),
            vec!["--window-frame", "client"],
            "Usage:",
        ),
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        private(
            &mut command,
            &root.0,
            "unix:path=/unavailable-private-fixture-bus",
        );
        command
            .args(["capture-portal", "window"])
            .arg(&output)
            .args(extra);
        let result = run(&mut command, Duration::from_secs(2))?;
        ensure!(
            !result.status.success() && String::from_utf8_lossy(&result.stderr).contains(message),
            "Portal preflight did not reject before native work: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        ensure!(
            output == existing || !output.exists(),
            "Preflight published output"
        );
    }
    ensure!(
        std::fs::read(existing)? == b"generated existing output",
        "Existing output changed"
    );
    Ok(())
}

#[test]
#[ignore = "explicit private D-Bus/PipeWire generated-pixel E2E; no user session"]
fn portal_memory_capture_uses_the_authorized_fd_and_exact_serial() -> Result<()> {
    let performance = std::env::var("SCREENSHOTS_PORTAL_PERFORMANCE").as_deref() == Ok("1");
    let root = Folder::new();
    let config = configuration(&root.0)?;
    let bus_config = root.0.join("bus.conf");
    std::fs::write(
        &bus_config,
        format!(
            "<busconfig><type>session</type><listen>unix:tmpdir={}</listen><auth>EXTERNAL</auth><policy context=\"default\"><allow send_destination=\"*\"/><allow receive_sender=\"*\"/><allow own=\"*\"/></policy></busconfig>",
            root.0.display()
        ),
    )?;
    let mut bus_command = Command::new("dbus-daemon");
    bus_command
        .arg("--config-file")
        .arg(bus_config)
        .args(["--nofork", "--print-address=1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    parent_death(&mut bus_command);
    let mut bus = ChildGuard(bus_command.spawn()?);
    let mut address = String::new();
    BufReader::new(bus.0.stdout.take().unwrap()).read_line(&mut address)?;
    let address = address.trim();
    let mut server = Command::new("pipewire");
    private(&mut server, &root.0, address);
    server
        .arg("-c")
        .arg(config)
        .env("PIPEWIRE_DEBUG", "0")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _server = ChildGuard(server.spawn()?);
    let until = Instant::now() + Duration::from_secs(5);
    while !root.0.join("screenshots-private").exists() {
        ensure!(Instant::now() < until, "Private server not ready");
        std::thread::sleep(Duration::from_millis(5));
    }
    let mut publisher = Command::new(std::env::current_exe()?);
    private(&mut publisher, &root.0, address);
    publisher
        .args(["--exact", "pipewire_publisher_fixture", "--nocapture"])
        .env("SCREENSHOTS_PORTAL_PUBLISHER", &root.0)
        .env(
            "SCREENSHOTS_PORTAL_WIDTH",
            if performance { "3840" } else { "4" },
        )
        .env(
            "SCREENSHOTS_PORTAL_HEIGHT",
            if performance { "2160" } else { "3" },
        )
        .env("PIPEWIRE_REMOTE", "screenshots-private")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _publisher = ChildGuard(publisher.spawn()?);
    let node: serde_json::Value = serde_json::from_str(&wait_file(
        &root.0.join("node.json"),
        Instant::now() + Duration::from_secs(5),
    )?)?;
    let portal = portal(
        address,
        &root.0,
        node["node"].as_u64().unwrap() as u32,
        node["serial"].as_u64().unwrap(),
    );
    eprintln!("Private generated-pixel publisher ready");
    if performance {
        return performance_case(&root.0, address, &portal);
    }
    for (index, mode) in [
        "v6",
        "v3",
        "v6-window",
        "denied",
        "unsupported",
        "unsupported-cursor",
        "multiple",
        "wrong-source",
        "missing-serial",
        "underscore-serial",
        "wrong-serial",
        "invalid-fd",
        "closed",
        "close-error",
    ]
    .into_iter()
    .enumerate()
    {
        eprintln!("Private portal case {mode}");
        *portal.mode.lock().unwrap() = mode.into();
        let output = root.0.join(format!("out-{index}.png"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        private(&mut command, &root.0, address);
        command
            .args([
                "capture-portal",
                if mode == "v6-window" {
                    "window"
                } else {
                    "display"
                },
            ])
            .arg(&output)
            .args(["--cursor", if index == 0 { "include" } else { "exclude" }]);
        let result = run(&mut command, Duration::from_secs(25))?;
        if !result.status.success() && matches!(mode, "v6" | "v3" | "v6-window") {
            eprintln!(
                "Private portal calls: {:?}",
                portal.observed.lock().unwrap().calls
            );
            eprintln!(
                "Private graph: {}",
                std::fs::read_to_string(root.0.join("graph.log")).unwrap_or_default()
            );
        }
        if matches!(mode, "v6" | "v3" | "v6-window") {
            ensure!(
                result.status.success(),
                "{mode} capture failed: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            let pixels = image::open(&output)?.into_rgba8();
            ensure!(
                pixels.dimensions() == (4, 3),
                "Logical portal size replaced native pixels"
            );
            for (x, y, pixel) in pixels.enumerate_pixels() {
                ensure!(pixel.0 == expected(x, y), "Wrong generated portal pixel");
            }
        } else {
            ensure!(
                !result.status.success() && !output.exists(),
                "{mode} published unexpected output"
            );
        }
        ensure!(
            portal.observed.lock().unwrap().sessions.is_empty(),
            "{mode} leaked portal session"
        );
    }
    *portal.mode.lock().unwrap() = "v6".into();
    let stall = root.0.join("stall");
    std::fs::write(&stall, b"generated stalled source")?;
    let lost_output = root.0.join("lost.png");
    let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
    private(&mut command, &root.0, address);
    command
        .args(["capture-portal", "display"])
        .arg(&lost_output);
    let owners = portal.observed.lock().unwrap().owners.len();
    let mut owner = ChildGuard(command.spawn()?);
    let until = Instant::now() + Duration::from_secs(5);
    let unique = loop {
        let seen = portal.observed.lock().unwrap();
        if seen.owners.len() > owners
            && seen
                .calls
                .last()
                .is_some_and(|call| call == "OpenPipeWireRemote")
        {
            break seen.owners.last().unwrap().clone();
        }
        drop(seen);
        ensure!(
            Instant::now() < until,
            "Stalled native portal did not start"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    owner.0.kill()?;
    owner.0.wait()?;
    let oracle = connection(address);
    let bus = oracle.with_proxy(
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        Duration::from_secs(1),
    );
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        let (alive,): (bool,) =
            bus.method_call("org.freedesktop.DBus", "NameHasOwner", (unique.as_str(),))?;
        if !alive && portal.observed.lock().unwrap().sessions.is_empty() {
            break;
        }
        ensure!(
            Instant::now() < until,
            "Parent loss retained native portal ownership"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    ensure!(!lost_output.exists(), "Parent loss published a late frame");
    let timeout_output = root.0.join("timeout.png");
    let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
    private(&mut command, &root.0, address);
    command
        .args(["capture-portal", "display"])
        .arg(&timeout_output);
    let started = Instant::now();
    let result = run(&mut command, Duration::from_secs(24))?;
    ensure!(
        !result.status.success()
            && !timeout_output.exists()
            && started.elapsed() < Duration::from_secs(23),
        "Missing frame did not enforce its 20s deadline"
    );
    std::fs::remove_file(stall)?;
    let profile = root.0.join("task.json");
    let task_output = root.0.join("task.png");
    let recipe = root.0.join("recipe.json");
    std::fs::write(
        &recipe,
        serde_json::to_vec(
            &serde_json::json!({"version":1,"steps":[{"op":"crop","x":1,"y":1,"width":2,"height":2}]}),
        )?,
    )?;
    std::fs::write(
        &profile,
        serde_json::to_vec(
            &serde_json::json!({"version":1,"source":{"type":"portal","source":"window"},"recipe":recipe,"actions":[{"type":"save","path":task_output}]}),
        )?,
    )?;
    let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
    private(&mut command, &root.0, address);
    command.args(["task", "run"]).arg(&profile);
    let result = run(&mut command, Duration::from_secs(25))?;
    ensure!(
        result.status.success(),
        "Typed portal task failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let pixels = image::open(task_output)?.into_rgba8();
    ensure!(
        pixels.dimensions() == (2, 2),
        "Portal task recipe was not applied"
    );
    for (x, y, pixel) in pixels.enumerate_pixels() {
        ensure!(
            pixel.0 == expected(x + 1, y + 1),
            "Portal task finalized wrong pixels"
        );
    }
    let before = portal.observed.lock().unwrap().calls.len();
    let legacy = root.0.join("legacy.png");
    let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
    private(&mut command, &root.0, address);
    command
        .args(["capture-task", "display", "123"])
        .arg(&legacy);
    let result = run(&mut command, Duration::from_secs(2))?;
    ensure!(
        !result.status.success()
            && !legacy.exists()
            && portal.observed.lock().unwrap().calls.len() == before,
        "Named Wayland source substituted a picker"
    );
    std::fs::write(root.0.join("stall"), b"generated source removal")?;
    let consumer = root.0.join("consumer");
    let _ = std::fs::remove_file(&consumer);
    let removed_output = root.0.join("removed.png");
    let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
    private(&mut command, &root.0, address);
    command
        .args(["capture-portal", "display"])
        .arg(&removed_output);
    let mut removed = ChildGuard(command.spawn()?);
    wait_file(&consumer, Instant::now() + Duration::from_secs(5))?;
    let started = Instant::now();
    std::fs::write(root.0.join("disconnect"), b"remove only generated source")?;
    loop {
        if let Some(status) = removed.0.try_wait()? {
            ensure!(!status.success(), "Removed source capture succeeded");
            break;
        }
        ensure!(
            started.elapsed() < Duration::from_secs(3),
            "Source removal was not observed promptly"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    ensure!(
        !removed_output.exists() && portal.observed.lock().unwrap().sessions.is_empty(),
        "Source removal retained output/session"
    );
    ensure!(
        portal.observed.lock().unwrap().selected.contains(&(1, 2)),
        "Embedded cursor request missing"
    );
    ensure!(
        portal
            .observed
            .lock()
            .unwrap()
            .calls
            .iter()
            .all(|call| call != "Screenshot" && call != "ScreenshotArea"),
        "Forbidden disk fallback"
    );
    let remaining: Vec<_> = std::fs::read_dir(&root.0)?
        .filter_map(|item| item.ok())
        .map(|item| item.file_name())
        .collect();
    ensure!(
        remaining
            .iter()
            .filter(|name| Path::new(name)
                .extension()
                .is_some_and(|extension| extension == "png"))
            .count()
            == 4,
        "Unexpected staging image"
    );
    Ok(())
}

#[test]
fn pipewire_publisher_fixture() -> Result<()> {
    let Some(root) = std::env::var_os("SCREENSHOTS_PORTAL_PUBLISHER") else {
        return Ok(());
    };
    let root = PathBuf::from(root);
    ensure!(
        std::env::var_os("PIPEWIRE_RUNTIME_DIR").as_deref() == Some(root.as_os_str())
            && std::env::var("PIPEWIRE_REMOTE").as_deref() == Ok("screenshots-private")
            && root.join("private.conf").is_file(),
        "Publisher requires its explicit private server"
    );
    publisher(&root)
}
fn performance_case(root: &Path, address: &str, portal: &Portal) -> Result<()> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let binary = std::env::var_os("SCREENSHOTS_PORTAL_BINARY")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_screenshots")));
    let image = std::fs::metadata(&binary)?;
    use std::os::unix::fs::MetadataExt;
    let identity = (image.dev(), image.ino());
    let mut file = std::fs::File::open(&binary)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let length = file.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        digest.update(&buffer[..length]);
    }
    let hash = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let mut capture = Vec::new();
    let mut total = Vec::new();
    for index in 0..33 {
        let image = std::fs::metadata(&binary)?;
        ensure!(
            (image.dev(), image.ino()) == identity,
            "Performance executable changed"
        );
        let output = root.join(format!("performance-{index}.png"));
        let mut command = Command::new(&binary);
        private(&mut command, root, address);
        command.args(["capture-portal", "display"]).arg(&output);
        let started = Instant::now();
        let result = run(&mut command, Duration::from_secs(25))?;
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        ensure!(
            result.status.success(),
            "Generated 4K portal capture failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&result.stdout)?;
        let pixels = image::open(&output)?.into_rgba8();
        ensure!(
            pixels.dimensions() == (3840, 2160),
            "Unexpected 4K geometry"
        );
        for (x, y, pixel) in pixels.enumerate_pixels() {
            ensure!(pixel.0 == expected(x, y), "4K portal pixels differ");
        }
        drop(pixels);
        std::fs::remove_file(output)?;
        if index >= 3 {
            capture.push(report["capture_elapsed_ms"].as_f64().unwrap());
            total.push(elapsed);
        }
        ensure!(
            portal.observed.lock().unwrap().sessions.is_empty(),
            "4K session remained active"
        );
    }
    let percentiles = |mut values: Vec<f64>| {
        values.sort_by(f64::total_cmp);
        serde_json::json!({"p50_ms":values[14],"p95_ms":values[28],"maximum_ms":values[29]})
    };
    let result = serde_json::json!({"width":3840,"height":2160,"samples":30,"warmups":3,"binary_sha256":hash,
        "capture":percentiles(capture.clone()),"cli_including_png":percentiles(total.clone()),"capture_ms":capture,"cli_including_png_ms":total,
        "exact_generated_pixels":true,"authorized_remote_fd":true,"serial_targeting":true,"sessions_closed":true,"hardware_acceptance":false,
        "scope":"Private mock permission selection and software source; capture elapsed includes portal/PipeWire setup and first frame; CLI includes worker transfer and final PNG output. No native compositor/user permission latency acceptance."});
    if let Some(path) = std::env::var_os("SCREENSHOTS_PORTAL_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&result)?)?;
    }
    eprintln!("{result}");
    Ok(())
}
fn publisher(root: &Path) -> Result<()> {
    use pipewire::{self as pw, properties::properties, spa};
    use std::{cell::RefCell, rc::Rc};
    let width = std::env::var("SCREENSHOTS_PORTAL_WIDTH")?.parse::<u32>()?;
    let height = std::env::var("SCREENSHOTS_PORTAL_HEIGHT")?.parse::<u32>()?;
    app_core::validate_image_size(width, height)?;
    let extent = width * height * 4;
    let stride = width * 4;
    pw::init();
    let main = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&main, None)?;
    let core = context.connect_rc(None)?;
    let stream = pw::stream::StreamRc::new(
        core.clone(),
        "fixture-pixels",
        properties! {"media.type"=>"Video","media.category"=>"Capture","media.class"=>"Video/Source","node.name"=>"screenshots-fixture-source","node.driver"=>"true","node.always-process"=>"true"},
    )?;
    let format = Rc::new(RefCell::new(None));
    let fmt = format;
    let root_path = root.to_owned();
    let _listener = stream
        .add_local_listener_with_user_data(())
        .param_changed(move |stream, _, id, param| {
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let Some(param) = param else {
                return;
            };
            let mut info = spa::param::video::VideoInfoRaw::new();
            info.parse(param).unwrap();
            *fmt.borrow_mut() = Some(info);
            let object = spa::pod::Object {
                type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
                id: spa::param::ParamType::Buffers.as_raw(),
                properties: vec![
                    spa::pod::Property::new(
                        spa::sys::SPA_PARAM_BUFFERS_buffers,
                        spa::pod::Value::Int(2),
                    ),
                    spa::pod::Property::new(
                        spa::sys::SPA_PARAM_BUFFERS_blocks,
                        spa::pod::Value::Int(1),
                    ),
                    spa::pod::Property::new(
                        spa::sys::SPA_PARAM_BUFFERS_size,
                        spa::pod::Value::Int(extent as i32),
                    ),
                    spa::pod::Property::new(
                        spa::sys::SPA_PARAM_BUFFERS_stride,
                        spa::pod::Value::Int(stride as i32),
                    ),
                ],
            };
            let bytes = spa::pod::serialize::PodSerializer::serialize(
                std::io::Cursor::new(Vec::new()),
                &spa::pod::Value::Object(object),
            )
            .unwrap()
            .0
            .into_inner();
            stream
                .update_params(&mut [spa::pod::Pod::from_bytes(&bytes).unwrap()])
                .unwrap();
        })
        .process(move |stream, _| {
            if root_path.join("stall").exists() {
                return;
            }
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            let datas = buffer.datas_mut();
            if datas.len() != 1 {
                return;
            }
            let data = &mut datas[0];
            let Some(bytes) = data.data() else {
                return;
            };
            assert!(bytes.len() >= extent as usize);
            for y in 0..height {
                for x in 0..width {
                    let at = (y * width + x) as usize * 4;
                    bytes[at..at + 4].copy_from_slice(&expected(x, y));
                }
            }
            *data.chunk_mut().offset_mut() = 0;
            *data.chunk_mut().size_mut() = extent;
            *data.chunk_mut().stride_mut() = stride as i32;
        })
        .register()?;
    let mut info = spa::param::video::VideoInfoRaw::new();
    info.set_format(spa::param::video::VideoFormat::RGBA);
    info.set_size(spa::utils::Rectangle { width, height });
    info.set_framerate(spa::utils::Fraction { num: 30, denom: 1 });
    let object = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: vec![
            spa::pod::property!(
                spa::param::format::FormatProperties::MediaType,
                Id,
                spa::param::format::MediaType::Video
            ),
            spa::pod::property!(
                spa::param::format::FormatProperties::MediaSubtype,
                Id,
                spa::param::format::MediaSubtype::Raw
            ),
            spa::pod::property!(
                spa::param::format::FormatProperties::VideoFormat,
                Id,
                spa::param::video::VideoFormat::RGBA
            ),
            spa::pod::property!(
                spa::param::format::FormatProperties::VideoSize,
                Rectangle,
                spa::utils::Rectangle { width, height }
            ),
            spa::pod::property!(
                spa::param::format::FormatProperties::VideoFramerate,
                Fraction,
                spa::utils::Fraction { num: 30, denom: 1 }
            ),
        ],
    };
    let bytes = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(object),
    )?
    .0
    .into_inner();
    stream.connect(
        spa::utils::Direction::Output,
        None,
        pw::stream::StreamFlags::DRIVER | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut [spa::pod::Pod::from_bytes(&bytes).unwrap()],
    )?;
    struct Graph {
        root: PathBuf,
        source: Option<(u32, u64)>,
        consumers: HashMap<u32, String>,
        ports: Vec<(u32, u32, String)>,
        links: HashMap<u32, pw::link::Link>,
        nodes: HashMap<u32, (pw::node::NodeListener, pw::node::Node)>,
    }
    impl Graph {
        fn link(&mut self, native: &pw::core::CoreRc) {
            if let Some((source, serial)) = self.source {
                let input = self
                    .ports
                    .iter()
                    .find(|(_, node, direction)| {
                        direction == "in" && self.consumers.get(node) == Some(&serial.to_string())
                    })
                    .cloned();
                let output = self
                    .ports
                    .iter()
                    .find(|(_, node, direction)| *node == source && direction == "out")
                    .cloned();
                if let (Some((input, node, _)), Some((output, _, _))) = (input, output) {
                    if !self.links.contains_key(&node) {
                        let link=native.create_object::<pw::link::Link>("link-factory",&properties!{"link.output.node"=>source.to_string(),"link.output.port"=>output.to_string(),"link.input.node"=>node.to_string(),"link.input.port"=>input.to_string()}).unwrap();
                        self.links.insert(node, link);
                        std::fs::write(self.root.join("consumer"), node.to_string()).unwrap();
                    }
                }
            }
        }
    }

    let graph = Rc::new(RefCell::new(Graph {
        root: root.to_owned(),
        source: None,
        consumers: HashMap::new(),
        ports: Vec::new(),
        links: HashMap::new(),
        nodes: HashMap::new(),
    }));
    let events = graph.clone();
    let removed = graph;
    let native = core.clone();
    let marker = root.to_owned();
    let registry = core.get_registry_rc()?;
    let access = registry.downgrade();
    let _registry = registry
        .add_listener_local()
        .global(move |object| {
            let Some(props) = object.props.as_ref() else {
                return;
            };
            let mut graph = events.borrow_mut();
            if matches!(
                object.type_,
                pw::types::ObjectType::Node | pw::types::ObjectType::Port
            ) {
                let mut log = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(marker.join("graph.log"))
                    .unwrap();
                writeln!(
                    log,
                    "{:?} id={} name={:?} target={:?} node={:?} direction={:?}",
                    object.type_,
                    object.id,
                    props.get("node.name"),
                    props.get("target.object"),
                    props.get("node.id"),
                    props.get("port.direction")
                )
                .unwrap();
            }
            if object.type_ == pw::types::ObjectType::Node {
                if props.get("node.name") == Some("screenshots-fixture-source") {
                    let serial = props.get("object.serial").unwrap().parse::<u64>().unwrap();
                    graph.source = Some((object.id, serial));
                    std::fs::write(
                        marker.join("node.json"),
                        serde_json::json!({"node":object.id,"serial":serial}).to_string(),
                    )
                    .unwrap();
                } else if props.get("node.name") == Some("screenshots-one-frame") {
                    let id = object.id;
                    let node: pw::node::Node = access.upgrade().unwrap().bind(object).unwrap();
                    let observed = Rc::downgrade(&events);
                    let core = native.clone();
                    let listener = node
                        .add_listener_local()
                        .info(move |info| {
                            if let (Some(graph), Some(props)) = (observed.upgrade(), info.props()) {
                                if let Some(target) = props.get("target.object") {
                                    let mut graph = graph.borrow_mut();
                                    graph.consumers.insert(id, target.into());
                                    graph.link(&core);
                                }
                            }
                        })
                        .register();
                    graph.nodes.insert(id, (listener, node));
                }
            } else if object.type_ == pw::types::ObjectType::Port {
                if let (Some(node), Some(direction)) =
                    (props.get("node.id"), props.get("port.direction"))
                {
                    graph
                        .ports
                        .push((object.id, node.parse().unwrap(), direction.into()));
                }
            }
            graph.link(&native);
        })
        .global_remove(move |id| {
            let mut graph = removed.borrow_mut();
            graph.ports.retain(|(port, _, _)| *port != id);
            graph.consumers.remove(&id);
            graph.links.remove(&id);
            graph.nodes.remove(&id);
        })
        .register();
    let drive = stream;
    let detached = std::cell::Cell::new(false);
    let control = root.to_owned();
    let timer = main.loop_().add_timer(move |_| {
        if !detached.get() && control.join("disconnect").exists() {
            detached.set(true);
            drive.disconnect().unwrap();
        }
        if detached.get() {
            return;
        }
        unsafe {
            pw::sys::pw_stream_trigger_process(drive.as_raw_ptr());
        }
    });
    timer
        .update_timer(
            Some(Duration::from_millis(10)),
            Some(Duration::from_millis(33)),
        )
        .into_result()?;
    main.run();
    Ok(())
}
