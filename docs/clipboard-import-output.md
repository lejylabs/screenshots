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

## Windows existing file-drop metadata bounds

This historical correction addressed existing CF_HDROP file references only.
Source review rounds 6/7 were clean before application. Actual local optimized
Linux and Windows GNU all-target metadata checks, Windows dependency-unit metadata
and all-target workspace Clippy passed for that source. Later sections describe
the integrated raw-image/URL receiver, printing and native drag-out adapters with
their exact owned-test and remaining native/resource gates.

Both existing GPUI consumers now share one all-or-nothing native-path reader:
OLE `IDropTarget::DragEnter` and the legacy GPUI `read_files` clipboard path.
`GlobalSize` must report 22..=65,536 bytes before metadata is locked/enumerated.
An allocation-free DROPFILES pass validates the byte offset, wide alignment,
double terminator, at most 32 references and at most 4,096 native bytes per path.
The whole HGLOBAL extent, including its header, list terminators and allocator
padding, fits 64 KiB. Bytes after the double terminator are allocator padding and
are not further file references. ANSI offers retain Shell conversion semantics;
wide offers retain exact Windows path units, including unpaired surrogates, using
`OsString::from_wide`, without a lossy String intermediate.

Only after structural validation does `DragQueryFileW` verify the count and
preflight every output length. Converted UTF-16 paths are at most 4,096 bytes each
and 65,536 bytes in total, excluding terminators. Each copy must match its queried
length, end at the expected terminator and contain no earlier NUL. Any failure
rejects the entire list before an Entered event or clipboard entry can escape;
valid initial files in an oversized/malformed offer are never accepted alone.
There is no truncation. These limits deliberately reject larger file metadata.

Temporary application storage is 32 stack lengths plus a fixed 2,049-unit UTF-16
scratch buffer (4,098 bytes), with no metadata Vec growth. The returned SmallVec's
actual capacity must be at most 32 path slots. Windows PathBuf heap capacities
are checked independently from UTF-16 length: at most 8,192 bytes per path and
196,608 bytes in total, reflecting WTF-8 storage/allocation capacity. A rejected
just-created path is immediately dropped. The native HGLOBAL and the toolkit's
existing drag clones are additional retained costs; GPUI currently keeps a path
clone for drag identity and the original for its drag view. No image buffers,
threads, workers, runtimes, queues, caches, dependencies, timers or idle work are
added. Per-target generation/phase/closed cells and a helper-open flag retain no
extra path clone for termination. Each open native Windows window now creates
its own IDropTargetHelper COM instance; the platform-wide singleton is removed.
The extra native object per open window is a demonstrated ownership cost, with
its opaque COM allocation/visual resources still unmeasured. FileDropState moves
inline into WindowsWindowInner and is shared through its existing Rc, with no
new state allocation. Native-destruction, close-request and explicit registration-lease state are inline
cells.

OLE acquires one guarded STGMEDIUM and releases it on every successful-GetData
exit, including invalid type/handle, malformed data and reentrant cancellation.
It checks `tymed` before reading the HGLOBAL union member and passes the movable
handle, rather than a locked pointer, to Shell. Rejected Enter stays NONE during
Over/Drop; only an admitted generation can submit. Leave retires the native
phase and emits GPUI Exited before calling the foreign helper. Drop keeps the
Accepted phase visible while its helper runs; a nested Enter must emit Exited
for A before installing B. Only an unchanged generation is retired/submitted
after helper return. Reentrant Enter/Leave cannot leave A attached to B, and an
outer stale callback cannot clear or submit B. New Enter also terminates any
previous helper lease before opening its own.

The helper-open flag is consumed before a terminal helper callback, so a nested
Enter can keep its own flag without an outer callback clearing it. An entered
rejected offer still calls helper Drop with NONE; missing data uses helper Leave
and never submits GPUI data. The terminal callback does not rely on a later OLE
Leave after Drop. Duplicate terminal notifications have no helper lease left.
No toolkit borrow spans source/helper COM calls; callbacks check generation and
the existing owner-close flag after those calls and medium release. Copy is
selected only if the source permits it. The existing file-drop feature and the
application's one-file/busy/decode admission remain in place.

The helper instance and drop state now have the same window lifetime. Closing a
window consumes its helper lease synchronously, invalidates its native drag
phase and permanently refuses later Enter/Leave/Drop work. Its existing deferred
teardown task holds that window/helper through an explicit final helper Leave,
dialog-idle wait, RevokeDragDrop and DestroyWindow. Revoke is used for its target
Release contract; no implicit DragLeave callback is assumed. A different window
creates a different helper instance, so delayed A cleanup and late A callbacks
cannot consume B's helper state. Close during helper Enter or a terminal helper
callback consumes the lease only once.

Accepted WM_CLOSE requests run the existing GPUI close callback and suppress
DefWindowProc's immediate DestroyWindow path. GPUI queues removal, which reaches
the same owned teardown while the HWND is valid for Revoke. A Cell flag suppresses
reentrant close requests while the veto callback runs; veto and later close
remain supported. The owner is closed before notifying the accepted-close
callback, so duplicate close cannot default-destroy the window early. Before a
close callback is installed, the original default-procedure fallback remains.

WM_DESTROY retires native input/helper ownership before source callbacks and
revokes the registered target while its HWND is still valid. WM_NCDESTROY is also
a final input/helper retirement seam. A claimed helper Leave is queued on the
existing foreground executor after native/state borrows unwind: at most one
operation task for unexpected native destruction, not a new queue or service.
Normal app/requested close uses its existing teardown task.

Registration has an explicit Absent/Registered/Revoking lease. It is recorded
before RegisterDragDrop's AddRef call and rolled back on registration failure.
Both normal teardown and WM_DESTROY use the same one-shot revocation seam;
Revoking is recorded before calling OLE so duplicate/reentrant destruction cannot
issue another Release. Successful/known-absent revocation clears the lease. A
native revocation error keeps the lease recorded and prevents normal teardown
from destroying the window or claiming release; actual native-error recovery
acceptance remains open. The WindowsWindow wrapper is established before
registration, so registration or later initialization errors also reach owned
teardown. No undocumented OS auto-revocation is assumed.

A native-destruction flag is set at WM_DESTROY, before revocation, and at
WM_NCDESTROY as a fallback. The initial ShowWindowAsync hide, normal Revoke and
Destroy paths are all skipped once destruction starts. The flag is rechecked
after foreign calls, so queued removal after unexpected native destruction
cannot hide, revoke or destroy a later B that reuses the HWND. No state lock or
RefCell borrow spans source/helper calls; no new timer/thread/runtime is added.
Native COM creation, API failure/ordering, allocation and visual cleanup still
require actual Windows acceptance.

The native `IDropTarget` implementation explicitly sets `Agile = false`.
The pinned windows-implement 0.60.2 macro otherwise adds IAgileObject and the
free-threaded marshaler by default, which is incompatible with the handler's
Rc/Cell-owned foreground window state. Windows GNU library/test metadata verifies
the actual binding parameter. A production native two-STA/QI assertion belongs
to the full isolated receiver fixture; this correction alone does not bound
foreground GetData or establish acquisition isolation.

Incoming GPUI payloads now also carry one private inline origin: the incoming
target WindowId or internal source WindowId. All four AnyDrag constructors are
updated. A new incoming path/image/URL offer may replace older incoming metadata;
it does not replace an internal drag. Window removal drops only a payload owned
by that window, including its view. File Pending/Submit require the target owner,
and Exited retires only that target's incoming payload. Removed windows refuse
input before dispatch. The generational WindowId, rather than a reused native
handle, identifies the owner. Late A callbacks/removal cannot clear or submit B.

Outgoing promotion transfers the internal origin to its native source window.
Restored-drag handoff verifies that origin; an outgoing terminal only clears its
own restored internal payload. Ending/removing a suspended outgoing source cannot
clear a newer incoming target. Explicit native-drop cancellation uses incoming
origin rather than payload type, preserving an internal drag whose value happens
to be ExternalPaths. This adds one enum containing a WindowId/tag per active or
suspended drag, without another payload clone, background service or idle work.
All in-repository construction sites are covered; AnyDrag's new ownership field
is private to this vendored toolkit.

The pending-native-drop seam remains separate. Window removal does not invoke a
global native cancellation that could cancel B. Wayland already checks the closed
window identity in drop_window before invalidating its native offer/reader; X11,
Windows and macOS ingress use the shared GPUI consumer. Windows owner closure
suppresses input dispatch from native callbacks; App window removal remains the
authoritative release of its incoming GPUI payload. Helper ownership is retired
explicitly through the window's shared drop state and deferred cleanup above,
without requiring a later OS Leave callback. The closed-target fixtures do not
prove native Shell callback ordering on window close.

Historical acquisition limit of the earlier f2f03a4 correction (superseded by
the integrated receiver source below; native runtime acceptance remains open):
the 64 KiB check was **post-acquisition**. Source `QueryGetData`/`GetData`, release
callbacks and drag-image helper calls remain synchronous COM/native operations.
`GetData` can allocate an arbitrarily large native medium or block before this
reader sees its extent; rejecting that returned medium does not undo that peak.
No hard native acquisition time/memory bound, foreground latency acceptance or
complete Windows clipboard/drop integration is claimed for this correction.

The next acquisition slice must trace COM apartments with a controlled source,
then move delayed rendering to one operation-owned, correctly marshaled STA
consumer, retaining its admission lease through delivery/discard. The candidate
hard-containment path is a lazily started helper with a Windows Job memory limit
installed before native acquisition and an externally enforced five-second kill
deadline. It must first prove that marshaled calls do not execute back on GPUI's
foreground apartment. Raw IDataObject pointers cannot be moved across threads.
`IDataObjectAsyncCapability` is optional; unsupported sources need a tested
compatibility path. `GetDataHere` permits a consumer-sized HGLOBAL but is not
universally supported and does not itself supply a time bound. Query, marshaling,
release and drag-image helper calls also require review; moving only GetData is
insufficient. If ordinary OLE proxies still route blocking calls through the
foreground, the receiver's apartment/process boundary needs a fresh design before
that gate can pass; adding another timed-out thread is not a fallback.

Regression sources include seven private pure structural/generation fixtures,
four private per-window helper factory/endpoint fixtures, four private
registration/resource-retention fixtures, four Windows-only
private HGLOBAL fixtures, three private COM/medium lifetime fixtures, three
private native-close routing fixtures, seven Windows-lifecycle GPUI headless
consumer fixtures and five shared GPUI incoming-ownership fixtures. They cover all-or-nothing
refusal, count/path/extent edges, malformed headers/termination, ANSI conversion,
exact native Unicode units, actual retained capacities, generation replacement/
retirement/exhaustion and both STGMEDIUM release ownership forms, including a
provider release that reenters a newer generation.

The GPUI fixtures invoke the exact production begin/Leave/Drop lifecycle methods
with injected helper notifications and the real Window::dispatch_event,
active_drag and on_drop consumer. They verify normal Drop, Enter A -> Leave/helper
reentry -> Enter B -> Drop B, Drop-helper reentry through Enter B or Leave only,
rejected offers, missing data and a rejected helper reentering accepted B. A must
not remain retained or be submitted as B, and every entered helper gets one
terminal notification. The private lifecycle module is included as test source
in GPUI; production crates gain no dependency or public API. The ownership
fixtures verify two-window close/reopen with weak payload release and only B
submitted, old A events/removal preserving live B, image/URL-body release,
internal ExternalPaths preservation during cancellation/other-window removal,
and suspended outgoing-source close while incoming B remains live.

No fixture opens or changes the user's clipboard or windows. Actual optimized
execution passed 15 standalone pure fixtures, five GPUI ownership fixtures and
22 GPUI Windows-consumer fixtures; the latter include those same 15 pure cases
and are not 22 additional consumer cases. Seven SVG/font regressions and three
style assertion tests also pass. The complete workspace run passed 1,065 regular
tests across 60 outer suites, with zero failures and 71 opt-ins excluded. Child
harness summaries are excluded from that total. Formatting and diff checks pass.
Logs and exact test-binary identity are under
`target/verification/windows-drop-integration/`. Native Windows execution and
resource acceptance remain open.
The factory/endpoint fixtures verify distinct helper identities, close with no
later OS Leave, deferred A cleanup after B Enter, late/duplicate A terminals,
close during helper Enter/Leave/Drop and destruction while close-Leave reenters.
The close-routing fixtures invoke the actual WM_CLOSE routing function with
private callbacks and an inert HWND, including veto/reentrant/duplicate cases.
The registration fixtures use the production lease and a fake native registry
holding real Rc resources until revoke. They verify unexpected destruction with
no queued window Drop releases that resource, duplicate/reentrant revoke never
releases twice, an already-destroyed A's queued teardown performs no hide/revoke/
destroy on a reused B handle, and registration/revocation failures retain or roll
back the resource lease truthfully.
The injected helper fixtures do not invoke an actual IDropTarget COM entry point,
CoCreateInstance or the Shell helper. Native apartment/drag-helper reentrancy, real Explorer
negotiation, presentation/CPU/private-memory measurements and full platform
acceptance remain unrun. These source fixtures are not runtime or hardware
acceptance evidence.

Native basis: [DROPFILES layout and encoding](https://learn.microsoft.com/en-us/windows/win32/api/shlobj_core/ns-shlobj_core-dropfiles),
[DragQueryFileW count/length/termination](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-dragqueryfilew),
[GlobalSize allocation extent](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-globalsize),
[GetData medium ownership](https://learn.microsoft.com/en-us/windows/win32/api/objidl/nf-objidl-idataobject-getdata),
[ReleaseStgMedium ownership](https://learn.microsoft.com/en-us/windows/win32/api/ole2/nf-ole2-releasestgmedium),
[DragEnter allowed effects](https://learn.microsoft.com/en-us/windows/win32/api/oleidl/nf-oleidl-idroptarget-dragenter),
[DoDragDrop terminal callbacks](https://learn.microsoft.com/en-us/windows/win32/api/ole2/nf-ole2-dodragdrop),
[RegisterDragDrop target AddRef](https://learn.microsoft.com/en-us/windows/win32/api/ole2/nf-ole2-registerdragdrop),
[RevokeDragDrop target release](https://learn.microsoft.com/en-us/windows/win32/api/ole2/nf-ole2-revokedragdrop),
[WM_DESTROY notification](https://learn.microsoft.com/en-us/windows/win32/winmsg/wm-destroy),
[WM_CLOSE default destruction](https://learn.microsoft.com/en-us/windows/win32/winmsg/wm-close),
[CoCreateInstance object creation](https://learn.microsoft.com/en-us/windows/win32/api/combaseapi/nf-combaseapi-cocreateinstance),
[IDropTargetHelper notifications and source calls](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nn-shobjidl_core-idroptargethelper),
[COM interface marshaling](https://learn.microsoft.com/en-us/windows/win32/api/combaseapi/nf-combaseapi-comarshalinterthreadinterfaceinstream),
[optional asynchronous data extraction](https://learn.microsoft.com/en-us/windows/win32/api/shldisp/nn-shldisp-idataobjectasynccapability)
and [consumer-provided storage](https://learn.microsoft.com/en-us/windows/win32/api/objidl/nf-objidl-idataobject-getdatahere).

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

## Integrated clipboard/Paste checkpoint (updated 2026-11-16)

Implemented so far: isolated native ClipboardImage/ClipboardInspect calls,
bounded native X11/Windows/macOS readers and parsers, explicit clipboard CLI,
focused asynchronous Wayland image read, existing cross-platform Paste action,
encoded/URL source routes into the existing editor worker, and bounded Wayland
raw/URI drop transport. The remaining X11/Windows/macOS raw/URL payload adapters,
print and file drag-out integration are still implementation work; the overall
family is not declared complete at this checkpoint.

The earlier ledger's descriptor-only Wayland startup defect is corrected in the
separate [startup integration](linux-wayland-startup.md). It snapshots authority
before descriptor consumption and retains it through GUI/native operations.
All 19 owned native startup/headless cases pass on the integrated optimized
executable. Automatic clipboard CLI refusal remains a separate early boundary;
actual X11-focused Paste follows the connected client. Physical clipboard/input,
remaining native payload adapters, printing and app drag-out remain open.

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

The seventh and eighth fresh source reviews both completed with no Blocker,
Major or Nit findings; all 20 frozen source hashes matched in each. This gives
source-review convergence for the compiling clipboard/Paste/Wayland-drop slice.
It does not close native platform/resource acceptance or implement the remaining
native payload adapters, printing and drag-out. Descriptor-only GUI startup
was subsequently integrated and independently verified in the linked contract;
those original clipboard reviews alone did not establish that result.
The private native evidence and the explicit opaque-X11 queue boundary above
retain their stated scope. These reviews ran read-only without user clipboard,
printer, device, account or native application calls.


## Integrated Windows receiver source (2026-11-17)

The receiver source is now integrated through a genuine three-way merge that
preserves current Unix services, Jiff and the earlier non-agile binding fix.
Original patch `0b7f93cbca2126938b40c85a688370e547dfdf2541815011676c74b569c0aae3`
received actual fresh clean source reviews 5/6. Root corrected one constructor
call to the inner window object during real GNU type checking. Optimized Linux
all-target checking, Windows GNU workspace/all-target and vendor-test metadata,
122 optimized existing application consumer tests (9 ignored), three optimized
actual deadline tests, Clippy and formatting pass. The foreground source-calling
target is removed. Actual Rust/native receiving, first-boundary/fault/lifecycle/
editor and resource acceptance remain unrun; source/metadata alone never closes
that gate. Exact preparation/resolution and named results are retained under
`target/verification/windows-production-integration/`.

The implementation installs only a process-spawner pointer at ordinary startup. A
single receiver process and two parent threads (controller/watchdog and bounded
private-pipe reader, each requesting a 256 KiB stack) exist while visible,
non-minimized GPUI native windows are eligible. Every GPUI Windows window uses
the same seam; hidden/minimized targets retire asynchronously, and a restore
waits for its previous registration to finish. A standalone GPUI probe without
an installed spawner has no incoming OLE registration. The receiver loads no
renderer, decoder, capture stream, clipboard reader or network runtime.

The parent arms an absolute five-second deadline before acknowledging each
receiver native interval, including initialization, registration, PeekMessage,
dispatch, source/helper/medium/error-reference cleanup and shutdown. Native
progress or nested callbacks cannot renew that deadline. The existing +96 MiB
probed private-commit growth guard is installed before native acquisition. The
parent retains the exact process and Job through reaping and joins its one
blocking reader before replacement. A failed OS termination/reap retains that
cleanup owner and prevents overlap. The foreground never waits synchronously.

A single native admission covers active and completed transfers through final
foreground delivery/discard, including cancellation and receiver replacement.
The fixed protocol carries at most one <=32 MiB encoded body or <=64 KiB native
metadata body (file framing adds at most 132 bytes), <=32 target records, and
coalesced per-target movement. Shell helper creation/release is per gesture and
remains inside the native interval. Native paths preserve UTF-16/WTF-8 semantics;
PNG/JFIF/WebP/DIBV5/DIB and credential-free HTTP(S) URLs reach the existing typed
GPUI drop and image-worker routes. Raster acquisition and URL fetch begin only
at final Drop; hover never fetches or decodes. Source diagnostic descriptions
are not read or logged, and native IErrorInfo TLS references are drained under
the same watchdog with a finite churn bound.

Revision 10 of the separate owned Windows topology fixture passed all 18
mandatory direct/actual-OLE cases, with three explicit non-input negative
controls recorded separately. It establishes the C++ receiving boundary and
first custom-unmarshal guard, not the Rust production integration. The integrated
Rust target explicitly disables windows-implement's default agility. Its QI and
two-STA callback fixture, payload/protocol/lease/deadline tests, production native
fault/editor/close tests and optimized joint resource observations remain unrun.
The 50/100 MiB idle/visible-shell and all PLAN performance budgets are unchanged.
Neither this source checkpoint nor the topology result establishes native user-session,
physical input/presentation, mixed-DPI or performance acceptance.

The first independent source review of patch c3bd41ac was not clean: a missing
Windows flag import, reentrant Leave revival and an overdue-event deadline bypass.
The held fix separates gesture retirement from captured operation/admission IDs
and checks the absolute clock before permits, completion and locked publication.
Actual Rust COM reentry and overdue-publication regressions are authored but
unrun. Extra resource cost is bounded scalar operation state per target and
one small Rust Rc allocation per active helper; no queue/thread/timer is added.
Later actual source reviews 5/6 are clean. Root metadata/consumer/deadline checks
above pass; actual native and aggregate resource checks remain required.

The separate cfg-only production-native fixture has authored real-spawner/helper
COM/OLE, terminal/effect/count, foreground rejection/cancel, failure/reap and
owned-desktop controls. Its evolving counts are authored inventory, never pass
counts. Original R10 remains a C++ prerequisite. The new fixture must clear its
own source review, cfg/MSVC/C++/PowerShell checks and actual positive/negative
execution before it supplies receiving-boundary evidence.

## Unix persistent file-reference output (held implementation)

`ImageClipboard::copy_file` accepts exactly one explicit absolute existing regular
file. Native path metadata is <=4096 bytes and its canonical target is rechecked
inside the isolated writer before publication. Linux offers `text/uri-list` and
`x-special/gnome-copied-files` with the literal `copy` operation; no move/delete
operation, image encode/decode, file-content read, temporary file, disk round-trip,
upload, History mutation or production clipboard verification is introduced.
A file reference is a pathname: later source mutation/deletion can change what a
recipient opens, and no immutable-content promise is made.

A lazy installed function pointer starts at most one file writer at a time. One
256 KiB Rust supervisor owns the authenticated inherited transport and process
family through final termination/reap; dropping the public handle only disconnects
a one-slot channel. The supervisor parks after publication, then performs active
cleanup when replaced/dropped. Busy admission remains held throughout cleanup.
Native startup/canonicalization/publication is contained by a five-second outer
deadline; failed or lost acknowledgement reports that clipboard content may have
changed. Success means publication was acknowledged, not a clipboard read-back or
recipient paste. An unpublished/cancelled owner can never later acknowledge a
new operation. Subsequent image/text output checks the installed file-owner
quiescence fence before publishing, so a canceled child cannot overtake later
output while cleanup is pending. Their native setters are otherwise unchanged.
Native dependency allocations retain the existing runtime-probed 96 MiB
VM-growth guard inside the private child. No process/thread starts at ordinary
startup and no permanent timer/polling loop is installed.

X11 uses a local numeric DISPLAY only, one connection/window, CLIPBOARD only, a
bounded one-request URI property and a native-fd/control-fd poll with no idle timer.
It offers no PRIMARY and performs no clipboard-manager handover on release.
Wayland uses ext-data-control-v1 or wlr-data-control-v1 on exactly one seat. These
privileged protocols are unavailable on some compositors, including usual GNOME
sessions: that capability failure is explicit, with no XWayland/text fallback.
No focused gesture control is added. The owner destroys unsolicited offers
without reading their content, retains one URI payload and at most four
nonblocking outbound file descriptors with five-second transfer deadlines.
A native event batch is bounded by 4096 handled events and the opaque dependency
queue stays inside the private worker memory ceiling. X11 materializes at most
one additional <=12 KiB Copy wrapper during a corresponding request; Wayland
streams the fixed Copy prefix plus its sole retained URI without duplicating it.
macOS writes immediate public.file-url data with the same Pasteboard Manager API
as the existing reader on the private child's native main thread, not GPUI's
foreground/background executor. The macOS supervisor cleans up immediately after
acknowledgement; native pasteboard storage preserves the copied file reference.
A named private pasteboard is used by acceptance
fixtures; the general pasteboard is never used by development tests.

`clipboard copy-file ABSOLUTE_FILE --hold-ms 1000..300000` is an explicit CLI
route that retains Linux delayed ownership for the requested duration. Ctrl+C
ends that lifetime and begins cleanup. No new UI controls are added. Existing
image/text setters retain their previous native-call deadline limitations;
this file-output implementation does not establish their hard deadline gate.
The existing Windows file setter likewise retains its previous native-call
deadline limit. Standalone CLI completion awaits file-owner reap and reports
that its requested Linux ownership lifetime has ended.

Resource acceptance remains unmeasured: helper private/resident VM, Wayland/X11
connection buffers, native pasteboard objects, supervisor stack/RSS (plus the
existing 128 KiB macOS parent-lifeline thread during its short helper lifetime), first/warm
publish latency, release/reap latency and idle wakeups require optimized private
server runs. Targets are not measurements. Root must run pure tests and private
Xvfb/data-control/named-pasteboard fixtures before claiming native completion.

Revision 2 adds an operation-scoped signal self-pipe for the finite CLI hold. Both
SIGINT and SIGTERM write the bounded native wake endpoint regardless of which
thread handles the signal; a byte remains readable if delivery occurs between
checking cancellation and entering poll. The read endpoint is polled with the
absolute hold deadline and no periodic timer. Handler registration and the socket
pair exist only for this explicit CLI operation and are released on exit; wake
errors still await native owner retirement/reap. The socket's requested send buffer
is 4096 bytes (the OS may round its actual bounded allocation). Wayland seat
Capabilities/Name events now use explicit bounded Dispatch, count against the
4096-event batch policy, and reject names over 128 bytes. Pure seat admission and
private subprocess signal-before-poll/other-thread hold fixtures are authored;
these checks have not been executed by the delivery agent.

Revision 3 keeps standalone completion authority through every ordinary CLI exit.
Resident publication/cancellation still returns at its existing deadline plus
250 ms cleanup grace, explicit resident retirement still returns after five
seconds, and Lease Drop remains nonblocking. Those bounded returns do not prove
reap. The standalone command additionally owns a zero-sized completion drain:
its condition variable waits without a polling timer until the actual supervisor
releases admission after Native's process-family cleanup and child.wait finish.
The clipboard/lease drops before this drain; cancellation's conditional shutdown
is enabled only after the drain completes. Thus publication errors and retirement
timeouts retain a live CLI until actual cleanup completes, rather than relying
on parent-death signaling after process::exit. A cleanup that remains kernel-blocked
keeps that standalone CLI alive; no finite completed-reap claim is fabricated.
The static mutex/condition variable add no thread, process, runtime, channel,
permanent timer or metadata cache. Private subprocess regression sources use the
actual supervisor with a controlled seven-second delay before actual reap,
observe the unchanged bounded API error, require the CLI to remain alive beyond
both existing thresholds, and permit ordinary error exit only after the same
owned child.wait marker. Their isolated Linux controller is a subreaper and
actually reaps adopted providers even when an early-exit regression fails.
These new source regressions remain unrun by the delivery agent.

## Integrated owned transfer checks — 2026-11-18

The optimized private X11 drag/drop suite passed 25 native cases, including real incoming provider cancellation/staleness, URL handoff into exact editor pixels and outgoing Copy/Status/Finish/Escape/close paths. All 23 observed authenticated helper identities exited. The real XI2 implicit press requires grabbing the matching XI device; unrelated devices cannot advance or release its transfer. These are owned virtual-server software results, not physical input or compositor-presentation acceptance.

Unix file output also passed six ordinary regressions and all five explicitly enabled private cases: stalled publication, seven-second delayed actual reap across standalone errors, authenticated X11 roundtrip through the existing isolated reader, both ext/wlr data-control paths and explicit refusal without data-control. Exact writer test SHA256 was `560ab575dbc3772245cc9e2d0f6713642d04f6277bcbf8282567ed8140616869`; the independent CLI reader SHA256 was `24a0d4907b5038575c43df3ae80ee45a936beb6c293d2f9c9e15284cd9f6fafc`. The test requires every ignored case to run; zero selection is failure. No user clipboard or desktop was touched.

Linux workspace/all-target and Windows GNU metadata checks pass. Windows production COM/editor acceptance is running separately after the optional visual-helper correction; the earlier complete 105-case run had 50 failures and is retained as failed evidence. macOS named-pasteboard runtime, real compositor/native gesture coverage, whole main/helper private memory and PLAN hardware budgets remain open. Later image/text setter isolation is a separate change; this checkpoint does not certify that path.
