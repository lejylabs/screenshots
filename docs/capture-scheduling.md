# Durable explicit capture schedules

This is the EGM implementation contract for the remaining non-UI timing family.
The scheduler is an explicitly started headless service; normal application and
tray startup do not open its database, initialize a timer, enumerate a target,
create a thread or load an optional runtime. The existing display-only
`capture-sequence` command remains compatible. A saved Task supplies primary,
chosen display, active/chosen window, region, or saved region-template sources;
Linux portal and explicit image-URL sources are also admitted by Task. A URL
and its HTTP policy are sealed; downloaded pixels are dynamic per run and have
no `{input}` filesystem path until explicitly saved. A finite repeating
schedule supplies a window/region capture sequence without
adding another capture/action implementation.

## Commands and bounded data

` schedule add DATABASE ID PROFILE --at RFC3339 [--every-ms N]
[--count N | --forever] [--until RFC3339] [--missed skip|once|pause]
[--grace-ms N] [--wake]` admits the entire Task before writing a schedule.
`--at` has an explicit known timezone and at most millisecond precision;
1970–9999 UTC is supported. A single occurrence is the default. Repetition
requires an interval of 100–31536000000 ms and either count 1–1000000 or explicit
`--forever`. `--until` is inclusive. Default missed policy is `pause`; grace is
1000 ms by default, within 0–60000 ms. IDs contain 1–64 ASCII letters, digits,
underscore or hyphen. There are at most 32 schedules.

` schedule list DATABASE` exposes bounded metadata and next-run/uncertain status,
never a task's source paths, action results, URLs or secret values.
` schedule pause|resume|remove DATABASE ID` changes offline schedules. Resume
explicitly discards elapsed occurrences without executing them and preserves the
original future UTC anchor; it never silently reseals a changed Task. Remove refuses a schedule with
an unresolved run. `schedule resolve DATABASE RUN_ID complete|skip` explicitly
acknowledges a possibly executed occurrence without running it again.
` schedule serve DATABASE` exclusively owns execution until Ctrl+C/termination.
Mutations fail with an actionable busy error while the service owns its native
file lock; read-only list remains available. Configuration does not need a
watcher, refresh poll or local network endpoint. Stop, edit and restart to change
it. The separate versioned SQLite database is at most 32 MiB, with at most one
active run and one bounded last-outcome record per schedule. It is not a history
database. Files are private, regular and not final-component symlinks/reparse
points or hard links. SQLite full synchronous transactions and an owned native
lock coordinate cooperative application processes. Foreign/unknown schemas fail.

## Frozen Task boundary

An opaque bounded Task seal records the admitted profile and every referenced
recipe, upload/provider/S3/SXCU configuration, options, program and source policy.
Each due occurrence prepares the complete graph again and compares it with that
seal before capture or actions. Changed references pause that schedule; only
removing and explicitly adding it again authorizes replacement. A changed upload
endpoint cannot gain authorization through replacing a profile. The prepared
one-shot Task owns all parsed action/source settings; execution consumes it.
Dynamic output UUID/timestamp placeholders expand separately for each occurrence.
No decoded image or native capture target is retained during a wait. Dynamic
primary/active targets are selected at execution. Chosen window/display, region
and template targets are resolved and revalidated by the existing native capture
worker before/after capture, so no captured frame survives an idle wait. The
scheduler adds no topology enumeration loop. Chosen numeric/native IDs retain
the existing Task semantics: they identify the native object available at each
run; they are not an invented permanent window-generation/monitor identity
across process gaps or handle reuse. Use a bound region template when persistent
monitor binding and geometry refusal are required. Invalid/changed native targets
fail truthfully rather than silently rebinding a template.

A Task adapter accepts a shared cancellation flag and a silent receipt sink.
The scheduler retains only success/failure/cancellation, run UUID and occurrence
ordinal. It does not print unattended upload links or command/action payloads.
The seal is authorization against cooperative configuration changes; it is not
an isolation boundary against a compromised current user replacing filesystem
objects during a native call. Source/program identity and reference bytes must
be checked through the same opened/snapshotted admission used for execution.

Expected file identities travel from preparation to each actual image/raw-input
open. The decoder/HTTP/S3/SXCU/provider then consumes that same verified handle;
a later pathname open cannot substitute an unadmitted object. Identity includes
native device/file IDs and size/modification/change stamps; Windows uses the
full 128-bit `FILE_ID_INFO`, with no pathname case-folding or short-ID fallback.
Metadata-only changes may conservatively refuse a run. No file handle or pixels
are retained during an idle wait, and no full-input copy is added. Concurrent
in-place writes after verification remain ordinary native file-consumption
limits, not a claimed immutable filesystem snapshot.

Commands retain expected identity until native launch. Unix verifies the opened
program/script immediately before `Command.spawn`; Windows verifies the actual
resolver-selected executable/batch script before `CreateProcessW` and again
immediately before its suspended initial thread is resumed. Checked handles are
released after native start. This detects ordinary replacements made before
launch; pathname, parent-directory, shebang and interpreter reads during/after
native start are not an atomic hostile-user executable pin. Standalone commands
use the existing unguarded path/script semantics. Windows change stamps are
metadata timestamps as described by
[FILE_BASIC_INFO](https://learn.microsoft.com/windows/win32/api/winbase/ns-winbase-file_basic_info).

## State machine and missed occurrences

Times are anchored at `start + ordinal * interval`, never completion-relative.
The next ordinal is durable. UTC arithmetic is checked; a finite count/until
boundary completes a schedule. Wall-clock rollback does not repeat ordinals
already consumed. One task executes at a time across all 32 schedules, ordered
by due time then ID. No frame, task or missed-run queue is accumulated.

A due occurrence within grace runs once. If late beyond grace: `skip` advances
past every elapsed slot without execution, `once` consumes elapsed slots and
executes only the latest due occurrence, and `pause` disables the schedule until
explicit intervention. `resume` discards elapsed slots; remove/add explicitly
changes the Task seal or start rule. A slow preceding task, sleep, unavailable service and
forward clock correction use the same rule. A backward clock correction rearms
the absolute deadline. Stopping during a wait executes nothing.

Before executing, one full-synchronous transaction records the singleton run
intent (UUID, schedule ID, ordinal and requested UTC) and advances the schedule.
Only then may the prepared Task execute. Confirmed successful completion clears
the intent and records a bounded outcome. Any error, cancellation, abrupt exit,
unknown completion or failed confirmation leaves an **uncertain** run, blocks
automatic execution and requires explicit `resolve complete|skip`. Neither
choice retries it. A preflight/seal failure occurs before an intent or effects
and pauses only its schedule. Thus an upload that may have completed is never
automatically repeated. This is at-most-once admission, not transactional remote
uploads or a claim that a failed Task performed no earlier actions.

## Native waiting, wake and resources

There is one main scheduler thread and at most one parked Unix signal waiter.
No timer/worker/runtime exists outside explicit service lifetime. At most two
one-shot timers are armed: the earliest occurrence overall and separately the
earliest explicit wake occurrence. An earlier non-wake task must not prevent a
later wake request being armed while asleep. No periodic timer is used. Native
waits are released by cancellation/control events. Task work remains off GPUI's
foreground executor and reuses its existing image, process and upload bounds.

Linux uses absolute `CLOCK_REALTIME` timerfds with cancellation-on-clock-set and
an event-driven fd wait. Explicit wake uses `CLOCK_REALTIME_ALARM`; lack of
`CAP_WAKE_ALARM` fails before executing instead of requesting privilege.
[Linux timerfd documentation](https://man7.org/linux/man-pages/man2/timerfd_create.2.html)
describes cancellation and alarm permissions. Windows uses absolute UTC
one-shot waitable timers and a native cancellation event; absolute timers track
system-time adjustments. Wake is requested only with `--wake`; unsupported
resume behavior fails rather than claiming it was enabled.
[SetWaitableTimer](https://learn.microsoft.com/en-us/windows/win32/api/synchapi/nf-synchapi-setwaitabletimer)
defines the absolute-time and resume behavior. macOS uses an operation-owned
one-shot Dispatch wall-clock timer, with no periodic interval; its clock is
`gettimeofday` as specified by
[dispatch_source_set_timer](https://developer.apple.com/documentation/dispatch/dispatch_source_set_timer).
Explicit macOS wake uses only an owned IOKit scheduled wake event, requires the
native privilege, journals its identity before publication and cancels that
exact event during rearm/shutdown/recovery. No interactive privilege prompt is
shown. Every writable offline action, including pause/remove, reconciles the
exact journaled wake under its lifetime lock before reporting success. Failed
native cancellation or failed durable journal clearing retains the intent and
refuses the mutation. A missing event (crash before publication/already fired)
is safe to reconcile; unrelated power events are never selected. [IOPMCancelScheduledPowerEvent](https://developer.apple.com/documentation/iokit/1557116-iopmcancelscheduledpowerevent)
requires matching date, owner and type. Kernel sleep/wake ability, battery policy
and device permission remain native acceptance work; no software timer proves
hardware wake.

Filesystem/SQLite and native capture calls retain their documented kernel-stall
limits. SQLite work has finite size, short busy timeout and execution deadline;
there is no busy retry loop. A held native lock blocks competing service/edit
operations without stale-PID guessing. The service does not auto-install,
autostart, upload outside the frozen Task or wake without an explicit flag.

## Implementation and verification gates

Buildable slices: (1) pure checked recurrence/missed-policy state machine and
strict CLI schema; (2) bounded SQLite schema/locking, run intent and recovery;
(3) native one-shot timers and cancellation; (4) the coordinated opaque Task
adapter and command wiring; (5) actual executable/isolated native tests. New
files and existing seams are listed exactly below; focused synthetic tests live
inside those modules. No GUI design changes are involved.

Create:

- `apps/desktop/src/capture_scheduling.rs`
- `apps/desktop/src/capture_schedule_model.rs`
- `apps/desktop/src/capture_schedule_store.rs`
- `apps/desktop/src/capture_schedule_private_windows.rs`
- `apps/desktop/src/capture_schedule_timer_linux.rs`
- `apps/desktop/src/capture_schedule_timer_windows.rs`
- `apps/desktop/src/capture_schedule_timer_macos.rs`
- `apps/desktop/src/task_seal.rs`
- `apps/desktop/src/task_ca.rs`
- `crates/app-core/src/file_identity.rs`
- `scripts/test-capture-scheduling.py`
- `docs/capture-scheduling.md`

Modify:

- `apps/desktop/src/task.rs`: loaded configuration digest, prepared scheduled
  Task, silent receipts/shared cancellation and typed Portal source.
- `apps/desktop/src/record.rs`: scoped shared-flag cancellation while retaining
  standalone terminal-handler retirement.
- `apps/desktop/src/record_windows.rs`: coordinated shared cancellation entrypoint.
- `crates/app-core/src/windows_child.rs` and
  `crates/app-core/src/windows_child_tests.rs`: resolver-selected identity through
  suspended startup and a native batch-replacement regression.
- `crates/app-core/src/clipboard.rs`, `crates/app-core/src/upload.rs` and
  `crates/app-core/src/s3_upload.rs`: verify actual consumed input handles.
- `crates/app-core/src/lib.rs`: exact opened-identity/image-loader API exports.
- `apps/desktop/src/sharing_provider.rs`: immutable CA snapshot and configuration digest.
- `apps/desktop/src/workflow.rs`: digest the actual parsed Recipe.
- `crates/app-core/src/sharex.rs`: digest admitted SXCU settings/options/credential
  reference, excluding credential-store token values.
- `crates/app-core/src/sharex_process.rs`: digest retained runtime manifest/path.
- `crates/app-core/src/capture_catalog.rs`: digest retained template/options.
- `apps/desktop/src/main.rs` and `apps/desktop/src/cli.rs`: explicit command wiring/help.
- `apps/desktop/Cargo.toml` and `Cargo.lock`: existing rusqlite direct dependency edge.
- `docs/tasks.md`, `docs/capture-sequence.md` and `docs/storage-recovery.md`:
  discoverability/recovery crosslinks.

Deterministic fixtures cover interval/count/until/UTC boundaries, all missed
policies, forward/backward clock changes, long-running tasks, no backlog,
concurrent ownership, canceled waits, seal replacement, crash after intent,
possibly successful upload with failed confirmation, explicit resolution and
read-only preservation. Private native timer fixtures do not change the system
clock or suspend the computer. Executable tests run only synthetic file/command
Tasks and private loopback uploads; no user pixels, clipboard, credentials or
real destination are used. Optimized idle CPU/context switches, private/RSS
memory and thread count are measured with stable process identity. Measured
evidence is recorded separately; targets from PLAN are not passing results.

## Operator examples and recovery

Create a finite sequence using any supported Task source, with fresh output
names in the Task such as `capture-{uuid}.png`:

```sh
screenshots schedule add ./timing/state.sqlite sequence ./task.json --at 2030-01-01T10:00:00Z --every-ms 1000 --count 10 --missed once
screenshots schedule serve ./timing/state.sqlite
```

The `timing` parent is created privately if absent (one level; its own parent
must exist). Windows requires a dedicated current-user-only inheritable parent
DACL so SQLite rollback sidecars remain private. Ctrl+C cancels a parked native
wait immediately; an active Task uses the existing cooperative/native teardown
limits and leaves an uncertain intent if its outcome is not confirmed. No
sensitive Task diagnostics/action payloads appear in service receipts.

After an error/crash, `schedule list` marks the affected entry `uncertain` and
returns its run UUID. Inspect the explicitly configured local outputs and remote
destination, then use `schedule resolve DATABASE UUID complete` or `skip`.
Neither command executes the Task. The consumed ordinal stays consumed; future
configured occurrences can run after service restart. Pause/remove the schedule
before restarting if those future captures/uploads are no longer wanted.
Schedule initialization interrupted before SQLite's schema transaction can leave
an empty/foreign-looking file; it is refused rather than silently adopted. No
Task starts during initialization. Inspect that file and its sidecars before
choosing a new database path; do not replace a live database.

Scheduled shared-flag cancellation owns and unregisters its signal handlers per
Task, so a retired Task cannot terminate a later run. Standalone commands retain
their prior terminal signal behavior after cleanup, including blocked final
output. Optional FTPS CA files are read into an immutable <=1 MiB snapshot per
prepared provider action, included in its configuration digest, and staged in a
private operation-owned file only while its child needs them. At most 16 such
actions add <=16 MiB admitted CA bytes; no CA file is read/retained while waiting.
Credential-store token rotation stays allowed for an unchanged configured
credential reference/origin.

## Development evidence

Focused optimized checks cover the pure model, strict UTC grammar, private
SQLite ownership, native one-shot rearm and Unix-epoch special value, cooperative
SQL VM cancellation/deadlines, durable intent/crash recovery and failed
confirmation. Synthetic native-source fixtures cover primary/active/chosen
window/display, region/template and Linux portal sources, without user pixels;
they prove admission captures nothing, each run releases pixels and native target
failure propagates. Provider fixtures retain standalone blocked-output signal
behavior and prove replacing an admitted CA file cannot change staged CA bytes.

`scripts/test-capture-scheduling.py` exercises the real executable using private
PNG files and loopback HTTP, never the user clipboard/credential store. It checks
finite serial UUID outputs, offline ownership/read-only status, profile-change
refusal, every missed policy, explicit resume, confirmed upload followed by
failure, and actual SIGKILL after a confirmed upload. Restart never repeats the
uncertain upload; explicit resolution does not execute it. Optional `--native-x11`
creates an authenticated private 128x96 Xvfb server with TCP disabled and checks
seven sources with two captures each, including exact window/region/template
pixels. Portal permission, physical Windows/macOS input, sleep/wake and clock
adjustment hardware acceptance are not implied by those fixtures.

The five-minute optimized Linux observation in
`target/verification/capture-scheduling-executable.json` records stable PID,
process starttime and executable device/inode. It observed one thread, no
additional process CPU ticks/context switches, and no disk read/write bytes.
Initial private resident memory was 14976 KiB (RSS 19288 KiB); final private
resident was 4 KiB (RSS 2960 KiB), with 3716 KiB swapped. No working-set trimming
was performed: the resident decrease does not mean zero retained footprint.
CPU is quantized by the OS accounting tick; this is a development-host
observation, not a reference-hardware or all-platform release gate. Subsequent
metadata-status/resume/Unix-epoch and standalone-signal compatibility fixes do
not change the measured parked future-deadline waiting path; a separate final
functional proof records its own executable hash.

The final checks recorded 12 scheduler unit/native fixtures, 20 Task/captureTask
unit fixtures, 11 provider fixtures, 11 actual Task CLI fixtures and 10 actual
workflow CLI fixtures passing. The final six-group executable proof is in
`target/verification/capture-scheduling-executable-final.json` (binary SHA-256
`7954a758e1bf9e4fdddde5aba630b8735b10086d2866c1febae8836e38f052d7`).
Windows GNU dev-profile app/test metadata checking and plain optimized Clippy
completed successfully; existing repository/dependency warnings remain. These
checks do not substitute for native Windows/macOS runtime acceptance. Fresh
Goldfish source review is a separate gate; its unresolved findings must be fixed
before this family is declared complete.

Fresh Goldfish round 1's two Majors are addressed by the opened-object/launch
identity checks and the common offline owned-wake gate above. Deterministic
replacement-after-preparation fixtures cover file decoding/save, raw upload with
no private HTTP connection, and executable/script marker suppression. Injected
power fixtures cover crash before/after publication plus pause/remove, and
preserve the wake intent/unchanged schedules on native-cancel or SQLite-clear
failure. The exact production Windows source and actual FileIdentity module
also link in the ignored same-source harness; linking is not native Windows
execution acceptance. Follow-up review remains a separate gate.


The second fresh source review found an expired Windows manual-reset timer could
keep an idle wait ready after cancellation. Native timers now reset when waited,
and disarming also consumes an unobserved expiry, including simultaneous normal
and alarm signals. Native regression fixtures cover completed/paused schedules
and both timers becoming ready together; they request no hardware wake. The
Unix checked-program descriptor is released immediately after successful native
spawn, matching its stated boundary. Both native timer regression fixtures passed in the isolated supplemental Wine
harness (`target/windows-schedule-timer-tests.log`). Fresh scheduler round three
found no Blocker or Major. This is source/fixture correction evidence, not native
Windows, clock-change or hardware-wake acceptance.

The author guard-fix proof is
`target/verification/capture-scheduling-guards-final.json`, SHA-256
`09e19015c960e6cf2d2e2a8ef9eca27b55ed92b7f25199b7dfd31f8e5236fcf2`.
Its seven groups add an actual CLI command/FIFO handshake followed by raw-file
replacement: the next upload refuses the newly opened object without making a
connection. Focused guard checks passed 14 scheduler fixtures, 7 seal/source
fixtures, 1 core opened-identity fixture, 44 HTTP/S3 fixtures (16 ignored), 11
provider fixtures, 11 Task CLI fixtures and 10 workflow CLI fixtures. Workspace
format checking, GNU Windows app/test metadata checking, production-source
Windows harness linking and plain optimized Clippy completed successfully.
Existing repository/dependency warnings remain.

Goldfish round 2 closed the original two Majors. It found a Windows manual-reset
one-shot timer spin and a Unix checked-program-handle lifetime Nit. Root applied
both corrections directly, adding native timer regressions without hardware wake.
The author proof hash above predates those two direct corrections; their focused
verification and any later final hash are separate evidence. The earlier Linux
idle observation remains a measurement of its stated frozen binary, not a claim
of native Windows/macOS or hardware acceptance.
