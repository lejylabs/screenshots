#![cfg(target_os = "linux")]
//! Real AccessKit/zbus services, independently observed on two private D-Bus daemons.
use accesskit::{Action, Node, NodeId, Role, Tree, TreeId, TreeUpdate};
use atspi_common::{Interface, InterfaceSet, ObjectRef, ObjectRefOwned, State, StateSet};
use dbus::{
    Message, Path,
    arg::{PropMap, Variant},
    blocking::{Connection, stdintf::org_freedesktop_dbus::Properties},
    channel::{Channel, MatchingReceiver, Sender},
    message::MatchRule,
};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use zbus::wire::{BE, LE, Type, serialized::Context, to_bytes};

const WAIT: Duration = Duration::from_secs(3);
const ROOT: &str = "/org/a11y/atspi/accessible/root";
const BUS_OBJECT: &str = "/org/a11y/bus";
const CHILD_ENV: &str = "SCREENSHOTS_ACCESSKIT_PRIVATE_CHILD";

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
        use std::os::unix::fs::DirBuilderExt;
        let folder = Self(
            std::env::temp_dir().join(format!("screenshots-accesskit-{}", uuid::Uuid::new_v4())),
        );
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&folder.0)
            .unwrap();
        folder
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn daemon(folder: &Folder) -> (ChildGuard, String) {
    let mut child = ChildGuard(
        Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .arg(format!("--address=unix:tmpdir={}", folder.0.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::sync_channel(1);
    let reader = std::thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(result.map(|_| line.trim().to_owned()));
    });
    let address = rx
        .recv_timeout(WAIT)
        .expect("private daemon startup deadline")
        .unwrap();
    reader.join().unwrap();
    assert!(address.starts_with("unix:"));
    (child, address)
}
fn connect(address: &str) -> Connection {
    let mut channel = Channel::open_private(address).unwrap();
    channel.register().unwrap();
    Connection::from(channel)
}
struct Mock {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Mock {
    fn start(connection: Connection) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                // Only the bounded fixture uses a timer; the production adapter uses native events.
                connection.process(Duration::from_millis(10)).unwrap();
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            if !std::thread::panicking() {
                thread.join().unwrap();
            } else {
                let _ = thread.join();
            }
        }
    }
}
fn session_mock(address: &str, accessibility_address: String) -> Mock {
    let connection = connect(address);
    connection
        .request_name("org.a11y.Bus", false, false, true)
        .unwrap();
    let enabled = Arc::new(AtomicBool::new(true));
    connection.start_receive(
        MatchRule::new_method_call(),
        Box::new(move |m, c| {
            match m.member().unwrap().as_ref() {
                "GetAddress" => {
                    c.send(m.method_return().append1(accessibility_address.clone()))
                        .unwrap();
                }
                "Get" => {
                    let (interface, property): (String, String) = m.read2().unwrap();
                    assert_eq!(interface, "org.a11y.Status");
                    assert!(matches!(
                        property.as_str(),
                        "IsEnabled" | "ScreenReaderEnabled"
                    ));
                    c.send(
                        m.method_return()
                            .append1(Variant(enabled.load(Ordering::Acquire))),
                    )
                    .unwrap();
                }
                "GetAll" => {
                    let interface: String = m.read1().unwrap();
                    assert_eq!(interface, "org.a11y.Status");
                    let mut values = PropMap::new();
                    for name in ["IsEnabled", "ScreenReaderEnabled"] {
                        values.insert(
                            name.into(),
                            Variant(Box::new(enabled.load(Ordering::Acquire))),
                        );
                    }
                    c.send(m.method_return().append1(values)).unwrap();
                }
                "FixtureSetEnabled" => {
                    let value: bool = m.read1().unwrap();
                    enabled.store(value, Ordering::Release);
                    let mut values = PropMap::new();
                    values.insert("IsEnabled".into(), Variant(Box::new(value)));
                    c.send(
                        Message::new_signal(
                            BUS_OBJECT,
                            "org.freedesktop.DBus.Properties",
                            "PropertiesChanged",
                        )
                        .unwrap()
                        .append3(
                            "org.a11y.Status",
                            values,
                            Vec::<String>::new(),
                        ),
                    )
                    .unwrap();
                    c.send(m.method_return()).unwrap();
                }
                member => panic!("unexpected private accessibility bus method {member}"),
            }
            true
        }),
    );
    Mock::start(connection)
}
fn registry_mock(address: &str) -> (Mock, Receiver<(String, Path<'static>)>, String) {
    let connection = connect(address);
    connection
        .request_name("org.a11y.atspi.Registry", false, false, true)
        .unwrap();
    let registry_name = connection.unique_name().to_string();
    let desktop = (registry_name.clone(), Path::new(ROOT).unwrap());
    let (tx, rx) = mpsc::sync_channel(2);
    connection.start_receive(
        MatchRule::new_method_call(),
        Box::new(move |m, c| {
            assert_eq!(&*m.interface().unwrap(), "org.a11y.atspi.Socket");
            assert_eq!(&*m.member().unwrap(), "Embed");
            let plug: (String, Path<'static>) = m.read1().unwrap();
            assert_eq!(plug.0, m.sender().unwrap().to_string());
            assert_eq!(&*plug.1, ROOT);
            tx.try_send(plug).unwrap();
            c.send(m.method_return().append1(desktop.clone())).unwrap();
            true
        }),
    );
    (Mock::start(connection), rx, registry_name)
}

fn button(label: &str) -> Node {
    let mut node = Node::new(Role::Button);
    node.set_label(label);
    node.add_action(Action::Click);
    node.add_action(Action::Focus);
    node.set_bounds(accesskit::Rect::new(5.0, 7.0, 45.0, 27.0));
    node
}
fn tree() -> TreeUpdate {
    let mut window = Node::new(Role::Window);
    window.set_label("Private capture window");
    window.set_children([NodeId(2)]);
    let mut tree = Tree::new(NodeId(1));
    tree.toolkit_name = Some("screenshots-private-fixture".into());
    tree.toolkit_version = Some("1".into());
    TreeUpdate {
        nodes: vec![(NodeId(1), window), (NodeId(2), button("Capture"))],
        tree: Some(tree),
        tree_id: TreeId::ROOT,
        focus: NodeId(2),
    }
}
struct Activate(SyncSender<&'static str>);
impl accesskit::ActivationHandler for Activate {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        self.0.try_send("activate").unwrap();
        Some(tree())
    }
}
struct Act(SyncSender<&'static str>);
impl accesskit::ActionHandler for Act {
    fn do_action(&mut self, request: accesskit::ActionRequest) {
        assert_eq!(request.target_node, NodeId(2));
        assert_eq!(request.action, Action::Click);
        self.0.try_send("action").unwrap();
    }
}
struct Deactivate(SyncSender<&'static str>);
impl accesskit::DeactivationHandler for Deactivate {
    fn deactivate_accessibility(&mut self) {
        self.0.try_send("deactivate").unwrap();
    }
}

#[test]
fn private_accesskit_child() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    assert_ne!(std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap(), "");
    assert!(std::env::var_os("AT_SPI_BUS_ADDRESS").is_none());
    assert!(std::env::var_os("DISPLAY").is_none());
    let (tx, events) = mpsc::sync_channel(8);
    let mut adapter =
        accesskit_unix::Adapter::new(Activate(tx.clone()), Act(tx.clone()), Deactivate(tx));
    adapter.set_root_window_bounds(
        accesskit::Rect::new(0.0, 0.0, 100.0, 80.0),
        accesskit::Rect::new(0.0, 0.0, 100.0, 80.0),
    );
    adapter.update_window_focus_state(true);
    println!("FIXTURE READY");
    std::io::stdout().flush().unwrap();
    let mut input = std::io::stdin().lock();
    let mut command = [0];
    loop {
        input.read_exact(&mut command).unwrap();
        match command[0] {
            b'A' => assert_eq!(events.recv_timeout(WAIT).unwrap(), "activate"),
            b'C' => assert_eq!(events.recv_timeout(WAIT).unwrap(), "action"),
            b'D' => {
                assert_eq!(events.recv_timeout(WAIT).unwrap(), "deactivate");
                adapter.update_if_active(|| panic!("inactive adapter must not build a tree"));
            }
            b'U' => adapter.update_if_active(|| TreeUpdate {
                nodes: vec![(NodeId(2), button("Capture updated"))],
                tree: None,
                tree_id: TreeId::ROOT,
                focus: NodeId(2),
            }),
            b'Q' => break,
            _ => panic!("invalid fixture command"),
        }
        println!("FIXTURE {}", command[0] as char);
        std::io::stdout().flush().unwrap();
    }
    drop(adapter);
}

fn wait_line(lines: &Receiver<String>, expected: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        let line = lines
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap_or_else(|error| panic!("waiting for {expected}: {error}"));
        if line.trim() == expected {
            return;
        }
    }
}
fn control(child: &mut ChildGuard, lines: &Receiver<String>, command: u8) {
    child
        .0
        .stdin
        .as_mut()
        .unwrap()
        .write_all(&[command])
        .unwrap();
    wait_line(lines, &format!("FIXTURE {}", command as char));
}
fn name(connection: &Connection, service: &str, path: &Path<'_>) -> Result<String, dbus::Error> {
    connection
        .with_proxy(service, path.clone(), Duration::from_millis(200))
        .get("org.a11y.atspi.Accessible", "Name")
}
fn wait_name(connection: &Connection, service: &str, path: &Path<'_>, expected: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        if name(connection, service, path).is_ok_and(|value| value == expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "actual accessibility node not published: {expected}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn assert_published_tree(
    connection: &Connection,
    plug: &(String, Path<'static>),
    registry: &str,
) -> Path<'static> {
    let deadline = Instant::now() + WAIT;
    let root = connection.with_proxy(&plug.0, plug.1.clone(), Duration::from_millis(200));
    let windows: Vec<(String, Path<'static>)> = loop {
        if let Ok((windows,)) = root.method_call::<(Vec<(String, Path<'static>)>,), _, _, _>(
            "org.a11y.atspi.Accessible",
            "GetChildren",
            (),
        ) {
            if !windows.is_empty() {
                break windows;
            }
        }
        assert!(
            Instant::now() < deadline,
            "actual root accessible interface not published"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0].0, plug.0);
    let toolkit: String = root
        .get("org.a11y.atspi.Application", "ToolkitName")
        .unwrap();
    assert_eq!(toolkit, "screenshots-private-fixture");
    root.set("org.a11y.atspi.Application", "Id", 73i32).unwrap();
    let id: i32 = root.get("org.a11y.atspi.Application", "Id").unwrap();
    assert_eq!(id, 73);
    let desktop: (String, Path<'static>) = root.get("org.a11y.atspi.Accessible", "Parent").unwrap();
    assert_eq!(desktop, (registry.to_owned(), Path::new(ROOT).unwrap()));
    wait_name(connection, &plug.0, &windows[0].1, "Private capture window");
    let window = connection.with_proxy(&plug.0, windows[0].1.clone(), WAIT);
    let (children,): (Vec<(String, Path<'static>)>,) = window
        .method_call("org.a11y.atspi.Accessible", "GetChildren", ())
        .unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].0, plug.0);
    let button_path = children[0].1.clone();
    wait_name(connection, &plug.0, &button_path, "Capture");
    let button = connection.with_proxy(&plug.0, button_path.clone(), WAIT);
    let (interfaces,): (Vec<String>,) = button
        .method_call("org.a11y.atspi.Accessible", "GetInterfaces", ())
        .unwrap();
    for interface in ["Accessible", "Action", "Component"] {
        assert!(interfaces.contains(&format!("org.a11y.atspi.{interface}")));
    }
    let (states,): (Vec<u32>,) = button
        .method_call("org.a11y.atspi.Accessible", "GetState", ())
        .unwrap();
    assert_eq!(states.len(), 2);
    let bits = u64::from(states[0]) | (u64::from(states[1]) << 32);
    assert_ne!(bits & State::Focusable as u64, 0);
    assert_ne!(bits & State::Focused as u64, 0);
    let (role,): (u32,) = button
        .method_call("org.a11y.atspi.Accessible", "GetRole", ())
        .unwrap();
    assert_eq!(role, atspi_common::Role::Button as u32);
    let actions: i32 = button.get("org.a11y.atspi.Action", "NActions").unwrap();
    assert_eq!(actions, 1);
    let (xml,): (String,) = button
        .method_call("org.freedesktop.DBus.Introspectable", "Introspect", ())
        .unwrap();
    assert!(xml.contains("org.a11y.atspi.Accessible"));
    assert!(xml.contains("type=\"au\""));
    assert!(xml.contains("type=\"as\""));
    assert!(xml.contains("type=\"(so)\""));
    button_path
}

#[test]
fn real_accesskit_activates_updates_dispatches_actions_and_reactivates_on_private_buses() {
    let folder = Folder::new();
    let (_session_daemon, session_address) = daemon(&folder);
    let (_atspi_daemon, atspi_address) = daemon(&folder);
    assert_ne!(session_address, atspi_address);
    let _session = session_mock(&session_address, atspi_address.clone());
    let (_registry, plugs, registry_name) = registry_mock(&atspi_address);
    let observer = connect(&atspi_address);
    let status = connect(&session_address);
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "private_accesskit_child", "--nocapture"])
        .env(CHILD_ENV, "1")
        .env("DBUS_SESSION_BUS_ADDRESS", &session_address)
        .env_remove("AT_SPI_BUS_ADDRESS")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for key in [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_RUNTIME_DIR",
    ] {
        command.env(key, &folder.0);
    }
    let mut child = ChildGuard(command.spawn().unwrap());
    let stdout = child.0.stdout.take().unwrap();
    let (tx, lines) = mpsc::sync_channel(32);
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap()).is_err() {
                break;
            }
        }
    });
    wait_line(&lines, "FIXTURE READY");
    let plug = plugs
        .recv_timeout(WAIT)
        .expect("real adapter must call Socket.Embed");
    control(&mut child, &lines, b'A');
    let button_path = assert_published_tree(&observer, &plug, &registry_name);
    let (done,): (bool,) = observer
        .with_proxy(&plug.0, button_path.clone(), WAIT)
        .method_call("org.a11y.atspi.Action", "DoAction", (0i32,))
        .unwrap();
    assert!(done);
    control(&mut child, &lines, b'C');
    control(&mut child, &lines, b'U');
    wait_name(&observer, &plug.0, &button_path, "Capture updated");
    let toggle = |enabled: bool| {
        let (): () = status
            .with_proxy("org.a11y.Bus", BUS_OBJECT, WAIT)
            .method_call("org.a11y.Bus", "FixtureSetEnabled", (enabled,))
            .unwrap();
    };
    toggle(false);
    control(&mut child, &lines, b'D');
    let (owned,): (bool,) = observer
        .with_proxy("org.freedesktop.DBus", "/org/freedesktop/DBus", WAIT)
        .method_call("org.freedesktop.DBus", "NameHasOwner", (&plug.0,))
        .unwrap();
    assert!(!owned, "deactivation must release the AT-SPI connection");
    toggle(true);
    let next = plugs
        .recv_timeout(WAIT)
        .expect("reactivation must embed again");
    assert_ne!(plug.0, next.0);
    control(&mut child, &lines, b'A');
    assert_published_tree(&observer, &next, &registry_name);
    child.0.stdin.as_mut().unwrap().write_all(b"Q").unwrap();
    let deadline = Instant::now() + WAIT;
    let exit = loop {
        if let Some(exit) = child.0.try_wait().unwrap() {
            break exit;
        }
        assert!(
            Instant::now() < deadline,
            "accessibility child teardown deadline"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut errors = String::new();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut errors)
        .unwrap();
    assert!(exit.success(), "private AccessKit child failed: {errors}");
    reader.join().unwrap();
}

#[test]
fn maintained_atspi_types_keep_their_exact_wire_signatures_and_roundtrip() {
    assert_eq!(StateSet::SIGNATURE.to_string(), "au");
    assert_eq!(InterfaceSet::SIGNATURE.to_string(), "as");
    assert_eq!(ObjectRef::SIGNATURE.to_string(), "(so)");
    assert_eq!(ObjectRefOwned::SIGNATURE.to_string(), "(so)");
    let states = StateSet::new(State::Focused | State::ManagesDescendants | State::Indeterminate);
    let interfaces =
        InterfaceSet::new(Interface::Accessible | Interface::Action | Interface::Component);
    // This is the exact type re-exported to the migrated AccessKit consumer.
    let _: accesskit_atspi_common::StateSet = states;
    let _: accesskit_atspi_common::InterfaceSet = interfaces;
    for endian in [LE, BE] {
        let context = Context::new(endian, 0);
        let encoded = to_bytes(context, &states).unwrap();
        let expected = [states.bits() as u32, (states.bits() >> 32) as u32];
        assert_eq!(
            encoded.bytes(),
            to_bytes(context, &expected.to_vec()).unwrap().bytes()
        );
        let (decoded, used) = encoded.deserialize::<StateSet>().unwrap();
        assert_eq!(decoded, states);
        assert_eq!(used, encoded.bytes().len());
        assert!(
            to_bytes(context, &vec![0u32])
                .unwrap()
                .deserialize::<StateSet>()
                .is_err()
        );
        let encoded = to_bytes(context, &interfaces).unwrap();
        let (names, _) = encoded.deserialize::<Vec<String>>().unwrap();
        assert_eq!(
            names,
            [
                "org.a11y.atspi.Accessible",
                "org.a11y.atspi.Action",
                "org.a11y.atspi.Component"
            ]
        );
        assert_eq!(encoded.deserialize::<InterfaceSet>().unwrap().0, interfaces);
        for reference in [
            ObjectRef::Null,
            ObjectRef::from_static_str_unchecked(":1.23", "/org/a11y/atspi/test"),
        ] {
            let encoded = to_bytes(context, &reference).unwrap();
            let name = reference.name_as_str().unwrap_or("");
            let path = zbus::wire::ObjectPath::try_from(reference.path_as_str()).unwrap();
            assert_eq!(
                encoded.bytes(),
                to_bytes(context, &(name, path)).unwrap().bytes()
            );
            assert_eq!(encoded.deserialize::<ObjectRef>().unwrap().0, reference);
            assert_eq!(
                encoded
                    .deserialize::<ObjectRefOwned>()
                    .unwrap()
                    .0
                    .into_inner(),
                reference
            );
        }
    }
}

struct Inserted(u32);
#[zbus::interface(name = "org.screenshots.MaintainedInsertion")]
impl Inserted {
    fn value(&self) -> u32 {
        self.0
    }
}
#[test]
fn maintained_object_server_distinguishes_fresh_and_existing_interfaces() {
    let folder = Folder::new();
    let (_daemon, address) = daemon(&folder);
    zbus::block_on(async {
        let connection = zbus::connection::Builder::address(address.as_str())
            .method_timeout(WAIT)
            .build()
            .await
            .unwrap();
        let server = connection.object_server();
        assert!(
            server
                .at("/org/screenshots/test", Inserted(19))
                .await
                .unwrap()
                .is_none()
        );
        let returned = server
            .at("/org/screenshots/test", Inserted(37))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(returned.0, 37);
        // Observe the service through an independent client, as real
        // accessibility clients do; don't couple this insertion check to self-RPC.
        let client = zbus::connection::Builder::address(address.as_str())
            .method_timeout(WAIT)
            .build()
            .await
            .unwrap();
        let proxy = zbus::Proxy::new(
            &client,
            connection.unique_name().unwrap().as_str(),
            "/org/screenshots/test",
            "org.screenshots.MaintainedInsertion",
        )
        .await
        .unwrap();
        let value: u32 = proxy.call("Value", &()).await.unwrap();
        assert_eq!(
            value, 19,
            "a duplicate insertion must preserve the existing service"
        );
    });
}
