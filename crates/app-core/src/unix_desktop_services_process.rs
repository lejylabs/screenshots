//! One eligible helper owns native services. Metadata only, never pixels.
use super::*;
use serde::{Deserialize, Serialize};
use std::{cell::RefCell, collections::VecDeque, io, os::fd::AsRawFd, sync::OnceLock};

const MAGIC: &[u8; 8] = b"SCDESK01";
const MAX: usize = 64 * 1024;
const HEADER: usize = 24;
const STARTUP: Duration = Duration::from_secs(5);
const CLEANUP: Duration = Duration::from_millis(250);

pub trait LinuxDesktopProcessOwner: Send {
    /// Retain the waitable leader through the final group signal, then reap its owned family.
    fn terminate_and_reap(&mut self) -> Result<()>;
}
pub struct LinuxDesktopProcess {
    pub socket: UnixStream,
    pub nonce: [u8; 16],
    pub owner: Box<dyn LinuxDesktopProcessOwner>,
}
type Spawner = fn() -> Result<LinuxDesktopProcess>;
static SPAWNER: OnceLock<Spawner> = OnceLock::new();
pub fn install_linux_desktop_spawner(spawner: Spawner) -> Result<()> {
    SPAWNER
        .set(spawner)
        .map_err(|_| anyhow::anyhow!("Desktop service spawner is already installed"))
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    available: bool,
    tray: String,
    shortcuts: String,
    generation: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Packet {
    Start {
        wayland: bool,
        display: Option<String>,
    },
    TaskStart {
        bindings: Vec<(String, String)>,
    },
    TaskAction {
        id: String,
    },
    BusReady {},
    Ready {
        snapshot: Snapshot,
    },
    Change {
        generation: u64,
        input_epoch: u64,
        change: Change,
    },
    Reply {
        generation: u64,
        report: UnixShortcutReport,
        snapshot: Snapshot,
    },
    Event {
        event: UnixDesktopEvent,
        snapshot: Snapshot,
    },
    Snapshot {
        snapshot: Snapshot,
    },
    Repeat {
        available: bool,
    },
    SetQuitting {
        input_epoch: u64,
        quitting: bool,
    },
    Shutdown {},
}
fn encode(packet: &Packet, nonce: &[u8; 16]) -> Result<Vec<u8>> {
    let data = serde_json::to_vec(packet)?;
    ensure!(
        data.len() <= MAX,
        "Private desktop metadata exceeds its limit"
    );
    let mut bytes = Vec::with_capacity(HEADER + data.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(nonce);
    bytes.extend_from_slice(&data);
    Ok(bytes)
}
fn send(socket: &UnixStream, packet: &Packet, nonce: &[u8; 16]) -> Result<bool> {
    let bytes = encode(packet, nonce)?;
    for _ in 0..64 {
        let count = unsafe {
            libc::send(
                socket.as_raw_fd(),
                bytes.as_ptr().cast(),
                bytes.len(),
                libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
            )
        };
        if count >= 0 {
            ensure!(
                count as usize == bytes.len(),
                "Private desktop packet was truncated"
            );
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::WouldBlock {
            return Ok(false);
        }
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error.into());
        }
    }
    anyhow::bail!("Private desktop packet send was interrupted repeatedly")
}
fn receive(socket: &UnixStream, nonce: &[u8; 16]) -> Result<Option<Packet>> {
    let mut bytes = vec![0u8; HEADER + MAX + 1];
    let mut received = None;
    for _ in 0..64 {
        let count = unsafe {
            libc::recv(
                socket.as_raw_fd(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
                libc::MSG_DONTWAIT,
            )
        };
        if count >= 0 {
            received = Some(count as usize);
            break;
        }
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::WouldBlock {
            return Ok(None);
        }
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error.into());
        }
    }
    let count = received.context("Private desktop packet receive was interrupted repeatedly")?;
    ensure!(count != 0, "Desktop service helper closed its channel");
    ensure!(
        (HEADER + 2..=HEADER + MAX).contains(&count)
            && &bytes[..8] == MAGIC
            && &bytes[8..HEADER] == nonce,
        "Invalid private desktop packet"
    );
    serde_json::from_slice(&bytes[HEADER..count])
        .map(Some)
        .map_err(|_| anyhow::anyhow!("Invalid private desktop metadata"))
}
fn poll(
    socket: &UnixStream,
    control: Option<&UnixStream>,
    writing: bool,
    deadline: Option<Instant>,
) -> Result<()> {
    let timeout = deadline
        .map(|time| {
            time.saturating_duration_since(Instant::now())
                .as_millis()
                .min(i32::MAX as u128) as i32
        })
        .unwrap_or(-1);
    let mut fds = [
        libc::pollfd {
            fd: socket.as_raw_fd(),
            events: libc::POLLIN | if writing { libc::POLLOUT } else { 0 },
            revents: 0,
        },
        libc::pollfd {
            fd: control.map(AsRawFd::as_raw_fd).unwrap_or(-1),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout) };
    if result < 0 && io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}
fn snapshot(state: &Shared) -> Snapshot {
    Snapshot {
        available: state.tray_available.load(Ordering::Acquire),
        tray: state.tray_status.lock().unwrap().clone(),
        shortcuts: state.shortcut_status.lock().unwrap().clone(),
        generation: state.generation.load(Ordering::Acquire),
    }
}
fn observe(value: Snapshot, state: &Shared, events: &mpsc::Sender<UnixDesktopEvent>) -> Result<()> {
    ensure!(
        value.generation <= state.generation.load(Ordering::Acquire)
            && value.tray.len() <= 4096
            && value.shortcuts.len() <= 4096,
        "Invalid desktop helper snapshot"
    );
    let changed = state.tray_available.swap(value.available, Ordering::AcqRel) != value.available
        || *state.tray_status.lock().unwrap() != value.tray;
    *state.tray_status.lock().unwrap() = value.tray.clone();
    if changed {
        deliver(
            events,
            UnixDesktopEvent::TrayStatus {
                available: value.available,
                status: value.tray,
            },
        );
    }
    if *state.shortcut_status.lock().unwrap() != value.shortcuts {
        *state.shortcut_status.lock().unwrap() = value.shortcuts.clone();
        deliver(events, UnixDesktopEvent::ShortcutStatus(value.shortcuts));
    }
    Ok(())
}
pub(super) fn supervise(
    mut control: UnixStream,
    mut requests: mpsc::Receiver<Request>,
    events: mpsc::Sender<UnixDesktopEvent>,
    state: &Arc<Shared>,
    wayland: bool,
    display: Option<String>,
) -> Result<()> {
    let mut child = SPAWNER
        .get()
        .context("Desktop service process spawner is not installed")?()?;
    child.socket.set_nonblocking(true)?;
    let result = (|| {
        let mut outgoing = Some(Packet::Start { wayland, display });
        let mut active: Option<Request> = None;
        let mut ready = false;
        let mut bus_ready = false;
        let mut deadline = Some(Instant::now() + STARTUP);
        loop {
            if state.stop.load(Ordering::Acquire) {
                return Ok(());
            }
            ensure!(
                !deadline.is_some_and(|d| Instant::now() >= d),
                "Desktop service helper operation timed out"
            );
            let mut discard = [0u8; 64];
            for _ in 0..64 {
                match control.read(&mut discard) {
                    Ok(0) => return Ok(()),
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(error.into()),
                }
            }
            if let Some(packet) = outgoing.as_ref() {
                if send(&child.socket, packet, &child.nonce)? {
                    outgoing = None;
                }
            }
            let mut full = true;
            for _ in 0..64 {
                let Some(packet) = receive(&child.socket, &child.nonce)? else {
                    full = false;
                    break;
                };
                match packet {
                    Packet::BusReady {} if !bus_ready && !ready => {
                        bus_ready = true;
                        deadline = Some(Instant::now() + STARTUP);
                    }
                    Packet::Ready { snapshot } if !ready && bus_ready => {
                        observe(snapshot, state, &events)?;
                        ready = true;
                        deadline = None;
                    }
                    Packet::Snapshot { snapshot } => observe(snapshot, state, &events)?,
                    Packet::Event { event, snapshot } => {
                        observe(snapshot, state, &events)?;
                        deliver(&events, event);
                    }
                    Packet::Reply {
                        generation,
                        report,
                        snapshot,
                    } => {
                        let request = active.take().context("Unexpected desktop helper reply")?;
                        ensure!(
                            generation == request.generation
                                && report.status.len() <= 4096
                                && report
                                    .outcome
                                    .as_ref()
                                    .err()
                                    .is_none_or(|s| s.len() <= 4096),
                            "Invalid desktop helper reply"
                        );
                        observe(snapshot, state, &events)?;
                        state.allow_capture.store(
                            !matches!(request.change, Change::Prepare(_))
                                || report.outcome.is_err(),
                            Ordering::Release,
                        );
                        *state.shortcut_status.lock().unwrap() = report.status.clone();
                        let _ = request.reply.send(report);
                        deadline = None;
                    }
                    _ => anyhow::bail!("Unexpected desktop helper packet"),
                }
            }
            if ready && outgoing.is_none() && state.policy_changed.swap(false, Ordering::AcqRel) {
                outgoing = Some(Packet::SetQuitting {
                    input_epoch: state.input_epoch.load(Ordering::Acquire),
                    quitting: state.quitting.load(Ordering::Acquire),
                });
            }
            if ready && outgoing.is_none() {
                if active.is_none() {
                    if let Ok(request) = requests.try_recv() {
                        deadline = Some(
                            Instant::now()
                                + if wayland {
                                    Duration::from_secs(185)
                                } else {
                                    Duration::from_secs(8)
                                },
                        );
                        outgoing = Some(Packet::Change {
                            generation: request.generation,
                            input_epoch: request.input_epoch,
                            change: request.change.clone(),
                        });
                        active = Some(request);
                    }
                }
                if outgoing.is_none() && state.repeat_changed.swap(false, Ordering::AcqRel) {
                    outgoing = Some(Packet::Repeat {
                        available: state.repeat.load(Ordering::Acquire),
                    });
                }
            }
            if full {
                continue;
            }
            poll(&child.socket, Some(&control), outgoing.is_some(), deadline)?;
        }
    })();
    state.allow_capture.store(false, Ordering::Release);
    state.tray_available.store(false, Ordering::Release);
    let _ = send(&child.socket, &Packet::Shutdown {}, &child.nonce);
    let until = Instant::now() + CLEANUP;
    while Instant::now() < until {
        match receive(&child.socket, &child.nonce) {
            Err(_) => break,
            Ok(Some(_)) => continue,
            Ok(None) => {
                if poll(&child.socket, None, false, Some(until)).is_err() {
                    break;
                }
            }
        }
    }
    let cleanup = child.owner.terminate_and_reap();
    result.and(cleanup)
}

struct ChildContext {
    socket: UnixStream,
    nonce: [u8; 16],
    state: Arc<Shared>,
    events: VecDeque<UnixDesktopEvent>,
    tasks: VecDeque<String>,
    dirty: bool,
    pending: Option<(u64, u64, Change)>,
    retire: bool,
    failed: bool,
}
thread_local! { static CHILD: RefCell<Option<ChildContext>> = const { RefCell::new(None) }; }
pub(super) fn child_fd() -> Option<RawFd> {
    CHILD.with(|c| c.borrow().as_ref().map(|c| c.socket.as_raw_fd()))
}
pub(super) fn wants_write() -> bool {
    CHILD.with(|c| {
        c.borrow()
            .as_ref()
            .is_some_and(|c| c.dirty || !c.events.is_empty() || !c.tasks.is_empty())
    })
}
fn capture_action(action: &UnixDesktopAction) -> bool {
    !matches!(
        action,
        UnixDesktopAction::Tray(TrayCommand::Show | TrayCommand::Quit)
    )
}
pub(crate) fn emit(event: UnixDesktopEvent) -> std::result::Result<(), UnixDesktopEvent> {
    CHILD.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(context) = slot.as_mut() else {
            return Err(event);
        };
        match event {
            UnixDesktopEvent::TrayStatus { available, status } => {
                context
                    .state
                    .tray_available
                    .store(available, Ordering::Release);
                *context.state.tray_status.lock().unwrap() = bounded_status(&status);
                context.dirty = true;
            }
            UnixDesktopEvent::ShortcutStatus(status) => {
                *context.state.shortcut_status.lock().unwrap() = bounded_status(&status);
                context.dirty = true;
            }
            UnixDesktopEvent::Action(action) => {
                if (!capture_action(&action)
                    || (context.state.allow_capture.load(Ordering::Acquire)
                        && !context.state.quitting.load(Ordering::Acquire)))
                    && context.events.len() + context.tasks.len() < 32
                {
                    context.events.push_back(UnixDesktopEvent::Stamped {
                        action,
                        generation: context.state.input_epoch.load(Ordering::Acquire),
                    });
                }
            }
            event => {
                if context.events.len() + context.tasks.len() < 32 {
                    context.events.push_back(event);
                }
            }
        }
        if flush(context).is_err() {
            context.failed = true;
            context.state.stop.store(true, Ordering::Release);
        }
        Ok(())
    })
}
fn flush(context: &mut ChildContext) -> Result<()> {
    if context.dirty {
        if !send(
            &context.socket,
            &Packet::Snapshot {
                snapshot: snapshot(&context.state),
            },
            &context.nonce,
        )? {
            return Ok(());
        }
        context.dirty = false;
    }
    for _ in 0..32 {
        let Some(event) = context.events.pop_front() else {
            break;
        };
        let packet = Packet::Event {
            event,
            snapshot: snapshot(&context.state),
        };
        if !send(&context.socket, &packet, &context.nonce)? {
            if let Packet::Event { event, .. } = packet {
                context.events.push_front(event);
            }
            break;
        }
    }
    for _ in 0..32 {
        let Some(id) = context.tasks.pop_front() else {
            break;
        };
        if !send(
            &context.socket,
            &Packet::TaskAction { id: id.clone() },
            &context.nonce,
        )? {
            context.tasks.push_front(id);
            break;
        }
    }
    Ok(())
}
pub(super) fn task_event(id: String) {
    CHILD.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(context) = slot.as_mut() else {
            return;
        };
        if context.state.allow_capture.load(Ordering::Acquire)
            && context.events.len() + context.tasks.len() < 32
        {
            context.tasks.push_back(id);
        }
        if flush(context).is_err() {
            context.state.stop.store(true, Ordering::Release);
            context.failed = true;
        }
    });
}
pub(super) fn pump() -> Result<bool> {
    CHILD.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(context) = slot.as_mut() else {
            return Ok(false);
        };
        ensure!(!context.failed, "Desktop helper IPC failed");
        flush(context)?;
        let mut consumed = false;
        for _ in 0..64 {
            let Some(packet) = receive(&context.socket, &context.nonce)? else {
                break;
            };
            consumed = true;
            match packet {
                Packet::Change {
                    generation,
                    input_epoch,
                    change,
                } => {
                    ensure!(
                        context.pending.is_none()
                            && generation > context.state.generation.load(Ordering::Acquire),
                        "Desktop helper configuration queue is full or stale"
                    );
                    context.pending = Some((generation, input_epoch, change));
                }
                Packet::SetQuitting {
                    input_epoch,
                    quitting,
                } => {
                    context
                        .state
                        .input_epoch
                        .fetch_max(input_epoch, Ordering::AcqRel);
                    context.state.quitting.store(quitting, Ordering::Release);
                    context.state.allow_capture.store(false, Ordering::Release);
                    context.retire = true;
                    context.events.retain(|event| {
                        matches!(
                            event,
                            UnixDesktopEvent::Stamped {
                                action: UnixDesktopAction::Tray(
                                    TrayCommand::Show | TrayCommand::Quit
                                ),
                                ..
                            }
                        )
                    });
                }
                Packet::Repeat { available } => {
                    context.state.repeat.store(available, Ordering::Release);
                    context.state.repeat_changed.store(true, Ordering::Release);
                }
                Packet::Shutdown {} => context.state.stop.store(true, Ordering::Release),
                _ => anyhow::bail!("Unexpected desktop parent packet"),
            }
        }
        Ok(consumed)
    })
}
pub(super) fn next_change() -> Option<(u64, u64, Change)> {
    CHILD.with(|c| c.borrow_mut().as_mut().and_then(|c| c.pending.take()))
}
pub(super) fn take_retire() -> bool {
    CHILD.with(|c| {
        c.borrow_mut()
            .as_mut()
            .is_some_and(|c| std::mem::take(&mut c.retire))
    })
}
fn required(packet: impl FnOnce(&Shared) -> Packet) -> Result<()> {
    let (packet, nonce) = CHILD.with(|c| {
        let c = c.borrow();
        let c = c.as_ref().unwrap();
        (packet(&c.state), c.nonce)
    });
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        let sent = CHILD.with(|c| {
            let c = c.borrow();
            let c = c.as_ref().unwrap();
            send(&c.socket, &packet, &nonce)
        })?;
        if sent {
            return Ok(());
        }
        ensure!(
            Instant::now() < until,
            "Desktop helper reply channel is blocked"
        );
        pump()?;
        CHILD.with(|c| {
            let c = c.borrow();
            let c = c.as_ref().unwrap();
            poll(&c.socket, None, true, Some(until))
        })?;
    }
}
pub(super) fn bus_ready() -> Result<()> {
    required(|_| Packet::BusReady {})
}
pub(super) fn ready() -> Result<()> {
    required(|state| Packet::Ready {
        snapshot: snapshot(state),
    })
}
pub(super) fn reply(generation: u64, report: UnixShortcutReport) -> Result<()> {
    required(|state| Packet::Reply {
        generation,
        report,
        snapshot: snapshot(state),
    })
}
pub fn serve_linux_desktop_child(socket: UnixStream, nonce: [u8; 16]) -> Result<()> {
    socket.set_nonblocking(true)?;
    let until = Instant::now() + STARTUP;
    let start = loop {
        if let Some(packet) = receive(&socket, &nonce)? {
            break packet;
        }
        ensure!(Instant::now() < until, "Desktop helper bootstrap timed out");
        poll(&socket, None, false, Some(until))?;
    };
    match &start {
        Packet::Start { display, .. } => ensure!(
            display.as_ref().is_none_or(|s| s.len() <= 65),
            "Desktop display authority exceeds its limit"
        ),
        Packet::TaskStart { bindings } => validate_task_bindings(bindings)?,
        _ => anyhow::bail!("Invalid desktop helper bootstrap"),
    }
    let address =
        std::env::var("DBUS_SESSION_BUS_ADDRESS").context("A local session bus is required")?;
    ensure!(
        address.len() <= 4096 && address.starts_with("unix:") && !address.contains(';'),
        "Native services require one explicit local session bus"
    );
    let state = super::shared();
    state.allow_capture.store(false, Ordering::Release);
    CHILD.with(|slot| {
        *slot.borrow_mut() = Some(ChildContext {
            socket,
            nonce,
            state: state.clone(),
            events: VecDeque::new(),
            tasks: VecDeque::new(),
            dirty: true,
            pending: None,
            retire: false,
            failed: false,
        });
    });
    let result = match start {
        Packet::Start { wayland, display } => super::run_native(state, wayland, display),
        Packet::TaskStart { bindings } => super::task_portal::run(state, bindings),
        _ => unreachable!(),
    };
    CHILD.with(|slot| {
        slot.borrow_mut().take();
    });
    result
}

fn validate_task_bindings(bindings: &[(String, String)]) -> Result<()> {
    ensure!(
        (1..=16).contains(&bindings.len()),
        "Task portal requires 1–16 bindings"
    );
    let mut ids = std::collections::HashSet::new();
    let mut keys = std::collections::HashSet::new();
    for (id, shortcut) in bindings {
        ensure!(
            !id.is_empty()
                && id.len() <= 64
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')),
            "Invalid task portal ID"
        );
        let key = CaptureHotkeys([
            shortcut.clone(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
        ])
        .validated()?
        .0[0]
            .clone();
        ensure!(
            !key.is_empty() && ids.insert(id) && keys.insert(key),
            "Duplicate or disabled task portal binding"
        );
    }
    Ok(())
}
pub struct LinuxTaskPortal {
    child: LinuxDesktopProcess,
    ids: std::collections::HashSet<String>,
}
impl LinuxTaskPortal {
    pub fn start(
        bindings: Vec<(String, String)>,
        control: &UnixStream,
        canceled: &dyn Fn() -> bool,
    ) -> Result<Self> {
        validate_task_bindings(&bindings)?;
        ensure!(!canceled(), "Task portal startup canceled");
        let child = SPAWNER
            .get()
            .context("Native service spawner is not installed")?()?;
        child.socket.set_nonblocking(true)?;
        let ids = bindings.iter().map(|(id, _)| id.clone()).collect();
        let mut value = Self { child, ids };
        let result = (|| {
            let packet = Packet::TaskStart { bindings };
            let mut deadline = Instant::now() + STARTUP;
            let mut sent = false;
            let mut bus_ready = false;
            loop {
                ensure!(!canceled(), "Task portal startup canceled");
                ensure!(Instant::now() < deadline, "Task portal startup timed out");
                if !sent {
                    sent = send(&value.child.socket, &packet, &value.child.nonce)?;
                }
                for _ in 0..64 {
                    match receive(&value.child.socket, &value.child.nonce)? {
                        Some(Packet::BusReady {}) if !bus_ready => {
                            bus_ready = true;
                            deadline = Instant::now() + Duration::from_secs(185);
                        }
                        Some(Packet::Ready { .. }) if bus_ready => return Ok(()),
                        Some(Packet::Snapshot { .. }) | Some(Packet::TaskAction { .. }) => {}
                        Some(_) => anyhow::bail!("Invalid task portal startup packet"),
                        None => break,
                    }
                }
                poll(&value.child.socket, Some(control), !sent, Some(deadline))?;
            }
        })();
        if let Err(error) = result {
            let _ = value.child.owner.terminate_and_reap();
            return Err(error);
        }
        Ok(value)
    }
    pub fn fd(&self) -> RawFd {
        self.child.socket.as_raw_fd()
    }
    pub fn take_activations(&mut self) -> Result<(Vec<String>, bool)> {
        let mut events = Vec::new();
        let mut full = true;
        for _ in 0..64 {
            match receive(&self.child.socket, &self.child.nonce)? {
                Some(Packet::TaskAction { id }) => {
                    ensure!(self.ids.contains(&id), "Unknown task portal activation");
                    events.push(id);
                }
                Some(Packet::Snapshot { .. }) => {}
                Some(_) => anyhow::bail!("Invalid task portal packet"),
                None => {
                    full = false;
                    break;
                }
            }
        }
        Ok((events, full))
    }
}
impl Drop for LinuxTaskPortal {
    fn drop(&mut self) {
        let _ = send(&self.child.socket, &Packet::Shutdown {}, &self.child.nonce);
        let _ = self.child.owner.terminate_and_reap();
    }
}

#[cfg(test)]
mod protocol_tests {
    use super::*;
    use std::os::fd::FromRawFd;
    fn pair() -> (UnixStream, UnixStream) {
        let mut fds = [-1; 2];
        assert_eq!(
            unsafe {
                libc::socketpair(
                    libc::AF_UNIX,
                    libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                    0,
                    fds.as_mut_ptr(),
                )
            },
            0
        );
        let pair = unsafe {
            (
                UnixStream::from_raw_fd(fds[0]),
                UnixStream::from_raw_fd(fds[1]),
            )
        };
        pair.0.set_nonblocking(true).unwrap();
        pair.1.set_nonblocking(true).unwrap();
        pair
    }
    #[test]
    fn private_packets_reject_wrong_nonce_and_unknown_fields() {
        let (first, second) = pair();
        let nonce = [17; 16];
        assert!(send(&first, &Packet::Repeat { available: true }, &nonce).unwrap());
        assert!(receive(&second, &[18; 16]).is_err());
        let mut malformed = MAGIC.to_vec();
        malformed.extend_from_slice(&nonce);
        malformed.extend_from_slice(br#"{"kind":"shutdown","unexpected":true}"#);
        assert_eq!(
            unsafe {
                libc::send(
                    first.as_raw_fd(),
                    malformed.as_ptr().cast(),
                    malformed.len(),
                    libc::MSG_NOSIGNAL,
                )
            },
            malformed.len() as isize
        );
        assert!(receive(&second, &nonce).is_err());
    }
    #[test]
    fn every_packet_variant_refuses_extra_fields_and_unit_wire_shape_is_unchanged() {
        let snapshot = || Snapshot {
            available: false,
            tray: "fixture".into(),
            shortcuts: "fixture".into(),
            generation: 1,
        };
        let packets = [
            Packet::Start {
                wayland: true,
                display: None,
            },
            Packet::TaskStart {
                bindings: Vec::new(),
            },
            Packet::TaskAction {
                id: "fixture".into(),
            },
            Packet::BusReady {},
            Packet::Ready {
                snapshot: snapshot(),
            },
            Packet::Change {
                generation: 1,
                input_epoch: 2,
                change: Change::Finish(false),
            },
            Packet::Reply {
                generation: 1,
                report: UnixShortcutReport::unavailable("fixture"),
                snapshot: snapshot(),
            },
            Packet::Event {
                event: UnixDesktopEvent::TrayStatus {
                    available: false,
                    status: "fixture".into(),
                },
                snapshot: snapshot(),
            },
            Packet::Snapshot {
                snapshot: snapshot(),
            },
            Packet::Repeat { available: false },
            Packet::SetQuitting {
                input_epoch: 2,
                quitting: true,
            },
            Packet::Shutdown {},
        ];
        for packet in packets {
            let mut value = serde_json::to_value(&packet).unwrap();
            assert!(serde_json::from_value::<Packet>(value.clone()).is_ok());
            value
                .as_object_mut()
                .unwrap()
                .insert("unexpected".into(), true.into());
            assert!(serde_json::from_value::<Packet>(value).is_err());
        }
        assert_eq!(
            serde_json::to_vec(&Packet::BusReady {}).unwrap(),
            br#"{"kind":"bus_ready"}"#
        );
        assert_eq!(
            serde_json::to_vec(&Packet::Shutdown {}).unwrap(),
            br#"{"kind":"shutdown"}"#
        );
        assert!(
            serde_json::from_str::<UnixShortcutReport>(
                r#"{"status":"fixture","outcome":{"Ok":null},"unexpected":true}"#
            )
            .is_err()
        );
        assert!(
            serde_json::from_str::<UnixDesktopEvent>(
                r#"{"TrayStatus":{"available":false,"status":"fixture","unexpected":true}}"#
            )
            .is_err()
        );
    }
    #[test]
    fn consumed_control_packets_request_dispatch_without_creating_a_sticky_idle_wake() {
        let (socket, parent) = pair();
        let nonce = [23; 16];
        let state = super::super::shared();
        CHILD.with(|slot| assert!(slot.borrow().is_none()));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                CHILD.with(|slot| *slot.borrow_mut() = None);
            }
        }
        let _reset = Reset;
        CHILD.with(|slot| {
            *slot.borrow_mut() = Some(ChildContext {
                socket,
                nonce,
                state: state.clone(),
                events: VecDeque::new(),
                tasks: VecDeque::new(),
                dirty: false,
                pending: None,
                retire: false,
                failed: false,
            })
        });
        assert!(
            send(
                &parent,
                &Packet::Change {
                    generation: 1,
                    input_epoch: 1,
                    change: Change::Finish(false)
                },
                &nonce
            )
            .unwrap()
        );
        assert!(pump().unwrap());
        assert!(matches!(next_change(), Some((1, 1, Change::Finish(false)))));
        assert!(!pump().unwrap());
        assert!(
            send(
                &parent,
                &Packet::SetQuitting {
                    input_epoch: 2,
                    quitting: true
                },
                &nonce
            )
            .unwrap()
        );
        assert!(pump().unwrap());
        assert!(state.quitting.load(Ordering::Acquire));
        assert!(
            !pump().unwrap(),
            "Retirement is not a permanent runnable wake"
        );
        assert!(take_retire());
        assert!(send(&parent, &Packet::Shutdown {}, &nonce).unwrap());
        assert!(pump().unwrap());
        assert!(state.stop.load(Ordering::Acquire));
        assert!(!pump().unwrap());
    }
    #[test]
    fn oversized_packet_is_rejected_as_one_record() {
        let (first, second) = pair();
        let data = vec![0u8; MAX + HEADER + 1];
        assert_eq!(
            unsafe {
                libc::send(
                    first.as_raw_fd(),
                    data.as_ptr().cast(),
                    data.len(),
                    libc::MSG_NOSIGNAL,
                )
            },
            data.len() as isize
        );
        assert!(receive(&second, &[0; 16]).is_err());
    }
    #[test]
    fn task_mode_rejects_collapsed_or_excessive_configuration() {
        assert!(
            validate_task_bindings(&[
                ("one".into(), "Ctrl+F9".into()),
                ("two".into(), "ctrl+f9".into())
            ])
            .is_err()
        );
        assert!(validate_task_bindings(&[("invalid id".into(), "F9".into())]).is_err());
        assert!(validate_task_bindings(&vec![("a".into(), "F9".into()); 17]).is_err());
    }
    #[test]
    fn canceled_quit_retires_both_pre_quit_and_while_quitting_captures() {
        let state = super::super::shared();
        let (write, _read) = pair();
        let (sender, _requests) = mpsc::channel(1);
        let mut services = super::super::Services {
            sender,
            wake: write,
            shared: state.clone(),
            worker: None,
        };
        services.set_quitting(true);
        let during = state.input_epoch.load(Ordering::Acquire);
        assert!(
            services
                .action(UnixDesktopEvent::Stamped {
                    action: UnixDesktopAction::Capture(CaptureCommand::Primary),
                    generation: during
                })
                .is_none()
        );
        services.set_quitting(false);
        assert!(
            services
                .action(UnixDesktopEvent::Stamped {
                    action: UnixDesktopAction::Capture(CaptureCommand::Primary),
                    generation: 0
                })
                .is_none()
        );
        assert!(
            services
                .action(UnixDesktopEvent::Stamped {
                    action: UnixDesktopAction::Capture(CaptureCommand::Primary),
                    generation: during
                })
                .is_none()
        );
        assert!(
            services
                .action(UnixDesktopEvent::Stamped {
                    action: UnixDesktopAction::Tray(TrayCommand::Quit),
                    generation: during
                })
                .is_none()
        );
        let fresh = state.input_epoch.load(Ordering::Acquire);
        assert!(matches!(
            services.action(UnixDesktopEvent::Stamped {
                action: UnixDesktopAction::Tray(TrayCommand::Quit),
                generation: fresh
            }),
            Some(UnixDesktopAction::Tray(TrayCommand::Quit))
        ));
        assert!(matches!(
            services.action(UnixDesktopEvent::Stamped {
                action: UnixDesktopAction::Capture(CaptureCommand::Primary),
                generation: fresh
            }),
            Some(UnixDesktopAction::Capture(CaptureCommand::Primary))
        ));
    }
    #[test]
    fn prior_capture_epoch_cannot_replay_after_settings_admission() {
        let state = super::super::shared();
        let (write, _read) = pair();
        let (sender, _requests) = mpsc::channel(1);
        let mut services = super::super::Services {
            sender,
            wake: write,
            shared: state.clone(),
            worker: None,
        };
        let old = UnixDesktopEvent::Stamped {
            action: UnixDesktopAction::Capture(CaptureCommand::Primary),
            generation: 0,
        };
        state.input_epoch.store(1, Ordering::Release);
        assert!(services.action(old).is_none());
        state.allow_capture.store(false, Ordering::Release);
        assert!(
            services
                .action(UnixDesktopEvent::Stamped {
                    action: UnixDesktopAction::Capture(CaptureCommand::Region),
                    generation: 1
                })
                .is_none()
        );
        assert!(matches!(
            services.action(UnixDesktopEvent::Stamped {
                action: UnixDesktopAction::Tray(TrayCommand::Show),
                generation: 0
            }),
            Some(UnixDesktopAction::Tray(TrayCommand::Show))
        ));
    }
}
