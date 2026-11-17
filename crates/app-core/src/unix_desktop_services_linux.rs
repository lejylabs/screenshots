//! One parked native worker multiplexes X11 or portal, SNI and control wake fds.
use super::*;
use anyhow::{Context, ensure};
use dbus::blocking::Connection;
use std::{
    io::{Read, Write},
    os::{
        fd::{AsRawFd, RawFd},
        unix::net::UnixStream,
    },
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
#[path = "unix_desktop_services_process.rs"]
mod process;
pub(super) use process::emit as emit_to_helper;
pub use process::{
    LinuxDesktopProcess, LinuxDesktopProcessOwner, LinuxTaskPortal, install_linux_desktop_spawner,
    serve_linux_desktop_child,
};
#[path = "unix_desktop_services_portal.rs"]
mod portal;
#[path = "unix_desktop_services_sni.rs"]
mod sni;
#[path = "unix_desktop_services_task_portal.rs"]
mod task_portal;
#[path = "unix_desktop_services_x11.rs"]
mod x11;
#[doc(hidden)]
pub use portal::{shortcut_portal_owner, shortcut_portal_request};
#[doc(hidden)]
pub use x11::Registrar as X11ShortcutRegistrar;

pub(super) fn daemon_owner_rule() -> dbus::message::MatchRule<'static> {
    let mut rule = dbus::message::MatchRule::new_signal("org.freedesktop.DBus", "NameOwnerChanged")
        .with_sender("org.freedesktop.DBus");
    rule.strict_sender = true;
    rule
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "operation",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
enum Change {
    Initialize(CaptureHotkeys),
    Prepare(CaptureHotkeys),
    Finish(bool),
}
struct Request {
    generation: u64,
    input_epoch: u64,
    change: Change,
    reply: oneshot::Sender<UnixShortcutReport>,
}
struct Shared {
    stop: AtomicBool,
    generation: AtomicU64,
    input_epoch: AtomicU64,
    quit_boundary: AtomicU64,
    quitting: AtomicBool,
    policy_changed: AtomicBool,
    allow_capture: Arc<AtomicBool>,
    tray_status: Mutex<String>,
    repeat: Arc<AtomicBool>,
    repeat_changed: AtomicBool,
    tray_available: Arc<AtomicBool>,
    shortcut_status: Arc<Mutex<String>>,
}
fn shared() -> Arc<Shared> {
    Arc::new(Shared {
        stop: AtomicBool::new(false),
        generation: AtomicU64::new(0),
        input_epoch: AtomicU64::new(0),
        quit_boundary: AtomicU64::new(0),
        quitting: AtomicBool::new(false),
        policy_changed: AtomicBool::new(false),
        allow_capture: Arc::new(AtomicBool::new(true)),
        tray_status: Mutex::new("Tray starting".into()),
        repeat: Arc::new(AtomicBool::new(false)),
        repeat_changed: AtomicBool::new(false),
        tray_available: Arc::new(AtomicBool::new(false)),
        shortcut_status: Arc::new(Mutex::new(
            "Waiting for settings; no global shortcuts active".into(),
        )),
    })
}

#[cfg(test)]
thread_local! { static AFTER_REQUEST_PUBLISHED: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) }; }
pub(super) struct Services {
    sender: mpsc::Sender<Request>,
    wake: UnixStream,
    shared: Arc<Shared>,
    worker: Option<JoinHandle<Result<()>>>,
}
impl Services {
    pub(super) fn start(
        owner: Arc<Mutex<crate::UnixSingleInstance>>,
        wayland: bool,
        display: Option<String>,
    ) -> Result<(Self, mpsc::Receiver<UnixDesktopEvent>)> {
        let (read, write) = UnixStream::pair()?;
        read.set_nonblocking(true)?;
        write.set_nonblocking(true)?;
        let (sender, requests) = mpsc::channel(1);
        let (events, receiver) = mpsc::channel(32);
        let shared = shared();
        let state = shared.clone();
        let worker = std::thread::Builder::new()
            .name("desktop-services".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                // Keep singleton ownership through every native connection/session destructor.
                let _owner = owner;
                let result =
                    process::supervise(read, requests, events.clone(), &state, wayland, display);
                state.stop.store(true, Ordering::Release);
                state.allow_capture.store(false, Ordering::Release);
                state.tray_available.store(false, Ordering::Release);
                if let Err(error) = &result {
                    failure_status(&state, &events, error);
                }
                result
            })?;
        Ok((
            Self {
                sender,
                wake: write,
                shared,
                worker: Some(worker),
            },
            receiver,
        ))
    }
    fn wake(&self) {
        let _ = (&self.wake).write(&[1]);
    }
    fn request(&self, change: Change) -> UnixShortcutOperation {
        let unavailable = || {
            Box::pin(std::future::ready(UnixShortcutReport::unavailable(
                "Native shortcut service is busy or unavailable",
            ))) as UnixShortcutOperation
        };
        if self.shared.stop.load(Ordering::Acquire) {
            return unavailable();
        }
        // Reserve admission before changing the gate/epochs; rejection has no published state.
        let permit = match self.sender.try_reserve() {
            Ok(permit) => permit,
            Err(_) => return unavailable(),
        };
        let (reply, response) = oneshot::channel();
        let generation = self
            .shared
            .generation
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        let input_epoch = self
            .shared
            .input_epoch
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        self.shared.allow_capture.store(false, Ordering::Release);
        permit.send(Request {
            generation,
            input_epoch,
            change,
            reply,
        });
        #[cfg(test)]
        AFTER_REQUEST_PUBLISHED.with(|hook| {
            if let Some(hook) = hook.borrow_mut().take() {
                hook();
            }
        });
        self.wake();
        Box::pin(async move {
            response.await.unwrap_or_else(|_| {
                UnixShortcutReport::unavailable("Native shortcut service stopped")
            })
        })
    }
    pub(super) fn initialize(&mut self, settings: &CaptureHotkeys) -> UnixShortcutOperation {
        self.request(Change::Initialize(settings.clone()))
    }
    pub(super) fn prepare(&mut self, settings: &CaptureHotkeys) -> UnixShortcutOperation {
        self.request(Change::Prepare(settings.clone()))
    }
    pub(super) fn finish(&mut self, commit: bool) -> UnixShortcutOperation {
        self.request(Change::Finish(commit))
    }
    pub(super) fn set_quitting(&mut self, quitting: bool) {
        if self.shared.quitting.swap(quitting, Ordering::AcqRel) == quitting {
            return;
        }
        let boundary = self.shared.input_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        self.shared.quit_boundary.store(boundary, Ordering::Release);
        self.shared.policy_changed.store(true, Ordering::Release);
        self.wake();
    }
    pub(super) fn set_repeat_region_available(&self, available: bool) {
        if self.shared.repeat.swap(available, Ordering::AcqRel) != available {
            self.shared.repeat_changed.store(true, Ordering::Release);
            self.wake();
        }
    }
    pub(super) fn shortcut_status(&self) -> String {
        self.shared.shortcut_status.lock().unwrap().clone()
    }
    pub(super) fn tray_status(&self) -> String {
        self.shared.tray_status.lock().unwrap().clone()
    }
    pub(super) fn tray_available(&self) -> bool {
        self.shared.tray_available.load(Ordering::Acquire)
    }
    pub(super) fn action(&mut self, event: UnixDesktopEvent) -> Option<UnixDesktopAction> {
        if self.shared.stop.load(Ordering::Acquire) {
            return None;
        }
        match event {
            UnixDesktopEvent::Stamped { action, generation } => {
                let always = match action {
                    UnixDesktopAction::Tray(TrayCommand::Show) => true,
                    UnixDesktopAction::Tray(TrayCommand::Quit) => {
                        generation >= self.shared.quit_boundary.load(Ordering::Acquire)
                    }
                    _ => false,
                };
                (always
                    || (generation == self.shared.input_epoch.load(Ordering::Acquire)
                        && !self.shared.quitting.load(Ordering::Acquire)
                        && self.shared.allow_capture.load(Ordering::Acquire)))
                .then_some(action)
            }
            _ => None,
        }
    }
    // Run the returned future on GPUI's existing background executor: join is blocking.
    pub(super) fn shutdown(mut self) -> impl Future<Output = Result<()>> + Send {
        self.shared.stop.store(true, Ordering::Release);
        self.wake();
        let worker = self.worker.take();
        async move {
            if let Some(worker) = worker {
                worker
                    .join()
                    .map_err(|_| anyhow::anyhow!("Native services worker panicked"))??;
            }
            Ok(())
        }
    }
}
impl Drop for Services {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        self.wake();
    }
}

enum Backend {
    X11(crate::hotkey::CaptureHotkey<x11::Registrar>),
    Portal(portal::Shortcuts),
    Unavailable(String),
}
impl Backend {
    fn change(
        &mut self,
        connection: &Connection,
        change: Change,
        state: &Shared,
        wake: RawFd,
    ) -> UnixShortcutReport {
        let result = match self {
            Self::X11(keys) => match change {
                Change::Initialize(settings) => keys.register_settings(&settings),
                Change::Prepare(settings) => keys.prepare(&settings),
                Change::Finish(commit) => keys.finish(commit),
            },
            Self::Portal(keys) => match change {
                Change::Initialize(settings) => {
                    keys.initialize(connection, &settings, &state.stop, wake)
                }
                Change::Prepare(settings) => keys.prepare(connection, &settings, &state.stop, wake),
                Change::Finish(commit) => keys.finish(connection, commit),
            },
            Self::Unavailable(message) => return UnixShortcutReport::unavailable(message),
        };
        if matches!(&*self, Self::Portal(keys) if !keys.alive()) {
            return UnixShortcutReport::unavailable(
                "Wayland shortcut portal disappeared; restart to register again",
            );
        }
        let status = match self {
            Self::X11(keys) => keys.status().into(),
            Self::Portal(keys) => keys.status().into(),
            Self::Unavailable(message) => message.clone(),
        };
        UnixShortcutReport::new(status, result)
    }
    fn close(&mut self, connection: &Connection) -> Result<()> {
        if let Self::Portal(keys) = self {
            keys.close(connection)
        } else {
            Ok(())
        }
    }
}
fn run_native(state: Arc<Shared>, wayland: bool, display: Option<String>) -> Result<()> {
    let (mut wake, _keep_wake) = UnixStream::pair()?;
    wake.set_nonblocking(true)?;
    let (events, _unused_receiver) = mpsc::channel(32);
    let mut channel = dbus::channel::Channel::get_private(dbus::channel::BusType::Session)
        .context("Could not connect native services to the session bus")?;
    channel.set_watch_enabled(true);
    let connection = Connection::from(channel);
    process::bus_ready()?;
    // Watcher and portal owner changes use separate local filters on this one connection.
    connection.set_signal_match_mode(true);
    let mut tray = sni::Tray::install(
        &connection,
        events.clone(),
        state.repeat.clone(),
        state.tray_available.clone(),
    )?;
    let (mut backend, signals) = if wayland {
        match portal::shortcut_portal_owner(&connection).and_then(|owner| {
            portal::install(
                &connection,
                &owner,
                events.clone(),
                state.shortcut_status.clone(),
                state.allow_capture.clone(),
            )
            .map(|(signals, alive)| (owner, signals, alive))
        }) {
            Ok((owner, signals, alive)) => (
                Backend::Portal(portal::Shortcuts::new(owner, alive)),
                Some(signals),
            ),
            Err(error) => (
                Backend::Unavailable(bounded_status(&format!(
                    "Wayland global shortcuts unavailable: {error:#}"
                ))),
                None,
            ),
        }
    } else {
        let backend = match display
            .as_deref()
            .context("Original X11 authority is unavailable")
            .and_then(|display| {
                let cancellation = state.clone();
                x11::Registrar::open_with_control(
                    display,
                    wake.try_clone()?,
                    Arc::new(move || cancellation.stop.load(Ordering::Acquire)),
                )
            }) {
            Ok(manager) => Backend::X11(crate::hotkey::CaptureHotkey::with_manager(manager)),
            Err(error) => Backend::Unavailable(bounded_status(&format!(
                "X11 global shortcuts unavailable: {error:#}"
            ))),
        };
        (backend, None)
    };
    let run_result = (|| {
        tray.refresh(&connection, &events);
        process::ready()?;
        let mut resume_pending = true;
        let mut configuration_pending = false;
        let mut deferred_reply: Option<(u64, UnixShortcutReport)> = None;
        while !state.stop.load(Ordering::Acquire) {
            process::pump()?;
            if process::take_retire() {
                resume_pending = true;
                if let Some(signals) = &signals {
                    signals
                        .lock()
                        .unwrap()
                        .retain(|event| !matches!(event, portal::Signal::Key(_, _, _)));
                }
            }
            let mut bytes = [0; 64];
            for _ in 0..64 {
                match wake.read(&mut bytes) {
                    Ok(0) => return Ok(()),
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error.into()),
                }
            }
            // Cap a bus batch; process controls and cancellation even under native event floods.
            let mut full_batch = true;
            for _ in 0..64 {
                if !connection.process(Duration::ZERO)? {
                    full_batch = false;
                    break;
                }
            }
            if state.stop.load(Ordering::Acquire) {
                break;
            }
            tray.refresh(&connection, &events);
            state
                .tray_available
                .store(tray.available(), Ordering::Release);
            if state.repeat_changed.swap(false, Ordering::AcqRel) {
                tray.repeat_changed(&connection);
            }
            if let Some(signals) = &signals {
                let pending: Vec<_> = signals.lock().unwrap().drain(..).collect();
                for signal in pending {
                    let updates_status = matches!(
                        &signal,
                        portal::Signal::Closed(_)
                            | portal::Signal::Changed(_, _)
                            | portal::Signal::OwnerGone
                            | portal::Signal::Overflow
                    );
                    if let Backend::Portal(keys) = &mut backend {
                        match keys.signal(signal) {
                            Ok(Some(command)) => deliver(
                                &events,
                                UnixDesktopEvent::Action(UnixDesktopAction::Capture(command)),
                            ),
                            Ok(None) if updates_status => {
                                *state.shortcut_status.lock().unwrap() = keys.status().into();
                                deliver(
                                    &events,
                                    UnixDesktopEvent::ShortcutStatus(keys.status().into()),
                                );
                            }
                            Ok(None) => {}
                            Err(error) => {
                                let status = bounded_status(&format!(
                                    "Global shortcuts unavailable: {error:#}"
                                ));
                                *state.shortcut_status.lock().unwrap() = status.clone();
                                let _ = backend.close(&connection);
                                backend = Backend::Unavailable(status.clone());
                                deliver(&events, UnixDesktopEvent::ShortcutStatus(status));
                            }
                        }
                    }
                }
            }
            let mut handled = false;
            if let Some((generation, input_epoch, change)) = process::next_change() {
                ensure!(
                    deferred_reply.is_none(),
                    "Desktop readiness reply is already pending"
                );
                handled = true;
                state.generation.store(generation, Ordering::Release);
                state.input_epoch.fetch_max(input_epoch, Ordering::AcqRel);
                state.allow_capture.store(false, Ordering::Release);
                if let Some(signals) = &signals {
                    signals
                        .lock()
                        .unwrap()
                        .retain(|event| !matches!(event, portal::Signal::Key(_, _, _)));
                }
                let prepared = matches!(change, Change::Prepare(_));
                let mut report = backend.change(&connection, change, &state, wake.as_raw_fd());
                configuration_pending = prepared && report.outcome.is_ok();
                resume_pending = !configuration_pending;
                if matches!(&backend, Backend::X11(keys) if keys.manager().poisoned()) {
                    let status = "X11 native connection or registration cleanup failed; all shortcuts released, restart required".to_owned();
                    backend = Backend::Unavailable(status.clone());
                    report = UnixShortcutReport::unavailable(&status);
                }
                *state.shortcut_status.lock().unwrap() = report.status.clone();
                if configuration_pending {
                    process::reply(generation, report)?;
                } else {
                    ensure!(
                        deferred_reply.is_none(),
                        "Desktop readiness reply is already pending"
                    );
                    deferred_reply = Some((generation, report));
                }
            }
            if let Backend::X11(keys) = &backend {
                match keys.manager().drain() {
                    Ok((pressed, full)) => {
                        full_batch |= full;
                        for id in pressed {
                            if !state.allow_capture.load(Ordering::Acquire) {
                                continue;
                            }
                            if let Some(command) = keys.command(global_hotkey::GlobalHotKeyEvent {
                                id,
                                state: global_hotkey::HotKeyState::Pressed,
                            }) {
                                deliver(
                                    &events,
                                    UnixDesktopEvent::Action(UnixDesktopAction::Capture(command)),
                                );
                            }
                        }
                    }
                    Err(error) => {
                        let status =
                            bounded_status(&format!("X11 global shortcuts unavailable: {error:#}"));
                        *state.shortcut_status.lock().unwrap() = status.clone();
                        backend = Backend::Unavailable(status.clone()); // Closing this connection releases all grabs.
                        deliver(&events, UnixDesktopEvent::ShortcutStatus(status));
                    }
                }
            }
            if state.stop.load(Ordering::Acquire) {
                break;
            }
            // A control already consumed here must be dispatched before an idle wait or readiness ACK.
            if process::pump()? {
                continue;
            }
            let quiet = !handled && !full_batch;
            if resume_pending
                && !configuration_pending
                && !state.quitting.load(Ordering::Acquire)
                && quiet
            {
                state.allow_capture.store(true, Ordering::Release);
                resume_pending = false;
            }
            if quiet && !configuration_pending {
                if let Some((generation, mut report)) = deferred_reply.take() {
                    if let Backend::Unavailable(status) = &backend {
                        report = UnixShortcutReport::unavailable(status);
                    } else {
                        report.status = state.shortcut_status.lock().unwrap().clone();
                    }
                    // Held keys remain tracked; no physical release is required to settle readiness.
                    process::reply(generation, report)?;
                }
            }
            if handled || full_batch || state.repeat_changed.load(Ordering::Acquire) {
                continue;
            }
            let x11_fd = match &backend {
                Backend::X11(keys) => Some(keys.manager().fd()),
                _ => None,
            };
            wait(&connection, x11_fd, wake.as_raw_fd(), None)?;
        }
        Ok(())
    })();
    // Native drops happen on this worker; closing the private bus also ends its portal sessions.
    let cleanup = backend.close(&connection);
    drop(backend);
    drop(connection);
    run_result.and(cleanup)
}

pub(super) fn wait(
    connection: &Connection,
    x11: Option<RawFd>,
    wake: RawFd,
    deadline: Option<Instant>,
) -> Result<()> {
    if process::pump()? {
        return Ok(());
    }
    let watch = connection.channel().watch();
    let mut fds = [
        libc::pollfd {
            fd: watch.fd,
            events: (if watch.read { libc::POLLIN } else { 0 })
                | (if watch.write { libc::POLLOUT } else { 0 }),
            revents: 0,
        },
        libc::pollfd {
            fd: wake,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: x11.unwrap_or(-1),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: process::child_fd().unwrap_or(-1),
            events: libc::POLLIN
                | if process::wants_write() {
                    libc::POLLOUT
                } else {
                    0
                },
            revents: 0,
        },
    ];
    let timeout = deadline
        .map(|d| {
            d.saturating_duration_since(Instant::now())
                .as_millis()
                .min(i32::MAX as u128) as i32
        })
        .unwrap_or(-1);
    let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout) };
    if ready < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted {
        anyhow::bail!("Native desktop event wait failed");
    }
    ensure!(
        fds[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) == 0,
        "Native desktop session bus disconnected"
    );
    if fds[1].revents & libc::POLLIN != 0 {
        let mut bytes = [0u8; 64];
        loop {
            let count = unsafe { libc::read(wake, bytes.as_mut_ptr().cast(), bytes.len()) };
            if count <= 0 {
                break;
            }
        }
    }
    ensure!(
        fds[2].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) == 0,
        "Native X11 shortcut connection disconnected"
    );
    process::pump()?;
    Ok(())
}

fn failure_status(state: &Shared, events: &mpsc::Sender<UnixDesktopEvent>, error: &anyhow::Error) {
    let shortcuts = bounded_status(&format!("Global shortcuts unavailable: {error:#}"));
    let tray = bounded_status(&format!("Native services stopped: {error:#}; Close quits"));
    *state.shortcut_status.lock().unwrap() = shortcuts.clone();
    *state.tray_status.lock().unwrap() = tray.clone();
    deliver(
        events,
        UnixDesktopEvent::TrayStatus {
            available: false,
            status: tray,
        },
    );
    deliver(events, UnixDesktopEvent::ShortcutStatus(shortcuts));
}
#[cfg(test)]
mod sender_tests {
    #[test]
    fn owner_changes_require_the_actual_bus_daemon_sender() {
        let rule = super::daemon_owner_rule();
        let mut message = dbus::Message::new_signal(
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameOwnerChanged",
        )
        .unwrap();
        message.set_sender(Some(":7.42".into()));
        assert!(!rule.matches(&message));
        message.set_sender(Some("org.freedesktop.DBus".into()));
        assert!(rule.matches(&message));
    }
    #[test]
    fn terminal_text_remains_authoritative_with_normal_or_full_event_channel() {
        for full in [false, true] {
            let state = super::shared();
            *state.tray_status.lock().unwrap() = "Tray ready".into();
            let (events, mut receiver) = super::mpsc::channel(1);
            if full {
                events
                    .try_send(super::UnixDesktopEvent::ShortcutStatus("occupied".into()))
                    .unwrap();
            }
            super::failure_status(&state, &events, &anyhow::anyhow!("owned fixture failure"));
            assert!(
                state
                    .tray_status
                    .lock()
                    .unwrap()
                    .contains("Native services stopped")
            );
            assert!(
                state
                    .shortcut_status
                    .lock()
                    .unwrap()
                    .contains("owned fixture failure")
            );
            if !full {
                match receiver.try_recv().unwrap() {
                    super::UnixDesktopEvent::TrayStatus { status, available } => {
                        assert!(!available);
                        assert_eq!(status, *state.tray_status.lock().unwrap());
                    }
                    _ => panic!("Expected terminal tray report"),
                }
            }
        }
    }
    fn service_without_native_worker() -> (
        super::Services,
        super::mpsc::Receiver<super::Request>,
        std::os::unix::net::UnixStream,
    ) {
        let (read, write) = std::os::unix::net::UnixStream::pair().unwrap();
        let (sender, receiver) = super::mpsc::channel(1);
        (
            super::Services {
                sender,
                wake: write,
                shared: super::shared(),
                worker: None,
            },
            receiver,
            read,
        )
    }
    #[test]
    fn rejected_request_preserves_capture_gate_and_epochs() {
        use std::sync::atomic::Ordering;
        for allowed in [false, true] {
            let (service, receiver, _read) = service_without_native_worker();
            service
                .shared
                .allow_capture
                .store(allowed, Ordering::Release);
            let permit = service.sender.try_reserve().unwrap();
            let _rejected = service.request(super::Change::Finish(false));
            assert_eq!(
                service.shared.allow_capture.load(Ordering::Acquire),
                allowed
            );
            assert_eq!(service.shared.generation.load(Ordering::Acquire), 0);
            assert_eq!(service.shared.input_epoch.load(Ordering::Acquire), 0);
            drop(permit);
            drop(receiver);
            let _closed = service.request(super::Change::Finish(false));
            assert_eq!(
                service.shared.allow_capture.load(Ordering::Acquire),
                allowed
            );
            assert_eq!(service.shared.generation.load(Ordering::Acquire), 0);
            assert_eq!(service.shared.input_epoch.load(Ordering::Acquire), 0);
        }
    }
    #[test]
    fn completion_before_publisher_returns_cannot_reclose_the_capture_gate() {
        use std::sync::atomic::Ordering;
        let (service, mut receiver, _read) = service_without_native_worker();
        service.shared.allow_capture.store(true, Ordering::Release);
        let state = service.shared.clone();
        let (done, completed) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let request = receiver.blocking_recv().unwrap();
            assert!(
                !state.allow_capture.load(Ordering::Acquire),
                "Gate must be closed before work is observable"
            );
            state.allow_capture.store(true, Ordering::Release);
            assert!(
                request
                    .reply
                    .send(super::UnixShortcutReport {
                        status: "ready".into(),
                        outcome: Ok(())
                    })
                    .is_ok()
            );
            done.send(()).unwrap();
        });
        super::AFTER_REQUEST_PUBLISHED.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                completed
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap()
            }))
        });
        let mut operation = service.request(super::Change::Finish(false));
        assert!(service.shared.allow_capture.load(Ordering::Acquire));
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert!(
            matches!(operation.as_mut().poll(&mut context), std::task::Poll::Ready(report) if report.outcome.is_ok())
        );
        worker.join().unwrap();
    }
}
