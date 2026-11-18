//! One app receiver, one bounded pipe reader and one watchdog/controller while targets exist.
use super::{
    native_drop_events as event, native_drop_process as process, native_drop_wire as wire,
};
use anyhow::{Result, ensure};
use futures::{
    StreamExt,
    channel::{mpsc, oneshot},
};
use std::{
    collections::BTreeMap,
    os::windows::io::{AsHandle, AsRawHandle, OwnedHandle},
    process::Command,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
        mpsc as sync_channel,
    },
    thread,
    time::Instant,
};
use windows::Win32::{
    Foundation::*,
    System::{JobObjects::TerminateJobObject, Threading::*},
};
const ARM: u32 = 30;
const DONE: u32 = 31;
const READY: u32 = 32;

static NEXT: AtomicU64 = AtomicU64::new(1);
fn identity() -> Result<u64> {
    NEXT.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
        n.checked_add(1).filter(|n| *n <= isize::MAX as u64)
    })
    .map_err(|_| anyhow::anyhow!("Native identity exhausted"))
}
fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}
#[derive(Clone, Default)]
pub(crate) struct NativeDropBroker {
    slot: Arc<Mutex<Option<Arc<Shared>>>>,
}
struct Shared {
    state: Mutex<State>,
    wake: OwnedHandle,
}
struct State {
    targets: BTreeMap<u64, Target>,
    version: u64,
    cancel: u64,
    fatal: bool,
    finished: bool,
    admission: Option<(u64, u64)>,
    released: Option<(u64, u64)>,
    delivering: bool,
    restart_credit: bool,
}
struct Target {
    hwnd: usize,
    window_id: u64,
    phase: Phase,
    last_gesture: u64,
    closing: bool,
    packet: Option<wire::Packet>,
    sender: mpsc::Sender<()>,
    closed: Option<oneshot::Sender<()>>,
}
#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Wanted,
    Registering,
    Registered,
    Removing,
}
pub(crate) struct Registration {
    shared: Arc<Shared>,
    token: u64,
    window_id: u64,
    closed: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
    wake: mpsc::Receiver<()>,
}
#[derive(Clone)]
pub(crate) struct TargetHandle {
    shared: Arc<Shared>,
    token: u64,
    window_id: u64,
    closed: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
}
pub(crate) struct Delivery {
    pub(crate) packet: wire::Packet,
    shared: Arc<Shared>,
    token: u64,
    version: u64,
    cancel: u64,
    terminal: bool,
}
impl Drop for Delivery {
    fn drop(&mut self) {
        if self.terminal {
            // Free native-owned body storage before making another acquisition possible.
            self.packet.bytes = Vec::new();
            let mut state = lock(&self.shared.state);
            if state.delivering {
                state.delivering = false;
                if state.admission == Some((self.token, self.packet.header.generation)) {
                    state.admission = None;
                    if state.version == self.version {
                        state.released = Some((self.token, self.packet.header.generation));
                    }
                }
            }
            drop(state);
            let _ = event::signal(&self.shared.wake);
        }
    }
}
impl NativeDropBroker {
    pub(crate) fn register(&self, hwnd: usize, window_id: u64) -> Result<Registration> {
        ensure!(process::installed(), "Native drop receiver is unavailable");
        let token = identity()?;
        let (sender, wake) = mpsc::channel(1);
        let (closed_tx, closed) = oneshot::channel();
        let mut slot = lock(&self.slot);
        if slot
            .as_ref()
            .is_some_and(|shared| lock(&shared.state).finished)
        {
            slot.take();
        }
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetPropW(
                HWND(hwnd as _),
                super::native_drop_receiver::PROPERTY,
                Some(HANDLE(token as usize as _)),
            )
        }?;
        let start = slot.is_none();
        if start {
            *slot = Some(Arc::new(Shared {
                state: Mutex::new(State {
                    targets: BTreeMap::new(),
                    version: identity()?,
                    cancel: 0,
                    fatal: false,
                    finished: false,
                    admission: None,
                    released: None,
                    delivering: false,
                    restart_credit: true,
                }),
                wake: event::event(false)?,
            }));
        }
        let shared = slot.as_ref().unwrap().clone();
        let mut state = lock(&shared.state);
        ensure!(
            state.targets.len() < 32,
            "Native drop target bound exceeded"
        );
        state.targets.insert(
            token,
            Target {
                hwnd,
                window_id,
                phase: Phase::Wanted,
                last_gesture: 0,
                closing: false,
                packet: None,
                sender,
                closed: Some(closed_tx),
            },
        );
        state.restart_credit = true;
        drop(state);
        if start {
            let worker = shared.clone();
            let controller = thread::Builder::new()
                .name("native-drop-controller".into())
                .stack_size(256 * 1024)
                .spawn(move || supervise(worker));
            if controller.is_err() {
                let mut state = lock(&shared.state);
                state.targets.remove(&token);
                state.finished = true;
                slot.take();
                anyhow::bail!("Native drop controller unavailable");
            }
            #[cfg(screenshots_native_drop_acceptance)]
            super::native_drop_acceptance::own_controller(controller.unwrap());
        }
        event::signal(&shared.wake)?;
        Ok(Registration {
            shared,
            token,
            window_id,
            closed: Arc::new(Mutex::new(Some(closed))),
            wake,
        })
    }
    #[cfg(screenshots_native_drop_acceptance)]
    pub(crate) fn acceptance_begin_completion(
        &self,
    ) -> Result<oneshot::Receiver<super::native_drop_acceptance::CompletionProof>> {
        let shared = lock(&self.slot)
            .as_ref()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Missing actual broker"))?;
        let receiver = super::native_drop_acceptance::request_completion();
        event::signal(&shared.wake)?;
        Ok(receiver)
    }
    pub(crate) fn cancel(&self) {
        if let Some(shared) = lock(&self.slot).as_ref() {
            let mut state = lock(&shared.state);
            if let Some(next) = state.cancel.checked_add(1) {
                state.cancel = next;
            } else {
                state.fatal = true;
            }
            if !state.delivering {
                state.admission = None;
            }
            state.released = None;
            state.restart_credit = true;
            for target in state.targets.values_mut() {
                target.packet = None;
                let _ = target.sender.try_send(());
            }
            drop(state);
            let _ = event::signal(&shared.wake);
        }
    }
}
impl Registration {
    #[cfg(screenshots_native_drop_acceptance)]
    pub(crate) fn acceptance_pending_kind(&self) -> Option<u32> {
        lock(&self.shared.state)
            .targets
            .get(&self.token)
            .and_then(|target| target.packet.as_ref().map(|packet| packet.header.kind))
    }
    pub(crate) fn handle(&self) -> TargetHandle {
        TargetHandle {
            shared: self.shared.clone(),
            token: self.token,
            window_id: self.window_id,
            closed: self.closed.clone(),
        }
    }
    pub(crate) async fn next(&mut self) -> Option<Delivery> {
        loop {
            self.wake.next().await?;
            let mut state = lock(&self.shared.state);
            let version = state.version;
            let cancel = state.cancel;
            let target = state.targets.get_mut(&self.token)?;
            if target.closing || target.window_id != self.window_id {
                return None;
            }
            if let Some(packet) = target.packet.take() {
                let terminal = matches!(
                    packet.header.kind,
                    wire::SUBMIT | wire::LEAVE | wire::FAILED
                );
                if terminal {
                    state.delivering = true;
                }
                return Some(Delivery {
                    packet,
                    shared: self.shared.clone(),
                    token: self.token,
                    version,
                    cancel,
                    terminal,
                });
            }
            // Cancellation/fault clears only this target's incoming drag on the UI.
            return Some(Delivery {
                packet: wire::Packet {
                    header: wire::Header {
                        kind: {
                            #[cfg(screenshots_native_drop_acceptance)]
                            {
                                super::native_drop_acceptance::failure_terminal_kind()
                            }
                            #[cfg(not(screenshots_native_drop_acceptance))]
                            {
                                wire::FAILED
                            }
                        },
                        window: self.token,
                        version,
                        ..Default::default()
                    },
                    bytes: Vec::new(),
                },
                shared: self.shared.clone(),
                token: self.token,
                version,
                cancel,
                terminal: false,
            });
        }
    }
    fn retire(&self, unexpected: bool) {
        self.handle().retire(unexpected);
    }
}
impl TargetHandle {
    pub(crate) fn token(&self) -> u64 {
        self.token
    }
    pub(crate) fn close(&self) -> oneshot::Receiver<()> {
        self.retire(false);
        lock(&self.closed).take().unwrap_or_else(|| {
            let (_, rx) = oneshot::channel();
            rx
        })
    }
    pub(crate) fn abandon(&self) {
        self.retire(true);
    }
    fn retire(&self, unexpected: bool) {
        let mut state = lock(&self.shared.state);
        if let Some(target) = state.targets.get_mut(&self.token) {
            target.closing = true;
            target.packet = None;
            state.fatal |= unexpected;
        }
        drop(state);
        let _ = event::signal(&self.shared.wake);
    }
    pub(crate) fn current(&self, delivery: &Delivery) -> bool {
        let state = lock(&self.shared.state);
        !state.fatal
            && self.token == delivery.token
            && state.version == delivery.version
            && state.cancel == delivery.cancel
            && state.targets.get(&self.token).is_some_and(|target| {
                !target.closing
                    && target.window_id == self.window_id
                    && (delivery.packet.header.generation == 0
                        || target.last_gesture == delivery.packet.header.generation)
            })
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.retire(false);
    }
}
fn notify(target: &mut Target) {
    let _ = target.sender.try_send(());
    #[cfg(screenshots_native_drop_acceptance)]
    super::native_drop_acceptance::publication_changed();
}
fn publish(
    shared: &Arc<Shared>,
    packet: wire::Packet,
    expected_cancel: u64,
    deadline: &super::native_drop_deadline::Deadline,
    now: impl FnOnce() -> Instant,
) -> Result<()> {
    let mut state = lock(&shared.state);
    // The packet event may be signaled after the absolute deadline, including
    // while waiting for this mutex. Expiry is independent of the wake reason.
    ensure!(
        deadline.may_publish(now()),
        "Native publication deadline expired"
    );
    let h = packet.header;
    ensure!(
        !state.fatal && state.cancel == expected_cancel,
        "Canceled native result"
    );
    ensure!(h.version == state.version, "Stale native receiver packet");
    let Some(target) = state.targets.get(&h.window) else {
        anyhow::bail!("Unknown native target");
    };
    if target.closing {
        return Ok(());
    }
    match h.kind {
        wire::ENTER => {
            ensure!(
                h.generation > 0 && state.admission.is_none(),
                "Native acquisition already occupied"
            );
            state.admission = Some((h.window, h.generation));
        }
        wire::FAILED | wire::LEAVE | wire::SUBMIT if state.admission.is_none() => {
            ensure!(h.generation > 0, "Invalid native terminal generation");
            state.admission = Some((h.window, h.generation));
        }
        wire::MOTION | wire::LEAVE | wire::SUBMIT | wire::FAILED => ensure!(
            state.admission == Some((h.window, h.generation)),
            "Stale native gesture"
        ),
        _ => anyhow::bail!("Invalid native result"),
    }
    let target = state.targets.get_mut(&h.window).unwrap();
    if matches!(
        h.kind,
        wire::ENTER | wire::FAILED | wire::LEAVE | wire::SUBMIT
    ) {
        target.last_gesture = h.generation;
    }
    if h.kind == wire::MOTION {
        if let Some(existing) = target.packet.as_mut() {
            if existing.header.kind == wire::ENTER || existing.header.kind == wire::MOTION {
                existing.header.x = h.x;
                existing.header.y = h.y;
            }
        } else {
            target.packet = Some(packet);
        }
    } else {
        ensure!(
            target
                .packet
                .as_ref()
                .is_none_or(|p| p.header.kind != wire::SUBMIT),
            "Native result retained twice"
        );
        target.packet = Some(packet);
    }
    #[cfg(screenshots_native_drop_acceptance)]
    super::native_drop_acceptance::record_publish(h);
    notify(target);
    Ok(())
}
// The receiver and reader have been reaped before either caller enters here.
fn retire_reaped_targets(state: &mut State) {
    state.targets.retain(|_, target| {
        if !target.closing {
            return true;
        }
        target.packet = None;
        if let Some(done) = target.closed.take() {
            let _ = done.send(());
        }
        false
    });
    // A foreground Delivery still owns its admitted bytes even with no windows.
    state.finished = state.targets.is_empty() && !state.delivering;
}
fn supervise(shared: Arc<Shared>) {
    loop {
        {
            let mut state = lock(&shared.state);
            if state.targets.is_empty() {
                state.finished = true;
                return;
            }
            state.fatal = false;
        }
        let result = run_one(&shared);
        #[cfg(screenshots_native_drop_acceptance)]
        if result.is_err() {
            super::native_drop_acceptance::before_failure_transition();
        }
        let mut state = lock(&shared.state);
        // run_one returns only after the exact process is signaled and pipe reader joined.
        retire_reaped_targets(&mut state);
        if !state.delivering {
            state.admission = None;
        }
        state.released = None;
        for target in state.targets.values_mut() {
            target.phase = Phase::Wanted;
            target.packet = None;
            notify(target);
        }
        if state.finished {
            #[cfg(screenshots_native_drop_acceptance)]
            super::native_drop_acceptance::failure_published(state.version);
            return;
        }
        let retry = state.restart_credit && !state.targets.is_empty();
        state.restart_credit = false;
        state.version = match identity() {
            Ok(value) => value,
            Err(_) => {
                state.finished = true;
                return;
            }
        };
        #[cfg(screenshots_native_drop_acceptance)]
        super::native_drop_acceptance::failure_published(state.version);
        drop(state);
        if !retry {
            // Failure remains unavailable until a concrete window/cancel event, never poll/retry.
            loop {
                if event::wait(&[event::raw(&shared.wake)], INFINITE).is_err() {
                    return;
                }
                let mut state = lock(&shared.state);
                retire_reaped_targets(&mut state);
                if state.finished {
                    return;
                }
                if state.restart_credit && !state.targets.is_empty() {
                    break;
                }
            }
        }
        let _ = result; // Error strings and source data never enter logs.
    }
}
#[derive(Debug)]
struct ReceiverEof;
impl std::fmt::Display for ReceiverEof {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Native receiver pipe ended before another header")
    }
}
impl std::error::Error for ReceiverEof {}
fn read_receiver(input: &mut impl std::io::Read) -> Result<wire::Packet> {
    use std::io::{Cursor, ErrorKind, Read as _};
    let mut first = [0u8; 1];
    loop {
        match input.read(&mut first) {
            Ok(0) => return Err(ReceiverEof.into()),
            Ok(_) => break,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == ErrorKind::BrokenPipe => return Err(ReceiverEof.into()),
            Err(error) => return Err(error.into()),
        }
    }
    // Only zero bytes before a header is clean EOF. Partial/corrupt packets stay errors.
    wire::read(&mut Cursor::new(first).chain(input))
}
fn complete_interval(
    shared: &Shared,
    initial_cancel: u64,
    deadline: &mut super::native_drop_deadline::Deadline,
    sequence: u64,
    processed: u64,
    now: Instant,
) -> bool {
    let state = lock(&shared.state);
    if state.fatal {
        return false;
    }
    // cancel() takes this same lock: cancellation before DONE cannot be missed
    // while DONE clears the active cutoff. Cancellation afterward is an idle stop.
    if state.cancel != initial_cancel {
        deadline.retire(now);
    }
    deadline.done(sequence, processed, now)
}
struct ChildOwner(process::NativeDropProcess);
impl ChildOwner {
    fn reap(&self) {
        unsafe {
            let _ = TerminateJobObject(event::raw(&self.0.job), 70);
        }
        if unsafe { WaitForSingleObject(event::raw(&self.0.process), INFINITE) } != WAIT_OBJECT_0 {
            // A kernel wait failure cannot be treated as proof of process exit.
            loop {
                thread::park();
            }
        }
    }
}
impl Drop for ChildOwner {
    fn drop(&mut self) {
        self.reap();
    }
}
fn run_one(shared: &Arc<Shared>) -> Result<()> {
    let command_event = event::event(true)?;
    let permit = event::event(true)?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg(process::CHILD_ARGUMENT)
        .arg((command_event.as_raw_handle() as usize).to_string())
        .arg((permit.as_raw_handle() as usize).to_string());
    command.env_clear();
    for (name, value) in std::env::vars_os() {
        if matches!(
            name.to_str(),
            Some("SystemRoot" | "WINDIR" | "TEMP" | "TMP")
        ) {
            command.env(name, value);
        }
    }
    #[cfg(screenshots_native_drop_acceptance)]
    super::native_drop_acceptance::decorate(&mut command);
    let mut child = ChildOwner(process::spawn(
        &mut command,
        &[command_event.as_handle(), permit.as_handle()],
    )?);
    #[cfg(screenshots_native_drop_acceptance)]
    super::native_drop_acceptance::child_started();
    let input = child.0.read.try_clone()?;
    let packets_ready = Arc::new(event::event(false)?);
    let reader_ready = packets_ready.clone();
    let (tx, rx) = sync_channel::sync_channel::<Result<wire::Packet>>(1);
    let reader = thread::Builder::new()
        .name("native-drop-pipe".into())
        .stack_size(256 * 1024)
        .spawn(move || {
            let mut input = input;
            loop {
                let packet = read_receiver(&mut input);
                let failed = packet.is_err();
                #[cfg(screenshots_native_drop_acceptance)]
                if let Ok(packet) = &packet {
                    super::native_drop_acceptance::record_read(packet.header);
                }
                if let Err(error) = tx.send(packet) {
                    drop(error);
                    // Parent reaps the helper before dropping rx. Drain its remaining FIFO to EOF.
                    #[cfg(screenshots_native_drop_acceptance)]
                    while let Ok(packet) = wire::read(&mut input) {
                        super::native_drop_acceptance::record_read(packet.header);
                    }
                    break;
                }
                if event::signal(&*reader_ready).is_err() || failed {
                    break;
                }
            }
        })?;
    // Every path below reaps the helper and joins the one owned blocking read.
    let result = (|| {
        wire::write(
            &mut child.0.write,
            wire::Header {
                kind: wire::HELLO,
                ..Default::default()
            },
            &child.0.nonce,
        )?;
        let started = Instant::now();
        let mut deadline = super::native_drop_deadline::Deadline::startup(started);
        let mut authenticated = false;
        let mut ready = false;
        let mut idle = false;
        let mut command_sequence = 0u64;
        #[cfg(screenshots_native_drop_acceptance)]
        let mut completion_command: Option<(u64, bool)> = None;
        let mut stopped = false;
        let mut allowed = false;
        let initial_cancel = lock(&shared.state).cancel;
        loop {
            {
                let state = lock(&shared.state);
                ensure!(!state.fatal, "Native operation canceled");
                // Revoke on the original STA before replacing an ordinarily canceled
                // receiver. Its current absolute deadline still bounds retirement.
                if state.cancel != initial_cancel {
                    deadline.retire(Instant::now());
                }
            }
            ensure!(
                deadline.unexpired(Instant::now()),
                "Native receiver deadline expired"
            );
            let timeout = deadline.milliseconds(Instant::now());
            let signaled = event::wait(
                &[
                    event::raw(&child.0.process),
                    event::raw(&shared.wake),
                    event::raw(&*packets_ready),
                ],
                timeout,
            )?;
            #[cfg(screenshots_native_drop_acceptance)]
            if signaled == 258 || !deadline.unexpired(Instant::now()) {
                super::native_drop_acceptance::deadline_expired();
            }
            ensure!(
                signaled != 258 && deadline.unexpired(Instant::now()),
                "Native receiver deadline expired"
            );
            if signaled == 0 {
                ensure!(
                    stopped && idle && !deadline.pending(),
                    "Native receiver exited before STOP completion"
                );
                let mut code = 259u32;
                unsafe { GetExitCodeProcess(event::raw(&child.0.process), &mut code) }?;
                ensure!(code == 0, "Native receiver did not stop cleanly");
                return Ok(());
            }
            while let Ok(packet) = rx.try_recv() {
                let packet = match packet {
                    Ok(packet) => packet,
                    // DONE/permit precedes ExitProcess; EOF may win the event race.
                    Err(error)
                        if error.is::<ReceiverEof>() && stopped && idle && !deadline.pending() =>
                    {
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                if lock(&shared.state).cancel != initial_cancel {
                    deadline.retire(Instant::now());
                }
                ensure!(
                    deadline.unexpired(Instant::now()),
                    "Native packet deadline expired"
                );
                let h = packet.header;
                match h.kind {
                    wire::ACK => {
                        ensure!(
                            !authenticated && packet.bytes == child.0.nonce,
                            "Native authentication failed"
                        );
                        authenticated = true;
                    }
                    ARM => {
                        ensure!(
                            authenticated && deadline.arm(h.reader, Instant::now()),
                            "Invalid native dispatch guard"
                        );
                        idle = false;
                        ensure!(
                            deadline.may_publish(Instant::now()),
                            "Native permit deadline expired"
                        );
                        event::signal(&permit)?;
                    }
                    DONE => {
                        let cutoff = deadline
                            .expires_at()
                            .ok_or_else(|| anyhow::anyhow!("Missing native interval deadline"))?;
                        ensure!(
                            complete_interval(
                                shared,
                                initial_cancel,
                                &mut deadline,
                                h.reader,
                                h.generation,
                                Instant::now()
                            ),
                            "Invalid native interval completion"
                        );
                        idle = true;
                        ensure!(
                            Instant::now() < cutoff,
                            "Native completion permit deadline expired"
                        );
                        event::signal(&permit)?;
                        #[cfg(screenshots_native_drop_acceptance)]
                        if completion_command.is_some_and(|(sequence, _)| sequence == h.generation)
                        {
                            let (_, ack) = completion_command.take().unwrap();
                            super::native_drop_acceptance::completion_done(ack, false);
                        }
                    }
                    #[cfg(screenshots_native_drop_acceptance)]
                    41 => {
                        ensure!(
                            deadline.armed()
                                && h.window == 0
                                && h.generation == 0
                                && h.version == lock(&shared.state).version,
                            "Completion reply outside exact guarded version"
                        );
                        let pending = completion_command
                            .as_mut()
                            .ok_or_else(|| anyhow::anyhow!("Unexpected completion reply"))?;
                        ensure!(h.reader == pending.0, "Wrong completion command");
                        pending.1 = true;
                    }
                    READY => {
                        ensure!(deadline.armed() && !ready, "Invalid native startup reply");
                        ready = true;
                    }
                    wire::REGISTERED | wire::REMOVED => {
                        ensure!(deadline.armed(), "Native registration outside guard");
                        let mut state = lock(&shared.state);
                        ensure!(
                            deadline.may_publish(Instant::now()),
                            "Native registration deadline expired"
                        );
                        ensure!(h.version == state.version, "Stale native registration");
                        if h.kind == wire::REGISTERED {
                            let target = state
                                .targets
                                .get_mut(&h.window)
                                .ok_or_else(|| anyhow::anyhow!("Unknown native registration"))?;
                            ensure!(
                                target.phase == Phase::Registering,
                                "Unexpected native registration"
                            );
                            target.phase = Phase::Registered;
                        } else {
                            if !state.delivering
                                && state
                                    .admission
                                    .is_some_and(|(window, _)| window == h.window)
                            {
                                state.admission = None;
                            }
                            let mut target = state
                                .targets
                                .remove(&h.window)
                                .ok_or_else(|| anyhow::anyhow!("Unknown native removal"))?;
                            ensure!(target.phase == Phase::Removing, "Unexpected native removal");
                            if let Some(done) = target.closed.take() {
                                let _ = done.send(());
                            }
                        }
                    }
                    wire::ENTER | wire::MOTION | wire::LEAVE | wire::SUBMIT | wire::FAILED => {
                        ensure!(deadline.armed() && ready, "Native data outside guard");
                        if h.kind == wire::ENTER {
                            allowed = false;
                        }
                        if let Err(error) =
                            publish(shared, packet, initial_cancel, &deadline, Instant::now)
                        {
                            let state = lock(&shared.state);
                            // publish rejects canceled data under the same lock before
                            // changing admission; only this canceled result is discarded.
                            if state.fatal || state.cancel == initial_cancel {
                                return Err(error);
                            }
                        }
                    }
                    _ => anyhow::bail!("Invalid native receiver reply"),
                }
            }
            if ready && idle && !deadline.pending() && !stopped {
                let mut state = lock(&shared.state);
                let version = state.version;
                let mut next = None;
                if state.cancel != initial_cancel {
                    stopped = true;
                    next = Some((
                        wire::Header {
                            kind: wire::STOP,
                            ..Default::default()
                        },
                        Vec::new(),
                    ));
                } else if let Some((window, generation)) = state.released.take() {
                    allowed = true;
                    next = Some((
                        wire::Header {
                            kind: wire::RELEASE,
                            window,
                            generation,
                            version,
                            ..Default::default()
                        },
                        Vec::new(),
                    ));
                }
                if next.is_none()
                    && state.admission.is_none()
                    && !allowed
                    && state
                        .targets
                        .values()
                        .any(|t| t.phase == Phase::Registered && !t.closing)
                {
                    allowed = true;
                    next = Some((
                        wire::Header {
                            kind: wire::ALLOW,
                            version,
                            ..Default::default()
                        },
                        Vec::new(),
                    ));
                }
                if next.is_none() {
                    // One command fits the existing >=64 KiB pipe; no foreground writer.
                    for (&window, target) in &mut state.targets {
                        if target.closing && target.phase == Phase::Wanted {
                            continue;
                        }
                        let kind = if target.closing && target.phase == Phase::Registered {
                            target.phase = Phase::Removing;
                            wire::REMOVE
                        } else if !target.closing && target.phase == Phase::Wanted {
                            target.phase = Phase::Registering;
                            wire::REGISTER
                        } else {
                            continue;
                        };
                        let bytes = if kind == wire::REGISTER {
                            (target.hwnd as u64).to_le_bytes().to_vec()
                        } else {
                            Vec::new()
                        };
                        next = Some((
                            wire::Header {
                                kind,
                                window,
                                version,
                                ..Default::default()
                            },
                            bytes,
                        ));
                        break;
                    }
                }
                #[cfg(screenshots_native_drop_acceptance)]
                if next.is_none() && super::native_drop_acceptance::take_completion() {
                    next = Some((
                        wire::Header {
                            kind: 40,
                            version,
                            ..Default::default()
                        },
                        Vec::new(),
                    ));
                }
                let abandoned = state
                    .targets
                    .iter()
                    .filter_map(|(id, target)| {
                        (target.closing && target.phase == Phase::Wanted).then_some(*id)
                    })
                    .collect::<Vec<_>>();
                for id in abandoned {
                    if let Some(mut target) = state.targets.remove(&id) {
                        if let Some(done) = target.closed.take() {
                            let _ = done.send(());
                        }
                    }
                }
                if next.is_none() && state.targets.is_empty() {
                    stopped = true;
                    next = Some((
                        wire::Header {
                            kind: wire::STOP,
                            ..Default::default()
                        },
                        Vec::new(),
                    ));
                }
                drop(state);
                if let Some((mut header, bytes)) = next {
                    command_sequence = command_sequence
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!("Native command exhausted"))?;
                    header.reader = command_sequence;
                    #[cfg(screenshots_native_drop_acceptance)]
                    if header.kind == 40 {
                        completion_command = Some((command_sequence, false));
                    }
                    ensure!(
                        deadline.issue(command_sequence, Instant::now()),
                        "Native command already pending"
                    );
                    if header.kind == wire::STOP {
                        // DONE must retain the finite bound until actual process exit.
                        deadline.retire(Instant::now());
                    }
                    // Bounds plus one in-flight command make this write non-growing.
                    wire::write(&mut child.0.write, header, &bytes)?;
                    event::signal(&command_event)?;
                }
            }
        }
    })();
    child.reap();
    drop(rx);
    let joined = reader.join();
    #[cfg(screenshots_native_drop_acceptance)]
    {
        let mut code = 259u32;
        let queried =
            unsafe { GetExitCodeProcess(event::raw(&child.0.process), &mut code) }.is_ok();
        let pid = unsafe { GetProcessId(event::raw(&child.0.process)) };
        let verified = queried && pid > 0 && code != 259;
        println!(
            "{{\"actual_receiver_exit\":true,\"pid\":{pid},\"exit_code\":{code},\"verified\":{verified}}}"
        );
    }
    ensure!(joined.is_ok(), "Native pipe reader failed");
    #[cfg(screenshots_native_drop_acceptance)]
    super::native_drop_acceptance::reader_reaped();
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    fn publish_now(shared: &Arc<Shared>, packet: wire::Packet, cancel: u64) -> Result<()> {
        let now = Instant::now();
        let mut deadline = super::super::native_drop_deadline::Deadline::startup(now);
        assert!(deadline.arm(1, now));
        publish(shared, packet, cancel, &deadline, || now)
    }
    fn shared() -> Arc<Shared> {
        let (sender, _) = mpsc::channel(1);
        Arc::new(Shared {
            state: Mutex::new(State {
                targets: BTreeMap::from([(
                    2,
                    Target {
                        hwnd: 5,
                        window_id: 6,
                        phase: Phase::Registered,
                        last_gesture: 3,
                        closing: false,
                        packet: None,
                        sender,
                        closed: None,
                    },
                )]),
                version: 1,
                cancel: 0,
                fatal: false,
                finished: false,
                admission: Some((2, 3)),
                released: None,
                delivering: true,
                restart_credit: true,
            }),
            wake: event::event(false).unwrap(),
        })
    }
    #[test]
    fn cancel_between_packet_sample_and_done_keeps_the_active_cutoff() {
        use std::time::Duration;
        let shared = shared();
        let initial_cancel = lock(&shared.state).cancel;
        let start = Instant::now();
        let mut deadline = super::super::native_drop_deadline::Deadline::startup(start);
        assert!(deadline.arm(1, start));
        // Deterministically place foreground cancellation after the stale packet sample.
        assert_eq!(lock(&shared.state).cancel, initial_cancel);
        lock(&shared.state).cancel += 1;
        let done = start + Duration::from_millis(4800);
        assert!(complete_interval(
            &shared,
            initial_cancel,
            &mut deadline,
            1,
            0,
            done
        ));
        assert!(deadline.issue(1, done));
        deadline.retire(done);
        assert_eq!(deadline.expires_at(), Some(start + Duration::from_secs(5)));
        assert!(!deadline.unexpired(start + Duration::from_secs(5)));
    }
    #[test]
    fn receiver_eof_is_distinct_from_partial_or_corrupt_trailing_packets() {
        use std::io::Cursor;
        let clean = read_receiver(&mut Cursor::new([]));
        assert!(
            clean
                .as_ref()
                .err()
                .is_some_and(|error| error.is::<ReceiverEof>())
        );
        for bytes in [vec![1], vec![0; 64]] {
            let invalid = read_receiver(&mut Cursor::new(bytes));
            assert!(invalid.is_err());
            assert!(!invalid.as_ref().err().unwrap().is::<ReceiverEof>());
        }
        let mut raw = Vec::new();
        wire::write(
            &mut raw,
            wire::Header {
                kind: wire::SUBMIT,
                format: wire::URL,
                ..Default::default()
            },
            b"x",
        )
        .unwrap();
        let mut valid = Cursor::new(raw.clone());
        assert_eq!(read_receiver(&mut valid).unwrap().bytes, b"x");
        assert!(
            read_receiver(&mut valid)
                .as_ref()
                .err()
                .is_some_and(|error| error.is::<ReceiverEof>())
        );
        raw.pop();
        let partial = read_receiver(&mut Cursor::new(raw));
        assert!(partial.is_err());
        assert!(!partial.as_ref().err().unwrap().is::<ReceiverEof>());
    }
    fn held(shared: Arc<Shared>) -> Delivery {
        Delivery {
            packet: wire::Packet {
                header: wire::Header {
                    kind: wire::SUBMIT,
                    window: 2,
                    generation: 3,
                    version: 1,
                    ..Default::default()
                },
                bytes: vec![1; 65536],
            },
            shared,
            token: 2,
            version: 1,
            cancel: 0,
            terminal: true,
        }
    }
    fn enter(generation: u64, version: u64) -> wire::Packet {
        wire::Packet {
            header: wire::Header {
                kind: wire::ENTER,
                window: 2,
                generation,
                version,
                format: wire::PNG,
                ..Default::default()
            },
            bytes: Vec::new(),
        }
    }
    #[test]
    fn completed_body_holds_actual_publish_admission_through_256_generations() {
        let shared = shared();
        let body = held(shared.clone());
        for generation in 4..260 {
            assert!(publish_now(&shared, enter(generation, 1), 0).is_err());
        }
        assert_eq!(lock(&shared.state).admission, Some((2, 3)));
        drop(body);
        assert_eq!(lock(&shared.state).released, Some((2, 3)));
        assert!(publish_now(&shared, enter(260, 1), 0).is_ok());
    }
    #[test]
    fn canceled_delivery_retains_slot_until_discard_across_receiver_replacement() {
        let shared = shared();
        let body = held(shared.clone());
        let broker = NativeDropBroker {
            slot: Arc::new(Mutex::new(Some(shared.clone()))),
        };
        broker.cancel();
        {
            let mut state = lock(&shared.state);
            state.version = 2;
        }
        assert!(publish_now(&shared, enter(4, 2), 1).is_err());
        drop(body);
        assert!(lock(&shared.state).released.is_none());
        assert!(publish_now(&shared, enter(4, 2), 1).is_ok());
    }
    #[test]
    fn completion_from_old_controller_cannot_be_retagged_after_cancel() {
        let shared = shared();
        let broker = NativeDropBroker {
            slot: Arc::new(Mutex::new(Some(shared.clone()))),
        };
        broker.cancel();
        assert!(publish_now(&shared, enter(4, 1), 0).is_err());
        assert!(
            lock(&shared.state)
                .targets
                .get(&2)
                .unwrap()
                .packet
                .is_none()
        );
    }
    #[test]
    fn old_closed_target_cannot_fault_a_live_other_window() {
        let shared = shared();
        let handle = TargetHandle {
            shared: shared.clone(),
            token: 99,
            window_id: 100,
            closed: Arc::new(Mutex::new(None)),
        };
        handle.abandon();
        assert!(!lock(&shared.state).fatal);
        assert!(lock(&shared.state).targets.contains_key(&2));
    }
    #[test]
    fn overdue_completed_packet_cannot_publish_despite_a_ready_event() {
        let ready = event::event(false).unwrap();
        event::signal(&ready).unwrap();
        assert_eq!(event::wait(&[event::raw(&ready)], 0).unwrap(), 0);
        let shared = shared();
        let start = Instant::now();
        let mut deadline = super::super::native_drop_deadline::Deadline::startup(start);
        assert!(deadline.arm(1, start));
        let packet = wire::Packet {
            header: wire::Header {
                kind: wire::SUBMIT,
                window: 2,
                generation: 3,
                version: 1,
                format: wire::PNG,
                ..Default::default()
            },
            bytes: vec![7; 65536],
        };
        assert!(
            publish(&shared, packet, 0, &deadline, || start
                + std::time::Duration::from_secs(5))
            .is_err()
        );
        assert!(
            lock(&shared.state)
                .targets
                .get(&2)
                .unwrap()
                .packet
                .is_none()
        );
        assert_eq!(lock(&shared.state).admission, Some((2, 3)));
        assert!(!deadline.done(1, 0, start + std::time::Duration::from_secs(5)));
        assert!(deadline.armed());
    }
    #[test]
    fn coalesced_terminal_packet_reserves_admission_without_a_prior_enter_packet() {
        for kind in [wire::LEAVE, wire::SUBMIT] {
            let shared = shared();
            {
                let mut state = lock(&shared.state);
                state.admission = None;
                state.delivering = false;
            }
            let packet = wire::Packet {
                header: wire::Header {
                    kind,
                    window: 2,
                    generation: 4,
                    version: 1,
                    format: if kind == wire::SUBMIT { wire::PNG } else { 0 },
                    ..Default::default()
                },
                bytes: if kind == wire::SUBMIT {
                    vec![7]
                } else {
                    Vec::new()
                },
            };
            assert!(publish_now(&shared, packet, 0).is_ok());
            let state = lock(&shared.state);
            assert_eq!(state.admission, Some((2, 4)));
            assert_eq!(state.targets.get(&2).unwrap().last_gesture, 4);
            assert_eq!(
                state
                    .targets
                    .get(&2)
                    .unwrap()
                    .packet
                    .as_ref()
                    .unwrap()
                    .header
                    .kind,
                kind
            );
        }
    }
    #[test]
    fn unavailable_receiver_acknowledges_each_closed_window_without_releasing_delivery() {
        let shared = shared();
        let delivery = held(shared.clone());
        let (sender, _) = mpsc::channel(1);
        let (close_a, mut done_a) = oneshot::channel();
        let (close_b, mut done_b) = oneshot::channel();
        {
            let mut state = lock(&shared.state);
            state.restart_credit = false;
            state.targets.get_mut(&2).unwrap().closed = Some(close_a);
            state.targets.insert(
                4,
                Target {
                    hwnd: 7,
                    window_id: 8,
                    phase: Phase::Wanted,
                    last_gesture: 0,
                    closing: false,
                    packet: None,
                    sender,
                    closed: Some(close_b),
                },
            );
        }
        let target = |token, window_id| TargetHandle {
            shared: shared.clone(),
            token,
            window_id,
            closed: Arc::new(Mutex::new(None)),
        };
        target(2, 6).retire(false);
        {
            let mut state = lock(&shared.state);
            retire_reaped_targets(&mut state);
            assert!(!state.targets.contains_key(&2) && state.targets.contains_key(&4));
            assert!(state.delivering && !state.finished && !state.restart_credit);
            assert_eq!(state.admission, Some((2, 3)));
        }
        assert_eq!(done_a.try_recv().unwrap(), Some(()));
        assert_eq!(done_b.try_recv().unwrap(), None);
        target(4, 8).retire(false);
        {
            let mut state = lock(&shared.state);
            retire_reaped_targets(&mut state);
            assert!(state.targets.is_empty() && !state.finished && state.delivering);
        }
        assert_eq!(done_b.try_recv().unwrap(), Some(()));
        drop(delivery);
        {
            let mut state = lock(&shared.state);
            retire_reaped_targets(&mut state);
            assert!(state.finished && !state.delivering);
        }
    }
}
