#![cfg(target_os = "linux")]
//! Generated pixels on private D-Bus/PipeWire only; no ambient session/permissions.
use anyhow::{Context, Result, ensure};
#[path = "../src/capture_portal_linux.rs"]
mod finite_adapter;
#[path = "../src/capture_portal_formats.rs"]
mod negotiated_formats;
#[path = "fixtures/region_analysis_portal.rs"]
mod region_analysis_fixture;

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
            let queued = matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("initial-queued.json" | "old-queued.json" | "post-queued.json")
            );
            let generation_matches = !queued
                || path
                    .parent()
                    .and_then(|root| std::fs::read_to_string(root.join("freshness-mode")).ok())
                    .is_none_or(|expected| {
                        serde_json::from_str::<serde_json::Value>(&text)
                            .ok()
                            .is_some_and(|value| {
                                value["generation"].as_str() == Some(expected.as_str())
                            })
                    });
            if !text.is_empty() && generation_matches {
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
    authorized_remotes: usize,
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
                    if mode != "invalid-fd" {
                        seen.lock().unwrap().authorized_remotes += 1;
                    }
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

fn negotiation_mode() -> Result<Option<String>> {
    let Ok(mode) = std::env::var("SCREENSHOTS_PORTAL_NEGOTIATION") else {
        return Ok(None);
    };
    ensure!(
        matches!(
            mode.as_str(),
            "untagged-dual" | "untagged-float" | "tagged-dual"
        ),
        "Unknown private format negotiation case"
    );
    Ok(Some(mode))
}
fn fixture_bpp(video: pipewire::spa::param::video::VideoFormat) -> u32 {
    match video {
        pipewire::spa::param::video::VideoFormat::RGBA_F16 => 8,
        pipewire::spa::param::video::VideoFormat::RGBA_F32 => 16,
        _ => 4,
    }
}
fn negotiation_case(
    root: &Path,
    address: &str,
    portal: &Portal,
    mode: &str,
    color: Option<&serde_json::Value>,
) -> Result<()> {
    use pipewire::spa;
    if mode == "tagged-dual" {
        color_case(
            root,
            address,
            portal,
            color.context("Missing private HDR control")?,
        )?;
    } else {
        *portal.mode.lock().unwrap() = "v6".into();
        let output = root.join("negotiated.png");
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        private(&mut command, root, address);
        command
            .args(["capture-portal", "display"])
            .arg(&output)
            .args(["--cursor", "exclude"]);
        let result = run(&mut command, Duration::from_secs(25))?;
        if mode == "untagged-float" {
            ensure!(
                !result.status.success() && !output.exists(),
                "Unproved float-only source published pixels"
            );
        } else {
            ensure!(
                result.status.success(),
                "Dual-format SDR fallback failed: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            let pixels = image::open(&output)?.into_rgba8();
            ensure!(
                pixels.dimensions() == (4, 3),
                "Dual-format fallback changed the exact raster"
            );
            for (x, y, pixel) in pixels.enumerate_pixels() {
                ensure!(
                    pixel.0 == expected(x, y),
                    "Dual-format fallback changed legacy bytes/alpha"
                );
            }
        }
    }
    if mode != "untagged-float" {
        let observed: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("negotiated-format.json"))?)?;
        if mode == "untagged-dual" {
            ensure!(
                observed["format"] == spa::param::video::VideoFormat::RGBA.as_raw(),
                "Legacy alternative was not negotiated"
            );
            ensure!(
                observed["color"] == serde_json::json!([0, 0, 0, 0]),
                "Legacy fallback invented color proof"
            );
        } else {
            ensure!(
                observed["format"] == spa::param::video::VideoFormat::RGBA_F16.as_raw(),
                "Fully tagged HDR was replaced by unconditional RGB8 preference"
            );
            ensure!(
                observed["color"]
                    == serde_json::json!([
                        spa::sys::SPA_VIDEO_COLOR_RANGE_0_255,
                        spa::sys::SPA_VIDEO_COLOR_MATRIX_RGB,
                        spa::sys::SPA_VIDEO_COLOR_PRIMARIES_BT709,
                        spa::sys::SPA_VIDEO_TRANSFER_GAMMA10
                    ]),
                "Native managed negotiation changed source color tags"
            );
        }
    }
    ensure!(
        portal.observed.lock().unwrap().sessions.is_empty(),
        "Negotiation case retained permission session"
    );
    Ok(())
}
fn color_fixture() -> Result<Option<serde_json::Value>> {
    let name = match std::env::var("SCREENSHOTS_PORTAL_COLOR") {
        Ok(name) => name,
        Err(_) if negotiation_mode()?.as_deref() == Some("tagged-dual") => "portal-linear16".into(),
        Err(_) => return Ok(None),
    };
    let cases: Vec<serde_json::Value> = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/app-core/tests/fixtures/color/native-rgb-oracle.json"
    )))?;
    ensure!(
        name.starts_with("portal-"),
        "Native portal color fixtures require an opaque transport case"
    );
    Ok(Some(
        cases
            .into_iter()
            .find(|case| case["name"] == name)
            .context("Unknown private native color fixture")?,
    ))
}
fn color_case(root: &Path, address: &str, portal: &Portal, case: &serde_json::Value) -> Result<()> {
    *portal.mode.lock().unwrap() = "v6".into();
    let expected: Vec<u8> = serde_json::from_value(case["rgba"].clone())?;
    let output = root.join("native-color.png");
    let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
    private(&mut command, root, address);
    command
        .args(["capture-portal", "display"])
        .arg(&output)
        .args(["--cursor", "exclude"]);
    let result = run(&mut command, Duration::from_secs(25))?;
    if std::env::var("SCREENSHOTS_PORTAL_COLOR_OMIT").is_ok() {
        ensure!(
            !result.status.success() && !output.exists(),
            "Missing native color proof published pixels"
        );
        ensure!(
            portal.observed.lock().unwrap().sessions.is_empty(),
            "Rejected native color source retained permission session"
        );
        return Ok(());
    }
    ensure!(
        result.status.success(),
        "Private negotiated native color capture failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let image = image::open(&output)?.into_rgba8();
    ensure!(
        image.dimensions() == (4, 3)
            && image
                .as_raw()
                .iter()
                .zip(&expected)
                .all(|(a, b)| a.abs_diff(*b) <= 1),
        "Native format/transfer/primaries pixels disagree with independent oracle"
    );
    ensure!(
        portal.observed.lock().unwrap().sessions.is_empty(),
        "Native color still retained permission session"
    );
    let video = root.join("native-color.mkv");
    let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
    private(&mut command, root, address);
    command
        .args(["record", "portal-display"])
        .arg(&video)
        .args([
            "--seconds",
            "1",
            "--fps",
            "3",
            "--width",
            "4",
            "--height",
            "3",
            "--cursor",
            "hide",
        ]);
    let result = run(&mut command, Duration::from_secs(20))?;
    ensure!(
        result.status.success(),
        "Native color finite recording failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut decode = Command::new("ffmpeg");
    private(&mut decode, root, address);
    decode.args(["-v", "error", "-i"]).arg(&video).args([
        "-map",
        "0:v:0",
        "-frames:v",
        "1",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "pipe:1",
    ]);
    let result = run(&mut decode, Duration::from_secs(5))?;
    let expected: Vec<u8> = expected
        .chunks_exact(4)
        .flat_map(|p| (0..3).map(move |c| ((u16::from(p[c]) * u16::from(p[3]) + 127) / 255) as u8))
        .collect();
    ensure!(
        result.status.success()
            && result.stdout.len() == expected.len()
            && result
                .stdout
                .iter()
                .zip(expected)
                .all(|(a, b)| a.abs_diff(b) <= 1),
        "Finite native HDR conversion/encoding disagrees with independent oracle"
    );
    ensure!(
        portal.observed.lock().unwrap().sessions.is_empty(),
        "Native color recorder retained permission session"
    );
    Ok(())
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
    let finite = std::env::var("SCREENSHOTS_PORTAL_FINITE").as_deref() == Ok("1");
    let analysis = std::env::var("SCREENSHOTS_PORTAL_ANALYSIS").as_deref() == Ok("1");
    ensure!(!analysis || (!performance && !finite), "Run analysis separately from timing/freshness modes");
    let color = color_fixture()?;
    let negotiation = negotiation_mode()?;
    match negotiation.as_deref() {
        Some("untagged-dual" | "untagged-float") => ensure!(
            color.is_none(),
            "Untagged producer fixture cannot have a tagged color source"
        ),
        Some("tagged-dual") => ensure!(
            color
                .as_ref()
                .is_some_and(|case| case["name"] == "portal-linear16"),
            "Tagged dual-format control requires portal-linear16"
        ),
        _ => {}
    }

    ensure!(
        negotiation.is_none() || (!performance && !finite),
        "Run negotiation scenarios separately from freshness/performance cases"
    );
    ensure!(
        color.is_none() || (!performance && !finite),
        "Run native color fixtures separately from finite/performance fixture modes"
    );
    ensure!(!analysis || (color.is_none() && negotiation.is_none()), "Analysis requires its own generated raster");
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
            if analysis {
                "512"
            } else if performance {
                "3840"
            } else if finite {
                "16"
            } else {
                "4"
            },
        )
        .env(
            "SCREENSHOTS_PORTAL_HEIGHT",
            if analysis {
                "384"
            } else if performance {
                "2160"
            } else if finite {
                "12"
            } else {
                "3"
            },
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
    if analysis {
        return region_analysis_fixture::run_case(&root.0, address, &portal);
    }
    if let Some(mode) = negotiation {
        return negotiation_case(&root.0, address, &portal, &mode, color.as_ref());
    }
    if let Some(color) = color {
        return color_case(&root.0, address, &portal, &color);
    }
    if finite {
        return finite_case(&root.0, address, &portal);
    }
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
    let color = color_fixture()?;
    let negotiation = negotiation_mode()?;
    ensure!(
        negotiation.is_none() || (width, height) == (4, 3),
        "Private negotiation raster must be 4x3"
    );
    let bpp = color.as_ref().map_or(
        if matches!(
            negotiation.as_deref(),
            Some("untagged-dual" | "untagged-float")
        ) {
            8
        } else {
            4
        },
        |case| match case["source"]["packing"].as_str().unwrap() {
            "rgba_f16_le" => 8,
            "rgba_f32_le" => 16,
            _ => 4,
        },
    );
    let stride = width * bpp;
    let payload = color
        .as_ref()
        .map(|case| -> Result<Vec<u8>> {
            let bytes: Vec<u8> = serde_json::from_value(case["bytes"].clone())?;
            let input_stride = case["source"]["stride"].as_u64().unwrap() as usize;
            Ok(bytes
                .chunks(input_stride)
                .take(height as usize)
                .flat_map(|row| row[..stride as usize].iter().copied())
                .collect())
        })
        .transpose()?;
    let payload = if std::env::var("SCREENSHOTS_PORTAL_ANALYSIS").as_deref() == Ok("1") {
        ensure!((width, height) == (512, 384) && payload.is_none(), "Invalid private analysis publisher dimensions");
        Some(region_analysis_fixture::pixels()?.into_raw())
    } else {
        payload
    };

    let (wire, primaries, transfer) = if let Some(case) = &color {
        let source = &case["source"];
        let wire = match source["packing"].as_str().unwrap() {
            "rgba8" => spa::param::video::VideoFormat::RGBA,
            "bgra8" => spa::param::video::VideoFormat::BGRA,
            "rgbx8" => spa::param::video::VideoFormat::RGBx,
            "bgrx8" => spa::param::video::VideoFormat::BGRx,
            "rgba_f16_le" => spa::param::video::VideoFormat::RGBA_F16,
            "rgba_f32_le" => spa::param::video::VideoFormat::RGBA_F32,
            "xrgb210_le" => spa::param::video::VideoFormat::xRGB_210LE,
            "xbgr210_le" => spa::param::video::VideoFormat::xBGR_210LE,
            "rgbx102_le" => spa::param::video::VideoFormat::RGBx_102LE,
            "bgrx102_le" => spa::param::video::VideoFormat::BGRx_102LE,
            "argb210_le" => spa::param::video::VideoFormat::ARGB_210LE,
            "abgr210_le" => spa::param::video::VideoFormat::ABGR_210LE,
            "rgba102_le" => spa::param::video::VideoFormat::RGBA_102LE,
            "bgra102_le" => spa::param::video::VideoFormat::BGRA_102LE,
            _ => unreachable!(),
        };
        let primaries = match source["color"]["primaries"].as_str().unwrap() {
            "bt709" => spa::sys::SPA_VIDEO_COLOR_PRIMARIES_BT709,
            "display_p3" => spa::sys::SPA_VIDEO_COLOR_PRIMARIES_SMPTEEG432,
            "bt2020" => spa::sys::SPA_VIDEO_COLOR_PRIMARIES_BT2020,
            "adobe_rgb" => spa::sys::SPA_VIDEO_COLOR_PRIMARIES_ADOBERGB,
            _ => unreachable!(),
        };
        let transfer = match source["color"]["transfer"].as_str().unwrap() {
            "linear" => spa::sys::SPA_VIDEO_TRANSFER_GAMMA10,
            "srgb" => spa::sys::SPA_VIDEO_TRANSFER_SRGB,
            "bt709" => spa::sys::SPA_VIDEO_TRANSFER_BT709,
            "bt2020_twelve" => spa::sys::SPA_VIDEO_TRANSFER_BT2020_12,
            "gamma22" => spa::sys::SPA_VIDEO_TRANSFER_GAMMA22,
            "adobe_rgb" => spa::sys::SPA_VIDEO_TRANSFER_ADOBERGB,
            "pq" => spa::sys::SPA_VIDEO_TRANSFER_SMPTE2084,
            "hlg" => spa::sys::SPA_VIDEO_TRANSFER_ARIB_STD_B67,
            _ => unreachable!(),
        };
        (wire, primaries, transfer)
    } else if matches!(
        negotiation.as_deref(),
        Some("untagged-dual" | "untagged-float")
    ) {
        (spa::param::video::VideoFormat::RGBA_F16, 0, 0)
    } else {
        (spa::param::video::VideoFormat::RGBA, 0, 0)
    };
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
    let fmt = format.clone();
    let root_path = root.to_owned();
    let negotiated_log = root.join("negotiated-format.json");
    let finite = std::env::var("SCREENSHOTS_PORTAL_FINITE").as_deref() == Ok("1");
    let mut freshness_mode = String::new();
    let mut freshness_frames = 0u32;
    let mut last_pts = 0i64;
    let streaming = Rc::new(std::cell::Cell::new(false));
    let stream_state = streaming.clone();
    let _listener = stream
        .add_local_listener_with_user_data(())
        .state_changed(move |_, _, _, new| {
            stream_state.set(matches!(new, pw::stream::StreamState::Streaming));
        })
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
            let stride=width*fixture_bpp(info.format());
            let extent=stride*height;
            std::fs::write(&negotiated_log,serde_json::json!({"format":info.format().as_raw(),"color":[info.color_range(),info.color_matrix(),info.color_primaries(),info.transfer_function()]}).to_string()).unwrap();
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
            let header = spa::pod::serialize::PodSerializer::serialize(
                std::io::Cursor::new(Vec::new()),
                &spa::pod::Value::Object(spa::pod::Object {
                    type_: spa::utils::SpaTypes::ObjectParamMeta.as_raw(),
                    id: spa::param::ParamType::Meta.as_raw(),
                    properties: vec![
                        spa::pod::Property::new(
                            spa::sys::SPA_PARAM_META_type,
                            spa::pod::Value::Id(spa::utils::Id(spa::sys::SPA_META_Header)),
                        ),
                        spa::pod::Property::new(
                            spa::sys::SPA_PARAM_META_size,
                            spa::pod::Value::Int(
                                std::mem::size_of::<spa::sys::spa_meta_header>() as i32
                            ),
                        ),
                    ],
                }),
            )
            .unwrap()
            .0
            .into_inner();
            stream
                .update_params(&mut [
                    spa::pod::Pod::from_bytes(&bytes).unwrap(),
                    spa::pod::Pod::from_bytes(&header).unwrap(),
                ])
                .unwrap();
        })
        .process(move |stream, _| {
            if root_path.join("stall").exists() && streaming.get()
                && format.borrow().is_some() && root_path.join("analysis-cancel-armed").exists() {
                std::fs::write(root_path.join("analysis-stalled-stream"), b"streaming source withheld its frame").unwrap();
            }
            if root_path.join("stall").exists()
                || (finite
                    && (root_path.join("freshness-child-finished").exists()
                        || root_path.join("freshness-paused").exists()))
            {
                return;
            }
            let mode_request = if finite {
                std::fs::read_to_string(root_path.join("freshness-mode")).unwrap_or_default()
            } else {
                String::new()
            };
            if mode_request != freshness_mode {
                freshness_mode = mode_request.clone();
                freshness_frames = 0;
                last_pts = 0;
            }
            let mode = mode_request.lines().next().unwrap_or_default().to_owned();
            if !mode.is_empty() {
                if freshness_frames == 1 && !root_path.join("ready-entered").is_file() {
                    return;
                }
                if freshness_frames >= 2
                    && (mode == "queued-quiet"
                        || !root_path.join("source-start.json").is_file()
                        || freshness_frames >= 256)
                {
                    return;
                }
            }
            let Some(info)=format.borrow().as_ref().copied() else {return;};
            let stride=width*fixture_bpp(info.format());
            let extent=stride*height;
            // Raw ownership lets this oracle stamp Header metadata and mark only after actual queueing.
            let raw = unsafe { stream.dequeue_raw_buffer() };
            if raw.is_null() {
                return;
            }
            let body = unsafe { &mut *(*raw).buffer };
            assert!(body.n_datas == 1 && !body.datas.is_null());
            let data = unsafe { &mut *body.datas };
            assert!(!data.data.is_null() && !data.chunk.is_null() && data.maxsize >= extent);
            let bytes = unsafe {
                std::slice::from_raw_parts_mut(data.data.cast::<u8>(), data.maxsize as usize)
            };
            let post_start = !mode.is_empty() && freshness_frames >= 2;
            let scroll = if post_start {
                7
            } else if finite {
                std::fs::read_to_string(root_path.join("scroll_rows"))
                    .ok()
                    .and_then(|text| text.parse::<u32>().ok())
                    .unwrap_or(0)
            } else {
                0
            };
            if info.format()==wire && payload.is_some() {
                let payload=payload.as_ref().unwrap();bytes[..payload.len()].copy_from_slice(payload);
            } else if info.format()==spa::param::video::VideoFormat::RGBA_F16 {
                // Genuine untagged FP16 producer data; never reinterpret RGBA8 as floats.
                for pixel in bytes[..extent as usize].chunks_exact_mut(8) {
                    pixel.copy_from_slice(&[0,0x3c,0,0x38,0,0x34,0,0x3c]);
                }
            } else {
                assert_eq!(info.format(),spa::param::video::VideoFormat::RGBA);
                for y in 0..height {for x in 0..width {
                    let at=(y*width+x) as usize*4;bytes[at..at+4].copy_from_slice(&expected(x,y+scroll));
                }}
            }
            let pts = if post_start && mode == "post-old" {
                last_pts + 1
            } else if post_start && mode == "post-unknown" {
                i64::MIN
            } else {
                fixture_monotonic_ns()
            };
            let header = unsafe {
                spa::sys::spa_buffer_find_meta_data(
                    body,
                    spa::sys::SPA_META_Header,
                    std::mem::size_of::<spa::sys::spa_meta_header>(),
                )
                .cast::<spa::sys::spa_meta_header>()
            };
            assert!(
                header.is_null() == false || mode.is_empty(),
                "Private video freshness source did not negotiate Header metadata"
            );
            unsafe {
                if !header.is_null() {
                    (*header).flags = 0;
                    (*header).offset = 0;
                    (*header).pts = pts;
                    (*header).dts_offset = 0;
                    (*header).seq = u64::from(freshness_frames) + 1;
                }
                (*data.chunk).offset = 0;
                (*data.chunk).size = extent;
                (*data.chunk).stride = stride as i32;
                stream.queue_raw_buffer(raw);
            }
            if !mode.is_empty() {
                let marker = match freshness_frames {
                    0 => "initial-queued.json",
                    1 => "old-queued.json",
                    _ => "post-queued.json",
                };
                let marker = root_path.join(marker);
                let temporary = marker.with_extension("new");
                std::fs::write(
                    &temporary,
                    serde_json::json!({"pts":pts,"sequence":freshness_frames + 1,"generation":mode_request}).to_string(),
                )
                .unwrap();
                std::fs::rename(temporary, marker).unwrap();
                freshness_frames += 1;
            }
            last_pts = pts;
        })
        .register()?;
    let mut info = spa::param::video::VideoInfoRaw::new();
    info.set_format(wire);
    info.set_size(spa::utils::Rectangle { width, height });
    info.set_framerate(spa::utils::Fraction { num: 30, denom: 1 });
    let mut object = spa::pod::Object {
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
            spa::pod::property!(spa::param::format::FormatProperties::VideoFormat, Id, wire),
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
    if color.is_some() {
        for (key, value) in [
            (
                spa::param::format::FormatProperties::VideoColorRange.as_raw(),
                spa::sys::SPA_VIDEO_COLOR_RANGE_0_255,
            ),
            (
                spa::param::format::FormatProperties::VideoColorMatrix.as_raw(),
                spa::sys::SPA_VIDEO_COLOR_MATRIX_RGB,
            ),
            (
                spa::param::format::FormatProperties::VideoColorPrimaries.as_raw(),
                primaries,
            ),
            (
                spa::param::format::FormatProperties::VideoTransferFunction.as_raw(),
                transfer,
            ),
        ] {
            let omit = std::env::var("SCREENSHOTS_PORTAL_COLOR_OMIT").ok();
            let omitted_key = match omit.as_deref() {
                Some("range") => {
                    Some(spa::param::format::FormatProperties::VideoColorRange.as_raw())
                }
                Some("matrix") => {
                    Some(spa::param::format::FormatProperties::VideoColorMatrix.as_raw())
                }
                Some("primaries") => {
                    Some(spa::param::format::FormatProperties::VideoColorPrimaries.as_raw())
                }
                Some("transfer") => {
                    Some(spa::param::format::FormatProperties::VideoTransferFunction.as_raw())
                }
                Some(_) => panic!("Unknown private color-proof omission"),
                None => None,
            };
            if omitted_key != Some(key) {
                object.properties.push(spa::pod::Property::new(
                    key,
                    spa::pod::Value::Id(spa::utils::Id(value)),
                ));
            }
        }
    }
    let primary = if negotiation.as_deref() == Some("untagged-dual") {
        dual_source_format_pod()?
    } else {
        spa::pod::serialize::PodSerializer::serialize(
            std::io::Cursor::new(Vec::new()),
            &spa::pod::Value::Object(object),
        )?
        .0
        .into_inner()
    };
    let mut offers = vec![primary];
    if negotiation.as_deref() == Some("tagged-dual") {
        offers.push(source_format_pod(
            spa::param::video::VideoFormat::RGBA,
            None,
        )?);
    }
    let mut params = offers
        .iter()
        .map(|bytes| spa::pod::Pod::from_bytes(bytes).unwrap())
        .collect::<Vec<_>>();
    stream.connect(
        spa::utils::Direction::Output,
        None,
        pw::stream::StreamFlags::DRIVER | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut params,
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

/// This exercises native transport and installed codecs on owned services, not a real compositor or permission dialog.
fn finite_case(root: &Path, address: &str, portal: &Portal) -> Result<()> {
    assert_finite_preflight(root, address, Some(portal))?;
    freshness_cases(root, address, portal)?;
    for (mode, extension, audio) in [
        ("portal-display", "mkv", true),
        ("portal-window", "gif", false),
    ] {
        *portal.mode.lock().unwrap() = if mode == "portal-window" {
            "v6-window"
        } else {
            "v6"
        }
        .into();
        let output = root.join(format!("finite.{extension}"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        private(&mut command, root, address);
        let initial_calls = portal.observed.lock().unwrap().calls.len();
        command.args(["record", mode]).arg(&output).args([
            "--seconds",
            "1",
            "--fps",
            "5",
            "--width",
            "16",
            "--height",
            "12",
            "--cursor",
            "hide",
        ]);
        if audio {
            command.args(["--audio-mode", "test", "--audio-channels", "1"]);
        }
        let result = run(&mut command, Duration::from_secs(15))?;
        ensure!(
            result.status.success(),
            "Finite portal recording failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let calls = portal.observed.lock().unwrap().calls[initial_calls..].to_vec();
        ensure!(
            [
                "CreateSession",
                "SelectSources",
                "Start",
                "OpenPipeWireRemote"
            ]
            .iter()
            .all(|method| calls.iter().any(|call| call == method)),
            "Valid recording did not reach the owned portal control"
        );
        let report: serde_json::Value = serde_json::from_slice(&result.stdout)?;
        ensure!(
            report["frames"] == 5
                && report["source"]
                    == if audio {
                        "portal:display"
                    } else {
                        "portal:window"
                    },
            "Missing truthful finite portal summary"
        );
        ensure!(
            portal.observed.lock().unwrap().sessions.is_empty(),
            "Finite recorder retained portal session"
        );
        if audio {
            let mut decode = Command::new("ffmpeg");
            private(&mut decode, root, address);
            decode.args(["-v", "error", "-i"]).arg(&output).args([
                "-map",
                "0:v:0",
                "-frames:v",
                "1",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgb24",
                "pipe:1",
            ]);
            let pixels = run(&mut decode, Duration::from_secs(5))?;
            ensure!(
                pixels.status.success() && pixels.stdout.len() == 16 * 12 * 3,
                "Finite portal decoder failed"
            );
            for y in 0..12u32 {
                for x in 0..16u32 {
                    let pixel = expected(x, y);
                    let expected = std::array::from_fn::<_, 3, _>(|channel| {
                        ((u16::from(pixel[channel]) * u16::from(pixel[3]) + 127) / 255) as u8
                    });
                    let offset = ((y * 16 + x) * 3) as usize;
                    ensure!(
                        pixels.stdout[offset..offset + 3] == expected,
                        "Finite recorder encoded the wrong restricted source pixels"
                    );
                }
            }
        }
    }
    *portal.mode.lock().unwrap() = "v6-window".into();
    let observed = portal.observed.clone();
    let previous = observed.lock().unwrap().calls.len();
    let rows = root.join("scroll_rows");
    let driver = std::thread::spawn(move || -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !observed.lock().unwrap().calls[previous..]
            .iter()
            .any(|call| call == "OpenPipeWireRemote")
        {
            ensure!(Instant::now() < deadline, "Manual source did not start");
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(250));
        std::fs::write(&rows, "3")?;
        std::thread::sleep(Duration::from_millis(500));
        std::fs::write(&rows, "6")?;
        Ok(())
    });
    let output = root.join("manual.png");
    let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
    private(&mut command, root, address);
    command
        .args(["scroll-capture-portal", "window"])
        .arg(&output)
        .args([
            "--width",
            "16",
            "--height",
            "12",
            "--frames",
            "3",
            "--interval-ms",
            "500",
            "--minimum-overlap",
            "4",
        ]);
    let result = run(&mut command, Duration::from_secs(10))?;
    driver
        .join()
        .map_err(|_| anyhow::anyhow!("Private manual-source driver failed"))??;
    ensure!(
        result.status.success(),
        "Manual portal scrolling failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout)?;
    ensure!(
        report["scroll_actions"] == 0
            && report["complete"] == false
            && report["vertical_advances"] == serde_json::json!([3, 3]),
        "Manual source claimed input or wrong seams"
    );
    let pixels = image::open(&output)?.into_rgba8();
    ensure!(
        pixels.dimensions() == (16, 18),
        "Manual source returned wrong raster"
    );
    for (x, y, pixel) in pixels.enumerate_pixels() {
        ensure!(
            pixel.0 == expected(x, y),
            "Manual source stitched wrong pixels"
        );
    }
    ensure!(
        portal.observed.lock().unwrap().sessions.is_empty(),
        "Manual source retained portal session"
    );
    let profile = root.join("manual-task.json");
    let task_output = root.join("manual-task.png");
    std::fs::write(
        &profile,
        serde_json::to_vec(
            &serde_json::json!({"version":1,"source":{"type":"portal_scroll","request":{"stream":{"source":"window","cursor":"exclude","width":16,"height":12},"frames":2,"interval_ms":100,"minimum_overlap":4}},"actions":[{"type":"save","path":task_output}]}),
        )?,
    )?;
    let mut task = Command::new(env!("CARGO_BIN_EXE_screenshots"));
    private(&mut task, root, address);
    task.args(["task", "run"]).arg(profile);
    let result = run(&mut task, Duration::from_secs(10))?;
    ensure!(
        result.status.success() && task_output.is_file(),
        "Task did not reach manual portal adapter: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    for mode in ["denied", "wrong-serial", "closed", "close-error"] {
        *portal.mode.lock().unwrap() = mode.into();
        let output = root.join(format!("failed-{mode}.mkv"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        private(&mut command, root, address);
        command
            .args(["record", "portal-display"])
            .arg(&output)
            .args([
                "--seconds",
                "1",
                "--fps",
                "5",
                "--width",
                "16",
                "--height",
                "12",
            ]);
        let result = run(&mut command, Duration::from_secs(10))?;
        ensure!(
            !result.status.success() && !output.exists(),
            "Portal source failure published a recording"
        );
        ensure!(
            portal.observed.lock().unwrap().sessions.is_empty(),
            "Failed source retained portal session"
        );
    }
    ensure!(
        !std::fs::read_dir(root)?.any(|entry| entry.is_ok_and(|entry| entry
            .file_name()
            .to_string_lossy()
            .starts_with(".screenshots-record-"))),
        "Finite source retained private staging"
    );
    eprintln!(
        "Private portal/PipeWire finite MKV+synthetic-audio/GIF/manual-scroll/Task and failure fixtures passed; real compositor acceptance remains open"
    );
    Ok(())
}

fn invalid_finite_inputs() -> Vec<(Vec<&'static str>, &'static str)> {
    vec![
        (
            vec![
                "record",
                "portal-window",
                "out.mkv",
                "--seconds",
                "61",
                "--fps",
                "5",
                "--width",
                "16",
                "--height",
                "12",
            ],
            "Recording seconds must be 1–60",
        ),
        (
            vec![
                "record",
                "portal-window",
                "out.mkv",
                "--seconds",
                "1",
                "--fps",
                "5",
                "--width",
                "16",
                "--height",
                "12",
                "--x",
                "0",
                "--y",
                "0",
            ],
            "Offsets apply only to desktop recording",
        ),
        (
            vec![
                "record",
                "portal-window",
                "out.gif",
                "--seconds",
                "1",
                "--fps",
                "5",
                "--width",
                "16",
                "--height",
                "12",
                "--audio-mode",
                "test",
            ],
            "Audio recording requires Matroska output; GIF has no audio",
        ),
        (
            vec![
                "scroll-capture-portal",
                "window",
                "out.png",
                "--width",
                "16",
                "--height",
                "12",
                "--frames",
                "129",
                "--interval-ms",
                "5000",
            ],
            "Manual portal scrolling preflight rejected invalid limits",
        ),
        (
            vec![
                "scroll-capture-portal",
                "window",
                "out.png",
                "--width",
                "16",
                "--height",
                "12",
                "--frames",
                "3",
                "--interval-ms",
                "500",
                "--minimum-overlap",
                "12",
            ],
            "Manual portal scrolling preflight rejected invalid limits",
        ),
    ]
}
fn assert_finite_preflight(root: &Path, address: &str, portal: Option<&Portal>) -> Result<()> {
    for (args, diagnostic) in invalid_finite_inputs() {
        let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
        private(&mut command, root, address);
        let before = portal.map(|portal| portal.observed.lock().unwrap().calls.len());
        command.current_dir(&root).args(args);
        let result = run(&mut command, Duration::from_secs(3))?;
        ensure!(
            !result.status.success(),
            "Invalid finite request was accepted"
        );
        let error = String::from_utf8_lossy(&result.stderr);
        ensure!(
            error.contains(diagnostic),
            "Wrong preflight failure; expected {diagnostic:?}, received {error:?}"
        );
        if let Some(before) = before {
            ensure!(
                portal.unwrap().observed.lock().unwrap().calls.len() == before,
                "Invalid finite preflight initialized the owned portal"
            );
        }
        ensure!(
            !root.join("out.mkv").exists()
                && !root.join("out.gif").exists()
                && !root.join("out.png").exists(),
            "Invalid finite request published output"
        );
    }
    Ok(())
}
#[test]
fn finite_portal_cli_rejects_invalid_bounds_before_opening_any_session() -> Result<()> {
    let root = Folder::new();
    assert_finite_preflight(
        &root.0,
        "unix:path=/missing-private-finite-portal-bus",
        None,
    )
}

fn fixture_monotonic_ns() -> i64 {
    let mut time: libc::timespec = unsafe { std::mem::zeroed() };
    assert_eq!(
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) },
        0
    );
    time.tv_sec as i64 * 1_000_000_000 + time.tv_nsec as i64
}
fn prepare_freshness_case(root: &Path, mode: &str) -> Result<()> {
    std::fs::write(
        root.join("freshness-paused"),
        b"changing private source case",
    )?;
    for marker in [
        "initial-queued.json",
        "old-queued.json",
        "post-queued.json",
        "ready-entered",
        "source-start.json",
        "freshness-result.json",
        "freshness-child-finished",
        "encoder-started",
    ] {
        match std::fs::remove_file(root.join(marker)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    // Consecutive identical modes are distinct producer incarnations.
    std::fs::write(
        root.join("freshness-mode"),
        format!("{mode}\n{}", uuid::Uuid::new_v4()),
    )?;
    std::fs::remove_file(root.join("freshness-paused"))?;
    Ok(())
}
fn freshness_cases(root: &Path, address: &str, portal: &Portal) -> Result<()> {
    *portal.mode.lock().unwrap() = "v6".into();
    cli_queued_freshness_case(root, address, portal)?;
    for mode in ["queued-quiet", "post-old", "post-unknown", "post-new"] {
        prepare_freshness_case(root, mode)?;
        let mut command = Command::new(std::env::current_exe()?);
        private(&mut command, root, address);
        command
            .args([
                "--exact",
                "portal_video_freshness_fixture_child",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("SCREENSHOTS_PORTAL_FRESHNESS_CHILD", root)
            .env("SCREENSHOTS_PORTAL_FRESHNESS_MODE", mode);
        let result = run(&mut command, Duration::from_secs(15))?;
        ensure!(
            result.status.success(),
            "Native PTS freshness case {mode} failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let initial: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("initial-queued.json"))?)?;
        let old: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("old-queued.json"))?)?;
        let start: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("source-start.json"))?)?;
        let generation = std::fs::read_to_string(root.join("freshness-mode"))?;
        ensure!(
            initial["generation"].as_str() == Some(generation.as_str())
                && old["generation"].as_str() == Some(generation.as_str()),
            "Source markers belong to an earlier case"
        );
        ensure!(
            initial["pts"].as_i64().unwrap() < old["pts"].as_i64().unwrap()
                && old["pts"].as_i64().unwrap() < start["cutoff_ns"].as_i64().unwrap(),
            "Old source buffers did not precede the explicit source start boundary"
        );
        if mode == "queued-quiet" {
            ensure!(
                !root.join("post-queued.json").exists(),
                "Quiet source unexpectedly produced a post-start buffer"
            );
        } else {
            let post: serde_json::Value =
                serde_json::from_slice(&std::fs::read(root.join("post-queued.json"))?)?;
            let pts = post["pts"].as_i64().unwrap();
            let boundary = start["cutoff_ns"].as_i64().unwrap();
            ensure!(
                post["sequence"]
                    .as_u64()
                    .is_some_and(|sequence| sequence >= 3 && sequence <= 256),
                "Post-start source buffer was not actually queued"
            );
            match mode {
                "post-old" => ensure!(
                    pts > old["pts"].as_i64().unwrap() && pts <= boundary,
                    "Old replay was not monotonic but pre-boundary"
                ),
                "post-unknown" => ensure!(
                    pts == i64::MIN,
                    "Unknown-PTS control did not reach the producer"
                ),
                "post-new" => ensure!(
                    pts > boundary,
                    "Fresh control did not follow the source boundary"
                ),
                _ => unreachable!(),
            }
        }
        let report: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("freshness-result.json"))?)?;
        ensure!(
            report["success"] == (mode == "post-new"),
            "Freshness oracle accepted stale or unknown source PTS"
        );
        ensure!(
            portal.observed.lock().unwrap().sessions.is_empty(),
            "Freshness case retained an owned portal session"
        );
    }
    std::fs::remove_file(root.join("freshness-mode"))?;
    std::fs::remove_file(root.join("freshness-child-finished"))?;
    Ok(())
}

/// Calls the real native adapter on the owned bus/PipeWire. The callback supplies the same
/// post-handshake VideoStart proof as the worker; it is not a model or fake captured frame.
#[test]
#[ignore = "Owned private portal/PipeWire fixture subprocess only"]
fn portal_video_freshness_fixture_child() -> Result<()> {
    let root = PathBuf::from(
        std::env::var_os("SCREENSHOTS_PORTAL_FRESHNESS_CHILD")
            .ok_or_else(|| anyhow::anyhow!("Missing private freshness root"))?,
    );
    ensure!(
        std::env::var_os("PIPEWIRE_RUNTIME_DIR").as_deref() == Some(root.as_os_str())
            && root.join("private.conf").is_file(),
        "Freshness child requires its owned native server"
    );
    let mode = std::env::var("SCREENSHOTS_PORTAL_FRESHNESS_MODE")?;
    let (sink, mut reader) = std::os::unix::net::UnixStream::pair()?;
    sink.set_nonblocking(true)?;
    reader.set_read_timeout(Some(Duration::from_secs(12)))?;
    let receiver = std::thread::spawn(move || -> Result<Vec<u8>> {
        use std::io::Read;
        let mut bytes = Vec::new();
        reader.take(16 * 12 * 4 * 5 + 1).read_to_end(&mut bytes)?;
        Ok(bytes)
    });
    let request = app_core::PortalVideoRequest {
        stream: app_core::PortalStreamSource {
            source: app_core::PortalCaptureSource::Display,
            cursor: app_core::PortalCaptureCursor::Exclude,
            width: 16,
            height: 12,
        },
        seconds: 1,
        fps: 5,
    };
    let result = finite_adapter::video(
        request,
        Instant::now() + Duration::from_secs(10),
        sink,
        &mut || {
            std::fs::write(
                root.join("ready-entered"),
                b"encoder-start handshake fixture",
            )
            .map_err(|_| app_core::CaptureError::NativeCanceled)?;
            let old = wait_file(
                &root.join("old-queued.json"),
                Instant::now() + Duration::from_secs(3),
            )
            .map_err(|_| app_core::CaptureError::NativeTimeout)?;
            let old: serde_json::Value =
                serde_json::from_str(&old).map_err(|_| app_core::CaptureError::NativeCanceled)?;
            let start = finite_adapter::VideoStart::after_encoder_start()?;
            if old["pts"]
                .as_i64()
                .is_none_or(|pts| pts >= start.cutoff_ns())
            {
                return Err(app_core::CaptureError::NativeCanceled);
            }
            std::fs::write(
                root.join("source-start.json"),
                serde_json::json!({"cutoff_ns":start.cutoff_ns()}).to_string(),
            )
            .map_err(|_| app_core::CaptureError::NativeCanceled)?;
            Ok(start)
        },
    );
    std::fs::write(
        root.join("freshness-child-finished"),
        b"native source finished",
    )?;
    let bytes = receiver
        .join()
        .map_err(|_| anyhow::anyhow!("Freshness raw receiver failed"))??;
    if mode == "post-new" {
        let summary = result?;
        summary.validate(request)?;
        ensure!(
            bytes.len() == 16 * 12 * 4 * 5,
            "Fresh source did not deliver the finite output count"
        );
        for frame in bytes.chunks_exact(16 * 12 * 4) {
            for y in 0..12u32 {
                for x in 0..16u32 {
                    let pixel = expected(x, y + 7);
                    let expected = [
                        ((u16::from(pixel[2]) * u16::from(pixel[3]) + 127) / 255) as u8,
                        ((u16::from(pixel[1]) * u16::from(pixel[3]) + 127) / 255) as u8,
                        ((u16::from(pixel[0]) * u16::from(pixel[3]) + 127) / 255) as u8,
                        255,
                    ];
                    let offset = ((y * 16 + x) * 4) as usize;
                    ensure!(
                        frame[offset..offset + 4] == expected,
                        "Old queued pixels reached the fresh source output"
                    );
                }
            }
        }
        let post: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("post-queued.json"))?)?;
        let start: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.join("source-start.json"))?)?;
        ensure!(
            post["pts"].as_i64().unwrap() > start["cutoff_ns"].as_i64().unwrap(),
            "Successful source PTS did not postdate its boundary"
        );
    } else {
        match (&result, mode.as_str()) {
            (Err(app_core::CaptureError::NativeTimeout), "queued-quiet" | "post-old") => {}
            (Err(app_core::CaptureError::Unsupported(_)), "post-unknown") => {}
            _ => anyhow::bail!(
                "Freshness failure had the wrong native result: {:?}",
                result
            ),
        }
        ensure!(
            bytes.is_empty(),
            "Unproved source pixels escaped before the freshness failure"
        );
    }
    std::fs::write(root.join("freshness-result.json"), serde_json::json!({"success":mode == "post-new", "raw_output_bytes":bytes.len(), "native_adapter":true, "mock_consent":true, "model_fixture":false}).to_string())?;
    Ok(())
}

/// The real CLI/owned worker must send VIDEO_START only after this encoder gate.
/// The publisher has already queued its old buffer, then remains quiet forever.
fn cli_queued_freshness_case(root: &Path, address: &str, portal: &Portal) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let search = std::env::var_os("PATH").unwrap_or_default();
    let engine = std::env::split_paths(&search)
        .map(|path| path.join("ffmpeg"))
        .find(|path| {
            path.metadata()
                .is_ok_and(|info| info.is_file() && info.permissions().mode() & 0o111 != 0)
        })
        .ok_or_else(|| anyhow::anyhow!("Private codec fixture requires installed FFmpeg"))?;
    let engine = std::path::absolute(engine)?;
    let wrappers = root.join("freshness-engines");
    std::fs::create_dir(&wrappers)?;
    let wrapper = wrappers.join("ffmpeg");
    std::fs::write(
        &wrapper,
        r#"#!/bin/sh
if [ "$2" = "-h" ]; then
    : > "$SCREENSHOTS_PRIVATE_SOURCE/ready-entered"
    attempt=0
    while ! python3 - "$SCREENSHOTS_PRIVATE_SOURCE" <<'SOURCE_GENERATION'
import json,sys
from pathlib import Path
root=Path(sys.argv[1])
try:
    expected=(root/'freshness-mode').read_text()
    observed=json.loads((root/'old-queued.json').read_text())
    sys.exit(0 if observed.get('generation') == expected else 1)
except (OSError,ValueError):
    sys.exit(1)
SOURCE_GENERATION
    do
        attempt=$((attempt + 1))
        [ "$attempt" -le 500 ] || exit 71
        sleep 0.01
    done
elif [ "$2" = "-loglevel" ]; then
    : > "$SCREENSHOTS_PRIVATE_SOURCE/encoder-started"
fi
exec "$SCREENSHOTS_PRIVATE_FFMPEG" "$@"
"#,
    )?;
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700))?;
    prepare_freshness_case(root, "queued-quiet")?;
    let output = root.join("queued-worker-must-not-publish.mkv");
    let mut command = Command::new(env!("CARGO_BIN_EXE_screenshots"));
    private(&mut command, root, address);
    let path =
        std::env::join_paths(std::iter::once(wrappers).chain(std::env::split_paths(&search)))?;
    command
        .env("PATH", path)
        .env("SCREENSHOTS_PRIVATE_FFMPEG", engine)
        .env("SCREENSHOTS_PRIVATE_SOURCE", root)
        .args(["record", "portal-display"])
        .arg(&output)
        .args([
            "--seconds",
            "1",
            "--fps",
            "5",
            "--width",
            "16",
            "--height",
            "12",
        ]);
    let started = Instant::now();
    let result = run(&mut command, Duration::from_secs(15))?;
    std::fs::write(
        root.join("freshness-child-finished"),
        b"native worker finished",
    )?;
    let error = String::from_utf8_lossy(&result.stderr);
    ensure!(
        !result.status.success()
            && (error.contains("Native capture deadline exceeded")
                || error.contains("Native capture canceled"))
            && !output.exists(),
        "Actual worker accepted the queued pre-VIDEO_START source or failed for the wrong reason: {error}"
    );
    ensure!(
        started.elapsed() >= Duration::from_secs(4)
            && started.elapsed() < Duration::from_secs(12)
            && root.join("initial-queued.json").is_file()
            && root.join("old-queued.json").is_file()
            && root.join("encoder-started").is_file()
            && !root.join("post-queued.json").exists(),
        "Actual worker freshness gate did not exercise initial + queued-old + post-start quiet source"
    );
    ensure!(
        portal.observed.lock().unwrap().sessions.is_empty(),
        "Queued-worker freshness case retained portal session"
    );
    Ok(())
}

include!("fixtures/portal-negotiation-tests.rs");
