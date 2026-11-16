# Native task hotkeys and authenticated local task IPC

Author: NewtTheWolf
Date: 2026-11-15
Status: implemented; focused protocol/IPC and private-Xvfb native-hotkey/capture
checks passed. Platform-native acceptance and final release gates remain distinct.

## User-visible behavior

Run saved task profiles through arbitrary configured global hotkeys or a typed,
authenticated local request. This is an explicitly started headless host, not
an automatically installed service or a second GUI owner. It dispatches the
same `task run PROFILE` implementation and output actions already available to
the CLI. No task starts merely because its configuration is read. New UI,
application launch forwarding, the six existing capture-only GUI shortcuts and
tray behavior remain unchanged.

```
screenshots task-host validate CONFIG.json
screenshots task-host serve CONFIG.json ENDPOINT.json
screenshots task-host trigger ENDPOINT.json PROFILE_ID
screenshots task-host status ENDPOINT.json
screenshots task-host stop ENDPOINT.json
```

`serve` stays alive until a stop request or termination signal. Startup prints
and flushes only `{ "ready": true, "profiles": N, "hotkeys": K }` after native
registration, IPC listening and private endpoint publication have succeeded.
Failure to report readiness shuts down and cleans up. Trigger returns an
accepted job ID or an explicit busy/error result; it is asynchronous. For a
synchronous task use the existing `task run` command. Status reports the one
active or most recent job without task output, URLs, app titles or source paths.
A stopped host does not delete or roll back output from already completed task
actions. A fixed output filename can legitimately fail on the second run. Task
output paths support the runner's `{uuid}`/`{timestamp}` naming; the host does
not rewrite profiles or guess output paths.

## Configuration and admission

CONFIG is a regular JSON file, at most 64 KiB. Require object form, version 1,
strict known fields and no duplicates/trailing data. Relative task paths resolve
against its containing directory and are converted to absolute native paths.
Reject empty/overlong paths, directories, nonregular files and FIFO substitution
using existing nonblocking-open/handle checks. Each referenced task profile is
regular and at most 64 KiB; the task runner performs its full current admission
again when invoked, since output paths and source files can change.

```
{
  "version": 1,
  "task_timeout_ms": 300000,
  "bindings": [
    {"id":"capture-and-share", "task":"capture.json", "hotkey":"Ctrl+Shift+F9"},
    {"id":"copy-current", "task":"copy.json"}
  ]
}
```

Require 1–16 bindings. IDs contain 1–64 ASCII letters, digits, `_` or `-`, unique
within the file. A task path has at most 4096 native encoded bytes. Optional
hotkey omission/empty string means IPC-only. Keys accept Ctrl/Alt/Shift/Super,
letters, digits, F1–F24 or PrintScreen, using existing capture shortcut validation
rules and platform capabilities. Letters/digits need Ctrl/Alt/Super. Reject
unsupported platform keys and normalized duplicate bindings before registration.
All requested native keys must register successfully; roll back partial startup
on conflict rather than leaving a partially working host. Resource retention is
at most 16 profile paths/IDs/normalized hotkeys and one recent job result.

Task timeout is 100–5400000 ms (default 300000), no implicit retry. No hotkey
reconfiguration polling, config watcher or periodic registration check. Restart
the explicit host to load an edited config.

## Native event sources and lifecycle

The native hotkey loop runs on its platform-correct thread. Registration and
unregistration stay on that thread. A hotkey callback only attempts bounded job
admission and returns; it does not execute task/capture/network/image work.
Duplicate key-repeat events while busy are dropped, with no queued task or
repeated log. Pressing a configured key after completion can start another job.

- **Linux X11:** use one operation-owned Xlib connection, XGrabKey registrations
  with bounded lock-modifier variants and an event-driven wait on the X11 fd
  plus a shutdown pipe. Require local numeric DISPLAY and the appropriate X11
  session. Map normalized keys to X keysyms/keycodes. Trap registration errors,
  undo all reservations on conflict, and release before closing the connection.
  No permanent `poll_for_event`/sleep loop. The pinned global-hotkey Linux backend
  has a 50 ms polling sleep and is intentionally not used here.
- **Windows:** use RegisterHotKey with thread-owned registrations, MOD_NOREPEAT
  and GetMessageW. A native posted wake/quit message releases the loop. Use
  explicit VK mapping for the admitted key set. The pinned global-hotkey backend
  starts release-poll threads; this host needs only presses and does not use it.
- **macOS:** use the installed global-hotkey Carbon registration path for normal
  keys only, with a native Carbon application event loop. A CFRunLoop source
  wakes the main thread to request loop exit; unregister keys and remove that
  source on the same thread. No media-key event tap is created. Signed identity,
  permission and actual OS hotkey acceptance remain native checks.
- **Wayland:** implement GlobalShortcuts via the session portal when an available
  D-Bus binding can be used without an eager additional runtime. Portal consent
  can be required by the OS when the user explicitly starts this host; do not
  bypass it or pretend a denied/unavailable portal registered keys. IPC-only
  profiles remain usable on Wayland without starting a hotkey backend. A missing
  adapter must fail explicitly for configured keys and stay an open capability,
  rather than fall back to X11 input grabs on a Wayland session.

No permanent timers are introduced. SIGINT/SIGTERM or console termination sets
shutdown, stops native registrations, wakes the blocking IPC accept and cancels
the active job. Unix signal observation may use one parked signal waiter; it
must block on its native pipe rather than periodically sample a flag. Windows
uses a console handler plus native loop wake. Worker and listener stacks request
256 KiB. All owned threads/handles are joined/closed on ordinary shutdown.

## Local IPC authentication and private endpoint

Bind a TCP listener only to IPv4 loopback with an ephemeral port. There is one
IPC listener thread with blocking accept and per-connection read/write deadlines
of two seconds. It handles one client at a time; no per-connection thread or app
request queue. Stop wakes a blocked accept through an operation-owned loopback
connection. A slow/malformed local client cannot create unbounded buffers or
threads. No DNS, nonloopback binding or remote connection is performed.

Publish ENDPOINT as a new private file only after readiness. It contains version,
loopback port, process ID and a random 64-hex-character bearer token constructed
from two UUIDv4 values (244 random bits). Token values are never in arguments,
stdout, logs, error messages or environment variables. The endpoint is explicit
user state necessary for the requested host; it is not a credential-store entry.
Use mode 0600 and current-owner/regular/no-symlink checks on Unix, and a protected
current-user-only DACL plus owner validation on Windows. Use one restrictive
operation-owned directory where necessary to protect publication/removal. Reject
unsafe ownership/permissions, hard links, reparse points and preexisting endpoint
paths, including broken symlinks. Do not silently remove a stale endpoint from a
previous crash; report it and require explicit operator removal after verifying
its process state. Ordinary shutdown removes only the exact endpoint inode/file
identity this host published, leaving a replaced path untouched.

The client opens and validates that endpoint with the same native ownership and
permission checks before trusting its port/token. File size is at most 4096
bytes. Use the opened file's bounded extent, not a second pathname read.
Requests and replies use a four-byte big-endian byte length followed by one
UTF-8 JSON object. Admit 1–4096 payload bytes, strict object/field/version checks,
no duplicate fields or multiple objects. The request shape is:

```
{"version":1,"token":"...","command":{"type":"trigger","profile":"capture-and-share"}}
{"version":1,"token":"...","command":{"type":"status"}}
{"version":1,"token":"...","command":{"type":"stop"}}
```

Token comparison uses equal-length constant-time byte accumulation. A request
can name only an existing profile ID, never arbitrary paths, commands or task
JSON. Authentication failures give a fixed response without echoing tokens or
payloads. Unknown fields/IDs, oversized/truncated frames, wrong versions and
invalid tokens cannot launch a task. The bearer endpoint authenticates same-user
clients; it does not isolate against a compromised current-user process or an
administrator. Do not claim resistance to those stronger adversaries.

Responses contain a fixed `ok`/`error` result, job ID and state as appropriate.
Status exposes only idle/running/completed/canceled/timed-out/failed and an exit
code/boolean, never child diagnostics or action results. A disconnected requester
after accepted admission does not cancel or repeat the task. Trigger's success
means admission, not task completion. Stop's reply acknowledges shutdown begun;
the host exits only after child cleanup and endpoint/registration release.

## One active task and child ownership

Use a shared mutex/state for one active job, one last outcome and a shutdown
flag. Busy checks and job-ID assignment happen atomically. A hotkey and IPC
trigger racing can admit at most one task. No backlog is retained. An active
operation can have one coordinator thread; it owns the child and waits only
while work exists. Its bounded timeout/cancellation checks may run at 25 ms
intervals, consistent with existing active process controllers; idle native/IPC
threads stay blocked. A finished worker releases admission and retains only
its small status. Reap/join completed worker ownership before replacement.

Spawn the exact current executable with `task run ABSOLUTE_PROFILE`, without a
shell and with stdin null. Linux uses `/proc/self/exe` to pin the running image,
so atomically replacing its pathname cannot break later jobs or mix parent/
child bridge versions. Other platforms use their current executable path and
retain native upgrade/restart requirements. Drain and discard bounded streams (at
most 256 KiB/16 KiB), do not print them. Exceeding the stream limit aborts rather
than allocating/logging arbitrary output. A profile must not become able to
replace the host's command line or inject extra CLI options.

On Windows create suspended, assign to a kill-on-close Job Object, then resume,
so shutdown/timeout cannot lose a running unowned child. On Linux use a dedicated
process group and parent-death SIGKILL, with the same pre/post parent identity
checks as existing recording controllers. Ordinary cancellation sends termination
and reaps; after a bounded grace it kills the group. The task runner already
owns separate subprocess groups for external commands; cooperative termination
allows their cleanup. Linux descendant parent-death behavior and Windows nested
Job Objects are additional containment. macOS escaped process groups and arbitrary
kernel stalls cannot receive an invented hard teardown guarantee; preserve and
document the actual native limits. Do not launch a replacement after a timeout
while an old task remains unreaped.

Hosted copy actions use an [anonymous clipboard bridge](task-clipboard.md),
transferring finalized RGBA/link bytes to one host-owned native clipboard
instance. The task can then finish and the next hotkey is ready while paste
continues to work. No native clipboard is initialized until an explicit copy
action. Partial transfers cannot replace its prior content; native copy errors
remain truthful errors. A live owned clipboard is a retained consumer, counted
separately from an empty host's idle baseline.

## File ownership and verification phases

Own `apps/desktop/src/task_host.rs`, native siblings `task_host_x11.rs`,
`task_host_windows.rs`, `task_host_macos.rs`, endpoint/security and process-control
siblings as needed; `apps/desktop/tests/task_host.rs`; a private-Xvfb end-to-end
script; and this document. Root wires main/CLI and dependency features only after
review. No existing GUI rendering/control source changes. Existing task runner
and record helpers are changed only after coordination with their owners.

First implement strict config/frame/auth/status and private endpoint lifecycle;
second the one-job process runner and IPC server/client; third native hotkeys;
then deterministic/real local end-to-end tests and release evidence. Focused
fixtures cover schema/size/owner/symlink boundaries, exact IPC framing and failed
auth, duplicate host startup, accept/shutdown wake, concurrent admission, timeout,
output caps, process cleanup and non-destructive replaced-endpoint handling.
An actual synthetic saved task must run through both loopback IPC and a physical
key event generated only on a private authenticated Xvfb server. Observe that a
second trigger reports busy, no hidden queue starts, and completion allows the
next run. Foreign-user/ACL tests run only with synthetic entries where the OS
permits; missing native Windows/macOS/Wayland runtime evidence stays open.
Measure optimized five-minute idle CPU/wakeups and retained host memory, then
active-job CPU/RSS using synthetic files. Report every thread/runtime and target
versus measured value; no broad real-desktop capture, credential access or user
clipboard overwrite is needed for these host tests.

## Completed development checks

Three focused internal host fixtures passed strict JSON/framing/token checks,
profile resolution/duplicates, and Unix endpoint permissions/replacement safety.
Seven actual-executable fixtures passed authenticated dispatch, repeated dynamic
outputs, failed authentication/unknown profile refusal, duplicate-host refusal,
one-job/busy behavior, timeout, owned subprocess cleanup and private endpoint
replacement preservation. The absolute two-second frame deadline rejects a
trickled unauthenticated payload. On Linux, a copied running host continues to
execute tasks after its executable is unlinked and replaced at the same path
(`target/task-host-executable-final.log`). Windows GNU app/test compilation passed.

The explicit `scripts/test-task-host.py` check passed real Ctrl+Shift+F9 events
on a private authenticated Xvfb server with TCP disabled. Two hotkey jobs and
authenticated IPC jobs captured only the script's own active window/display and
primary display. Independent FFmpeg checks verified finalized/redacted output
pixels. Holding a key across completion did not launch repeat jobs; a second
physical press did. The fixture marks its own private RandR output primary and
publishes only private-server EWMH identity metadata; no user display receives
input. Actual Windows/macOS/Wayland permission/runtime acceptance is not implied.

An initial five-minute idle observation lacked saved process starttime/executable
identity and reported a decreasing `/proc` high-water counter. It is preserved
as ambiguous in `target/verification/task-host-native-idle-pre-bridge-ambiguous.json`
and is not counted as performance acceptance. A repeat with stable process
identity, executable inode and swap/private/PSS/RSS evidence is recorded separately.

The final clipboard-consumer observation held a finalized image for 300 seconds.
PID, process starttime, executable device/inode and command name matched in every
sample. CPU time and summed thread context switches did not increase, and five
threads remained present. An explicit paste after idle still returned exact
alpha and hidden RGB; the native encoded PNG representation was 252 bytes.
Observed RSS ranged from 16,624 to 2,928 KiB, with 2,856 KiB swapped at the end;
the largest observed resident-private snapshot was 12,672 KiB. The `/proc`
VmHWM counter was nonmonotonic even with stable identity, so it is not used as a
validated peak-memory gate. Private resident approaching zero does not mean
zero private footprint: swap remains a live-consumer cost. No trimming was
performed. Linux documents VmHWM/RSS counter inaccuracy in
[proc_pid_status](https://www.man7.org/linux/man-pages/man5/proc_pid_status.5.html)
and asynchronous RSS accounting in the
[kernel proc documentation](https://www.kernel.org/doc/html/latest/filesystems/proc.html).
Raw snapshots and the post-idle paste result are in
`target/verification/task-host-final-clipboard-idle.json`. This development-PC
observation does not establish all reference-hardware/platform performance gates.
