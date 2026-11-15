# Persistent clipboard ownership for hosted tasks

Author: NewtTheWolf
Date: 2026-11-15
Status: implemented; six focused protocol/inheritance fixtures and private-Xvfb
image/link ownership checks passed. Native Windows/macOS runtime and full release
acceptance remain separate.

## Outcome and ownership

A hosted task's explicit Copy image/Copy link action transfers finalized data to
one clipboard owner in the long-lived task host, then the task can finish and
release its job slot. The host keeps only the native clipboard representation
until another explicit copy replaces it or the host exits. This removes the
finite-child hold/busy conflict on Linux. Standalone `task run` preserves its
existing explicit clipboard hold behavior. Nothing reads/copies a clipboard at
host startup, validation, capture, or a task without a copy action.

Store one `Arc<Mutex<ImageClipboard>>` in Host and initialize it lazily on the
first actual transfer. The existing native adapter is reused, not an independent
clipboard implementation. A running task retains its source; the host temporarily
allocates only the bounded final transfer image/text and drops that decoded copy
after the native copy. No PNG/base64/disk roundtrip, source snapshot, copy
verification or extra idle timer is introduced. Native ownership can retain one
output representation when hidden, counted separately under PLAN's clipboard
consumer budget. Host stop releases that ownership; it does not claim to keep
paste data available after the user explicitly stops the owner.

## Anonymous inherited bridge

Use an operation-owned anonymous full-duplex Unix socket pair on Unix, or two
anonymous pipes on Windows. No named filesystem socket/pipe path or listening
port carries image data. Parent-only endpoints are noninheritable; only the two
child pipe handles (or one Unix socket fd) can reach the exact `task run` child.
On Unix clear CLOEXEC only inside the child pre-exec closure, leaving the parent
copy protected. On Windows use inheritable child ends and noninheritable parent
ends during the one bounded host spawn. The child immediately restores
CLOEXEC/noninherit before any profile operation or external command.

The environment contains only descriptor/handle IDs, a version and a fresh UUID
challenge, never pixels, links or credential payload. Task entry calls
`Client::from_env()` before reading/capturing its source. No environment value
means standalone execution. A partial/invalid value is an error, not fallback.
Reject stdio/invalid descriptor numbers and handles of the wrong native type.
Clear the bridge environment on every external `Command` so nested programs do
not inherit an ambient channel reference; this avoids unsafe process-environment
mutation after threads exist.

Before any image can be written, perform a fixed-magic/version handshake with
the exact per-spawn UUID echoed by the server. A stale or forged ambient fd must
fail before capture/transfer rather than sending source pixels to an unrelated
file or pipe. The IDs/challenge are anti-confusion framing, not an authentication
claim against a compromised current-user process. Actual privacy comes from
anonymous inherited handle ownership. Root's existing Job Object/process-group
controls still own the child.

## Bounded binary protocol

Each packet has a fixed 32-byte header: eight-byte magic `SCTCLIP1`, u32 big-endian
kind, width, height, u64 big-endian payload length, and a reserved u32 zero.
Handshake kind 0 has zero dimensions and exactly 16 UUID bytes; response kind
100 echoes those bytes. Image kind 1 requires nonzero dimensions, exact tightly
packed RGBA8 `width*height*4`, and at most 256 MiB. Text kind 2 requires zero
dimensions and 1–2048 UTF-8 bytes without control characters. Success kind 101
and failure kind 102 have zero dimensions/length/reserved. No packet or error
echoes image/link data or native diagnostics. The host accepts at most 16 copy
actions on one task channel, matching the task action bound.

Read/write through bounded 64 KiB buffers. Validate header arithmetic and byte
limits before allocation; allocate one image Vec with try_reserve, consume it as
RGBA without a second decoded copy, and do not touch the clipboard before a
complete validated payload arrives. A truncated/canceled/oversized/unknown packet
fails without changing the previous owner. Native copy failure returns a fixed
failure acknowledgment and truthful error; the OS may already have changed its
clipboard, so there is no invented rollback guarantee.

Only a successful native copy sends acknowledgment. The task reports that action
complete only after receiving it. Native calls retain the existing platform's
blocking/permission limitations; hard interruption inside a clipboard API is not
promised. Task/pipe deadlines and parent cancellation close the stream/kill the
child and join bridge workers. Do not clear the busy slot or launch another task
while an old bridge/native copy is still unjoined. Late canceled work must not
replace a newer task's clipboard.

## Interfaces and exact files

Create `apps/desktop/src/task_clipboard.rs` with `Client::from_env() -> Result<Option<Client>>`,
`Client::copy_image(&mut self, &RgbaImage) -> Result<()>`,
`Client::copy_text(&mut self, &str) -> Result<()>`, and a public-to-crate environment
constant. Host-side `Prepared::new(&mut Command) -> Result<Prepared>` configures
handles/environment; after child spawn `Prepared::start(Arc<Mutex<ImageClipboard>>,
Arc<AtomicBool>) -> Result<Bridge>` drops parent copies of child ends and starts
the bounded reader, checking job cancellation before a native copy.
`Bridge::failed()` informs the child controller and `Bridge::stop()` closes/wakes
I/O and joins. Drop performs the same owned cleanup.

Modify only `task_host.rs` to hold/pass the owner and `task_host_process.rs` for
bridge preparation/start/cancellation/cleanup. Root owns main module wiring and
`task.rs`: initialize Client before Task::read; route copy actions through it when
present, otherwise use the existing standalone adapter; skip the finite hold for
a hosted copy, and remove the bridge environment from external commands. The
standalone Linux hold requirement must be enforced before source capture, while
hosted profiles can use zero hold. Do not rewrite a profile file or add UI state.
Update task-host/tasks/delivery/status documentation only after running checks.

## Verification

Unit-test fixed binary framing, UUID/magic mismatch, native descriptor/handle
type checks, exact alpha/hidden RGB transfer, text UTF-8, budgets and overflow,
unknown kinds, truncated payload and no side effect before the full payload.
Use an injected test sink for ordinary tests; do not read/write user clipboards.
Test child handle inheritance/noninherit restoration through a private helper
process. Explicit private-Xvfb E2E must run an actual task copying its finalized
image, observe job idle while host remains owner, read exact pixels through an
independent clipboard consumer, run a second hotkey task replacing the image,
and verify new exact pixels without an unbounded queue. Include text/link
ownership through a synthetic local upload/action chain without real providers.
Confirm host stop removes its endpoint and releases clipboard workers.

Run focused optimized tests and Windows compilation, then full workspace and
release checks. The five-minute idle observation must state that clipboard
ownership is a live consumer and include the retained representation separately.
No external account, user image, real desktop event or user clipboard is touched
by unattended tests; all native clipboard work occurs on the private X server.

The six optimized fixtures passed independent binary framing, exact alpha/text,
malformed/oversized/truncated refusal before side effects, fixed failure ACKs,
client backing/text bounds, and real child handshake/noninherit restoration. A
grandchild deliberately receives stale environment IDs and cannot reopen the
protected channel; the injected test sink performs no native copy.

The actual task-host private-Xvfb fixture passed image/link ownership after job
completion with zero finite hold: finalized opaque redaction and surrounding
pixels matched through independent image/png selection retrieval plus FFmpeg.
A second physical hotkey job replaced the image while the host remained alive;
one local upload copied an inert URL, then another image job replaced that text.
The worker slot was idle between these copies. Host stop removed the endpoint
and released the native selection owner. Windows GNU app/test compilation passed;
these Linux checks do not imply native Windows/macOS/Wayland runtime acceptance.
The final private fixture also preserved 0/128/255 alpha and unrelated RGB under
zero alpha through the native image/png owner and an independent decoder. After
300 idle seconds with verified process identity, an explicit paste still
returned those exact bytes. Full source/representation/swap observations and
the nonmonotonic VmHWM caveat are recorded in [the host evidence](task-host.md).
