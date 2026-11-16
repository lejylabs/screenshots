# Atomic Windows child startup

Status: implemented for source review, 2026-11-16. The parent/Elephant approved
the plan and released the shared files after the preceding commits. Native
Windows runtime acceptance remains a CI gate.

## Problem and scope

SXCU, OCR, recording, provider transfer and hosted Tasks previously created a
suspended child with `std::process::Command`, then assigned it to a kill-on-close
Job Object. Suspension prevents early user code, but a controller killed between
creation and assignment leaves a suspended process outside its job. Rust error
cleanup cannot run after abrupt parent loss. Recording's launcher also serves
audio, combined A/V, probes, scrolling helpers and explicit Task commands.

The existing capture worker already supplies both JOB_LIST and HANDLE_LIST to
CreateProcessW. Reuse that mechanism for all five vulnerable launchers and keep
one implementation instead of duplicating their native quoting, environment and
handle management. Autostart's native CreateProcess fixture intentionally tests
an independent resident launch and is not one of these operation workers.

Readiness and correctness requirements are unchanged: optional children exist
only during explicit jobs, no idle polling/runtime, no sensitive argv or logs,
fixed input/output bounds, existing cancellation/deadlines and sanitized
environments, private pipe inheritance, and kill/reap before joining pipe workers.
SXCU keeps its 256 MiB process limit and one-process limit. The other families
keep their existing kill-on-close descendant containment without inventing new
resource budgets or silently preventing legitimate descendants.

## Native contract

CreateProcessW receives an initialized STARTUPINFOEXW attribute list containing
the existing job handle and exactly the permitted child handles. Microsoft
documents JOB_LIST for Windows 10 and newer; HANDLE_LIST requires inheritable
handles and `bInheritHandles=TRUE`. Job, process, initial-thread and parent pipe
handles are never included in the inherited list. Unsupported assignment or
attribute creation fails closed, with no fallback to an uncontained process.
[Microsoft process-attribute contract](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute).

The job is already configured and owned by the controller before creation.
Atomic assignment removes the interval where a created child has no job.
Closing the controller's sole noninheritable job handle terminates its family,
including a child whose initial thread has not resumed.
[Microsoft kill-on-close contract](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_basic_limit_information).

The returned initial-thread handle lets each controller choose its guarded
resume point. Capture, recording, OCR and Task host retain it through their
pipe-worker or clipboard-bridge setup. The provider starts its already-contained
protocol helper before installing drain workers; transfer configuration arrives
through the parent's private pipe afterward. ResumeThread uses the actual handle
once after startup checks, replacing ToolHelp enumeration and thread-ID lookup. No child code executes before job assignment. Parent loss
before or after resume has the same kill-on-close behavior.

Rust 1.100 documents `CommandExt::spawn_with_attributes` as nightly-only. The
implementation will use existing windows-sys stable bindings, with no unstable
flags, compiler bootstrap or assumptions about the private layout of std Child.
[Rust CommandExt](https://doc.rust-lang.org/std/os/windows/process/trait.CommandExt.html#tymethod.spawn_with_attributes).

## Shared primitive

Add a Windows-only, narrowly exported core module `windows_child`. It borrows
the supplied job while creating one child, owns process and initial-thread
handles, and exposes only the operations already required by the controllers:
id, resume, kill, wait, try_wait, a native wait for capture, and optional stdin/
stdout/stderr `std::fs::File` endpoints. Exit status uses stable ExitStatusExt.
Existing family-specific guards continue owning the job and enforcing deadlines,
cancellation, byte limits, drain workers and result/error semantics. There is no
resident shared manager, registry, thread or process cache.

Stdio is explicit rather than inferred from Command's opaque stdio settings:
three declared null/pipe choices, plus a bounded list of additional inherited
handles. Parent pipe ends have inheritance cleared; only child ends enter the
handle list. All intermediate handles use RAII from creation. Every failure
after process creation kills/reaps before returning, and the job remains owned
through caller-guard construction. Task Prepared exposes its two clipboard
child handles, preserving their numeric values and one-use handshake. The
provider's SID-private named config pipe remains noninherited and PID-verified.

Command supplies program, structured arguments, optional working directory and
environment overrides/removals. An explicit environment mode distinguishes the
capture worker's already-cleared allowlist from inherited environments with
scrubbed removals; stable Command getters cannot reveal env_clear. Environment
materialization preserves Windows case-insensitive keys and double-NUL Unicode
blocks with a fixed 1 MiB bound. Strings reject embedded NUL and command lines
respect the native 32,767 UTF-16-unit limit. Standard Windows CRT quoting is
reused from capture and verified with empty, quoted, Unicode and trailing-slash
arguments. Raw CommandExt arguments are outside this internal interface; no
affected caller uses them. Explicit cmd.exe invocations keep ordinary std
argument serialization. Implicit .bat/.cmd dispatch adapts Rust 1.100's stable
batch algorithm, including doubled internal quotes, percent-expansion
protection, disabled autorun/delayed expansion and argument CR/LF/NUL refusal.
Its MIT attribution and full license are in THIRD_PARTY_NOTICES.md. This retains
Task script behavior without treating substituted paths/links as shell syntax.

Absolute private helpers keep their exact executable path. Bare installed
engines (ffmpeg.exe, ffprobe.exe, tesseract.exe, curl.exe) need explicit resolution
using the effective child PATH before passing an exact application path. Do not
silently use CreateProcessW's different partial-application-name rules or lose
test fixture PATH overrides. Missing or invalid executables retain each
family's sanitized engine-unavailable error.
[Microsoft application-path and command-line rules](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessw).

## Files and ownership

- New `crates/app-core/src/windows_child.rs`: shared native process, pipes,
  attribute lifetime, bounded environment/quoting/path resolution, job creation
  with the existing optional memory/process limits, and deterministic native
  unit fixtures. `crates/app-core/src/windows_child_tests.rs` contains native
  fixtures and `windows_child_batch.rs` contains the adapted batch serializer.
- `crates/app-core/src/lib.rs` and its Windows Cargo feature list: parent wiring
  for the Windows-only module and Win32 Pipes/SystemInformation/Globalization
  bindings, no package or lockfile dependency change.
- `crates/app-core/src/sharex_process_windows.rs`: retain SXCU job limits and
  replace std spawn/assignment/thread enumeration with atomic shared creation.
- `crates/app-core/src/sharex_process.rs`: platform child/pipe type aliases or
  a generic Write input worker so Windows File pipe endpoints work without
  pretending synchronous handles are std's asynchronous ChildStdin handles.
  Unix isolation and the five-second controller stay unchanged.
- `apps/desktop/src/ocr_windows.rs`, `record_windows.rs`,
  `sharing_provider_windows.rs`, `task_host_process.rs`: replace only native
  startup/child types, preserve each existing admission/guard/controller.
  Recording additionally exposes engine_with_cancel with the Task's existing
  shared flag, checked before launch/resume and every active wait iteration;
  its existing engine entry point delegates with a false flag.
- `apps/desktop/src/sharing_provider.rs`: conditional Windows child type for its
  existing Process trait, with Unix retaining std Child.
- `apps/desktop/src/task_clipboard.rs`: narrow read-only Prepared child-handle
  accessor used for the explicit startup inheritance list. No protocol change.
- `apps/desktop/src/capture_worker_windows.rs`: adapt its already-contained
  Process/Container interface to the common primitive, deleting duplicate native
  serialization/attribute creation; capture policy and transport stay unchanged.
- This document: record implementation, source review, exact checks and remaining
  native acceptance separately after work completes.
- `THIRD_PARTY_NOTICES.md`: retain the Rust batch serializer's MIT notice in
  existing packaged notices.

The parent owns commits. Task scheduling, profile sealing/digests, CA snapshots
and Task execution changes remain with their separate owner; the launcher work
changes only the narrow clipboard handle accessor and process-facing aliases.

## Verification plan

1. Native deterministic fixture confirms a child is already in the intended job
   while still suspended, cannot emit a startup marker before resume, and sees
   its containment from its first user-code operation.
2. A separate uncontained test owner creates the job and child, reports PID and
   pauses before resume. The test opens an observation handle, forcibly kills
   the owner, then requires that the suspended child is signaled within a fixed
   deadline and its startup marker was never created. Repeat after resume with
   a child/descendant holding pipe handles. Fixtures touch only unique synthetic
   temp state and their own processes, without microphones/screens/credentials.
3. Pipe round trips preserve exact binary stdin/stdout, discarded diagnostics,
   EOF after controller closure, and an allowed extra handle. A separate
   inheritable sentinel and the job handle cannot be used by the child.
4. Missing/invalid image, invalid job, deadline/cancel before resume and partial
   startup failure leave no live owned child. Native query checks keep SXCU's
   one-process/256 MiB limits and rejects a second child before it executes.
5. Argument/environment/current-directory fixtures cover Unicode, quotes,
   trailing slashes, empty args, case-insensitive overrides, removals, cleared
   environments, omitted CLR/profiler hooks and effective PATH fixture engines.
6. Run focused core tests plus the existing Windows OCR, record, provider,
   Task clipboard/host and capture worker suites. Unix focused regressions check
   the shared type changes did not change their process behavior. Crosscompile
   core and desktop Windows metadata. Native runtime fixtures belong to Windows
   CI; Linux metadata or Wine results alone do not establish native acceptance.

The change removes bounded ToolHelp process-wide enumeration from vulnerable
starts and adds only operation-scoped native attributes/pipes and capped command/
environment buffers. It adds no resident work, runtime or third-party package.
Latency/CPU/private-memory performance budgets remain targets until measured on
reference Windows hardware; no improvement percentage is claimed from source.

## Recorded checks and remaining acceptance

Focused optimized Unix SXCU process regression: five tests passed, including
private pipe delivery, cancellation/reaping and the five-second blocked-input
deadline. Windows GNU core and desktop all-target metadata checks passed. The
same-production-source standalone Windows harness links without GPUI or optional
engines, and its strict all-target Clippy check passed. These checks do not
establish native Windows runtime behavior.

Supplemental Wine 11.20 execution passed nine native fixture tests: containment
before first user code, abrupt parent loss before resume and with descendants,
binary pipe bytes/EOF, inherited-handle isolation, environment/current-directory
and argv preservation, explicit child PATH resolution, invalid job/image/input
refusal and batch compatibility. Native and batch unpaired UTF-16 behavior is
compared with the stable std oracle; the direct /u shell path preserves exact
units, while batch execution retains the oracle's success or refusal result.
The private subprocess fixture itself is intentionally ignored by the outer
runner and invoked explicitly by those tests.

One strict native resource test fails on Wine: querying an installed job's
ActiveProcessLimit returns zero after SetInformationJobObject succeeds. The
one-process/256 MiB assertions and second-child refusal remain mandatory and
unchanged in source; there is no Wine-specific production or test bypass.
Native Windows CI must run this resource test and the existing OCR/recording/
provider/Task clipboard/capture suites before runtime acceptance. The recording
shared-cancellation regression is included in that native suite.

Evidence logs are `target/windows-atomic-core-check.log`,
`target/windows-atomic-desktop-check.log`,
`target/windows-atomic-unix-regression.log`,
`target/windows-atomic-harness-build.log`,
`target/windows-atomic-harness-clippy.log`, and `target/windows-atomic-wine.log`.
The ignored harness includes the actual production module/test source by path;
it does not replace process creation with a stub or modify parent-loss semantics.

## Relationship to SXCU review evidence

The capped fifth fresh SXCU review reported a prefix-body authentication Major.
The parent subsequently fixed it and ran 26 Rust tests, 134 helper assertions,
and the actual 94-request HTTPS/installation/Task matrix recorded in
`target/verification/sharex-prefix-e2e.json`. Those checks are fix evidence, not
a clean additional fresh-review round. This startup design addresses a separate
parent-loss issue found afterward and requires its own focused review/checks.


The first fresh source review found a std-resolution mismatch for bare leading-dot
program names and a documentation sequencing contradiction for provider startup.
The resolver now treats any dot in a bare name as an extension, matching std;
a native std-oracle regression includes a conflicting `.exe` sibling. The text
above describes provider's actual guarded sequencing rather than claiming all
families resume at an identical point. Focused correction checks passed the exact std-oracle leading-dot case in the
isolated supplemental Wine harness; native Windows remains separate. Fresh
`windows_atomic_round2` found no Blocker or Major. Current source hashes and
correction-log scope are recorded in
`target/verification/windows-atomic-corrections.json`; the original author report
retains its earlier source hashes rather than being silently rewritten.
