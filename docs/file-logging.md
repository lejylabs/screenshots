# Bounded desktop file logging

Status (2026-11-16): implemented, with fresh source reviews six and seven clean.
The optimized workspace, private executable/worker checks and native callback
report below pass. This implements PLAN's bounded file-logging requirement;
native Windows/macOS runtime and reference-hardware idle/startup gates remain open.

## Startup and privacy

Normal graphical startup initializes the logger after native-helper and headless
CLI dispatch. Commands, owned helper processes and failed CLI requests return
before logging starts. They do not create an application log directory. The
logger does not read the clipboard, frames, credentials, image files or history.
The worker uses the parent directory of the normal application-data history path,
with a dedicated `logs` subdirectory; it does not open the database.

Each event retains only static tracing metadata and a monotonic instant. The
record contains elapsed milliseconds, level, bounded symbolic target/module,
source line number and the cumulative dropped-record count. All event fields,
including the message, errors, paths, titles, filenames, URLs and text, are
omitted. The layer never calls their visitor, Debug or Display implementation.
Metadata file paths and event names are also omitted: these can contain absolute
compiler/registry paths. Target/module values longer than 96 bytes or containing
anything except ASCII alphanumeric, underscore and colon are replaced entirely
by `redacted`. A record is at most 512 bytes, including its newline.

`RUST_LOG` accepts level/target directives through the existing tracing library's
static `Targets` filter, with an INFO default when absent/invalid. Environment
input is limited to 2048 bytes; bracket/brace directives are explicitly rejected.
Span/field predicates are unsupported; the filter
never formats dynamic fields. Increasing a level does not enable sensitive field
logging. There is no automatic crash report, backtrace collection, frame dump or
network delivery. Any future such diagnostic needs a separate explicit opt-in;
this logger adds no implicit consent mechanism.

## Resource and ownership contract

The foreground logging layer's record/transport path performs no file access,
text formatting, heap allocation, Mutex acquisition or retry-until-success loop.
It retains a static metadata reference and instant in a fixed-capacity 128-message
`crossbeam-queue` ArrayQueue. Version 0.3.14 was already in the locked production
graph; it is now an explicit dependency for this demonstrated bounded path.

A single strong atomic compare-exchange attempts exclusive queue access once.
Contention and full capacity drop the event and increment its counter. The guard
uses the maintained queue's `push_mut`/`pop_mut`, whose pinned source contains no
retry loop or allocation, and releases access before native notification, waits,
formatting or file I/O. The UnsafeCell wrapper only establishes that exclusive
mutable borrow; it does not implement a new queue algorithm. Shared ArrayQueue
`push`/`pop` have retry/backoff paths and are deliberately unused. Ordinary
tracing callsite registration remains external library behavior; dynamic field
formatting is still never requested by the layer.

Successful publication leaves one coalesced native wake credit: Linux uses a
nonblocking CLOEXEC eventfd, Windows an unnamed noninheritable auto-reset event,
and macOS one owned Mach semaphore. Signal/wait entrypoints are warmed before
attaching the layer. A notification attempts at most two native signals, with no
wait-for-space or userspace spin. Native calls and OS scheduling are explicit
costs; this is not a hard real-time kernel-latency claim. There is still only one
worker with a requested 256 KiB stack. It blocks in native poll/event/semaphore
wait when idle, without timers, idle polling, redraws or disk writes. Completion
and test acknowledgements may use ordinary channels away from the event callback.

Publication precedes the pending flag's AcqRel RMW. After consuming a native
credit the worker clears pending using another AcqRel RMW, then checks the queue
again. A producer coalescing into an existing true flag precedes the consumer's
clear, so its publication is included in that recheck. A later producer leaves
another native credit. If a worker's one queue attempt finds a producer-owned
gate busy, that owner's release notifies afterwards. The worker never signals
itself merely for an empty pop, and never sleeps holding queue access. The same
native credit wakes shutdown and the last admitted sender. Repeated native signal
failure closes admission and marks failure; it does not create another worker.

Four fixed files, `events-0.log` through `events-3.log`, each have a 256 KiB cap
including a fixed ownership header, for at most 1 MiB written by this logger.
All four files are opened and exclusively locked before any existing log is
truncated. Opening uses native no-follow/reparse-point semantics and rejects
nonregular or multiply-linked files. Unix slots must belong to the current user
and have no group/other permissions. Existing foreign headers, oversized files,
symlinks, nonregular slots or another live owner disable logging without deleting
or truncating those paths. New files use mode 0600 on Unix and normal user
application-data security on Windows.

Rotation selects the oldest retained slot at startup and cycles through the four
already-opened handles. It truncates only those admitted handles, never shared
pathnames. There is no rename/unlink rollback or accumulation of dated/session
files. Partial initialization releases all admitted locks explicitly; a
concurrent Unix fork cannot extend ownership beyond the original logger's
unlock. Malformed pre-existing headers, including an interrupted initial header,
remain untouched and disable logging until repaired outside the logger. Unrelated
files in the directory are never retention-cleaned and are outside its own-file
disk ceiling.

Any file error stops that worker, closes admission and emits one generic
stderr notice without printing a path or underlying diagnostic. No replacement
worker is spawned. A later queued event is never presented as a successful write.
Normal shutdown closes admission and waits for already admitted foreground
callbacks before its final bounded-queue drain. A fixed atomic counter registers
each producer before a second closure check; the final producer wakes the worker
without polling. A producer paused before enqueue therefore cannot leave an
acknowledged record behind the final summary. The worker records final write/drop
counters and releases the files. The caller
waits for at most 250 ms for acknowledgement. If native filesystem I/O stalls,
it stops waiting and leaves the single worker to finish or terminate with the
process; there is no unbounded join or repeated stranded-thread creation. An
acknowledgement means write calls completed, not fsync/power-loss durability.
Abrupt process termination can lose queued events.

## Verification plan and current evidence

`file_logging_tests.rs` exercises the real layer → bounded queue → worker → file
path, dynamic-field nonvisitation, compiler-path omission, caps/rotation,
exclusive ownership, partial initialization, foreign data preservation, real
read-only-handle write failure, idle file stability and bounded shutdown waiting.
Unix fixtures include a private FIFO, symlink and hardlink. All use owned
temporary directories; they do not inspect or change real application logs.

`scripts/test-file-logging.py` additionally runs help/version/ruler/error CLI
requests with isolated HOME/XDG/application-data paths and verifies no logging
directory or files are created on Unix. Windows KnownFolder lookup ignores those
environment paths, so the driver explicitly skips main-executable CLI requests
there until an actually private profile is available; it does not risk real
application data to test a regression. The sixteen Unix/fifteen Windows worker fixtures
use explicit private temporary paths. The driver records both executable hashes.
No GUI, user input,
clipboard, provider, certificate or real application-data access is authorized by
these checks. Optimized callback cost and idle CPU/private-memory evidence must
be recorded separately from bounds and targets.

The ignored `optimized_callback_cost_against_formatted_sink` fixture compares
100 batches of 2000 caller events against the old formatter routed to a sink.
It reports distributions of batch-average nanoseconds/event for full-queue and
held-gate contention. A separate 100 successful callback sample set runs after a
stable native-wait entry counter and cleared notification; its callbacks include
actual native signaling. It does not prove per-sample scheduler sleep state, GUI
presentation, idle CPU/private memory or reference-hardware acceptance. Thread-
local test allocator counters verify zero Rust allocations on contention and
successful native-notify paths; the production allocator is unchanged. No new
benchmark result is inferred from source bounds alone.

The coordinated `release-fast` workspace run passed 953 tests, failed zero and
left 69 explicit opt-ins excluded across 59 outer Cargo suites. The current
private driver separately passed all 16 Unix worker fixtures and four actual
headless CLI requests, with the complete isolated state tree unchanged. The
main executable SHA256 is
`d2bc869a6ea132bc00e5f3da81a22c0ebe6d59ffe712bdf958975d36e1fc90a9`;
the driver records its matching test executable hash in
`target/verification/file-logging-current-private-e2e.json`.

On this Linux development machine (Ryzen 7 9800X3D, Rust 1.100.0), the explicit
optimized report observed p50/p95 of 730/1220 ns for 100 successful callbacks
including native notification. Full-queue batch averages were 42/64 ns/event,
held-gate averages 32/33 ns/event, and the old formatted sink 320/365 ns/event
(100 batches of 2000 events each). Full-queue and held-gate events are dropped;
these samples do not measure successful file delivery. The 24-byte message
makes the 128-item queue payload 3072 bytes, excluding queue/atomic metadata.
Raw reports, machine details and hashes are in
`target/verification/current-three-performance.json`. These short development
observations do not establish native Windows/macOS ABI, five-minute idle CPU,
process private memory, GPU, presentation or reference-hardware budgets.


## Historical review corrections

The pending-check statements below describe each earlier checkpoint. The
coordinated results above supersede those execution states.

### First review

The first fresh EGM review found a Windows fixture reading through a separately
opened handle while the log owner held its exclusive native lock. The baseline
now reads through the admitted owner handle before replacing it for the real
read-only write-failure check. The same review identified the shutdown sender
race and the pinned filter's static field-predicate grammar. Both are corrected
above. A signaled private fixture pauses a genuinely admitted producer until the
worker has observed shutdown and deferred exit, then verifies delivery before
the final summary. Additional filter controls verify normal target/level rules
and explicit field/span rejection. The admission counter adds one fixed atomic,
no foreground lock, allocation, timer or queue. The synchronization hook exists
only in test builds. All new compilation/execution checks remain pending; the
first review was not clean and no convergence is claimed.

The second fresh review found stale fixture counts in the executable driver and
this document. At that checkpoint they became 12 Unix/11 Windows regular fixtures;
the later native-wake cases bring the current counts to 16/15. That round was
not clean. Failure closes event admission before attempting its generic stderr
notice, so a blocked diagnostic pipe cannot keep accepting new file records.
Compilation/execution of this exact latest ordering and current driver remain
pending; no clean convergence is inferred from earlier checks.

The third source review found a weak-memory ordering gap in the last-producer
wake. Shutdown, worker observation and producer wake now all use sequentially
consistent stopping operations alongside the in-flight counter. If the worker
observed shutdown while a producer remained active, that producer's later
decrement and stopping read must observe the stop and deliver its wake. Otherwise
the worker observes zero and drains directly. The signaled interleaving fixture
remains; it is not claimed to simulate every weak-memory execution. Review three
was not clean, and exact-current execution is pending.

The fourth source review found that the CLI driver checked files but missed
empty directories. It now compares the complete private directory-entry set
before/after headless commands, including directories and symlinks. Its own
initial fixture folders form the baseline. Review four was not clean; current
driver execution remains pending.


### Fifth review: replace the standard channel primitive

Review five identified that the earlier `SyncSender::try_send` could acquire
`SyncWaker`'s Mutex while notifying a sleeping receiver in pinned Rust 1.100. Its
nonblocking queue-capacity API therefore did not meet PLAN's no-render-path-lock
contract, and the earlier full-queue benchmark did not exercise this success/wake
path. The primitive is replaced by the one-attempt queue/native-credit path above;
that review is not clean and no convergence is inferred.

Source checks cover the complete foreground call chain, the pinned mutable queue
operations, actual native wake APIs and the prior sequentially consistent sender/
shutdown proof. New private cases retain a wake before wait, coalesce many
notifications into one credit, wake the parked writer with a successful callback,
exercise an intentionally held queue gate without Rust allocations or waiting,
and inject notification failure before last-sender cleanup. Earlier file/header,
filter, directory-tree and admitted-producer regressions remain. All current
compilation/execution/native ABI checks are pending until explicitly recorded.

Primary contracts: [pinned queue source](https://docs.rs/crossbeam-queue/0.3.14/src/crossbeam_queue/array_queue.rs.html),
[eventfd semantics](https://man7.org/linux/man-pages/man2/eventfd.2.html),
[Windows auto-reset events](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-createeventw),
[SetEvent](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-setevent),
and Apple's [direct semaphore signal/wait traps](https://github.com/apple-oss-distributions/xnu/blob/main/libsyscall/mach/semaphore.c),
[semaphore ABI](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/mach/semaphore.h)
and [create/destroy ABI](https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/mach/task.defs).
Exact local source hashes and downloaded primary contracts are retained in
`target/verification/file-logging-native-wake-proof`.
