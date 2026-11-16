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

Limits are 256 offered formats, 128 native bytes per format label, 64 KiB UTF-8
text, 32 file references, 4096 native bytes per file reference and 64 KiB total
native path bytes. Image input is <=32 MiB encoded, with existing Windows DIB
admission <=256 MiB +64 KiB headers; decoded RGBA stays within 256 MiB. One
snapshot kind per request keeps worst-case escaped JSON below the existing
1 MiB metadata packet ceiling. Binary/native file references retain an explicit
encoding plus bounded display representation; displayed lossy paths are not
silently used for filesystem actions. Metadata inspection never requests image
or text payloads just to discover offered types.

X11 uses one requestor window/connection in the owned worker and CLIPBOARD only,
never PRIMARY. TARGETS and property reads use finite requested extents; INCR
hints/chunks and cumulative bytes are checked before append, with one absolute
deadline and owner/generation checks. No arboard/GPUI unbounded image getter or
clipboard-manager handover is introduced by import. Windows checks HGLOBAL
extent before copying/locking payload data; format iteration, Unicode text and
CF_HDROP parsing are bounded. Delayed native rendering stays in the owned worker.

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

The existing Paste image action starts a bounded image-specific asynchronous
GPUI offer read. Offer selection/receive/flush stays on the native event thread;
only a captured pipe descriptor and fixed metadata reach the existing background
executor. Pipe reads use <=64 KiB chunks, cumulative encoded limit and absolute
four-second deadline. A one-reader lease, focus/offer generation and UI job token
cancel obsolete work; no old result may open an editor after Quit, another input
or window close. External encoded image data is never kept in GPUI's clipboard
cache after transfer. Legacy synchronous offer paths gain byte/absolute-time
bounds; image import itself never waits on the foreground executor.

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
- Modify `crates/app-core/src/capture.rs`, `native_capture_job.rs`, `lib.rs`,
  `clipboard_windows.rs` and `clipboard.rs` (the latter also exports the actual
  opened-identity plus remaining-decoded-budget loader for the editor consumer).
- Modify `apps/desktop/src/capture_worker.rs`, `capture_worker_protocol.rs`,
  `main.rs` and `cli.rs` for explicit dispatch only. Existing Portal/ImageUrl
  semantics/error 11 are preserved; clipboard failures use private category 12.
- Modify `vendor/gpui-pre/src/platform.rs`, `app.rs`, `interactive.rs` and
  `window.rs`, plus `vendor/gpui-pre-linux/src/linux/platform.rs`,
  `display_connection.rs`, `wayland/clipboard.rs` and `wayland/client.rs`.
- Modify `crates/app-ui/src/lib.rs`, `workspace.rs` and `ui_tests.rs` for existing
  action/source dispatch, cancellation/job generation and private fixtures.

The actual private X11 CLI proof is
`target/verification/clipboard-x11-executable.json` (six groups), and the focused
native GPUI Wayland proof is
`target/verification/clipboard-wayland-native.json`. Both use only generated data
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
