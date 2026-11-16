# Bounded native still-capture jobs

Author: NewtTheWolf
Date: 2026-11-16
Status: implemented; focused protocol/process and private native checks passed.
Fresh review round two found no remaining findings after the corrections below;
native-platform CI remains a separate gate. Wayland follow-up
remains an implementation obligation, not closed by this stage.

## Problem and boundaries

Previously, still capture/enumeration ran synchronously on the invoking CLI or
existing GUI worker. Task process supervision could kill a complete Task child,
but the GUI backend could not interrupt a blocked XCap/GDI/compositor call.
Checks before/after a native call alone were not a native timeout. Replacing a
timed-out thread would have left live captures and unbounded memory behind.

Use one operation-owned child for native enumeration/capture, with a bounded raw
RGBA channel. Preserve the existing in-memory capture contract, source process
guards, pixel/geometry admission, explicit outputs and UI controls. No worker,
connection, runtime or timer starts at normal launch. No PNG, file, clipboard,
history, upload, socket endpoint file or image log is part of this protocol.

The first scope is display/window listing, display/window/region batches,
foreground targets and prepared bound-region capture. Thread-correct parent
window/hotkey/DPI operations stay on their required thread. This does not claim
to interrupt arbitrary kernel calls or every native UI operation.

## Core and executable integration

Core remains independent of the desktop executable. Add a small typed native
call/reply API and an explicitly installed executor function pointer. Installation
stores metadata only. The normal desktop entry point installs its private
process executor before routing regular CLI/GUI commands. The hidden child entry
is handled first and never installs that executor, preventing recursion. Core
unit consumers with no installed executor retain the explicit inline backend;
that case must not be advertised as having native process deadlines.

Calls contain only bounded target/options/geometry metadata, not task profiles
or output actions. The child uses the same core inline implementation. Requests
carry the originating process ID so own-window rejection continues to protect
the parent, rather than accidentally excluding only the new helper process.
Prepared-template capture is one native operation after its parent-side delay,
so repeated metadata lookups do not require five executable starts. Parent output
paths remain under parent control; returned native pixels cannot select a save
path or execute a task. Task Source Template never executes catalog profile
metadata recursively.

Expose cancellation-aware core entry points alongside the existing convenience
methods. The existing task token/deadline and GUI session cancellation must reach
the process controller. Delays happen before child startup and use the existing
finite operation wait. Native job deadlines exclude an explicitly selected
one-hour capture delay. Late results are discarded by the existing request/
session generation checks; a synchronous transport also checks cancellation
before returning any completed image group.

## Private transport and lifetime

Use inherited anonymous bidirectional sockets on Unix and anonymous pipes on
Windows. IDs may be passed in a dedicated environment variable; pixels and task
credentials never are. A fixed protocol magic/version and per-spawn random
challenge must complete before native work or image transfer. Reject absent,
stale, standard-stream or wrong-type handles. Set close-on-exec/noninheritance
in the child immediately after accepting those handles and before any native
backend can invoke an external helper. Remove stale Task clipboard bridge
environment entries from the native worker's command environment.

Parent admission permits one active native child and no waiting backlog. A
concurrent capture returns busy. A capture may cancel active enumeration; one
caller may await its actual relinquishment for at most 500 ms, while additional
contenders are refused. The handoff reservation remains exclusive after the
enumerator releases admission and until that owning capture claims it; neither
a fresh enumeration nor a third capture can take the idle-looking slot.
No native job is queued or started early. Display/window
inventory and prepared-template capture are coalesced into one operation each.
A timed-out/cancelled job retains admission
until its child and transport ownership are finished; do not overlap a replacement
with an escaped native call. Linux uses a process group and parent-death signal;
Windows atomically assigns a kill-on-close Job Object and explicit inherited
handle list with `PROC_THREAD_ATTRIBUTE_JOB_LIST`/`HANDLE_LIST` at creation.
There is no unassigned suspended-child interval.
macOS uses an operation-owned parent-pipe watcher because it has no Linux
parent-death signal. After validating the inherited channel, Linux arms a
parent-loss signal handler; both handlers kill the worker's own process group,
including ordinary descendants. The pre-exec Linux parent-death signal covers
the earlier startup interval. This does not contain a native helper that
deliberately leaves its owned group. Failed handshake, malformed data,
read/write failure and ordinary cancellation terminate the owned family.

On Unix the leader stays waitable, including as a zombie after normal exit,
until every group signal has finished and no live group member remains. Its
unreaped PID reserves the group identity; no group signal is sent after reaping
it. Active Linux `/proc` inspection is capped at 65,536 entries, 8 MiB read,
1,024 live members and 50 ms per inspection. One `getpgid` per enumerated PID
filters unrelated processes before opening a stat file; matching stat records
confirm group and live state again. There is no UID-based omission of members.
macOS uses a bounded libproc group
query with the same member/time caps. Exit-ready pidfd/kqueue waits handle live
members without an idle scan. An incomplete/failed inspection preserves owned
pending state instead of guessing that the group is empty. Membership is
rechecked after exit-event registration so a reused member PID cannot become
a wait on an unrelated process.
The inspection uses a 4-KiB stat buffer and a bounded PID list; an unresolved
Linux group can retain at most 1,024 pidfds plus their kernel objects in its
already-owned coordinator. macOS uses one kqueue. These are active/pending
cleanup costs, with no resident scanner or additional cleanup thread.

One operation-owned parent coordinator thread owns OS startup and transport, so
a blocked startup syscall does not trap the GUI caller indefinitely. The parent
controller uses finite active waits and the existing cancellation callback,
with no timer outside an active operation. Defaults are 5 seconds
for enumeration/identity and 20 seconds for a still-capture operation, including
startup and transfer. Portal consent needs a separately explicit longer policy;
do not silently reuse a 20-second deadline while promising unbounded permission
input. Native errors return fixed bounded categories, preserving stale-region
handling without replaying native strings. Stdout/stderr point to the null
device; rasters travel only over the authenticated private channel. The worker
inherits a whitelist of necessary OS/session/library variables, with unneeded
task credentials and stale clipboard capabilities removed.

Linux `/proc/self/exe` pins the running image across updater unlink/replacement.
Other platforms retain their documented executable replacement/restart limits;
protocol/version mismatch fails before frames, rather than mixing implementations.
Kernel-uninterruptible termination remains an OS limitation. After at most
250 ms cleanup grace, the caller receives `NativeCleanupPending` while one
global pending slot retains the coordinator/reaper handle, process/job ownership
and admission. No replacement starts. The reaper waits for confirmed exit;
unresolved descendants remain quarantined and are rechecked on a later explicit
request. There is no pending-state polling timer. Completed/partial parent
frame results are dropped before a pending reap can block. This is a truthful
pending state, not a promise to terminate every kernel call on demand.

## Wire and memory admission

Use a small length-prefixed typed request with strict object shape/fields and a
64-KiB limit. Frame replies use fixed binary headers plus bounded metadata and
tightly packed RGBA bytes; no JSON/base64 raster payload. Admit at most 32 frames
and 256 MiB total decoded pixels before parent allocation. Require nonzero exact
dimensions, checked width×height×4, exact payload lengths and known pixel format.
Reject missing/extra frames, identity changes relative to ordered requests,
window/display mismatch and invalid geometry. Explicitly repeated targets still
count against aggregate admission. Display/window metadata replies have count/string/total
byte caps; an excessive inventory fails explicitly rather than truncating it.

Stream in at most 64-KiB chunks, directly into the admitted parent raster. Keep
at most the admitted parent result group, the child group's existing source
buffers and bounded pipe/metadata scratch; release each child frame after its
transfer. A truncated/failed group is discarded without any save or callback.
The extra cross-process source representation counts in the PLAN active-image
budget. Native driver/OS allocations remain additional and must be reported.
Never retain a complete desktop, worker allocator or optional runtime in idle
merely to hide spawn cost.

Unix socket buffers request 128 KiB and verify the kernel-reported bound is at
most 256 KiB per send/receive option. Linux doubles the requested value; the sum
of all four reported limits is at most 1 MiB (actual Unix-stream flow control
can use less). Windows has two 128-KiB pipes. This is bounded active scratch,
not a relaxation of the PLAN image budget. Exit completion uses Linux pidfd,
macOS kqueue or Windows process readiness instead of an artificial completion
sleep. Kernels lacking the Unix event facility retain a finite active fallback.
No process, coordinator, native connection or event descriptor remains in
normal idle after a completed job.

Start with per-operation children because they add no resident footprint. Measure
optimized private-fixture startup/capture/transfer p50/p95 and process memory
against inline capture on the same build. A repeatable >10% regression requires
a recorded cause and remediation decision before merge. Introduce a parked
persistent worker only if measured need justifies its extra resident memory,
teardown and post-capture retention cost; it is not a default design assumption.

## Wayland follow-up within this family

The vendored xcap 0.9.10 Wayland still path still writes/decodes GNOME or portal
Screenshot PNG files and holds a global D-Bus mutex before trying wlroots.
Process isolation does not make those disk round trips acceptable.

Plan a genuine single-frame ScreenCast/PipeWire capture source, with explicit OS
source/permission selection and immediate session teardown after the admitted
frame. Reuse maintained ashpd/PipeWire packages already in the graph. Negotiate
supported CPU-readable RGB8 formats, bound dimensions/stride/buffers before
copying and reject unsupported DMA-only/color formats truthfully. Do not call
RGB8 capture HDR support. Do not silently substitute an OS-selected monitor for
an explicitly named existing display ID or claim stable physical identity that
Wayland does not expose.

This adapter needs its own exact API/permission/coordinate plan before code.
An explicit portal source may be necessary instead of pretending global window
enumeration exists. Verify with a private D-Bus portal plus a private PipeWire
daemon/publisher of generated pixels; no user display/audio/permission store is
used in tests. Native compositor permission acceptance remains separate.

## Files and checks

Implemented files: core native-call/executor module and capture/catalog delegation;
desktop capture-worker controller/entry/transport and narrow Unix/Windows job
adapters; main startup wiring; protocol/process tests; private-Xvfb comparison
script and this contract. No editor/layout controls change. Root owns concurrent
capture export work; this stage modifies only coordinated capture execution and
startup hooks. No new dependency was required.

Tests must cover failed/stale handshake, wrong handles, every length/geometry/
aggregate boundary, malformed/truncated frames, zero-output failure, one-job busy,
timeout/cancel/parent death, process-group/Job Object cleanup, executable replacement,
and repeated successful jobs without leaked descriptors or retained images.
Use a deliberate owned stalled helper to prove termination; native Windows DWM
coverage remains the separately added owned-window gate. Actual private-Xvfb
display/window/region/template captures must match independent pixels through
the new worker, with unchanged source/state and explicit optimized performance
evidence. No user desktop input, OS timer registration or sensitive frame logging
is authorized by these tests.

## Completed focused evidence

`target/native-review-fixes-worker-final.log` records 25 passing optimized regular checks and
one separately invoked timing-only case. Protocol fixtures verify exact hidden
RGB/alpha, output-path exclusion, strict handshake/object/length/aggregate caps,
truncation, trailing bytes and wrong-target refusal. Owned-process fixtures
verify timeout/cancel, parent loss, process-group descendants, noninheritance
into grandchildren, one active job, partial inventory results, priority handoff
with two contenders, and admission retention while cleanup remains owned.
The release-boundary test pauses the owning handoff after enumeration releases
admission, then rejects both a new enumerator and another capture before letting
the owner claim its reservation. Unix regressions cover normal root exit with a
live descendant, parent loss with a live descendant and a retired Job whose
numeric group ID now belongs to a different live owned fixture: it must never
signal that group. The root remains unreaped until owned-family signaling is
finished. The parent-loss regression also covers Linux's thread-specific
parent-death signal setting.
A deliberately blocked startup factory verifies the real pending return path
and that a late startup never launches after its deadline. That is controlled
lifecycle evidence, not an actual kernel-uninterruptible test. Environment
fixtures verify unneeded credentials and clipboard capabilities are absent.
The current XCap worker path requires an explicit nonempty X11/XWayland DISPLAY
on Linux, preventing its upstream default-display fallback. This is not a claim
that native Wayland still capture is complete.

`target/verification/capture-worker-private-review-fixes-final.json` records the actual
optimized executable on private authenticated Xvfb: 48,259 exact RGBA pixels,
17 CLI invocations, cursor/hotspot/clipping, display/window/region/template and
assigned-task captures, three rapid native hotkey taps, stale binding refusal,
live source-display preservation and clean endpoint/user-state paths. Changing
only the fixture's PID property proves the originating parent's self-window
guard still rejects capture across the worker boundary. The executable inode
is checked throughout; its actual SHA256 is recorded in the report.

Complete GNU Windows application/test compilation passed
(`target/native-review-fixes-windows-final.log`). This is compile evidence; native
Windows process/job and macOS lifeline/kqueue/permission checks remain native CI
obligations. No Wine result is counted as DWM or native-platform acceptance.

## Optimized 4K cost and Elephant decision

`scripts/test-capture-worker-performance.py` starts a private TCP-disabled,
cookie-authenticated 3840×2160 Xvfb server and paints generated solid pixels.
Hard links pin the exact optimized test/executable images. Three separate runs
per mode each use five warmups and 100 captures; every returned pixel is
verified outside the capture timer. The finite observer checks process
start-time/comm/executable identity and samples parent/helper smaps every 5 ms.
The timing includes native request/ownership/transfer readiness, not GUI hiding,
presentation, editing, clipboard or durable image output.

The initial observation had median p95 23.665 ms inline versus 36.817 ms isolated
(+55.6%). A short E2E overlapped its first case, so the preserved initial report
is explicitly qualified. Native exit readiness removed the avoidable 5 ms
completion sleep and bounded socket tuning reduced transfer backpressure.
The final comparison ran serially, with no overlapping E2E:

| Metric (median of three case results) | Inline | Per-operation worker |
| --- | ---: | ---: |
| p50 capture readiness | 20.809 ms | 29.247 ms |
| p95 capture readiness | 22.980 ms | 32.192 ms |
| Reported parent peak RSS | 77,944 KiB | 45,892 KiB |
| Largest child's reported peak RSS | 0 KiB | 85,752 KiB |

The remaining relative p95 increase is **40.1%, or 9.21 ms**. Its cause is process
startup/ownership, validated raw transfer and the extra process representation.
The full report preserves per-run timings, CPU/context-switch counters,
descriptor counts and all sampling caveats:
`target/verification/native-capture-4k-performance-final.json`.
It pins worker SHA256
`0f84915a00baefb648eda9a10f2cf42d807208512ab7c122c1058ee99bca6ec5`
and test SHA256
`0dddc7ea7dd31c50eaf40c6abace1049fcfe38adce59dd93d1b29cc177907180`.

Maximum observed near-simultaneous RSS/PSS sums were 78,320/74,546 KiB inline
and 132,708/123,317 KiB isolated. Worker swap reached 1,984 KiB. These are
sequentially sampled process sums, may miss a peak and count shared RSS in both
processes; they are not private footprint measurements. Independent rusage
maxima are not simultaneous peaks. All 105 helpers per isolated case were
observed gone, descriptor counts returned to baseline and no pending native
owner remained. There is no zero-footprint interpretation.

This development host was Ryzen 7 9800X3D (16 logical CPUs), approximately
30.4 GiB RAM and 30.4 GiB swap, CachyOS Linux 7.3 and performance governor. It is not PLAN's
four-core/8-GiB reference machine. The 32.192 ms component p95 leaves room under
the unchanged 250 ms 4K capture target, but does not establish that end-to-end or
reference-hardware target.

The Elephant explicitly accepted retaining the per-operation worker after this
measured remediation: the additional 9.21 ms p95 buys bounded ownership/deadlines
without an idle process or retained native image/runtime. No target was relaxed
and no parked worker was introduced. Hardware/end-to-end acceptance remains
open; repeatable future regressions still require the same performance review.

## Review corrections and cleanup-cost repeat

The fresh source review found three Major defects, now covered by the precise
regressions above: an exited Unix root left descendants unsignaled; an idle
caller could steal a capture handoff; and `run-task` discarded its validated
Task/catalog before reading the profile again. `run-task` now executes the same
owned Task and initial prepared catalog snapshot. Its deterministic replacement
fixture changes the catalog before Task preparation, then replaces both Task
source and actions before execution. Only the admitted pixels/output execute
(`target/native-review-fixes-task-final.log`). Four executable catalog checks
also pass (`target/native-review-fixes-catalog-final.log`).

The earlier 32.192-ms worker p95 predates the corrected group-completion proof.
The first corrected cleanup inspected every process stat record and measured
38.979 ms against inline 24.080 ms. A bounded `getpgid` filter now avoids opening
unrelated stat files. It preserves complete group inspection without assuming
that every descendant keeps its original UID. A serial unchanged-image repeat
of this remediation measured:

| Metric (median of three case results) | Inline | Per-operation worker |
| --- | ---: | ---: |
| p50 capture readiness | 20.782 ms | 32.959 ms |
| p95 capture readiness | 24.999 ms | 37.379 ms |
| Reported parent peak RSS | 77,820 KiB | 46,288 KiB |
| Largest child's reported peak RSS | 0 KiB | 82,488 KiB |

The current relative p95 increase is **49.52%, or 12.38 ms**. An immediately
preceding three-case run on these exact images measured worker p95 37.234 ms
against inline 23.915 ms; both reports preserve every case and host variability.
The corrected cleanup adds active process-group membership checks, keeping the
leader identity reserved through all signaling. Parent CPU in the repeated
100-capture worker case had median 628.251 ms, down from 862.737 ms before this
filter; child CPU median was 2,859.284 ms. No idle scan was introduced.

`target/verification/native-capture-4k-performance-group-cleanup-repeat.json`
records worker SHA256
`51c7f72b4618a7af76e4c5081a970463dec61166aec429c2da614516563a0b4d`
and test SHA256
`7c1041f8079873a717532ee0d71b6b00362008321404d2f20cffe70b12afd13f`.
The preserved first remediation report is
`target/verification/native-capture-4k-performance-group-cleanup-remediated.json`;
the first corrected, unfiltered inspection report is
`target/verification/native-capture-4k-performance-group-cleanup.json`.

Maximum observed near-simultaneous RSS/PSS sums in the repeated current build
were 78,108/74,222 KiB inline and 129,056/119,657 KiB isolated; no swap was
observed in that repeat. Earlier observations included swap; one zero-swap
repeat does not establish a general zero-swap result. The same
sampling/shared-page caveats above apply.
All 105 helpers per isolated case disappeared, descriptors returned from four
to four and no pending owner remained. These are active component measurements
on the same development host; the 250-ms target and reference-hardware/native
platform acceptance remain unchanged.

The Elephant retained the per-operation model and this bounded `getpgid`-first
ownership proof after reviewing both unchanged-image repeats, their process
memory and reduced parent CPU. The added active cost buys safe family cleanup
without reusing a live job's admission, leaving an idle worker or weakening the
PID/group reservation. This records the cause, remediation and decision for
PLAN's greater-than-10-percent review gate; it does not relax the latency target.
