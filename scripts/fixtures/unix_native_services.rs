//! Explicit private-bus/private-Xvfb fixture. It never requests a captured frame.
#[path = "../../apps/desktop/src/desktop_services_process.rs"]
mod desktop_services_process;
use anyhow::{Context, Result, ensure};
use app_core::{
    CaptureHotkeys, UnixDesktopEvent, UnixDesktopServices, UnixInstanceAdmission,
    UnixSingleInstance,
};
use dbus::{
    Message, Path,
    arg::{PropMap, RefArg, Variant},
    blocking::Connection,
    channel::{MatchingReceiver, Sender},
    message::MatchRule,
};
#[track_caller]
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    static STEP: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let step = STEP.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let line = std::panic::Location::caller().line();
    eprintln!("PRIVATE await {step} begin line {line}");
    let output = futures_lite::future::block_on(future);
    eprintln!("PRIVATE await {step} done");
    output
}
use std::{
    collections::HashMap,
    io::Write,
    os::{fd::AsRawFd, unix::net::UnixStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
const PORTAL: &str = "org.freedesktop.portal.Desktop";
const GLOBAL: &str = "org.freedesktop.portal.GlobalShortcuts";
const OBJECT: &str = "/org/freedesktop/portal/desktop";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";
fn property(value: impl RefArg + 'static) -> Variant<Box<dyn RefArg>> {
    Variant(Box::new(value))
}
#[derive(Default)]
struct State {
    stop: AtomicBool,
    host: AtomicBool,
    mode: AtomicUsize,
    closed: AtomicUsize,
    registered: Mutex<Option<String>>,
    sessions: Mutex<HashMap<String, Vec<String>>>,
    pending_request: Mutex<Option<String>>,
    pending_ready: Mutex<Option<std::sync::mpsc::SyncSender<()>>>,
    pending_result: Mutex<Option<PropMap>>,
}
fn mock(state: Arc<State>, ready: std::sync::mpsc::SyncSender<()>, wake: UnixStream) -> Result<()> {
    let mut channel = dbus::channel::Channel::get_private(dbus::channel::BusType::Session)?;
    channel.set_watch_enabled(true);
    let connection = Connection::from(channel);
    connection.request_name(PORTAL, false, true, true)?;
    connection.request_name(WATCHER, false, true, true)?;
    connection.set_signal_match_mode(true);
    let expired = state.clone();
    let mut gone = MatchRule::new_signal("org.freedesktop.DBus", "NameOwnerChanged")
        .with_sender("org.freedesktop.DBus");
    gone.strict_sender = true;
    connection.add_match(
        gone,
        move |(name, _, new): (String, String, String), _, _| {
            if name.starts_with(':') && new.is_empty() {
                let prefix = format!(
                    "/org/freedesktop/portal/desktop/session/{}/",
                    name.trim_start_matches(':').replace('.', "_")
                );
                expired
                    .sessions
                    .lock()
                    .unwrap()
                    .retain(|path, _| !path.starts_with(&prefix));
            }
            true
        },
    )?;
    let shared = state.clone();
    connection.start_receive(
        MatchRule::new_method_call(),
        Box::new(move |message, connection| {
            let response = answer(&message, connection, &shared).unwrap_or_else(|error| {
                Some(message.error(
                    &"org.freedesktop.DBus.Error.InvalidArgs".into(),
                    &std::ffi::CString::new(error.to_string()).unwrap(),
                ))
            });
            if let Some(response) = response {
                let _ = connection.send(response);
            }
            true
        }),
    );
    ready.send(()).unwrap();
    while !state.stop.load(Ordering::Acquire) {
        let mut full = true;
        for _ in 0..64 {
            if !connection.process(Duration::ZERO)? {
                full = false;
                break;
            }
        }
        if state.stop.load(Ordering::Acquire) {
            let _ = connection.channel().read_write(Some(Duration::ZERO));
            break;
        }
        if full {
            continue;
        }
        let watch = connection.channel().watch();
        let mut fds = [
            libc::pollfd {
                fd: watch.fd,
                events: libc::POLLIN | if watch.write { libc::POLLOUT } else { 0 },
                revents: 0,
            },
            libc::pollfd {
                fd: wake.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        ensure!(
            unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } >= 0,
            "Mock event wait failed"
        );
        if fds[1].revents != 0 {
            break;
        }
    }
    Ok(())
}
fn answer(message: &Message, connection: &Connection, state: &State) -> Result<Option<Message>> {
    let member = message.member().context("Missing member")?.to_string();
    let path = message.path().context("Missing path")?.to_string();
    if member == "ActivationReady" {
        return Ok(Some(message.method_return().append1(true)));
    }
    if member == "StopMock" {
        state.stop.store(true, Ordering::Release);
        return Ok(Some(message.method_return()));
    }
    if member == "HostGone" || member == "HostBack" {
        let available = member == "HostBack";
        state.host.store(available, Ordering::Release);
        let signal = if available {
            "StatusNotifierHostRegistered"
        } else {
            "StatusNotifierHostUnregistered"
        };
        connection
            .send(Message::new_signal("/StatusNotifierWatcher", WATCHER, signal).unwrap())
            .map_err(|_| anyhow::anyhow!("Mock host signal failed"))?;
        return Ok(Some(message.method_return()));
    }
    if member == "OldPress" {
        let (session, id, pressed): (Path<'static>, String, bool) = message.read3()?;
        connection
            .send(
                Message::new_signal(
                    OBJECT,
                    GLOBAL,
                    if pressed { "Activated" } else { "Deactivated" },
                )
                .unwrap()
                .append3(session, id, 0u64)
                .append1(PropMap::new()),
            )
            .map_err(|_| anyhow::anyhow!("Private key signal failed"))?;
        return Ok(Some(message.method_return()));
    }
    if member == "GrantPending" {
        let request = state
            .pending_request
            .lock()
            .unwrap()
            .take()
            .context("No pending grant")?;
        let result = state
            .pending_result
            .lock()
            .unwrap()
            .take()
            .context("No pending result")?;
        connection
            .send(
                Message::new_signal(request, "org.freedesktop.portal.Request", "Response")
                    .unwrap()
                    .append2(0u32, result),
            )
            .map_err(|_| anyhow::anyhow!("Private grant failed"))?;
        return Ok(Some(message.method_return()));
    }
    if member == "CancelPending" {
        state.pending_result.lock().unwrap().take();
        let request = state
            .pending_request
            .lock()
            .unwrap()
            .take()
            .context("No pending mock request")?;
        connection
            .send(
                Message::new_signal(request, "org.freedesktop.portal.Request", "Response")
                    .unwrap()
                    .append2(1u32, PropMap::new()),
            )
            .map_err(|_| anyhow::anyhow!("Mock cancellation failed"))?;
        return Ok(Some(message.method_return()));
    }
    if member == "Get" {
        let (_, property): (String, String) = message.read2()?;
        ensure!(
            property == "IsStatusNotifierHostRegistered",
            "Unexpected mock property"
        );
        return Ok(Some(
            message
                .method_return()
                .append1(property_value(state.host.load(Ordering::Acquire))),
        ));
    }
    if member == "RegisterStatusNotifierItem" {
        let item: String = message.read1()?;
        ensure!(item == "/StatusNotifierItem", "Unexpected tray path");
        *state.registered.lock().unwrap() = Some(message.sender().unwrap().to_string());
        return Ok(Some(message.method_return()));
    }
    if member == "Close" {
        if path.contains("/session/") {
            state.sessions.lock().unwrap().remove(&path);
            state.closed.fetch_add(1, Ordering::AcqRel);
        }
        return Ok(Some(message.method_return()));
    }
    ensure!(
        path == OBJECT && matches!(member.as_str(), "CreateSession" | "BindShortcuts"),
        "Unexpected mock method"
    );
    let client = message
        .sender()
        .unwrap()
        .to_string()
        .trim_start_matches(':')
        .replace('.', "_");
    let (options, shortcuts, session) = if member == "CreateSession" {
        let options: PropMap = message.read1()?;
        let token = options
            .get("session_handle_token")
            .and_then(|v| v.0.as_str())
            .context("Missing session token")?;
        let session = format!("/org/freedesktop/portal/desktop/session/{client}/{token}");
        (options, Vec::new(), session)
    } else {
        let (session, shortcuts, _parent, options): (
            Path<'static>,
            Vec<(String, PropMap)>,
            String,
            PropMap,
        ) = message.read4()?;
        (options, shortcuts, session.to_string())
    };
    let token = options
        .get("handle_token")
        .and_then(|v| v.0.as_str())
        .context("Missing request token")?;
    let request = format!("/org/freedesktop/portal/desktop/request/{client}/{token}");
    let mode = state.mode.load(Ordering::Acquire);
    let mut result = PropMap::new();
    let code = if member == "CreateSession" {
        result.insert("session_handle".into(), property(session.clone()));
        state
            .sessions
            .lock()
            .unwrap()
            .insert(session.clone(), Vec::new());
        0u32
    } else {
        let mut accepted = Vec::new();
        let mut ids = Vec::new();
        for (index, (id, fields)) in shortcuts.into_iter().enumerate() {
            let requested = fields
                .get("preferred_trigger")
                .and_then(|v| v.0.as_str())
                .context("Missing preferred trigger")?;
            ensure!(
                !requested.contains("Super"),
                "Main client must use portal notation"
            );
            if mode == 2 && index == 0 {
                continue;
            }
            let mut fields = PropMap::new();
            fields.insert(
                "trigger_description".into(),
                property(if mode == 3 {
                    "x".repeat(257)
                } else {
                    format!("Portal fixture trigger {index}")
                }),
            );
            ids.push(id.clone());
            accepted.push((id, fields));
        }
        state.sessions.lock().unwrap().insert(session.clone(), ids);
        result.insert("shortcuts".into(), property(accepted));
        if mode == 1 { 1 } else { 0 }
    };
    connection
        .send(
            message
                .method_return()
                .append1(Path::new(request.clone()).unwrap()),
        )
        .map_err(|_| anyhow::anyhow!("Mock reply failed"))?;
    if member == "BindShortcuts" && mode == 4 {
        *state.pending_request.lock().unwrap() = Some(request);
        *state.pending_result.lock().unwrap() = Some(result);
        if let Some(ready) = state.pending_ready.lock().unwrap().take() {
            let _ = ready.send(());
        }
        return Ok(None);
    }
    let signal = Message::new_signal(request, "org.freedesktop.portal.Request", "Response")
        .unwrap()
        .append2(code, result);
    connection
        .send(signal)
        .map_err(|_| anyhow::anyhow!("Mock response failed"))?;
    Ok(None)
}
fn property_value(value: bool) -> Variant<bool> {
    Variant(value)
}
fn bindings(first: &str) -> CaptureHotkeys {
    CaptureHotkeys([
        first.into(),
        "Ctrl+Alt+F2".into(),
        "Ctrl+Alt+F3".into(),
        "Ctrl+Alt+F4".into(),
        "Ctrl+Alt+F5".into(),
        "Ctrl+Alt+F6".into(),
    ])
}
fn direct_signal(connection: &Connection, client: &str, mut message: Message) -> Result<()> {
    message.set_destination(Some(client.into()));
    connection
        .send(message)
        .map_err(|_| anyhow::anyhow!("Could not send private directed signal"))?;
    Ok(())
}
fn alias_modifier_fixture(display: &str) -> Result<()> {
    use x11rb::{
        connection::Connection as _,
        protocol::xproto::{ConnectionExt as _, MappingStatus},
    };
    let (connection, _) = x11rb::rust_connection::RustConnection::connect(Some(display))?;
    let original = connection.get_modifier_mapping()?.reply()?;
    let width = original.keycodes.len() / 8;
    let mut groups = original
        .keycodes
        .chunks_exact(width)
        .map(|row| row.iter().copied().filter(|c| *c != 0).collect::<Vec<_>>())
        .collect::<Vec<_>>();
    let super_keys = std::mem::take(&mut groups[6]);
    ensure!(
        !super_keys.is_empty() && !groups[3].is_empty(),
        "Private Xvfb must expose Alt and Super"
    );
    groups[3].extend(super_keys);
    groups[3].sort_unstable();
    groups[3].dedup();
    let columns = groups.iter().map(Vec::len).max().unwrap();
    let map = groups
        .into_iter()
        .flat_map(|mut row| {
            row.resize(columns, 0);
            row
        })
        .collect::<Vec<_>>();
    ensure!(
        connection.set_modifier_mapping(&map)?.reply()?.status == MappingStatus::SUCCESS,
        "Could not install private alias mapping"
    );
    let result = (|| -> Result<()> {
        let first = app_core::X11ShortcutRegistrar::open(display)?;
        first.register_shortcut("Alt+F8")?;
        ensure!(
            first.register_shortcut("Super+F8").is_err(),
            "Distinct IDs resolving to one native chord must fail before grabbing"
        );
        let other = app_core::X11ShortcutRegistrar::open(display)?;
        ensure!(
            other.register_shortcut("Alt+F8").is_err(),
            "Alias rejection must not release the working native chord"
        );
        first.unregister_shortcut("Alt+F8")?;
        other.register_shortcut("Alt+F8")?;
        other.unregister_shortcut("Alt+F8")?;
        Ok(())
    })();
    let restored = connection
        .set_modifier_mapping(&original.keycodes)?
        .reply()?
        .status
        == MappingStatus::SUCCESS;
    ensure!(restored, "Could not restore the private modifier mapping");
    result
}
fn stalled_bus_fixture(
    owner: Arc<Mutex<UnixSingleInstance>>,
    runtime: &str,
    display: &str,
    close: bool,
) -> Result<()> {
    use std::io::Read as _;
    use std::os::unix::net::UnixListener;
    let listener = UnixListener::bind(format!("{runtime}/bus-stalled"))?;
    listener.set_nonblocking(true)?;
    let (connected, peer_ready) = std::sync::mpsc::sync_channel(1);
    let server = std::thread::spawn(move || -> Result<()> {
        let mut fd = libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        ensure!(
            unsafe { libc::poll(&mut fd, 1, 5000) } > 0,
            "Helper never connected to the stalled bus"
        );
        let (mut socket, _) = listener.accept()?;
        socket.set_read_timeout(Some(Duration::from_secs(8)))?;
        let mut initial = [0u8; 128];
        ensure!(
            socket.read(&mut initial)? > 0,
            "No authentication request reached the peer"
        );
        connected.send(()).unwrap();
        loop {
            if socket.read(&mut initial)? == 0 {
                break;
            }
        }
        Ok(())
    });
    let (services, mut events) =
        UnixDesktopServices::start(owner, false, Some(display.to_owned()))?;
    peer_ready.recv_timeout(Duration::from_secs(5))?;
    let began = std::time::Instant::now();
    if !close {
        loop {
            if matches!(
                block_on(events.recv()),
                Some(UnixDesktopEvent::TrayStatus {
                    available: false,
                    ..
                })
            ) {
                break;
            }
        }
    }
    let _ = block_on(services.shutdown());
    ensure!(
        began.elapsed() < Duration::from_secs(if close { 3 } else { 8 }),
        "Stalled bus owner was not reaped"
    );
    server
        .join()
        .map_err(|_| anyhow::anyhow!("Stalled peer panicked"))??;
    Ok(())
}
fn main() -> Result<()> {
    if desktop_services_process::try_child(&std::env::args_os().skip(1).collect::<Vec<_>>())? {
        return Ok(());
    }
    desktop_services_process::install()?;
    ensure!(
        std::env::var("SCREENSHOTS_PRIVATE_NATIVE_FIXTURE").as_deref() == Ok("1"),
        "Use the explicit private fixture runner"
    );
    let runtime = std::env::var("XDG_RUNTIME_DIR")?;
    ensure!(
        runtime.starts_with("/tmp/screenshots-native-"),
        "Fixture runtime must be private"
    );
    let bus = std::env::var("DBUS_SESSION_BUS_ADDRESS")?;
    ensure!(
        bus.starts_with(&format!("unix:path={runtime}/bus")),
        "Refusing a real session bus"
    );
    if std::env::args().any(|v| v == "--activated-portal-mock") {
        let state = Arc::new(State::default());
        state.host.store(true, Ordering::Release);
        let (ready, _receiver) = std::sync::mpsc::sync_channel(1);
        let (wake, _keep_alive) = UnixStream::pair()?;
        return mock(state, ready, wake);
    }
    if std::env::args().any(|v| v == "activation") {
        let connection = Connection::new_session()?;
        let daemon = connection.with_proxy(
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            Duration::from_secs(2),
        );
        let (running,): (bool,) =
            daemon.method_call("org.freedesktop.DBus", "NameHasOwner", (PORTAL,))?;
        ensure!(
            !running,
            "The activation fixture must begin with an installed idle service"
        );
        let owner = app_core::shortcut_portal_owner(&connection)?;
        ensure!(
            owner.starts_with(':'),
            "Activation must resolve the unique owner"
        );
        let portal = connection.with_proxy(owner.clone(), OBJECT, Duration::from_secs(2));
        let (ready,): (bool,) =
            portal.method_call("org.screenshots.PrivateFixture", "ActivationReady", ())?;
        ensure!(
            ready && app_core::shortcut_portal_owner(&connection)? == owner,
            "An already active owner must remain pinned"
        );
        let _: Result<(), dbus::Error> =
            portal.method_call("org.screenshots.PrivateFixture", "StopMock", ());
        return Ok(());
    }
    if std::env::args().any(|v| v == "show_only") {
        ensure!(
            matches!(
                UnixSingleInstance::acquire()?,
                UnixInstanceAdmission::Forwarded
            ),
            "Secondary Show must reach the existing owner"
        );
        return Ok(());
    }
    let wayland = std::env::args().any(|v| v == "wayland");
    let display = std::env::var("DISPLAY")?;
    ensure!(
        display.starts_with(':'),
        "Fixture requires private Xvfb display"
    );
    if !wayland {
        alias_modifier_fixture(&display)?;
    }
    let owner = match UnixSingleInstance::acquire()? {
        UnixInstanceAdmission::Owner(owner) => Arc::new(Mutex::new(owner)),
        _ => anyhow::bail!("Private fixture must own its unique session"),
    };
    if std::env::args().any(|v| v == "stalled_bus" || v == "close_stalled_bus") {
        return stalled_bus_fixture(
            owner,
            &runtime,
            &display,
            std::env::args().any(|v| v == "close_stalled_bus"),
        );
    }
    let shared = Arc::new(State::default());
    shared.host.store(true, Ordering::Release);
    let mock_state = shared.clone();
    let (ready, wait_ready) = std::sync::mpsc::sync_channel(1);
    let (mock_wake_read, mut mock_wake_write) = UnixStream::pair()?;
    let mock_thread = std::thread::spawn(move || mock(mock_state, ready, mock_wake_read));
    wait_ready.recv_timeout(Duration::from_secs(5))?;
    let listener_owner = owner.clone();
    let mut show_events = listener_owner.lock().unwrap().listen()?;
    let (mut services, mut events) =
        UnixDesktopServices::start(owner, wayland, Some(display.clone()))?;
    loop {
        if matches!(
            block_on(events.recv()),
            Some(UnixDesktopEvent::TrayStatus {
                available: true,
                ..
            })
        ) {
            break;
        }
    }
    let first = bindings("Ctrl+Alt+F1");
    let initialized = block_on(services.initialize(&first));
    ensure!(initialized.outcome.is_ok(), "{}", initialized.status);
    if wayland {
        ensure!(
            initialized.status.contains("Portal fixture trigger"),
            "Must report portal assignments rather than requested keys as active"
        );
    }
    services.set_quitting(true);
    let secondary = std::process::Command::new(std::env::current_exe()?)
        .arg("show_only")
        .output()?;
    ensure!(
        secondary.status.success(),
        "Show must remain accepted while recoverable output work is pending"
    );
    ensure!(
        block_on(show_events.recv()).is_some(),
        "Busy Show notification was lost"
    );
    if !wayland {
        let probe = app_core::X11ShortcutRegistrar::open(&display)?;
        ensure!(
            probe.register_shortcut(&first.0[0]).is_err(),
            "Provisional Quit must keep native registrations owned"
        );
    }
    // The connected UI check supplies the actual failed-save callback; this checks the native boundary.
    services.set_quitting(false);
    let secondary = std::process::Command::new(std::env::current_exe()?)
        .arg("show_only")
        .output()?;
    ensure!(
        secondary.status.success(),
        "Show must work after recoverable Quit is canceled"
    );
    ensure!(
        block_on(show_events.recv()).is_some(),
        "Post-recovery Show notification was lost"
    );
    let client = shared
        .registered
        .lock()
        .unwrap()
        .clone()
        .context("Missing native client")?;
    let attacker = Connection::new_session()?;
    while events.try_recv().is_ok() {}
    let before = services.shortcut_status();
    for name in [PORTAL, WATCHER] {
        direct_signal(
            &attacker,
            &client,
            Message::new_signal(
                "/org/freedesktop/DBus",
                "org.freedesktop.DBus",
                "NameOwnerChanged",
            )
            .unwrap()
            .append3(name, ":1.0", ""),
        )?;
    }
    direct_signal(
        &attacker,
        &client,
        Message::new_signal(
            "/StatusNotifierWatcher",
            WATCHER,
            "StatusNotifierHostUnregistered",
        )
        .unwrap(),
    )?;
    let menu_barrier = attacker.with_proxy(client.clone(), "/Menu", Duration::from_secs(2));
    let _: (Variant<bool>,) =
        menu_barrier.method_call("com.canonical.dbusmenu", "GetProperty", (6i32, "enabled"))?;
    ensure!(
        services.tray_available(),
        "A directed forged watcher/owner signal must not revoke the tray"
    );
    ensure!(
        services.shortcut_status() == before,
        "A directed forged daemon signal must not retire portal state"
    );
    while let Ok(event) = events.try_recv() {
        ensure!(
            !matches!(
                event,
                UnixDesktopEvent::TrayStatus {
                    available: false,
                    ..
                }
            ),
            "A directed forgery must not emit a transient false tray-loss event"
        );
    }
    let old_session = shared.sessions.lock().unwrap().keys().next().cloned();
    // Avoid XKB server Ctrl+Alt+Fn VT-switch actions in the owned Xvfb.
    let second = bindings("Ctrl+Alt+G");
    let prepared = block_on(services.prepare(&second));
    ensure!(prepared.outcome.is_ok(), "{}", prepared.status);
    ensure!(
        prepared.status == initialized.status,
        "Staged reservations must retain old routes/status"
    );
    services.set_quitting(true);
    ensure!(
        block_on(services.finish(false)).outcome.is_ok(),
        "Rollback must release new reservations"
    );
    services.set_quitting(false);
    let prepared_again = block_on(services.prepare(&second));
    ensure!(
        prepared_again.outcome.is_ok(),
        "Recovered service must admit a new preparation"
    );
    ensure!(
        block_on(services.finish(false)).outcome.is_ok(),
        "Recovered rollback must finish"
    );
    if wayland {
        // Original Wayland authority must prevent global XWayland grabs even when DISPLAY exists.
        let probe = app_core::X11ShortcutRegistrar::open(&display)?;
        for key in &first.0 {
            probe.register_shortcut(key)?;
            probe.unregister_shortcut(key)?;
        }
        shared.mode.store(4, Ordering::Release);
        let (ready, reached) = std::sync::mpsc::sync_channel(1);
        *shared.pending_ready.lock().unwrap() = Some(ready);
        let pending = services.prepare(&second);
        reached.recv_timeout(Duration::from_secs(5))?;
        let request = shared
            .pending_request
            .lock()
            .unwrap()
            .clone()
            .context("Missing pending portal request")?;
        let granted = (0..6)
            .map(|index| {
                let mut fields = PropMap::new();
                fields.insert(
                    "trigger_description".into(),
                    property("Forged grant".to_owned()),
                );
                (format!("capture_{index}"), fields)
            })
            .collect::<Vec<_>>();
        let mut result = PropMap::new();
        result.insert("shortcuts".into(), property(granted));
        direct_signal(
            &attacker,
            &client,
            Message::new_signal(request, "org.freedesktop.portal.Request", "Response")
                .unwrap()
                .append2(0u32, result),
        )?;
        let _: (Variant<bool>,) =
            menu_barrier.method_call("com.canonical.dbusmenu", "GetProperty", (6i32, "enabled"))?;
        let control = Connection::new_session()?;
        let watcher = control.with_proxy(WATCHER, "/StatusNotifierWatcher", Duration::from_secs(2));
        if let Some(session) = &old_session {
            let _: () = watcher.method_call(
                "org.screenshots.PrivateFixture",
                "OldPress",
                (
                    Path::new(session.clone()).map_err(anyhow::Error::msg)?,
                    "capture_0",
                    true,
                ),
            )?;
            let _: () = watcher.method_call(
                "org.screenshots.PrivateFixture",
                "OldPress",
                (
                    Path::new(session.clone()).map_err(anyhow::Error::msg)?,
                    "capture_0",
                    false,
                ),
            )?;
        }
        if std::env::args().any(|v| v == "close_consent") {
            let began = std::time::Instant::now();
            let _ = block_on(services.shutdown());
            ensure!(
                began.elapsed() < Duration::from_secs(3),
                "Quit did not reap a helper blocked in portal consent"
            );
            ensure!(
                block_on(pending).outcome.is_err(),
                "Canceled consent must not report successful reservation"
            );
            shared.stop.store(true, Ordering::Release);
            mock_wake_write.write_all(&[1])?;
            mock_thread
                .join()
                .map_err(|_| anyhow::anyhow!("Mock panicked"))??;
            return Ok(());
        }
        let _: () = watcher.method_call("org.screenshots.PrivateFixture", "HostGone", ())?;
        loop {
            if matches!(
                block_on(events.recv()),
                Some(UnixDesktopEvent::TrayStatus {
                    available: false,
                    ..
                })
            ) {
                break;
            }
        }
        ensure!(
            !services.tray_available(),
            "Tray availability must be revoked while portal consent is pending"
        );
        let _: () = watcher.method_call("org.screenshots.PrivateFixture", "CancelPending", ())?;
        ensure!(
            block_on(pending).outcome.is_err(),
            "Canceled pending replacement must preserve old session"
        );
        shared.mode.store(0, Ordering::Release);
        let _: () = watcher.method_call("org.screenshots.PrivateFixture", "HostBack", ())?;
        loop {
            if matches!(
                block_on(events.recv()),
                Some(UnixDesktopEvent::TrayStatus {
                    available: true,
                    ..
                })
            ) {
                break;
            }
        }
        shared.mode.store(4, Ordering::Release);
        let (ready, reached) = std::sync::mpsc::sync_channel(1);
        *shared.pending_ready.lock().unwrap() = Some(ready);
        let delayed = services.prepare(&second);
        reached.recv_timeout(Duration::from_secs(5))?;
        if let Some(session) = &old_session {
            let _: () = watcher.method_call(
                "org.screenshots.PrivateFixture",
                "OldPress",
                (
                    Path::new(session.clone()).map_err(anyhow::Error::msg)?,
                    "capture_0",
                    true,
                ),
            )?;
            let _: () = watcher.method_call(
                "org.screenshots.PrivateFixture",
                "OldPress",
                (
                    Path::new(session.clone()).map_err(anyhow::Error::msg)?,
                    "capture_0",
                    false,
                ),
            )?;
        }
        let _: () = watcher.method_call("org.screenshots.PrivateFixture", "GrantPending", ())?;
        ensure!(block_on(delayed).outcome.is_ok(), "Delayed grant failed");
        ensure!(
            block_on(services.finish(true)).outcome.is_ok(),
            "Delayed commit failed"
        );
        while let Ok(event) = events.try_recv() {
            ensure!(
                !matches!(
                    services.action(event),
                    Some(app_core::UnixDesktopAction::Capture(_))
                ),
                "An old consent key was replayed after commit"
            );
        }
        shared.mode.store(0, Ordering::Release);
        for mode in [1, 2, 3] {
            shared.mode.store(mode, Ordering::Release);
            let rejected = block_on(services.prepare(&second));
            ensure!(
                rejected.outcome.is_err(),
                "Cancel/partial/malformed portal grant must be rejected"
            );
            ensure!(
                services
                    .shortcut_status()
                    .contains("Portal fixture trigger"),
                "Failure must preserve previous active assignments"
            );
        }
        shared.mode.store(0, Ordering::Release);
    } else {
        let blocker = app_core::X11ShortcutRegistrar::open(&display)?;
        blocker.register_shortcut("Ctrl+Alt+F8")?;
        let mut conflicted = second.clone();
        conflicted.0[1] = "Ctrl+Alt+F8".into();
        ensure!(
            block_on(services.prepare(&conflicted)).outcome.is_err(),
            "Real XGrabKey conflict must reject replacement"
        );
        let probe = app_core::X11ShortcutRegistrar::open(&display)?;
        probe.register_shortcut(&second.0[0])?;
        probe.unregister_shortcut(&second.0[0])?;
        blocker.unregister_shortcut("Ctrl+Alt+F8")?;
    }
    ensure!(
        block_on(services.prepare(&second)).outcome.is_ok(),
        "Prepare replacement"
    );
    let committed = block_on(services.finish(true));
    ensure!(
        committed.outcome.is_ok(),
        "Commit replacement: status={}; outcome={:?}",
        committed.status,
        committed.outcome
    );
    if !wayland {
        use x11rb::{
            connection::Connection as _,
            protocol::{xproto::ConnectionExt as _, xtest::ConnectionExt as _},
        };
        let (injector, screen) = x11rb::rust_connection::RustConnection::connect(Some(&display))?;
        let setup = injector.setup();
        let mapping = injector
            .get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)?
            .reply()?;
        let code = |symbol| -> Result<u8> {
            mapping
                .keysyms
                .chunks_exact(mapping.keysyms_per_keycode as usize)
                .position(|r| r[0] == symbol)
                .map(|i| setup.min_keycode + i as u8)
                .context("Missing private Xvfb key")
        };
        let keys = [code(0xffe3)?, code(0xffe9)?, code(0x67)?]; // Control, Alt, G.
        for key in keys {
            injector
                .xtest_fake_input(2, key, 0, setup.roots[screen].root, 0, 0, 0)?
                .check()?;
        }
        for key in keys.into_iter().rev() {
            injector
                .xtest_fake_input(3, key, 0, setup.roots[screen].root, 0, 0, 0)?
                .check()?;
        }
        injector.flush()?;
        let until = std::time::Instant::now() + Duration::from_secs(3);
        loop {
            let event = block_on(futures_lite::future::or(
                async {
                    events
                        .recv()
                        .await
                        .context("Private event channel closed before committed Ctrl+Alt+G")
                },
                async {
                    async_io::Timer::at(until).await;
                    anyhow::bail!(
                        "Committed Ctrl+Alt+G did not reach current native admission within three seconds"
                    )
                },
            ))?;
            match &event {
                UnixDesktopEvent::Stamped { action, generation } => {
                    eprintln!("PRIVATE native action={action:?} epoch={generation}")
                }
                UnixDesktopEvent::ShortcutStatus(status) => {
                    eprintln!("PRIVATE shortcut status={status}")
                }
                _ => eprintln!("PRIVATE native non-capture event"),
            }
            let admitted = services.action(event);
            eprintln!("PRIVATE admitted={admitted:?}");
            if matches!(
                admitted,
                Some(app_core::UnixDesktopAction::Capture(
                    app_core::CaptureCommand::Primary
                ))
            ) {
                break;
            }
        }
    }
    let client = shared
        .registered
        .lock()
        .unwrap()
        .clone()
        .context("Tray did not register")?;
    let connection = Connection::new_session()?;
    let menu = connection.with_proxy(client, "/Menu", Duration::from_secs(2));
    let (revision, layout): (u32, (i32, PropMap, Vec<Variant<Box<dyn RefArg>>>)) = menu
        .method_call(
            "com.canonical.dbusmenu",
            "GetLayout",
            (0i32, -1i32, Vec::<String>::new()),
        )?;
    ensure!(
        revision == 1 && layout.0 == 0 && layout.2.len() == 8,
        "Existing eight commands must be available"
    );
    let (enabled,): (Variant<bool>,) =
        menu.method_call("com.canonical.dbusmenu", "GetProperty", (6i32, "enabled"))?;
    ensure!(!enabled.0, "Repeat starts disabled");
    let applied = Arc::new(AtomicBool::new(false));
    let observed = applied.clone();
    connection.add_match(
        MatchRule::new_signal("com.canonical.dbusmenu", "ItemsPropertiesUpdated")
            .with_path("/Menu"),
        move |(rows, _): (Vec<(i32, PropMap)>, Vec<(i32, Vec<String>)>), _, _| {
            if rows.iter().any(|(id, _)| *id == 6) {
                observed.store(true, Ordering::Release);
            }
            true
        },
    )?;
    services.set_repeat_region_available(true);
    let until = std::time::Instant::now() + Duration::from_secs(2);
    while !applied.load(Ordering::Acquire) {
        ensure!(
            std::time::Instant::now() < until,
            "Native repeat update did not arrive"
        );
        connection.process(until.saturating_duration_since(std::time::Instant::now()))?;
    }
    let (enabled,): (Variant<bool>,) =
        menu.method_call("com.canonical.dbusmenu", "GetProperty", (6i32, "enabled"))?;
    ensure!(enabled.0, "Repeat state follows controller without polling");
    let _: () = menu.method_call(
        "com.canonical.dbusmenu",
        "Event",
        (2i32, "clicked", Variant(0i32), 0u32),
    )?;
    loop {
        if matches!(
            block_on(events.recv()),
            Some(UnixDesktopEvent::Stamped {
                action: app_core::UnixDesktopAction::Tray(app_core::TrayCommand::Region),
                ..
            })
        ) {
            break;
        }
    }
    block_on(services.shutdown())?;
    if !wayland {
        let probe = app_core::X11ShortcutRegistrar::open(&display)?;
        for key in second.0 {
            probe.register_shortcut(&key)?;
            probe.unregister_shortcut(&key)?;
        }
    } else {
        ensure!(
            shared.sessions.lock().unwrap().is_empty(),
            "Every portal session must be closed"
        );
    }
    shared.stop.store(true, Ordering::Release);
    mock_wake_write.write_all(&[1])?;
    mock_thread
        .join()
        .map_err(|_| anyhow::anyhow!("Mock panicked"))??;
    println!("private native fixture completed; no frame/capture/clipboard/output requested");
    Ok(())
}
