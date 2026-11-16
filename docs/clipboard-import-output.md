# Explicit clipboard, import and output integration

This EGM contract extends existing actions/surfaces without redesigning them.
Normal startup adds no clipboard access, watcher, timer, transfer, printer query,
worker process or optional runtime. Tests must use private synthetic clipboard
sessions and print destinations; the user's current clipboard/printers are never
read, replaced or configured by development checks.

## Clipboard admission and worker boundary

The existing native job controller owns one operation, a hard/cooperative
five-second read deadline, cancellation and stale-result refusal. Add
`NativeCaptureCall::ClipboardImage` returning the normal one-frame RGBA reply
(`clipboard:image`, no physical geometry), and `ClipboardInspect { kind }`
returning one strict `ClipboardSnapshot`. Root's existing Portal/ImageUrl calls
and private error11 retain their wire meanings. No process is started until an
explicit read is requested. Native exceptions, blocked delayed rendering and
allocation failure do not cause an automatic retry.

`clipboard inspect` returns bounded format metadata only. `clipboard text` and
`clipboard files` explicitly return their requested content, never a passive
history. `clipboard image-info` explicitly reads one image and returns dimensions;
`clipboard image NEW.png` explicitly publishes that image with the existing
no-overwrite exporter. No command captures the desktop, changes clipboard
ownership, uploads, writes History or opens referenced files.

Limits are 256 offered formats, 128 UTF-8 bytes per retained format label, 64 KiB UTF-8
text, 32 file references, 4096 native bytes per file reference and 64 KiB total
native path bytes. Image input is <=32 MiB encoded, with existing Windows DIB
admission <=256 MiB +64 KiB headers; decoded RGBA stays within 256 MiB. One
snapshot kind per request keeps worst-case escaped JSON below the existing
1 MiB metadata packet ceiling. Binary/native file references retain an explicit
encoding plus bounded display representation; displayed lossy paths are not
silently used for filesystem actions. Metadata inspection never requests image
or text payloads just to discover offered types.

X11 uses one requestor window/connection in the owned worker and CLIPBOARD only,
never PRIMARY. The current reader accepts local numeric `DISPLAY` names such as
`:0` or `:10.0`, within 64 bytes. Forwarded/remote names such as
`localhost:10.0` are explicitly unsupported by this adapter.
TARGETS and property reads use finite requested extents; INCR
hints/chunks and cumulative bytes are checked before append, with one absolute
deadline and owner/generation checks. No arboard/GPUI unbounded image getter or
clipboard-manager handover is introduced by import. Windows checks HGLOBAL
extent before copying/locking payload data; format iteration, Unicode text and
CF_HDROP parsing are bounded. Delayed native rendering stays in the owned worker.
Registered Windows format labels have two additional UTF-16 buffer units, so a
truncated native copy cannot appear to meet the 128-byte label policy. Copied
length, termination and the resulting UTF-8 byte length are all checked before
retention, following [GetClipboardFormatNameW's truncation contract](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getclipboardformatnamew).

The X11 reader uses the already pinned x11rb connection and requires XFixes
selection tracking. It subscribes before the owner baseline/TARGETS, records each
server selection event plus its selection/event timestamps, and uses ordered
round trips on that same connection before accepting TARGETS or payload chunks.
Same-owner assertions and A→B→A ownership changes invalidate the operation; equal
timestamps do not hide an event. Synthetic XFixes events are rejected. The
operation owns one requestor window and refuses to process more than 4096 events.
This is a processed-event refusal, not a queued-event ceiling: blocking x11rb
replies may buffer an opaque internal event queue before the adapter can drain
and count it. No 4096-item guarantee is claimed for that dependency queue. Its
allocations remain inside the existing 96 MiB native VM-growth ceiling and the
owned helper's five-second hard deadline; cooperative checks do not make a
blocking reply itself interruptible. Neither global containment is relaxed.
Destroying the requestor ends observation, with no idle watcher. Protocol basis:
[XFixes selection tracking](https://xorg.freedesktop.org/archive/current/doc/fixesproto/fixesproto.txt).

macOS native pasteboard APIs can create an opaque data object before its length
is inspectable. That is not described as a 32 MiB transfer bound. The single-job
worker must install a native VM-growth ceiling before pasteboard access, verify
that ceiling with a small-success/over-limit-failure reservation probe, and fail
closed if unsupported. Native acquisition has 96 MiB growth allowance; after
bounded data admission/release, decoding may use 1 GiB growth allowance. Exact
private/native mapped memory is reported separately from decoded limits. The
current XNU implementation routes RLIMIT_AS to Mach VM size limits:
[XNU kern_resource.c](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/kern/kern_resource.c).
Runtime probing remains required rather than inferring support on every macOS
release. AppKit/pasteboard calls run on the worker's native main thread.

## Focused Wayland paste and drops

Standard Wayland selection is delivered only with keyboard focus; a headless CLI
without that authority fails with guidance to use focused application Paste.
It never falls through to an unrelated X11 selection or starts a global
clipboard-control protocol. This follows the
[Wayland selection/transfer protocol](https://wayland.freedesktop.org/docs/html/apa.html).
Linux authority detection includes descriptor-only `WAYLAND_SOCKET`, even when
`DISPLAY` also exists. Automatic reads reject that original context before worker
environment cleanup. Other native requests retain a frozen Wayland session marker
after cleanup, preserving display/region capability refusal while explicit X11
window operations keep their existing support. The original descriptor is marked
close-on-exec in the child only and its environment name is removed; the parent
descriptor is neither consumed nor changed.

Wayland's unassigned-offer queue holds at most two metadata wrappers. Selection
and Enter consume/remove an offer from that queue into the current clipboard or
drag role. Completed/rejected drags leave no historical queue entry, and selection
replacement destroys only the previous clipboard proxy. Selection churn cannot
evict a delayed drag; later drags cannot evict the current clipboard. The maximum
retention is one clipboard metadata wrapper plus two unassigned wrappers (each
<=256 labels of <=128 bytes), with one separate raw drag proxy. Native connection
teardown retires current roles and pending offers. This adds no timer, task,
thread, image cache or idle work.

The existing Paste image action starts a bounded image-specific asynchronous
GPUI offer read. Offer selection/receive/flush stays on the native event thread;
only a captured pipe descriptor and fixed metadata reach the existing background
executor. Pipe reads use <=64 KiB chunks, cumulative encoded limit and absolute
four-second deadline beginning when the background reader starts. Executor
scheduling latency is separate; offer/focus/job identity and cancellation apply
from admission through foreground delivery. The one-reader lease travels with
the background result and remains owned until foreground delivery or discard,
including a completed result waiting on that executor. A one-reader lease, focus/offer generation and UI job token
cancel obsolete work; no old result may open an editor after Quit, another input
or window close. External encoded image data is never kept in GPUI's clipboard
cache after transfer. Legacy synchronous offer paths gain byte/absolute-time
bounds; image import itself never waits on the foreground executor.
The foreground completion also checks the retained operation cancellation token
before releasing admission or opening an editor, covering Escape/Quit after
background render preparation has finished within the same generation.

The existing drop target accepts one file, one supported encoded image or one
explicit HTTP(S) image URL where native data offers expose them. Metadata-only
negotiation happens during movement. Raster bytes are requested only at the final drop. URI/file-reference metadata
may be negotiated on Enter, within 64 KiB, to preserve ordinary file targets;
HTTP(S) fetch/decode never begins before Drop. Both transfers have one absolute
deadline/cancel descriptor and at most one owned native reader, with no backlog. Decode/URL
fetch/render preparation reuses the existing image worker and ImageUrlSource;
there is no PNG staging/round trip, implicit upload or History insertion.
Malformed, multiple or unsupported payloads fail truthfully. Native transaction
finish/cancel and source/window generations prevent callbacks leaking into a
later drag. File paths retain their existing native encoding semantics.
Delayed URI metadata uses the native drag state's latest position. Metadata
captured at Enter cannot replace a later Motion position or the frozen final Drop
position when transfer completion arrives after Drop/Leave.

Native Drop acquisition can precede the application image-worker token. A short,
foreground-only cancellation seam links that phase to Escape, Quit and every new
application image admission (including delayed capture admission). It increments
the native drag generation, wakes its owned pipe reader, retires the incoming
offer and clears only incoming GPUI drag state, without synchronous/reentrant
window input dispatch. A newer input finishing cannot restore that generation.
Both raw and metadata reads retain their single lease through foreground
delivery/discard, bounding active plus completed native transfer bodies to one
<=32 MiB raster body or one <=64 KiB metadata body. New native generations cannot
start another read while a completed body still awaits foreground consumption.
The focused clipboard reader uses the same result-owned lifetime rule. No new
queue, runtime, permanent thread, polling timer or idle work is added.

## Print and file drag-out

Existing saved-image surfaces gain functional Print/drag bindings without layout
redesign. Only confirmed saved artifacts or explicitly finalized in-memory pixels
are eligible; dirty editor state must not accidentally print an older saved file.
All loading/render/spool preparation runs off GPUI's foreground executor. Native
print dialog/window calls and drag callbacks remain thread-correct and short or
run in an operation-owned native UI worker. Print cancellation does not invent a
success; after spool submission, status identifies acceptance/uncertainty and no
automatic retry. No default real-printer job is used in tests.

A one-page raster print document fits/centers the entire image, flattens alpha
against paper white explicitly, and preserves the original. PDF/spool generation
uses bounded strips and a 64 MiB encoded-output ceiling, not an extra unbounded
RGB frame. Native Windows printing uses the standard print dialog/GDI job;
macOS uses an owned main-thread print operation; Linux uses the desktop Print
portal, or an explicitly named CUPS destination for CLI. Private spool data is
operation-owned and never a preview PNG. OS spooler/driver allocations and native
modal-dialog latency are separate from application buffers.

File drag-out uses GPUI's existing ExternalDragPayload::Files API with one
snapshot path and directory bit, retaining no image bytes. Validation/selection
revision is prepared on the existing worker; the movement callback does no
filesystem/decode work. Native backends own a single bounded URI/CF_HDROP/file-URL
offer for the gesture, release it on cancel/end and never claim another program
accepted a file merely because native drag admission succeeded. The source must
remain available for the receiving application; it is not silently copied,
renamed or deleted by drag-out.

## Buildable slices and exact ownership

1. Core clipboard contracts/parsers and isolated X11/Windows/macOS readers;
   native request/reply/protocol plus explicit CLI commands.
2. Bounded asynchronous focused Wayland offer read and existing Paste/worker
   integration, then typed raw-image/URI drop negotiation.
3. Bounded print document/native output and existing saved-image surface wiring;
   native file drag-out adapters using the existing GPUI abstraction.
4. Synthetic protocol/limit/fault fixtures, private Xvfb/Wayland/Windows-station/
   named-mac-pasteboard tests, private CUPS/portal print tests, native mocks and
   optimized idle/active observations. Hardware/user-session gates stay separate.

Create `crates/app-core/src/clipboard_input.rs`, native clipboard siblings,
`crates/app-core/src/print_output.rs` and native print siblings,
`apps/desktop/src/clipboard_command.rs`, `apps/desktop/src/print_command.rs`,
private fixture scripts/tests and this document. Modify narrowly the existing
core clipboard/native-job exports, desktop worker/protocol/main/CLI, GPUI
clipboard/drop/drag platform seams, and `crates/app-ui/src/lib.rs`/
`workspace.rs` action/source dispatch. Exact final filenames and verification
commands/results are maintained here as each compiling slice lands. The editor
model and macOS capture/record adapters remain with their separate owners.

No development check may access the real general clipboard, send a real printer
job, change system printer settings, write a credential store or inject input
into the user's desktop. Private native helpers must establish their private
context before any native clipboard or print call. Bounds cover transfer and
metadata before decoder admission; a decoder cap alone is insufficient evidence.

## Current compiling slice checkpoint

Implemented so far: isolated native ClipboardImage/ClipboardInspect calls,
bounded native X11/Windows/macOS readers and parsers, explicit clipboard CLI,
focused asynchronous Wayland image read, existing cross-platform Paste action,
encoded/URL source routes into the existing editor worker, and bounded Wayland
raw/URI drop transport. The remaining X11/Windows/macOS raw/URL payload adapters,
print and file drag-out integration are still implementation work; the overall
family is not declared complete at this checkpoint.

The next native-integration ledger also includes descriptor-only Wayland GPUI
startup: the connection may consume `WAYLAND_SOCKET` before graphical-environment
backend detection, which currently considers only `DISPLAY`/`WAYLAND_DISPLAY`
and can then select X11. This is separate from the already refused Automatic
clipboard CLI context. It must be corrected before the entire integration family
is complete; it is not claimed fixed by the clipboard contract/test correction.

Native acquisition is 96 MiB growth for encoded clipboard formats; Windows DIB
size is checked first and may raise the copy phase to 256 MiB +64 KiB +16 MiB
without lowering its existing input ceiling. Decoding then has its separate
1 GiB growth allowance. Windows uses an owned nested process-memory Job limit;
Unix uses checked RLIMIT_AS and a reservation probe. This is native containment,
not a claim that every decoder's entire scratch budget equals encoded length.

The delivered file boundary is exact:

- Create `crates/app-core/src/clipboard_input.rs`,
  `clipboard_input_budget.rs`, `clipboard_input_x11.rs`,
  `clipboard_input_windows.rs` and `clipboard_input_macos.rs`.
- Create `apps/desktop/src/clipboard_command.rs`,
  `scripts/test-clipboard-input.py`, `scripts/test-wayland-clipboard.py` and this
  `docs/clipboard-import-output.md`.
- Create `vendor/gpui-pre-linux/src/linux/wayland/clipboard_image_read.rs`,
  `incoming_drop.rs` and `drop_metadata.rs`.
- Modify `crates/app-core/src/capture.rs`, `capture_native_linux.rs`,
  `native_capture_job.rs`, `lib.rs`,
  `clipboard_windows.rs` and `clipboard.rs` (the latter also exports the actual
  opened-identity plus remaining-decoded-budget loader for the editor consumer).
- Modify `apps/desktop/src/capture_worker.rs`, `capture_worker_protocol.rs`,
  `main.rs` and `cli.rs` for explicit dispatch only. Existing Portal/ImageUrl
  semantics/error 11 are preserved; clipboard failures use private category 12.
- Modify `vendor/gpui-pre/src/platform.rs`, `app.rs`, `interactive.rs` and
  `window.rs`, plus `vendor/gpui-pre-linux/src/linux/platform.rs`,
  `display_connection.rs`, `wayland/clipboard.rs` and `wayland/client.rs`.
- Modify `vendor/gpui-pre/src/app/test_context.rs` and
  `platform/test/platform.rs` for one test-only private native cancellation hook.
- Modify `crates/app-ui/src/lib.rs`, `workspace.rs` and `ui_tests.rs` for existing
  action/source dispatch, cancellation/job generation and private fixtures.

The current private X11 CLI proof is
`target/verification/clipboard-round6-x11.json` (nine groups, executable
SHA-256 `cd770d5abc37680ae5cbf330fe37fefda803051bf49bfb0bd0d4bb8807005406`), and
the focused native GPUI Wayland proof is
`target/verification/clipboard-round4-wayland.json` (test executable SHA-256
`e9600165097c625268eebce484155a52512751aba80467833a73e20fcbe5472e`). Both use only generated data
and owned private displays/compositors. The latter creates a private virtual
keyboard solely to make the headless compositor deliver ordinary keyboard-focus
and selection events; production reading uses standard `wl_data_offer.receive`,
never clipboard-control. Assertions include exact RGBA/hidden RGB/alpha in the
existing editor and unchanged saved capture/History/clipboard publication state.
Neither proof establishes hardware input/presentation or all-platform acceptance.

Pure native-pipe regressions cover exact byte boundary, first-byte overflow,
absolute timeout despite progress and cancellation wake; native offers cap count
and label retention, and owned-item refusal releases admission. UI fixtures cover
busy admission, transfer errors, stale callbacks and no image/disk/clipboard
output. The foreground validates offer/focus again after transfer, so a completed
background read cannot bypass a focus/selection change queued before delivery.
No external image is placed in GPUI's persistent clipboard cache.

The first fresh review reported four accepted defects; its fix batch additionally
passed actual CLI cases for descriptor-only private Wayland with an unrelated
private X11 selection (no selection request), XFixes same-owner/A→B→A changes
during TARGETS, and same-owner change during INCR. Focused optimized Rust tests
pass original-context refusal before helper launch, child-only descriptor
close-on-exec/frozen context, same-generation cancellation after rendering, and
delayed metadata after final Motion/Drop. Linux desktop check/build and Windows
GNU desktop/test metadata checks pass. Reused optimized test binaries pass all
57 UI tests and all 65 Linux platform tests. Formatting and focused core/UI/
desktop Clippy pass (existing warning categories remain). XFixes observation
reuses the operation's connection and ends with the requestor; extra ordered
round trips occur only during an explicit transfer. No numerical idle/resource
or hardware performance acceptance is inferred from these functional checks.
These fixes require a new independent
review; passing author checks do not declare that review complete.

The second fresh review confirmed those four fixes and found two additional
Wayland lifetime/fixture defects plus a Windows native-name truncation issue.
The role-owned offer correction passes 67 optimized Linux platform tests,
including clipboard→completed/rejected drag→new drag and delayed drag with 32
selection replacements/unassigned-offer churn. Four optimized core clipboard
contract tests pass, including the native-copy truncation/UTF-8 label model.
The actual ignored Wayland fixture now checks absence of `WAYLAND_SOCKET` before
its platform constructor; a pure constructor-sentinel test and a direct ignored
fixture invocation with an owned private descriptor both prove refusal. The
private Wayland script passes both that refusal and the successful real focused
Paste/editor path. All 57 optimized UI tests, Linux check, Windows GNU desktop/
test metadata, formatting, Clippy and diff/Python syntax checks pass. No actual
Windows general clipboard or user compositor was accessed by these checks.
Current author evidence is under `target/verification/clipboard-round2-*`;
the third fresh review found no Major/Blocker and one documentation Nit, which
was corrected. That single clean round was not convergence: the fourth review
then identified the acquisition-cancellation gap and prematurely released read
leases described above.

The fourth review's fix batch passes all 69 optimized Linux platform tests and
all 59 optimized UI tests. The actual bounded private-pipe reader completes while
foreground delivery is withheld; 256 native-generation/cancellation churn
attempts cannot acquire another reader lease until the completed body and lease
are discarded. Native cancellation writes its wake descriptor and rejects a late
raw-image completion. Headless app tests use a private native adapter and the
actual Escape/common image admission paths: both Escape during acquisition and
a newer image input finishing before old completion prevent Submit/editor open.
Clipboard success/refusal completions retain their own lease until foreground
delivery/discard. The real private Wayland fixture passes descriptor refusal and
focused Paste with exact pixels through that adjusted clipboard lifetime. Linux
check, Windows GNU desktop/test metadata, formatting, Clippy and diff checks pass.
This adds no production queue/thread/timer; the test cancellation hook exists
only on the test platform. Evidence is under `target/verification/clipboard-round4-*`.
The source is frozen for fresh fifth review; author checks do not establish
independent convergence or physical input/presentation acceptance.

The fifth fresh review was clean; the sixth found the queued-versus-processed
X11 event-count documentation error. The contract now states the adapter's
4096 processed-event refusal and explicitly describes opaque dependency buffering
inside the unchanged native VM-growth/hard-deadline containment. No native code,
compiler or application dependency changed in this correction.

The private authenticated Xvfb fixture sends 5000 genuine same-owner XFixes
notifications before the TARGETS reply. A finite observer uses the existing
private-worker helpers, verifies child lineage/identity and keeps owned pidfds;
the operation fails with no stdout/image publication, and all observed helper
pidfds signal exit. All nine private executable groups pass. One optimized
development observation took 11.98 ms, with five sequential process-family
memory samples (CLI plus one observed helper): maximum sampled summed RSS was
40804 KiB and PSS 22980 KiB; the helper's sampled address-space soft ceiling was
195645440 bytes. The 2 ms observer can miss peaks and summed RSS counts shared
pages repeatedly. These samples are not bounds, reference-hardware acceptance
or a latency benchmark. Native allocation/deadline limits provide containment
independently of the samples. Evidence is
`target/verification/clipboard-round6-x11.json`; this correction is frozen for
fresh seventh review, without a convergence claim.
