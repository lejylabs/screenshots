# Unix main-app ownership and reopen lifecycle

Author: NewtTheWolf
Date: 2026-11-16
Status: integrated; optimized Linux core, startup and private Wayland/X11 lifecycle checks passed. Native macOS and performance acceptance remain open.

## Operational contract

Ordinary Linux X11, Linux Wayland and macOS desktop launches acquire one
current-user, graphical-session owner before constructing GPUI or starting the
file logger. A later ordinary launch or `--show` submits exactly one Show request
and exits before platform/logger/controller initialization. If forwarding fails,
it reports an error and does not create another owner. Admission has a five-second
startup-contention deadline and two-second connected-client deadline; native
filesystem calls themselves remain ordinary blocking OS operations before GPUI.

`--open FILE` remains the explicitly separate existing file-editor process. Its
requested file is never downgraded to Show-only forwarding. Native worker and CLI
entries retain main's earlier return boundary. Explicit resource probes bypass
GUI ownership. macOS file URLs retain their existing bounded native-open route,
including the pending early URL. This does not integrate the separate headless
Task host into the GUI or claim Task-host hotkeys as main-app hotkeys.

The normal scoped Unix app starts with its existing visible control window and
then initializes singleton-owned native tray/global-shortcut services. Existing
controls and design stay intact. Close minimizes only when a real tray host is
available and releases the control thumbnail atlas entry; unavailable tray or
WindowedOnly fallback follows Quit. This implementation does not claim headless
startup or Unix graphics-detach resource acceptance. A launcher can request the
existing window, and macOS also registers GPUI's native `Application::on_reopen`
for Dock or Finder reopen. Reopen and IPC consumer work is deferred to GPUI's main thread;
no native window operation runs in the IPC thread. Native focus policy can grant
attention instead of focus on Wayland. Protocol success means Show was queued or
coalesced, not proof of physical presentation or forced focus.

Quit stays provisional while recoverable capture/save/editor/storage work is
pending. Native capture input pauses, and Show can expose the existing busy
window; it cannot start a replacement capture. Failed output save cancels Quit
and resumes the same owned services/listener. Only irreversible final exit stops
Show admission. The ownership guard survives `Application::run`, native service
cleanup and GPUI teardown; then Drop retires the listener and releases the lock.
Unexpected listener closure enters Quit. Unscoped fallback starts no optional
native registration. Current service/source and verification limits follow
[Unix desktop services](unix-native-services.md).

## Authority and capability discovery

- The fixed system temporary directory contains a private `0700`, current-user
  directory (`/tmp/screenshots-UID-launch-v1` on Linux,
  `/private/tmp/screenshots-UID-launch-v1` on macOS). HOME, TMPDIR and XDG variables
  do not redirect ownership files to an unrelated user directory. The directory
  is opened with `O_DIRECTORY|O_NOFOLLOW|O_CLOEXEC` and validated before use.
- X11 hashes its bounded server authority: screen suffixes and normal local
  aliases share ownership. Different display numbers and remote authorities
  stay separate. No X11 connection, input grab or enumeration is needed.
- Wayland takes precedence when `WAYLAND_SOCKET` or a nonempty `WAYLAND_DISPLAY`
  is present. Inspection does not consume, renumber, close or mutate the inherited
  descriptor. `SO_PEERCRED` identifies the live compositor by UID/PID across
  separate connections and inherited descriptor numbers. Its PID-namespace inode
  and bounded `/proc/PID/stat` start time distinguish namespace aliases and PID
  reuse; no process command/name bytes are logged or retained. A named connection is
  inspected with one short local connection (250 ms connect deadline), then
  immediately closed; it does not bind a capture stream or enumerate outputs.
  When a PID namespace/proxy hides PID or `/proc` lifetime/namespace evidence,
  stable ownership is unavailable. A second filename-only identity scheme is not
  used: mixing schemes could split one server into competing service owners.
- macOS uses `SessionGetInfo(callerSecuritySession, ...)` from the existing
  Security framework. The returned security/audit session ID is stable across
  processes for a graphical login. A Mach port name and the invoking terminal's
  process session are not used.

Unavailable stable discovery produces the typed `WindowedOnly` capability and a
fixed, privacy-safe stderr explanation. The established separate visible-window
mode remains usable with native global services disabled. This is an explicit
capability limit; it does not establish singleton ownership in that uncommon
session. Invalid inherited Wayland descriptors fail rather than falling back to
X11. No authority name, descriptor value, image, command, frame or secret is
logged. Session keys are SHA-256 hashes rather than exposed display strings.

The identity model assumes one graphical Wayland server per compositor process;
a compositor exposing multiple unrelated graphical sessions in one PID needs a
stronger native authority identifier. Nested/proxied runtimes and X11 authority
aliases beyond the documented local forms need runtime acceptance. Unknown
identity is a visible-only capability fallback instead of a competing native
service owner.

## Private endpoint and crash recovery

Each hashed session has one persistent app-marked `0600` regular lock file,
current-user-owned with one hard link. A fully written/synchronized marker is
published without replacement (`renameat2(RENAME_NOREPLACE)` on Linux,
`renameatx_np(RENAME_EXCL)` on macOS), so concurrent first launches see one inode.
`flock(LOCK_EX|LOCK_NB)` serializes ownership. The lock file is never unlinked by
normal operation, avoiding split lock ownership between existing waiters and a
newly created inode. Foreign markers, symlinks, FIFOs, unsafe permissions and
hard-link substitution are refused and preserved. Atomic no-replace publication
must be supported by the selected system temporary filesystem; otherwise startup
fails explicitly without replacing another file.

The owner binds a random short Unix stream socket inside the same private
directory, makes it `0600`, requests a backlog of eight and writes its exact
name/device/inode plus SHA-256 record checksum to the lock file. Total record
read is capped at 128 bytes. An interrupted or torn tail is not deletion
authority. After an owning process crashes, a new holder removes only the exact
recorded socket inode after type/UID/mode/link-count checks. A replaced foreign
file stops recovery and is preserved. Partial-record or pre-publication crashes
may leave one unrecorded random socket/staging file; these do not block a fresh
owner and are preserved for explicit inspection. There is no directory scan,
implicit removal of unknown files or persistent cleanup timer. This is portable
cooperative app ownership inside a private user directory, not atomic
compare-and-unlink against a hostile same-UID actor racing final cleanup.

The client validates the recorded endpoint before and after connecting. Kernel
peer credentials authenticate both directions (`SO_PEERCRED` on Linux,
`getpeereid` on macOS) against the effective UID. The only accepted message is the
16-byte `SCREENSHOT-SHOW\n`, followed by write EOF. Prefixes, trailing bytes,
paths, arbitrary CLI, Quit and image data are rejected. The listener reads at
most 17 bytes, serves one active client, and returns a three-byte fixed result.
One pending GUI token coalesces repeat requests. No bearer token, shell command,
network listener, arbitrary argument/path forwarding or per-client thread exists.

## Resource and thread cost

The only new resident worker is a native `poll` waiter with a requested 256 KiB
stack. It is needed to bridge the portable Unix socket and cancellation fd into
the existing GPUI foreground executor without a new asynchronous runtime or
backend-specific GPUI loop changes. Idle `poll` has no timeout. A shutdown socket
wakes both blocked accept and an active slow client immediately; Drop joins the
owned thread. Only an explicit contended startup retries with 10 ms sleeps,
within its five-second admission deadline. Active client waits are bounded and
interruptible by shutdown.

Resident owned descriptors: directory, lock, listener and shutdown socket pair
(five), with one additional accepted client descriptor during a request. The
kernel listen backlog requests eight, in addition to one served connection;
platform backlog accounting/buffers remain OS costs. No frames, pixel buffers,
GPU resources, queue of paths, cache, periodic logger work or eagerly started
Tokio runtime is added. The event channel has capacity one; request/record
buffers are 17/128 bytes. Scope inspection retains at most 4097 temporary
`/proc` bytes before platform startup. Filesystem publication uses a small transient record
buffer and existing SHA-256. Existing locked `libc`, `sha2_10`, `tokio` sync and
`uuid` support all behavior; Cargo and Cargo.lock need no changes.

Acquisition starts no worker and runs before Linux's one native platform
constructor. That constructor consumes startup activation/socket environment
before GPUI's own threads; the supplied logger starts afterward. `listen` runs
only after the window/controller exists and GUI event reception can be spawned.
No new process-environment mutation is performed anywhere in this implementation.

## Historical preparation and remaining integration gates

At the original held-preparation checkpoint, only rustfmt parsing/formatting and
patch checks had run. The optimized Linux verification below supersedes that
stage; native macOS, physical session behavior and resource acceptance remain
separate. Source formatting alone never established those results.

The held core regressions cover startup requests queued before the listener,
repeat coalescing, exact request/EOF validation, closed receivers, cancellation
while idle or serving a silent client, foreign file/lock preservation,
unsafe directory/symlink/FIFO refusal, partial-record preservation, independent
session keys and close-on-exec descriptors. Private actual subprocess fixtures
cover crash/stale endpoint recovery, eight simultaneous launches and inherited
Wayland descriptors renumbered to 100/101. Each subprocess clears environment,
uses synthetic HOME/XDG/runtime/socket/session state, and cannot connect a user
desktop. Pure startup regressions cover file/probe bypass, forwarded early exit,
unknown-scope capability and error admission. Sixteen core tests and seven startup
checks passed in optimized builds; native macOS execution remains separate.

The original verification plan calls for optimized core/UI startup regressions
and all-target Clippy; these now pass locally. A later complete workspace/release
checkpoint must include this slice alongside the remaining native work.
Build on native macOS Intel/ARM SDKs; source/metadata checking from Linux is not
macOS execution. Recheck the existing native worker/CLI/Open With and descriptor-
only Wayland startup tests, since the new owner must not alter those boundaries.

Private native runtime plan:

1. **X11:** owned Xvfb plus a small private WM, isolated HOME/XDG/session, synthetic
   frames only. Launch the real release binary twice concurrently, confirm one
   PID owns the private control window and the secondary exits without GPUI/logger
   initialization. Minimize and request `--show`; native private-server window
   inspection establishes map/activation. Close through the private WM and verify
   process/listener exit and restart; kill the first owner and verify stale recovery.
2. **Wayland:** owned headless Sway, private socket/runtime, no user input/clipboard
   or portal. Repeat ordinary and descriptor-only launches with different inherited
   descriptor numbers, including an X11 DISPLAY that must not override inherited
   authority. Verify one owner, native surface reopen/attention behavior, close/quit,
   restart and private compositor death. Activation/focus depends on compositor
   policy and is recorded separately from protocol acceptance.
3. **macOS:** native CI/private fixture scope verifies Security-session ownership,
   peer authentication, startup race/crash tests and native Dock/Finder reopen on
   the main thread. Run bundled Open With separately to ensure a requested file
   reaches the existing decode worker. Real signed identity/permissions, IME,
   accessibility, physical display topology and user-facing Finder behavior remain
   native acceptance, never inferred from this source snapshot.
4. **Resources:** optimized before/after same-workload observations of process CPU,
   wakeups, thread/FD count, RSS/private/PSS (or macOS footprint), and reopen latency.
   Use a stable PID/starttime/executable identity and a five-minute idle sample.
   Compare reference budgets in PLAN; do not interpret the declared 256 KiB stack
   or fd inventory as measured resident/private memory.

Main-app tray/global-shortcut implementation is integrated; its current source,
private adapter results and remaining native Mac/GUI/physical/resource gates are
recorded in [Unix desktop services](unix-native-services.md). Actual unknown-scope sessions,
focus policy, sleep/session-lock/restart behavior, real permissions/accessibility,
signed release and reference-hardware performance remain explicit follow-ups.

Primary contracts: [Apple security session API](https://developer.apple.com/documentation/security/sessiongetinfo(_:_:_:)),
[Apple graphical login session lifetime](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPMultipleUsers/Concepts/FastUserSwitching.html).
The pinned GPUI `Application::on_reopen`, Linux `StartupEnvironment::take`, window
activation and native event-loop implementation are the local integration sources.


## Optimized private Linux verification (2026-11-17)

The actual application passed named Wayland, inherited Wayland descriptors
100/101/102 and authenticated Xvfb/Openbox route suites. Repeated Show requests
created no second window/logger owner; close/restart, crash recovery, four-launch
races, six separate file editors and three diagnostic bypasses passed. X11
minimized windows restored. Wayland scratchpad Show was acknowledged without
compositor focus or scratchpad visibility, so activation is not claimed.

A single 300-second current-only private Sway observation recorded zero process
CPU ticks, context switches and application I/O, with 54 threads and 34 FDs. The
launch-events listener independently recorded zero deltas. Show after idle took
30.54 ms in one observation, not a latency percentile. No trimming was requested.

Initial Linux private mappings were 237.68 MiB, above PLAN's 100 MiB visible-shell
target. Lower final resident mappings coincided with 86.51 MiB swap and do not
close that gate. A separate passive snapshot distinguishes heap, LLVM, application
code and graphics-library mappings; it does not attribute allocator ownership.
GPU memory, physical input/presentation, actual unknown-identity sessions, signed
macOS lifecycle and reference-hardware acceptance remain unverified.

All-target optimized workspace Clippy and Windows GNU metadata compilation also
passed. Source reviews reached two clean rounds; the test-only unsafe-directory
fixture now sets its intended 0755 permissions explicitly rather than depending
on the invoking shell umask. Full current native macOS execution remains pending.

## Listener retirement ordering correction (2026-11-17)

`b3f0a27` closes the native listener and shutdown reader before dropping the
Show sender. Observing channel EOF therefore cannot race a still-connectable
listener. Ownership still remains held through application teardown. The strict
existing regression first failed in the full optimized workspace run; after the
production correction its 16 focused controls and 32 fresh exact-case repeats
pass. The later complete core run passes 523 cases (53 ignored), with optimized
all-target Clippy and formatting passing. No polling or new worker is added;
this is ordering of the same owned resources, not a hardware/idle measurement.
