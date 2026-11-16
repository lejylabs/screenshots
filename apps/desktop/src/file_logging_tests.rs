use super::*;
use std::alloc::GlobalAlloc as _;
use std::{fs, sync::atomic::AtomicUsize};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("screenshots-log-{}", uuid::Uuid::new_v4()));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt as _;
            builder.mode(0o700);
        }
        builder.create(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn private_file(path: &Path, content: &[u8]) {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    no_follow(&mut options);
    options.open(path).unwrap().write_all(content).unwrap();
}
fn channel(capacity: usize) -> (Arc<Mailbox>, Arc<Mailbox>) {
    let state = Arc::new(State::default());
    state.accepting.store(true, Ordering::Release);
    let mailbox = Mailbox::new(capacity, state).unwrap();
    (mailbox.clone(), mailbox)
}
fn events(sender: Arc<Mailbox>) -> Events {
    Events {
        state: sender.state.clone(),
        sender,
    }
}
fn next(receiver: &Mailbox) -> Message {
    loop {
        match receiver.try_recv() {
            Ok(message) => return message,
            Err(_) => receiver.wait().unwrap(),
        }
    }
}
fn send(sender: &Mailbox, mut message: Message) {
    for _ in 0..1000 {
        match sender.try_send(message) {
            Ok(()) => return,
            Err(error) => message = error.into_inner(),
        }
        std::thread::sleep(Duration::from_micros(50));
    }
    panic!("private test queue did not become available")
}
fn count(receiver: &Mailbox) -> usize {
    std::iter::from_fn(|| receiver.try_recv().ok()).count()
}
fn barrier(events: &Events) {
    let (sender, receiver) = mpsc::sync_channel(1);
    send(&events.sender, Message::Barrier(sender));
    receiver.recv_timeout(Duration::from_secs(2)).unwrap();
}

#[test]
fn default_layer_never_visits_dynamic_fields_or_messages() {
    struct Sensitive<'a>(&'a AtomicUsize);
    impl fmt::Debug for Sensitive<'_> {
        fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.fetch_add(1, Ordering::Relaxed);
            panic!("sensitive diagnostic must never be formatted")
        }
    }
    let (sender, receiver) = channel(1);
    let layer = events(sender);
    let calls = AtomicUsize::new(0);
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        tracing::warn!(
            path = "/private/secret.png",
            token = "private-bearer-token",
            window_title = "Private window title",
            error = ?Sensitive(&calls),
            "sensitive message {}", "private user content"
        );
    });
    let Message::Record(record) = next(&receiver) else {
        panic!("expected record")
    };
    let line = record_line(&record, record.at, 0).unwrap();
    let text = std::str::from_utf8(line.as_bytes()).unwrap();
    assert!(text.contains("level=WARN") && text.contains("fields=redacted"));
    for forbidden in [
        "secret.png",
        "private-bearer",
        "Private window",
        "private user",
        "sensitive message",
    ] {
        assert!(!text.contains(forbidden));
    }
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert!(line.len <= RECORD_BYTES);
}

#[test]
fn queue_overflow_drops_without_waiting_and_close_stops_admission() {
    let (sender, receiver) = channel(2);
    let layer = events(sender);
    let state = layer.state.clone();
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
        for _ in 0..100 {
            tracing::info!("bounded event");
        }
        state.accepting.store(false, Ordering::Release);
        tracing::info!("closed event");
    });
    assert_eq!(count(&receiver), 2);
    assert_eq!(state.dropped.load(Ordering::Relaxed), 98);
}

#[test]
fn records_are_ascii_bounded_even_for_long_or_control_metadata() {
    let mut line = Line::new();
    line.component(&"bad\n\r\t日本語/target".repeat(200))
        .unwrap();
    assert_eq!(line.as_bytes(), b"redacted");
    assert!(line.as_bytes().iter().all(|byte| byte.is_ascii_graphic()));
    assert!(line.write_str(&"x".repeat(RECORD_BYTES)).is_err());
    assert_eq!(line.as_bytes(), b"redacted");
}

#[test]
fn source_file_and_event_name_are_never_persisted() {
    static METADATA: Metadata<'static> = Metadata::new(
        "event /home/private/person/path.rs:17",
        "safe_target",
        tracing::Level::INFO,
        Some("/home/private/person/path.rs"),
        Some(17),
        Some("safe_module::child"),
        tracing::field::FieldSet::new(&[], tracing::callsite::Identifier(&CALLSITE)),
        tracing::metadata::Kind::EVENT,
    );
    static CALLSITE: tracing::callsite::DefaultCallsite =
        tracing::callsite::DefaultCallsite::new(&METADATA);
    let record = Record {
        metadata: &METADATA,
        at: Instant::now(),
    };
    let line = record_line(&record, record.at, 0).unwrap();
    let text = std::str::from_utf8(line.as_bytes()).unwrap();
    assert!(text.contains("module=safe_module::child line=17"));
    assert!(!text.contains("private") && !text.contains("person") && !text.contains("path.rs"));
    let mut component = Line::new();
    component.component("/home/private/person").unwrap();
    assert_eq!(component.as_bytes(), b"redacted");
}

#[test]
fn ring_rotates_only_four_retained_files_with_exact_byte_caps() {
    let temporary = Temp::new();
    let limit = HEADER.len() as u64 + RECORD_BYTES as u64;
    let mut ring = Ring::open(&temporary.0, limit).unwrap();
    for number in 0..20 {
        let mut record = [b'x'; RECORD_BYTES];
        record[0] = b'A' + number;
        ring.write(&record).unwrap();
    }
    drop(ring);
    let paths: Vec<_> = fs::read_dir(&temporary.0)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(paths.len(), SLOTS);
    let mut letters = Vec::new();
    for path in paths {
        let content = fs::read(&path).unwrap();
        assert_eq!(content.len() as u64, limit);
        assert!(content.starts_with(HEADER));
        letters.push(content[HEADER.len()]);
    }
    letters.sort_unstable();
    assert_eq!(letters, vec![b'Q', b'R', b'S', b'T']);
}

#[test]
fn partial_initialization_preserves_foreign_files_and_releases_locks() {
    let temporary = Temp::new();
    let foreign = temporary.0.join("events-2.log");
    private_file(&foreign, b"foreign content must survive");
    assert!(Ring::open(&temporary.0, SLOT_BYTES).is_err());
    assert_eq!(fs::read(&foreign).unwrap(), b"foreign content must survive");
    let first = OpenOptions::new()
        .read(true)
        .write(true)
        .open(temporary.0.join("events-0.log"))
        .unwrap();
    first.try_lock().unwrap();
    first.unlock().unwrap();
    assert!(!temporary.0.join("events-3.log").exists());
}

#[test]
fn live_owner_lock_and_real_write_failure_are_fail_closed() {
    let temporary = Temp::new();
    let mut ring = Ring::open(&temporary.0, SLOT_BYTES).unwrap();
    assert!(Ring::open(&temporary.0, SLOT_BYTES).is_err());
    let index = ring.next;
    let path = temporary.0.join(format!("events-{index}.log"));
    let mut before = Vec::new();
    ring.files[index].rewind().unwrap();
    ring.files[index].read_to_end(&mut before).unwrap();
    ring.files[index] = Slot(File::open(&path).unwrap());
    assert!(ring.write(b"event\n").is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
}

#[test]
fn field_predicate_filter_syntax_is_rejected_without_inspecting_values() {
    let ordinary = level_targets(Some("warn,screenshots=trace"));
    assert!(ordinary.would_enable("screenshots", &tracing::Level::TRACE));
    assert!(!ordinary.would_enable("other", &tracing::Level::INFO));
    for rejected in [
        "screenshots[{token}]=trace",
        "screenshots[span]=trace",
        "[{secret}]=trace",
    ] {
        let fallback = level_targets(Some(rejected));
        assert!(fallback.would_enable("screenshots", &tracing::Level::INFO));
        assert!(!fallback.would_enable("screenshots", &tracing::Level::TRACE));
    }
}

#[test]
fn shutdown_waits_for_an_admitted_producer_before_its_final_summary() {
    let temporary = Temp::new();
    let directory = temporary.0.clone();
    let (layer, mut guard) = start(move || Ok(directory)).unwrap();
    barrier(&layer);
    let (waiting, observed_wait) = mpsc::sync_channel(1);
    layer.state.waiting_for_producer.set(waiting).unwrap();
    let admission = layer.admit().unwrap();
    let done = guard.done.take().unwrap();
    layer.state.accepting.store(false, Ordering::SeqCst);
    layer.state.stopping.store(true, Ordering::SeqCst);
    layer.sender.signal();
    observed_wait.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(done.try_recv(), Err(mpsc::TryRecvError::Empty)));

    let (sender, receiver) = channel(1);
    tracing::subscriber::with_default(tracing_subscriber::registry().with(events(sender)), || {
        tracing::info!("admitted before shutdown")
    });
    let record = next(&receiver);
    send(&layer.sender, record);
    drop(admission);
    done.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(layer.state.written.load(Ordering::Relaxed), 1);
    assert_eq!(layer.state.dropped.load(Ordering::Relaxed), 0);
    let contents: Vec<_> = (0..SLOTS)
        .flat_map(|index| fs::read(temporary.0.join(format!("events-{index}.log"))).unwrap())
        .collect();
    assert!(
        String::from_utf8(contents)
            .unwrap()
            .contains("shutdown written=1 dropped_total=0")
    );
    assert!(Ring::open(&temporary.0, SLOT_BYTES).is_ok());
}

#[test]
fn parked_worker_has_no_idle_file_writes_and_shutdown_releases_files() {
    let temporary = Temp::new();
    let directory = temporary.0.clone();
    let (layer, mut guard) = start(move || Ok(directory)).unwrap();
    tracing::subscriber::with_default(tracing_subscriber::registry().with(layer.clone()), || {
        tracing::info!(path = "/private/never-recorded", "event");
    });
    barrier(&layer);
    let metadata = || {
        (0..SLOTS)
            .map(|index| {
                let m = fs::metadata(temporary.0.join(format!("events-{index}.log"))).unwrap();
                (m.len(), m.modified().unwrap())
            })
            .collect::<Vec<_>>()
    };
    let before = metadata();
    std::thread::sleep(Duration::from_millis(35));
    assert_eq!(metadata(), before);
    assert_eq!(guard.state.written.load(Ordering::Relaxed), 1);
    assert_eq!(
        guard.shutdown(Duration::from_secs(2)),
        ShutdownResult::Complete
    );
    assert!(!guard.state.failed.load(Ordering::Acquire));
    let reopened = Ring::open(&temporary.0, SLOT_BYTES).unwrap();
    drop(reopened);
}

#[test]
fn initialization_failure_is_recorded_and_shutdown_wait_is_bounded() {
    let (layer, mut guard) = start(|| Err(io::ErrorKind::PermissionDenied.into())).unwrap();
    assert_eq!(
        guard.shutdown(Duration::from_secs(2)),
        ShutdownResult::Complete
    );
    assert!(guard.state.failed.load(Ordering::Acquire));
    assert!(!layer.state.accepting.load(Ordering::Acquire));

    let (sender, _receiver) = channel(1);
    let (_done_sender, done) = mpsc::sync_channel(1);
    let state = Arc::new(State::default());
    state.accepting.store(true, Ordering::Release);
    let mut blocked = LoggingGuard {
        sender,
        state,
        done: Some(done),
    };
    let started = Instant::now();
    assert_eq!(
        blocked.shutdown(Duration::from_millis(20)),
        ShutdownResult::TimedOut
    );
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!(!blocked.state.accepting.load(Ordering::Acquire));
    assert_eq!(
        blocked.shutdown(Duration::from_millis(20)),
        ShutdownResult::Complete
    );
}

#[cfg(unix)]
#[test]
fn symlinks_fifo_and_hardlinks_cannot_become_log_slots() {
    use std::os::unix::ffi::OsStrExt as _;
    for kind in ["symlink", "fifo", "hardlink"] {
        let temporary = Temp::new();
        let foreign = temporary.0.join("foreign");
        private_file(&foreign, HEADER);
        let slot = temporary.0.join("events-0.log");
        match kind {
            "symlink" => std::os::unix::fs::symlink(&foreign, &slot).unwrap(),
            "hardlink" => fs::hard_link(&foreign, &slot).unwrap(),
            "fifo" => {
                let path = std::ffi::CString::new(slot.as_os_str().as_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
            }
            _ => unreachable!(),
        }
        let started = Instant::now();
        assert!(Ring::open(&temporary.0, SLOT_BYTES).is_err());
        assert!(started.elapsed() < Duration::from_millis(500));
        assert_eq!(fs::read(&foreign).unwrap(), HEADER);
    }
}

#[test]
#[ignore = "optimized callback/native-wake comparison with private files; no GUI or user logs"]
fn optimized_callback_cost_against_formatted_sink() {
    fn emit() {
        tracing::info!(width = 1280, height = 720, "synthetic callback benchmark");
    }
    fn measure(dispatch: &tracing::Dispatch, warm: impl FnOnce()) -> (u128, u128) {
        let mut samples = Vec::with_capacity(100);
        tracing::dispatcher::with_default(dispatch, || {
            for _ in 0..1000 {
                emit();
            }
            warm();
            for _ in 0..100 {
                let started = Instant::now();
                for _ in 0..2000 {
                    emit();
                }
                samples.push(started.elapsed().as_nanos() / 2000);
            }
        });
        samples.sort_unstable();
        (samples[49], samples[94])
    }
    let old = tracing::Dispatch::new(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(io::sink)
            .finish(),
    );
    let (old_p50, old_p95) = measure(&old, || {});
    let (sender, receiver) = channel(QUEUE);
    let layer = events(sender);
    let state = layer.state.clone();
    let new = tracing::Dispatch::new(tracing_subscriber::registry().with(layer));
    let (new_p50, new_p95) = measure(&new, || state.dropped.store(0, Ordering::Relaxed));
    assert_eq!(count(&receiver), QUEUE);
    assert_eq!(state.dropped.load(Ordering::Relaxed), 200_000);
    let metadata = synthetic_metadata();
    let mut contended = Vec::with_capacity(100);
    let (held, _) = channel(QUEUE);
    let contended_layer = events(held.clone());
    held.hold_gate(|| {
        for _ in 0..100 {
            let started = Instant::now();
            for _ in 0..2000 {
                contended_layer.record(metadata);
            }
            contended.push(started.elapsed().as_nanos() / 2000);
        }
    });
    assert_eq!(
        contended_layer.state.dropped.load(Ordering::Relaxed),
        200_000
    );
    let temporary = Temp::new();
    let path = temporary.0.clone();
    let (sleeping, mut guard) = start(move || Ok(path)).unwrap();
    let mut native_success = Vec::with_capacity(100);
    for _ in 0..100 {
        wait_for_native_sleep(&sleeping);
        let started = Instant::now();
        sleeping.record(metadata);
        native_success.push(started.elapsed().as_nanos());
        barrier(&sleeping);
    }
    assert_eq!(sleeping.state.written.load(Ordering::Relaxed), 100);
    assert_eq!(
        guard.shutdown(Duration::from_secs(2)),
        ShutdownResult::Complete
    );
    native_success.sort_unstable();
    contended.sort_unstable();
    println!(
        "{}",
        serde_json::json!({
            "scope": "100 batches of2000events, nanoseconds/event batch averages; full bounded queue vs formatted sink; not GUI/disk/reference acceptance",
            "formatted_sink_ns_p50": old_p50,
            "formatted_sink_ns_p95": old_p95,
            "bounded_full_queue_ns_p50": new_p50,
            "bounded_full_queue_ns_p95": new_p95,
            "record_bytes": std::mem::size_of::<Message>(),
            "queue_item_bound": QUEUE,
            "queue_payload_bytes": QUEUE * std::mem::size_of::<Message>(),
            "held_gate_ns_p50": contended[49],
            "held_gate_ns_p95": contended[94],
            "native_successful_callback_ns_p50": native_success[49],
            "native_successful_callback_ns_p95": native_success[94],
            "native_successful_callback_samples": native_success.len(),
            "sleep_scope": "stable native-wait entry counter and cleared notification, no per-sample scheduler-state proof",
        })
    );
}

thread_local! {
    static TRACK_ALLOCATIONS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static ALLOCATION_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
struct CallbackAllocator;
#[global_allocator]
static CALLBACK_ALLOCATOR: CallbackAllocator = CallbackAllocator;
unsafe impl std::alloc::GlobalAlloc for CallbackAllocator {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        TRACK_ALLOCATIONS.with(|track| {
            if track.get() {
                ALLOCATION_COUNT.with(|count| count.set(count.get() + 1));
            }
        });
        unsafe { std::alloc::System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
        TRACK_ALLOCATIONS.with(|track| {
            if track.get() {
                ALLOCATION_COUNT.with(|count| count.set(count.get() + 1));
            }
        });
        unsafe { std::alloc::System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: std::alloc::Layout) {
        unsafe { std::alloc::System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        TRACK_ALLOCATIONS.with(|track| {
            if track.get() {
                ALLOCATION_COUNT.with(|count| count.set(count.get() + 1));
            }
        });
        unsafe { std::alloc::System.realloc(pointer, layout, size) }
    }
}
fn synthetic_metadata() -> &'static Metadata<'static> {
    static META: Metadata<'static> = Metadata::new(
        "synthetic callback",
        "screenshots",
        tracing::Level::INFO,
        None,
        Some(1),
        Some("screenshots::file_logging"),
        tracing::field::FieldSet::new(&[], tracing::callsite::Identifier(&SITE)),
        tracing::metadata::Kind::EVENT,
    );
    static SITE: tracing::callsite::DefaultCallsite =
        tracing::callsite::DefaultCallsite::new(&META);
    &META
}
fn measured_allocations(body: impl FnOnce()) -> usize {
    ALLOCATION_COUNT.with(|count| count.set(0));
    TRACK_ALLOCATIONS.with(|track| track.set(true));
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            TRACK_ALLOCATIONS.with(|track| track.set(false));
        }
    }
    let reset = Reset;
    body();
    drop(reset);
    ALLOCATION_COUNT.with(std::cell::Cell::get)
}
fn wait_for_native_sleep(layer: &Events) {
    barrier(layer);
    for _ in 0..200 {
        let waits = layer.sender.native_waits.load(Ordering::Acquire);
        if waits != 0 && !layer.sender.notification_pending() {
            std::thread::sleep(Duration::from_millis(2));
            if layer.sender.native_waits.load(Ordering::Acquire) == waits
                && !layer.sender.notification_pending()
            {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    panic!("private worker did not reach its native wait")
}

#[test]
fn native_credits_coalesce_and_remember_pre_wait_publication() {
    let (sender, _) = channel(2);
    for _ in 0..1000 {
        sender.signal();
    }
    assert_eq!(sender.signal_count(), 1);
    sender.wait().unwrap();
    assert!(!sender.notification_pending());
    let waiting = sender.clone();
    let (entered_sender, entered) = mpsc::sync_channel(1);
    let (done_sender, done) = mpsc::sync_channel(1);
    let thread = std::thread::spawn(move || {
        entered_sender.send(()).unwrap();
        waiting.wait().unwrap();
        done_sender.send(()).unwrap();
    });
    entered.recv_timeout(Duration::from_secs(2)).unwrap();
    std::thread::sleep(Duration::from_millis(20));
    assert!(matches!(done.try_recv(), Err(mpsc::TryRecvError::Empty)));
    sender.signal();
    done.recv_timeout(Duration::from_secs(2)).unwrap();
    thread.join().unwrap();
    assert_eq!(sender.signal_count(), 2);
}

#[test]
fn native_sleeping_worker_accepts_a_successful_callback() {
    let temporary = Temp::new();
    let path = temporary.0.clone();
    let (layer, mut guard) = start(move || Ok(path)).unwrap();
    wait_for_native_sleep(&layer);
    let signals = layer.sender.signal_count();
    assert_eq!(
        measured_allocations(|| layer.record(synthetic_metadata())),
        0
    );
    barrier(&layer);
    assert_eq!(layer.state.written.load(Ordering::Relaxed), 1);
    assert!(layer.sender.signal_count() > signals);
    assert_eq!(
        guard.shutdown(Duration::from_secs(2)),
        ShutdownResult::Complete
    );
    assert!(!layer.state.failed.load(Ordering::Acquire));
}

#[test]
fn producer_contention_is_one_attempt_without_allocation_or_waiting() {
    let (sender, receiver) = channel(QUEUE);
    let layer = events(sender.clone());
    let metadata = synthetic_metadata();
    sender.hold_gate(|| {
        assert_eq!(
            measured_allocations(|| {
                for _ in 0..10_000 {
                    layer.record(metadata);
                }
            }),
            0
        );
    });
    assert_eq!(layer.state.dropped.load(Ordering::Relaxed), 10_000);
    assert_eq!(count(&receiver), 0);
    assert_eq!(measured_allocations(|| layer.record(metadata)), 0);
    assert_eq!(count(&receiver), 1);
}

#[test]
fn native_signal_failure_closes_admission_and_last_sender_wakes_cleanup() {
    let temporary = Temp::new();
    let path = temporary.0.clone();
    let (layer, mut guard) = start(move || Ok(path)).unwrap();
    wait_for_native_sleep(&layer);
    layer.sender.fail_next_signals(2);
    layer.record(synthetic_metadata());
    assert!(layer.state.failed.load(Ordering::Acquire));
    assert!(!layer.state.accepting.load(Ordering::SeqCst));
    assert!(layer.state.stopping.load(Ordering::SeqCst));
    assert_eq!(
        guard.shutdown(Duration::from_secs(2)),
        ShutdownResult::Complete
    );
    assert!(Ring::open(&temporary.0, SLOT_BYTES).is_ok());
}
