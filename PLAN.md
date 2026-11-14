# Project Plan – Open-Source Capture & Sharing Application

> **Simple for beginners. Powerful for professionals.**

**Status (2026-11-09):** This is a production delivery plan, not a claim that the current application is production-ready. Phase 0 remains open; see [the measured spike evidence](docs/phase-0.md). The performance contract below applies to the entire application and every future feature. Budgets are acceptance targets, not measurements of the current prototype.

## Session handoff — start here (2026-11-12)

This section is the current continuation brief. The numbered sections below contain architecture, release requirements and detailed implementation evidence; their original bootstrap sequence is historical. Development has progressed through functional slices of Phases 1–5 while the cross-platform/performance release gates remain open. Do not restart the spike or treat implemented features as production acceptance.

### User decisions to preserve

- Continue the UI/UX work now; it is the foundation for further features, not a final-release cosmetic task. The current layout is an initial implementation, not an approved final design.
- **Use an independent native workbench.** Keep a compact task sidebar and a separate workspace. Dark is the default; Oldschool remains available as the compact alternative. The product goal is a clear, polished capture experience; platform and resource acceptance remain tracked below.
- Keep both appearances consistent through the main window, editor and text input. Define all UI colors in `crates/app-ui/src/theme.rs`; screenshot/annotation colors remain separate document data.
- All app-owned UI is **English**. User conversation may remain German. Native OS dialog/error language follows the OS.
- All screenshot editing tools must remain available **before final output in the frozen overlay**, as well as in the standalone editor. Never save/copy/upload an unedited source implicitly.
- Raw performance remains mandatory: near-zero idle work, bounded RAM/GPU use and immediate feedback. Read `AGENTS.md` and the performance contract before execution-path changes. Keep blocking work off GPUI's foreground executor, cancel obsolete jobs and preserve the in-memory capture pipeline.
- Prefer complete, practical progress with focused checks. Do not spend sessions repeatedly running broad benchmarks or rebuilding already verified features; do not omit checks needed for an affected behavior.

### Current implementation

- **GPUI Kit rewrite (2026-11-11):** the complete native presentation uses Kit 0.9.1 / GPUI 0.3.12: Sidebar, Button, Input, Checkbox, RadioGroup, PopupMenu, Kbd and bundled Lucide icons. Dark stays the default; Oldschool keeps compact square controls and colorful icons. Every window has a Base Root, with transparent surfaces preserved for overlays. Typed content access preserves native lifecycle jobs and waiting for work on Quit. Library search/menu callbacks retain byte, paging and stale-result bounds. The image canvas and bounded source-label editor keep their domain-specific contracts. See [the rewrite and measured evidence](docs/ui-redesign.md).
- **Windows functional prototype:** tray/resident lifecycle, single-instance forwarding, hotkeys, display/window/region capture, countdown, clipboard, PNG output and save-folder settings. Graphics attach on demand and detach when the last UI consumer closes.
- **Shared editor:** crop, shapes, arrows, lines/endpoints, pen, text/style, numbering, highlight/spotlight, blur, pixelation and opaque redaction; selection/transforms, duplicate/reorder, bounded metadata undo/redo and zoom/pan. Copy, Save and Save & copy are explicit. Original files are preserved when editing a copy.
- **Storage/sharing:** paged SQLite capture history with prefix search, filters, PNG import/relink, explicit preview, bounded selection/removal/export; explicit configurable POST/PUT uploads, progress/cancel, remembered credential-free profiles and upload history. Windows Uploads can save Bearer/API-key/Basic secrets to Credential Manager. Other secret-store integrations remain pending.
- **Current main UI:** a responsive 200/208-logical-pixel sidebar with Recent capture / Library / Displays & windows / Uploads / Settings. Main windows start at 1060×760 with a 680×480 minimum. Content scrolls at compact sizes; library rows are paged, identified by database ID and expose Kit menus. Switching pages releases search/preview state and cancels stale requests. A thumbnail gallery remains a separate feature.
- **Current editor UI:** grouped Kit tool buttons, accessible labels/shortcut tooltips, contextual selection actions, swatches and separate Copy / Save / Save & copy actions. Toolbar scrolling leaves bounded image space. The in-memory canvas and exported image algorithms remain in Rust.

### Continue from here

1. Keep the Kit component/theme/root contracts when adding UI. Use existing history/capture operations and bounded paging; do not introduce eager full-history decoding.
2. Complete platform acceptance: interactive Windows/macOS/Wayland, manual keyboard/IME/accessibility, mixed DPI/multi-monitor/topology, failure/recovery and packaging. CI checks compilation; it does not establish native runtime acceptance.
3. Follow the delivery sequence below and the [functional gap analysis](docs/gap-analysis.md). Arbitrary effect/layer-order agreement, thumbnail gallery and full reference-hardware resource acceptance remain open. Recording, OCR, scrolling capture, pinning and workflows are required for the ShareX functional-scope goal, even when delivered after the everyday screenshot milestone. Plugins and cloud remain separate later product decisions.

### Delivery sequence toward ShareX functional scope (2026-11-12)

The everyday screenshot release is an intermediate milestone. The original MVP
and version headings below describe staged delivery, not the full parity scope.
The gap analysis inventories missing, partial and unverified capabilities; its
priorities do not establish implementation or runtime acceptance.

1. **Dependable desktop baseline:** complete post-Kit Windows capture/editor,
   clipboard, tray/hotkey, DPI/topology, cancellation and recovery acceptance;
   produce a reproducible portable or installable artifact. Track this alongside
   feature work. Linux/macOS obligations and all performance budgets remain.
2. **Everyday capture:** configurable hotkeys, then saved output profiles;
   active-window/monitor and last/exact-region commands; Open/Paste/Drop;
   resize/rotate/flip, JPEG/WebP and naming/folder preferences.
3. **Sharing and library:** native destination/auth setup, named destinations,
   sharing finalized editor pixels, explicit metadata policy and a bounded
   thumbnail gallery. Preserve truthful outcomes and explicit network consent.
4. **Major capabilities:** scrolling capture, OCR/QR, pinning and video/GIF
   recording, each with its own resource budget, cancellation and native checks.
5. **Advanced parity:** richer capture/editor/effect behavior, custom-uploader
   compatibility, providers, workflows/CLI/actions and remaining utility families.
   Verify the capability matrix before claiming ShareX parity.

#### Current feature slice: open PNG, JPEG and WebP directly in the editor

Implemented (2026-11-12): Recent capture and Library offer Open image, also available
with Ctrl+O (Cmd+O on macOS). The native single-file picker opens a copy through
worker-side bounded PNG/JPEG/WebP decoding and the shared standalone editor, without
importing history, changing the last capture, writing a source copy or copying to
the clipboard. Final outputs remain explicit; the original file stays unchanged.

The Library entry reuses the same job/admission path and releases its search and
preview state on entry to the editor workflow. An optimized headless UI fixture
opens a WebP with exact alpha/pixels and verifies unchanged source bytes and no
output files. This adds only a page-local Kit button; no decoding, cache, worker,
timer or persistent state is added to Library rendering. Native picker/accessibility
and platform/resource acceptance remain open.
Cancellation and selection/load errors release admission; Quit discards late picker
results. Existing capture admission covers selection and decoding, preventing
queued pickers/decodes. The existing close-during-load behavior waits for work;
interactive picker/close behavior remains a native acceptance check.

Resource cost: one action/key binding, one button and one operation-scoped picker
future with at most one selected path. Existing workers, image limits and pixel
ownership are reused. No dependency, runtime, thread, cache, timer or idle action
is added. Optimized UI tests cover button/key routing, cancellation, duplicate
admission, missing input, Quit during selection and synthetic image opening with
unchanged source bytes and no output files. Native dialog latency, physical
presentation, CPU/GPU memory and idle budgets were not measured for this entry
point; native keyboard/accessibility acceptance remains open.

Input format is detected from file contents, independent of extension. Direct
opening admits at most 32 MiB of encoded input; a seekable extent-limited file
reader prevents concurrent file growth from bypassing that bound. This is needed
because the installed JPEG decoder retains the complete encoded input before
applying decode limits. The decoded representation and final RGBA size are each
checked against the existing 256 MiB admission limit before pixel decoding.
EXIF orientation is applied on the worker; only RGBA pixels enter the editor, not
source metadata. PNG/WebP alpha is preserved. Animated inputs use the decoder's
default still frame; animation editing, color-managed import and metadata retention
are outside this slice. Existing last-capture/history PNG loaders keep their
previous limits and behavior. No dependencies or idle infrastructure are added.

Optimized fixtures cover all three formats, misleading extensions, JPEG EXIF
rotation, exact lossless pixels/alpha, original-file preservation, malformed and
unsupported data, oversized dimensions and encoded files, bounded seeks and file
growth. The UI fixture opens a JPEG with a misleading PNG extension without output.
A five-sample warm synthetic 4K PNG development comparison observed median decode
times of 60.30 ms for the existing PNG loader and 60.39 ms for direct editor input.
This short worker-only observation is inconclusive for performance differences;
it excludes presentation, dialog time and process/GPU memory and establishes no
hardware acceptance. Open With and exact-region entry remain pending;
Windows clipboard image paste is implemented in the slice below.

#### Current feature slice: paste a Windows clipboard image into the editor

Implemented (2026-11-12): Recent capture offers Paste image (Ctrl+Shift+V) on
Windows. It opens the shared standalone editor with explicit final outputs and
leaves the clipboard, last capture, history and files unchanged. The dedicated
shortcut avoids replacing normal text paste. Other platforms do not advertise
image paste yet; their clipboard/session integration remains pending.

The existing single capture admission and worker prepare the image/render. Only
this explicit action opens/reads the native clipboard; no watcher, retry timer,
eager initialization, dependency or thread is added. A busy/missing/invalid image
reports failure, and Quit suppresses a late editor result. PNG is preferred, with
24/32-bit RGB/bitfield DIB/DIBV5 fallback. Indexed/compressed DIBs and DIBs with
embedded profiles are rejected with guidance to use PNG. Dimensions and decoder
allocations retain the 256 MiB image limit. PNG payloads are capped at 32 MiB;
DIB payloads at 256 MiB plus 64 KiB of header allowance, checked before copying.

Native clipboard access is held only through the bounded payload copy; decoding
and render preparation happen after it closes. The DIB path temporarily creates
one additional bounded BMP byte vector with an explicit pixel offset to avoid a
V4/V5 offset defect in the pinned decoder. That wrapper and the clipboard payload
are released before render preparation; they count toward active-image memory.
PNG/DIB decoding reuses the installed image library and shared size/orientation
validation. There is no disk/PNG round trip for DIB pixels or clipboard read-back
after output. Native delayed-rendering stalls remain an OS integration acceptance
case; no hard clipboard-access deadline is claimed.

Optimized synthetic tests verify PNG alpha, 24-bit padded rows, V5 bitfields/alpha,
bottom-up and top-down rows, invalid/truncated/oversized input and byte-admission
boundaries. UI checks verify that the paste shortcut cannot bypass active image
admission; the existing editor/output-free fixtures remain in use. No test in
this slice reads or replaces the user's real clipboard. Five warm synthetic 4K
solid-color samples observed median decode of 4.18 ms for PNG and 19.19 ms for DIB,
including the DIB wrapper copy, excluding native clipboard access/presentation.
This is a development observation, not a latency distribution or resource gate.
Real cross-application paste, clipboard contention/delayed rendering, keyboard and
accessibility interaction, process/GPU memory and other-platform acceptance remain
open. Open With is still separate work.

#### Current feature slice: drop one image into the control window

Implemented (2026-11-12): the control window accepts one external file drop and
opens PNG/JPEG/WebP pixels in the shared standalone editor. A visible hint in
Recent capture explains the action. All control pages route the accepted drop to
Recent capture; the existing editor is never replaced. Multiple paths are rejected
with an explanation, and active capture/edit/output/settings/teardown work prevents
drop admission. Missing files, directories and invalid formats report load failure.

The foreground callbacks inspect the payload type/count and clone only the single
selected path. No filesystem access, format sniffing or image decoding occurs
during drag movement or in the drop callback. Worker loading, the 32 MiB encoded
and 256 MiB decoded limits, EXIF orientation, explicit outputs and Quit handling
reuse direct Open image. There is no new dependency, thread, timer, runtime, image
buffer, cache or queue; resource cost is a weak controller reference in the drop
predicate and one transient path at dispatch. GPUI owns the native drag payload
for the duration of the drag; the application does not retain the full path list.

An optimized headless UI test dispatches Entered/Pending/Submit/Ended file-drop
events, verifies busy and multi-file rejection, failure for missing files and
directories, exact WebP pixels/alpha in the editor and unchanged original bytes
with no output files. Existing Open/Paste and Kit UI tests remain in use. The
decode path is unchanged; no new latency/process/GPU memory measurement is claimed.
Real Explorer/Finder/Linux file-manager drag negotiation, keyboard/accessibility,
mixed DPI and reference-hardware input/resource acceptance remain open. Raw bitmap
or URL drops, multiple editors and Explorer Open With are outside this slice.

#### Current feature slice: rotate and flip before annotations

Implemented (2026-11-12): both the frozen overlay editor and standalone editor
offer Rotate 90 degrees clockwise, Flip horizontal and Flip vertical before
cropping or adding marks. The UI explains the restriction; Undo can remove marks
to make these actions available again. Transforming existing annotations/text is
not implemented. All drawing/effect/crop tools remain available after transformation.

Eight lossless orientations are represented by two small metadata fields in the
existing bounded undo history. Undo/Redo/Reset compute a relative rotation/mirror
from the current pixels; no original-image copy or full-image undo snapshots are
retained. One operation at a time transforms pixels and prepares the render on the
existing worker. A temporary loading view blocks editing/output until both agree,
with Cancel still available. Transformation checks cancellation at each row;
closed-window/canceled callbacks cannot replace an editor image. Effect previews
are released before dispatch, and history changes wait for active effect/text jobs.

The current source, current render, new source and new render are the bounded
transition representations (at most four full-size RGBA buffers before renderer
internals); old pixels/render are released on commit. There is no new dependency,
thread, runtime, cache, queue or idle timer. The existing 256 MiB source admission
and active-image budgets remain unchanged. Only original display geometry is kept
as metadata: transformed pixels cannot seed an incorrect repeat-region rectangle;
returning to original orientation restores that geometry. Export uses the same
transformed source and the existing crop/redaction pipeline, with explicit output.

Optimized tests cover all eight orientations and every relative transition on
rectangular, skinny and one-pixel images, exact RGBA/alpha, cancellation, metadata
Undo/Redo/Reset, UI buttons/history, and redaction/export after transforming.
All regular app-ui tests pass. Five warm synthetic 4K clockwise-rotation samples
observed median 11.52 ms for the installed imageops reference and 10.56 ms for the
row-cancellable worker helper. This small development observation is inconclusive
for performance differences and excludes render preparation, native presentation,
process/GPU memory and reference-hardware acceptance. Native overlay/standalone
interaction, mixed DPI, accessibility, cancellation soak and memory gates remain
open. Resize/resampling and transforms of already annotated documents remain pending.

#### Current feature slice: exact numeric capture regions

Implemented (2026-11-12): Displays & windows offers four bounded Kit inputs for
X, Y, Width and Height, and an Exact region button per display where the backend
advertises region capture. Values are whole physical pixels relative to that
display's top-left, independent of negative desktop origins and logical DPI.
Validation rejects empty/non-ASCII/noninteger/overflowing values, zero dimensions,
out-of-display rectangles and the existing image admission limit before dispatch.

The request snapshots display identity and geometry without retaining pixels.
The existing repeat-region worker resolves the monitor and checks position,
resolution, scale and rotation before capture and rechecks geometry before
publication. ExactRegion is a separate request variant so a failed numeric capture
does not invalidate an unrelated previously remembered area. Successful capture
updates the session's last region; Windows can repeat it through the existing
command. Output profiles, in-memory pixels, cancellation and truthful output status
reuse the existing single-job pipeline. Named presets are implemented in the
follow-up below; changing pages or closing the UI releases the four inputs and status.

Resource cost: four page-local inputs holding at most ten ASCII digits each and
one bounded ID/geometry/rectangle request. There is no new thread, dependency,
runtime, image buffer, cache, timer, polling or queue. Validation/callbacks inspect
only bounded metadata; enumeration and capture remain on the existing worker.
The capture/geometry algorithm is shared with Repeat last region; no new native
latency or process/GPU memory measurements are claimed for this entry point.

Optimized core/UI tests cover negative display origins, scale/rotation/topology
rejection, coordinate overflow and rectangle bounds, strict parsing, accessible
input labels, invalid capture without output and field release/stale callbacks
after page exit. The explicit external-window fixture now also compares exact
2x2 capture against direct/repeated capture; it was not run for this slice.
Interactive capture and output profiles from these fields, mixed DPI/hotplug,
keyboard/IME/accessibility and full native/resource acceptance remain open.

#### Current feature slice: saved named region templates

Implemented (2026-11-12): Displays & windows offers a bounded Preset name field
and Save / replace preset per display. Up to 16 named rectangles persist in the
existing settings file. Names are trimmed, case-sensitive, unique and at most
64 UTF-8 bytes without control characters; same-name saves replace the rectangle.
Use restores the four inputs and name without capturing; the user explicitly
chooses the destination display. Remove deletes only the template. No monitor ID,
display name, frame, texture or credential is stored in a preset.

Saving first validates the rectangle against the selected display. Later capture
validates it against the newly selected display and snapshots/rechecks current
geometry through Exact region. Settings parsing validates list length, names,
rectangle bounds/overflow and the existing image budget under the existing 64 KiB
config-file limit. Older files default to an empty list. Folder/theme/hotkey/output
profile saves preserve presets, and preset saves preserve those settings.
Single-admission worker-side atomic persistence commits the active list only after
success; failed writes retain the previous list. Value/page guards reject stale
Use/Remove callbacks, and Quit follows the existing settings-job lifecycle.

Resource cost: at most 16 small name/rectangle records in resident settings, one
page-local 64-byte name input and at most 16 bounded template rows. The name input
is released with the coordinate fields on page exit/UI close. No dependencies,
threads, runtimes, timers, watchers, images or new queues are added. Explicit saves
reuse the existing filesystem worker; no native capture or image path changes.
No new process/GPU/latency measurements are claimed for this metadata-only slice.

Optimized settings/UI fixtures cover persistence/reload, legacy defaults, list/name/
rectangle limits, malformed stored input, failed-save file/list preservation,
explicit no-capture Use, stale callbacks and input release. Native keyboard/IME/
accessibility, packaged restart workflow, disk-full/recovery and full platform/
resource gates remain open. Automatic monitor binding and per-preset hotkeys or
output chains remain outside this slice.

#### Current feature slice: JPEG/WebP export copies through Save as

Implemented (2026-11-12): Recent capture's Save as accepts .png, .jpg/.jpeg and
.webp case-insensitively; a missing extension defaults to PNG, and unsupported
extensions fail rather than being silently renamed. JPEG defaults to quality 90
and rejects any nonopaque alpha with PNG/WebP guidance. WebP is lossless and keeps
RGBA, including alpha. New encoders receive only finalized pixels, not source
metadata. Existing PNG byte-copy behavior remains unchanged. Direct editor Save
and quick-capture outputs still use PNG; the default Save as preference is described below.

JPEG/WebP are explicit export copies: their paths never enter the PNG-only library
or replace its current source. Existing in-memory/source-path ownership remains
unchanged, preserving exact-pixel Copy/Edit and preventing a lossy re-import into
the capture pipeline. The status identifies the actual format/path and explains
the copy behavior. PNG Save as retains its existing source/history transition.
An active editor/overlay blocks Save as so it cannot export an earlier unedited
source while pending edits exist. No export copies or reads the clipboard.

Encoding/transcoding stays on the existing single-admission worker. In-memory
sources encode directly; saved PNGs decode only when conversion is required, while
PNG-to-PNG retains bounded streaming. All formats share durable temporary-file
publication without replacement and cleanup on failure; cancellation of the dialog
preserves the source, and Quit follows the existing active-save lifecycle. There
is no new dependency, thread, timer, queue, runtime, cache or retained frame. New
formats use a 64 KiB output writer; JPEG reads RGBA by tiles without a full RGB
conversion image, and WebP uses the installed encoder's active scratch/encoded
buffers. Those allocations count toward existing active-image budgets; encoder
peak/process/GPU memory and full platform/resource acceptance remain unmeasured.

Optimized fixtures verify signatures, dimensions, lossless pixels/alpha, saved-PNG
conversion, original/existing-file preservation, unsupported formats, transparent
JPEG rejection, encoder failure and temporary cleanup. A headless save-dialog
fixture verifies JPEG/WebP source/library/clipboard preservation, unsupported suffix,
conflict and cancel outcomes. Three warm synthetic 4K encode plus durable-publication
samples observed medians of 28.42 ms PNG, 85.40 ms JPEG and 14.55 ms WebP. Different
formats/quality/output sizes and the small structured fixture prevent a general
speed/compression claim; these are worker development observations, excluding
dialog, physical input/presentation and process memory. Native picker interaction,
disk-full/recovery and reference-hardware/resource gates remain open.

#### Current feature slice: saved JPEG export quality

Implemented (2026-11-12): Settings offers JPEG export quality 1–100 with an explicit
Save quality action and separate active-value label. Legacy settings default to
90; malformed/out-of-range stored values are rejected. Folder/theme/hotkey/output-
profile/region-template saves preserve quality, and quality saves preserve those
settings. Worker-side persistence commits the active value only after success;
validation/write failure retains it. Save as snapshots quality before awaiting its
dialog for both retained pixels and saved-PNG conversion. Encoder boundaries reject
invalid quality before publication and before PNG decoding for conversion.

PNG/WebP, transparency handling, source/library preservation and no-overwrite
publication remain unchanged. Changing this preference starts no image job.
Resource cost is one resident scalar, one scalar per export job and a three-digit
Kit input owned only by Settings and released on page exit/UI close. No dependency,
runtime, thread, timer, watcher, image, cache or queue is added. All active-image
budgets and encoder/platform resource gates remain unchanged.

Optimized settings tests cover defaults, bounds, reload, invalid configuration,
failed-save preservation and other preference retention. Encoder tests verify
quality effects and matching in-memory/saved-PNG exports. UI tests cover validation,
failed writes, active-value preservation, scrolling, field release and passing a
nondefault quality to Save as. Three warm synthetic 4K JPEG encode/durable-save
samples observed median 77.25 ms at quality 50, 80.46 ms at 90 and 101.89 ms at 100.
These content/quality-specific development observations exclude dialog, physical
presentation and process/GPU memory; they establish no general latency distribution
or reference-hardware acceptance. Native Settings/picker/keyboard/IME/accessibility,
packaged restart and resource gates remain open. Transparency flattening and lossy
WebP remain outside this slice; the Save as default is implemented below.

#### Current feature slice: saved default Save as format

Implemented (2026-11-12): Settings offers PNG/JPEG/WebP under Default Save as
format. The choice is persisted on the existing worker and committed only after
success; failures retain the previous active selection. Older settings default to
PNG, unknown stored formats are rejected, and all folder/theme/hotkey/output/
template/JPEG-quality saves preserve this preference and vice versa.

Save as snapshots the choice before opening its dialog, suggests Screenshot.png,
Screenshot.jpg or Screenshot.webp, and supplies that suffix if the chosen name has
none. An explicit supported extension wins; unsupported extensions still fail.
The in-memory/saved-PNG export and existing JPEG quality/transparency policies are
reused. Direct editor saves and quick captures continue to use PNG, with the PNG
library and JPEG/WebP copy semantics preserved. Choosing a default does not start
an image operation or silently convert a capture.

Resource cost: one resident enum, one per pending export and three page-local
radio choices. No dependency, runtime, thread, timer, watcher, image, cache or queue
is added. Persistence uses existing single-admission settings jobs; filename work
is bounded metadata. Encoder/native capture paths are unchanged, so no new image
latency/process/GPU measurement is claimed for this preference.

Optimized settings/UI tests cover all formats, legacy defaults, unknown values,
preference preservation, failed-save rollback, explicit suffix override and a
suffix-free WebP export that keeps the value snapshotted before the dialog even
if the model changes while it is pending. The headless picker does not expose
suggested filenames for inspection; actual native suggestions, keyboard/IME/
accessibility, packaged restart and full resource gates remain open.

#### Current feature slice: automatic PNG filename templates

Implemented (2026-11-12): Settings offers Capture filename template, up to 96
UTF-8 bytes, with an explicit Save action. Empty retains the legacy naming path.
Allowed literals are letters/digits/spaces/hyphen/underscore; {date} and {time}
expand to UTC save date/time. Paths, dots/extensions, control characters and other
tokens are rejected. A UUID suffix plus .png is always added; expanded names are
bounded to 240 bytes and one path component, including Windows reserved-name cases
made safe by the suffix. No source title, monitor ID or document text is inserted.

The template is snapshotted for explicit automatic PNG output, including display,
window, region and editor/crop saves. Naming occurs on the existing save worker
after any clipboard stage; Copy-only/preview/cancel does not generate a name or
file. Manual Save as names remain explicit. Save/history outcomes use the actual
published path, and publication still refuses replacement. In particular, last_saved
now comes from the worker result rather than an earlier provisional source path.
The source pixels and capture/render pipeline are unchanged.

The existing bounded settings file persists the validated template; older files
default to empty. Other preference saves preserve it and template saves preserve
them. Validation/write failure retains the active value, and the page-local input
is released on exit/UI close. Resource cost is one at-most-96-byte resident string,
one save-job snapshot and bounded temporary filename/date strings. There is no
new image buffer, dependency, thread, runtime, queue, watcher or timer. Empty/copy-
only paths avoid optional naming work; all existing resource budgets remain.

Optimized fixtures cover deterministic UTC expansion, bounds, traversal/extension/
token rejection, Unicode literals, unique names, legacy defaults, persistence and
failed-write preservation. A headless synthetic two-image save verifies exact
pixels, unchanged clipboard state, distinct files, the actual last path and matching
SQLite history paths. A 1,000-call optimized metadata probe observed p95 100 ns with
the template disabled and 1 microsecond enabled, timer-resolution limited; this
excludes filesystem/image/native input/presentation and establishes no end-to-end
or hardware acceptance. Native Settings/IME/accessibility, filesystem/path/recovery
and full process/GPU/latency/soak gates remain open. Rich naming macros, directory
templates and renaming existing captures are outside this slice.

#### Current feature slice: smaller Save as export copies

Implemented (2026-11-12): Recent capture's Save as size offers Original (default),
50% and 25%. The session-only choice is snapshotted before the native save dialog.
Smaller outputs round each dimension up to whole pixels, at least one pixel, with
no upscaling. All smaller outputs are copies, including PNG: current source, PNG
library and clipboard remain unchanged. Original PNG retains its usual save/
history transition and saved-PNG streaming path. Formats/default suffix/JPEG quality,
transparency rejection and no-overwrite publication reuse the existing export path.

The worker averages non-overlapping 2x2 or 4x4 source blocks, with smaller edge
blocks. RGB is weighted by alpha before averaging/unpremultiplication, and pixels
whose rounded alpha is zero have zero RGB. JPEG checks original source alpha
before shrinking, so near-opaque input cannot bypass its transparency restriction.
Resampling uses stored color values, without color-managed/gamma-correct filtering.
Finalized/redacted pixels are the input; no unedited source is fetched. Shared PNG
encoding remains Fast/Adaptive and now serves direct and resized output alike.

Resource cost: one session percentage, three page-local radio choices, one job
snapshot and one tightly packed RGBA output E, plus the existing encoder scratch.
The kernel uses fixed stack accumulators (at most 16 source samples per output),
no full-size float intermediate, extra source copy, GPU texture, undo snapshot,
dependency, runtime, thread, cache or queue. Source/output admission retains the
256 MiB image limit, and outputs can only shrink. Existing single-save admission,
dialog cancellation and confirmed-save/Quit lifecycle apply; no background resize
starts when selecting a percentage. Active-image/platform resource budgets remain.

Optimized core/UI fixtures cover dimensions/edge rounding/skinny images, exact box
values, alpha weighting and transparent RGB, invalid sizes, both in-memory/saved-PNG
sources, all formats, source/target preservation and JPEG source-alpha rejection.
A headless UI fixture verifies size snapshotting, redaction preservation and no
PNG history/source/clipboard change for a smaller copy. Three warm structured 4K
PNG samples observed median resize/encode/durable-save times of 27.50 ms Original,
23.31 ms at 50% and 13.07 ms at 25%. Different output dimensions and the small
fixture prevent general speed claims; these are observations, not native
presentation/process/GPU/reference-hardware
acceptance. Native UI/picker/accessibility, slow/full storage, interruption/recovery,
peak memory and full platform/resource gates remain open. Independent width/height,
upscaling, canvas resize and higher-quality filter choices remain separate work.

Follow-up (2026-11-12): Custom width accepts positive physical-pixel widths no
larger than the source; height is the aspect-preserving ratio rounded up. This
session-only choice uses one lazy, ten-digit input and the same pre-dialog
snapshot/copy semantics. Live-image bounds are checked before the dialog; saved
PNG bounds are checked on the worker. Equal dimensions encode without an extra
resampling allocation. Selecting or editing the width starts no image work.

Custom widths partition the entire source into non-overlapping integer boxes.
Alpha-weighted sums use fixed u64 accumulators for large reductions, with bounded
u32 division for blocks of at most 65,535 samples. The fixed percentage geometry
is unchanged. Memory remains one output RGBA image plus existing encoder scratch,
under the same 256 MiB admission limit; no new thread, cache, timer or dependency.
Optimized fixtures cover uneven ratios, full coverage, very large reduction sums,
alpha, invalid/upscale widths, all formats, input lifetime and modal snapshotting.
Three warm synthetic 4K samples measured median resize/encode/durable PNG save at
27.75 ms Original, 25.87 ms Half, 13.65 ms Quarter and 12.49 ms Width(640).
These small development observations exclude dialog, presentation and process/GPU
memory; the native interaction, recovery and resource acceptance gates above remain open.

Follow-up (2026-11-12): Custom height adds the symmetric physical-pixel choice;
width follows the aspect ratio rounded up, with no upscaling. Width and height
share one lazy ten-digit input, reset when switching choices and released on page
exit/UI close. Both snapshot their value before the dialog and export copies from
finalized pixels, preserving source/library/clipboard. Saved-PNG dimensions remain
worker-validated. No new buffer, worker, runtime, cache, timer or dependency is
introduced; the existing output allocation, encoder scratch and admission apply.
Optimized core/UI fixtures cover invalid/upscale heights, uneven ratios, skinny
images, transposed width/height pixel and alpha agreement, all export formats,
live/saved sources, choice switching, snapshotting and input release. Three warm
synthetic 4K PNG samples observed median resize/encode/durable publication of
12.14 ms for Width(640) and 12.25 ms for Height(360), both producing 640x360.
This short comparison is inconclusive for performance differences and excludes
native dialog/presentation and process/GPU/peak memory; resource acceptance remains
open. Independent width/height is implemented below; upscaling and canvas resize
remain separate work.

Follow-up (2026-11-12): Exact dimensions allows independent positive physical-pixel
width and height, each no larger than the source. The UI explicitly warns that
this choice can change the aspect ratio. It exports finalized pixels as a copy
through the existing worker and no-overwrite publication; source, clipboard and
PNG library remain unchanged. Both values are snapshotted before the save dialog;
live dimensions are checked before it, saved-PNG dimensions on the worker.
The existing integer-box alpha-weighted kernel handles both axes independently.
Resource cost is one extra lazy ten-digit input only while Exact dimensions is
visible, released on choice/page exit or UI close, and two scalar job values.
There is no new dependency, worker, runtime, queue, cache, timer or image scratch;
the existing bounded RGBA output and encoder allocations apply. Optimized core/UI
fixtures cover axis bounds, allocation admission, uneven ratios, exact pixels and
alpha, live/saved sources in all formats, modal snapshots and input release.
Three synthetic optimized 4K PNG samples observed median resize/encode/durable
publication of 16.15 ms Width(640), 15.91 ms Height(360) and 15.70 ms Exact(640,360).
All produce the same output dimensions and encoded length. Concurrent desktop
compilation and the short sample make performance differences inconclusive;
these development observations exclude native presentation and process/GPU memory.
Native interaction/accessibility, recovery and process/GPU memory acceptance
remain open. Upscaling and canvas resize remain outside this slice.

#### Current feature slice: copy the saved capture's file path

Recent capture offers Copy file path only for its confirmed last_saved path.
Unsaved pixel-only captures cannot copy a provisional filename. JPEG/WebP and
resized export copies retain their existing source semantics: this action refers
to the saved current capture, not the last exported copy. The explicit action
replaces clipboard content with plain path text, without quoting/escaping, file
transfer data, image decoding, filesystem checks or clipboard verification.
Paths that cannot be represented losslessly as Unicode text report failure.

The existing lazy native clipboard instance performs set_text on the existing
worker with shared single-copy admission. Success is reported only after the
native call succeeds; native/lock failures release admission and report failure.
Quit waits for admitted work. Resource cost is one transient path string and
one operation-scoped future; no new runtime, thread, timer, queue, cache or image
buffer is introduced. Clipboard ownership follows the existing native instance.
Optimized headless checks cover disabled unsaved state, busy/preview admission,
duplicate commands, worker failure and source preservation without touching the
user's clipboard. A separate ignored native Unicode/space path round-trip fixture
is available for explicit clipboard acceptance. Native cross-application paste,
contention, ownership, latency and process-memory/resource acceptance remain open;
no new performance measurement is claimed for this metadata-only action.

Library follow-up (2026-11-12): each row's More menu offers Copy file path for
that entry's recorded path, including paths whose files are currently missing.
The shared path-copy worker replaces clipboard content with text only, with the
same Unicode/error/admission/Quit behavior. Menu dispatch resolves the entry ID
against the current bounded page and rejects stale revisions, missing IDs and
loading/mutating pages. Completion updates Library status only while its original
page revision is still visible; it cannot replace a newer page's status.
No filesystem access, decoding, image reference, queue, dependency or idle work is
added. Resource cost is one existing menu item and one transient path/revision
snapshot. Optimized headless checks cover menu routing, stale/missing entry
rejection, native-state failure, source preservation and stale completion status,
without touching the real clipboard. Native clipboard/resource acceptance remains
open; no additional performance measurement is claimed for this shared metadata path.

#### Current feature slice: Windows file clipboard output

Recent capture offers Copy file for its confirmed saved capture; Library More
offers it for the selected row. This explicit action replaces clipboard content
with a single native file reference through the installed arboard file_list API
([Windows CF_HDROP](https://learn.microsoft.com/en-us/windows/win32/shell/clipboard)).
It does not read/encode image pixels or copy file contents into application memory.
The existing shared worker/admission/clipboard instance and history-revision checks
serve path text and file references. Source files and current capture/library state
are unchanged by publication. Export copies retain their existing separate semantics.

Worker-side validation requires an absolute path to an existing regular file
before opening the native clipboard. Missing files/directories/relative paths fail;
a deletion/replacement after validation can still make a later paste fail. The
file must remain available until the receiving application consumes it. Native
publication failures are reported; a native failure after clearing may have changed
clipboard content, so failure does not promise clipboard preservation. Success
means the file reference was published, not that another application accepted it.

Resource cost is one path and a single native file-list payload with path encoding
limited by OS path bounds, with no file-content buffer, image, new dependency, runtime, worker,
queue, watcher or timer. The native arboard instance remains lazy and shared.
Optimized synthetic core/UI tests reject invalid/missing sources before native
clipboard creation, cover recent/menu routing, stale commands and failure status,
without touching the real clipboard. An ignored native Unicode/space file-list
round-trip fixture is available separately. Explorer/attachment paste, clipboard
contention/ownership, native latency, peak memory and other-platform acceptance
remain open; no native performance result is claimed for this metadata-only path.

#### Current feature slice: native manual upload destination setup

Uploads offers Set up destination without a JSON file for credential-free POST
multipart or PUT PNG-body endpoints. The bounded form accepts an endpoint URL,
POST file field and either a plain-text URL response or a JSON link pointer.
HTTPS is required unless the user explicitly checks Allow unencrypted HTTP.
The shared destination validator rejects embedded credentials, query/fragment
tokens, invalid fields/pointers and oversized values. Imported profiles retain
advanced authentication/response configuration. Native Windows credential entry
and destination connection testing are implemented below.

Use destination validates on the existing worker and applies to this session only;
the existing Remember action persists it explicitly. Invalid input keeps the
previous destination/link. Closing the form/page/window suppresses its pending
result, and Quit discards it. No connection, upload, runtime initialization, image
decode or file write occurs during setup. Final uploads remain separate explicit
actions using the existing bounded streaming/auth/redirect/cancellation policy.

Resource cost is three lazy Kit inputs limited to 2048/64/256 UTF-8 bytes plus
existing bounded input undo, three small choices and one admitted metadata future.
Inputs release on cancel, successful use, page exit or UI close; there is no new
dependency, worker, runtime, queue, image, cache, watcher or idle timer. Constructor
and headless UI fixtures verify URL/HTTP/security rules, response choices, POST/PUT,
profile serialization, failed validation preservation, duplicate admission, modal
button routing and stale-result suppression without networking or user settings
writes. Four optimized synthetic loopback upload fixtures also pass through the
manual constructor, checking POST/PUT bytes, response parsing, redirect blocking
and cancellation without external connections. The Windows test server explicitly
restores blocking mode on accepted sockets before bounded reads.
Native keyboard/IME/accessibility, real hosting-service interoperability
and latency/process/GPU/resource acceptance remain open; no new performance
measurement is claimed for this bounded metadata entry point.

#### Current feature slice: native Windows upload credential entry

Uploads offers Save credential on Windows after a destination is present. The
bounded form accepts a credential reference, Bearer/API-key/Basic, an optional
Basic username and a masked secret. Save validates on the existing worker, writes
the secret to Windows Credential Manager for the HTTPS origin/reference target,
and attaches the non-secret reference to the session destination. HTTPS is
required. Invalid input keeps the previous destination. Closing the form, page or
window suppresses a pending result, and Quit discards it. The destination JSON
never contains the secret; Forget destination still leaves the OS credential in
place. No connection, upload, image decode or settings write occurs while saving.

Resource cost is three lazy Kit inputs limited to 64/256/2560 UTF-8 bytes plus
existing bounded input undo, one small auth choice and one admitted metadata
future. The secret input is masked; inputs release on cancel, successful save,
page exit or UI close. There is no new dependency, worker, runtime, queue, image,
cache, watcher or idle timer. Constructor and headless UI fixtures verify HTTP
rejection, empty secrets, duplicate admission, stale-result suppression and that
status text never echoes the secret, without writing OS credentials or networking.
An ignored native round-trip writes one uniquely named synthetic credential,
reads it back and deletes it. Connection testing and credential removal are
implemented below. Named destinations and non-Windows secret stores remain
separate work.
Native keyboard/IME/accessibility, real Credential Manager UX and
latency/process/GPU/resource acceptance remain open; no new performance
measurement is claimed for this bounded secret-entry path.

#### Current feature slice: destination connection test

Uploads offers Test destination after a destination is present. The action
uploads a throwaway 1×1 PNG through the existing transfer, auth, redirect,
deadline and cancellation path, then deletes the local temp file. It does not
record upload history, change the last capture or last-saved image, or start
without an explicit click. It reuses the one-active-upload slot; Cancel test
stops the in-flight request. A successful test shows the returned link for
Copy link and notes that the server may keep the 1×1 image. Failure keeps the
previous link and reports Destination unchanged. Real screenshot uploads still
clear the displayed link when they start.

Resource cost is one unique OS temp directory with a 1×1 PNG, the existing
upload worker/runtime/client, and the existing progress channel. Drop removes
the temp directory on success, failure and cancellation. There is no new
dependency, idle timer, queue or history write. Constructor and headless UI
fixtures verify button presence, busy/missing-destination guards, cancel
labeling and that last capture/history stay unchanged, without networking.
A regular core fixture writes the 1×1 PNG and confirms cleanup when
destination validation fails before connect. An ignored loopback fixture
sends the probe through the existing local server. PUT destinations that
reject an existing target with HTTP 412 still need a unique resource URL.
Native keyboard/IME/accessibility, real hosting-service interoperability and
latency/process/GPU/resource acceptance remain open; no new performance
measurement is claimed for this explicit probe.

#### Current feature slice: Windows upload credential removal

Uploads offers Remove credential on Windows after the session destination has
an attached HTTPS credential reference. The action deletes only the
Screenshots/upload/https://… Generic Credential for that origin/reference,
then returns a credential-free session destination. Save credential remains
the rotation path for the same reference. Forget destination still leaves the
OS secret in place. Removal is session-only; Remember persists the anonymous
profile. A missing OS entry is treated as success so the session reference
cannot keep pointing at a deleted secret. Invalid or unrelated targets are
rejected before any CredDeleteW call. Closing the page discards a pending
status; Quit discards it. No upload, image decode or settings write occurs
while removing.

Resource cost is one admitted metadata future on the existing worker and the
existing destination clone. There is no new dependency, runtime, queue, image,
cache, watcher or idle timer. Constructor and headless UI fixtures verify
button presence only with an attached reference, busy/missing-reference guards
and that last capture stays unchanged, without deleting OS credentials.
A regular core fixture rejects destinations without a reference and unrelated
or HTTP targets. An ignored native round-trip writes one uniquely named
synthetic credential, deletes it through detach_credential, confirms lookup
fails and leaves no secret in JSON.
Named destinations and non-Windows secret stores remain separate work.
Native keyboard/IME/accessibility, real Credential Manager UX and
latency/process/GPU/resource acceptance remain open; no new performance
measurement is claimed for this bounded deletion path.

#### Current feature slice: configurable Windows global hotkeys

Implemented in the working tree (2026-11-12): three editable/disableable bindings
under Settings, defaults restoration, bounded validation/persistence and staged
native registration. Existing keys remain active until settings save succeeds;
conflict/save failure rolls back reservations. Native cleanup errors report actual
remaining bindings and block further changes until restart. Folder/theme saves
preserve bindings. No dependency, thread, cache, polling or eager UI is added;
retained configuration is three strings of at most 64 bytes. Settings alone owns
three Kit inputs with bounded text/undo state; page exit/window close releases them.
Checks and remaining acceptance limits are recorded in [hotkey verification](docs/hotkeys.md).

Start with the three existing commands: primary display, quick region and
capture-and-annotate. Add native Kit settings to assign, disable and restore their
bindings, preserving current defaults for existing installations. Keep platform
limitations explicit; this slice does not establish non-Windows hotkey support.

- Validate supported key combinations and duplicate assignments before applying.
  Report OS registration conflicts per command and retain usable bindings after
  a failed change; never report an inactive shortcut as active.
- Persist bounded settings on the existing worker path. Folder/theme changes must
  preserve bindings; failed persistence must leave an explained, recoverable state.
  Perform registration/unregistration on the native event-loop thread. Settings
  loading must not create an idle watcher, polling timer or eager graphics setup.
- Keep UI labels, status, event routing and shortcut diagnostics consistent with
  the applied bindings. Update fixtures that assume the hard-coded shortcuts.
- Acceptance: change a binding, capture while another app has focus, disable and
  restore it, restart and confirm persistence; exercise duplicate/OS conflicts,
  save failure and shutdown cleanup. Add focused validation/routing/persistence
  tests and optimized native checks; compare resident readiness and idle resources
  for the changed startup path. Record skipped checks separately from results.

#### Current feature slice: saved quick-capture output profile

Implemented in the working tree (2026-11-12): Settings → Capture output offers
Copy only, Save only, Save and copy (legacy default) and Annotate first. The saved
choice applies to quick/delayed region, display and window captures, including
tray/hotkey commands. New screenshot still opens the frozen editor, whose outputs
remain explicit. Region sessions snapshot their choice; ordinary jobs snapshot it
before worker dispatch. Settings changes commit only after persistence succeeds,
and folder/theme/hotkey saves preserve the profile. No profile starts an upload.

Copy only retains one bounded last image in memory without PNG/history output and
clears stale saved-path state. Save only preserves the clipboard. Annotate first
uses the existing editor after capture/region selection, with no source output on
cancel. Copy only/Annotate first reject multi-image capture before native work;
All displays is disabled for these profiles when multiple monitors are present.
Save-capable profiles retain separate per-display files and the existing last-image
clipboard semantics. Per-hotkey task profiles and arbitrary chains remain future
work; this is one saved global choice, not a workflow engine.

Resource cost: one profile enum per controller/region surface and a bounded
single-command deferred graphics request for resident annotate-first. Existing
workers, in-memory pixels, admission limits and output implementations are reused;
no dependency, runtime, thread, cache, polling or resident graphics is added.
Copy-only pixels are a live image consumer, separate from the empty resident base.

Optimized settings and Kit UI checks plus the native synthetic output/editor
fixture pass. Checks cover profile persistence/legacy defaults, failed saves,
single-image admission, snapshot routing, independent clipboard/files/history,
editor cancellation and preservation of the previous capture. Raw verification
data remains local under ignored target directories. Physical hotkey capture,
mixed DPI/accessibility, reference-hardware latency and full soak acceptance remain
open; successful fixtures do not establish those gates.

A short optimized resident comparison against the preceding release build was
completed without capture/clipboard actions. Its samples and machine details stay
only under `target/output-profile-verification/`, excluded from version control.
It is a development regression check, not the five-minute/reference-hardware gate.

#### Current feature slice: Windows active-display capture

The Capture sidebar and Windows tray offer Active display with the saved countdown
delay. After the user focuses an external application, the countdown snapshots
its foreground HWND and resolves the monitor before hiding UI/attaching graphics.
[MonitorFromWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-monitorfromwindow)
chooses the display with the largest window intersection; MONITOR_DEFAULTTONULL
rejects off-display windows rather than selecting an unrelated primary/nearest
display. Shared validation rejects missing, hidden/minimized, desktop/taskbar and
own application windows. The XCap monitor ID is derived using the pinned Windows
backend's HMONITOR representation, without monitor enumeration on the foreground.

The existing Display request resolves/captures on the worker using existing image
admission, output profiles, cancellation and publication. The request refers to
the display chosen at countdown expiry, even if the window moves afterward.
Native topology/handle reuse races retain the existing display-capture acceptance
limits; mixed DPI, spanning windows, monitor disconnect/replacement and real
foreground-focus/presentation acceptance remain open. The active-display hotkey
is implemented in the follow-up below; non-Windows integration remains open.

Resource cost is one menu item/command and a transient monitor-ID string; there
is no new worker, dependency, runtime, queue, cache, polling or resident timer.
The installed windows-sys dependency enables GDI API bindings for one native
MonitorFromWindow call; the shared short window validation is unchanged. Capture
buffers and worker behavior are reused, so no new capture-latency/process/GPU
measurement is claimed. Optimized tests reject invalid native handles and verify
both active-target countdown cancellations without capture. Separate ignored
native tray-routing and explicit external-window/monitor metadata fixtures remain
available for interactive acceptance; no test here captures the user's desktop.

Hotkey follow-up (2026-11-12): Settings adds a sixth, initially empty Active display
binding. Its command snapshots the foreground application's monitor immediately;
menu/tray retain the saved countdown. Three-/four-/five-binding settings preserve
their exact command order and gain disabled slots; six bindings serialize/reload
and validate with the existing 64-byte, normalization and duplicate rules.
Existing staged registration/persistence/rollback, startup conflict reporting and
native event delivery are reused. Resource cost adds one bounded string, one
Settings-only input and native metadata; concurrent old/new reservations are now
bounded to twelve keys. No worker, runtime, queue, timer, watcher, image allocation
or dependency is added, and all idle/active-image budgets remain unchanged.
Optimized fixtures cover migration, disabled defaults, sixth-slot persistence/
duplicates, event routing and six named UI fields with release on page exit.
Native registration/remapping/startup-conflict checks are extended to Active
display and pass without injecting input, reading pixels or changing the clipboard.
Reference-hardware CPU/memory/latency, native keyboard/focus, topology/DPI and
presentation acceptance remain open; historical hotkey measurements do not
establish a six-command resource result.

#### Current feature slice: Windows active-window capture

Implemented in the working tree (2026-11-12): Active window is a fourth configurable
global capture shortcut, initially disabled. Existing three-element hotkey settings
migrate by appending the disabled slot; other bindings, theme, folder and output
profile remain intact. Restore defaults disables the new slot. Menus no longer
advertise hard-coded shortcut labels after users change their bindings.

The hotkey snapshots foreground HWND/process identity before UI hiding/graphics
attachment. Only short native queries run on the UI thread. Window enumeration,
process revalidation and capture use the existing worker/admission pipeline.
The backend rechecks process identity before grabbing pixels; target validation
rejects own windows, desktop/taskbar surfaces, minimized, stale or changed-process
targets without capturing a fallback desktop.
Source pixels remain in memory and use the selected output profile/editor.

Capture menu and tray expose a configurable (default three-second) active-window
countdown so the user can focus the intended application. It reuses the existing
cancellable delay task; close/Quit cancels it. No foreground tracking hook, polling,
resident timer, thread, runtime, cache or dependency is introduced. Resource cost
is one optional bounded shortcut string, one menu item and a small ID/process
request payload; registration staging now owns at most eight keys. Native window
IDs remain session-scoped; revalidation is not a guarantee against every native
same-process handle-reuse race.

Optimized core/UI, legacy-settings, countdown-cancel, native hotkey/tray and own
window checks pass. An explicit external solid-color fixture verifies capture in
RAM, wrong-process rejection and failure after closing the target. Attempts to
automate foreground focus aborted before capture; full real hotkey/focus and
countdown targeting acceptance remain open alongside mixed DPI/accessibility and
reference-hardware/soak checks. Fixture/machine details stay only under ignored
`target/active-window-verification/`; no raw verification JSON is added to docs.
The optimized release build and a short resident CPU/private-memory comparison
were completed. Raw reports stay in that ignored directory; they do not establish
five-minute idle, physical presentation or reference-hardware acceptance.

#### Current feature slice: repeat the last selected region

Implemented in the working tree (2026-11-12): Windows exposes Repeat last region
in Recent capture, Capture menu, tray and a fifth configurable shortcut, initially
disabled. Three- and four-binding settings preserve existing values and gain the
disabled slot. Before a reusable selection exists, UI/tray entries are disabled.

The controller retains one session-only display ID, physical pixel rectangle and
geometry (origin, dimensions, scale, rotation), never an image/texture. A successful
quick-region crop commits the rectangle before optional editing; canceling that
editor produces no pixels but retains the chosen rectangle. An incomplete/canceled
selection preserves the previous rectangle. Explicit output of a crop from a fresh
full-display editor source also updates it. Imported and derived images cannot
create display-coordinate templates; crop/render outputs clear their source geometry.
Closing the control window preserves the session template; Quit forgets it.

Windows full-display workers read geometry before/after capture and tag the source
only when both readings and returned dimensions agree. Other platforms incur no
new geometry reads for this Windows feature. Repeats resolve the display afresh,
validate geometry/bounds and the existing pixel budget before capture, then recheck
geometry/dimensions before publication. Missing/changed displays invalidate the
template without clipboard/file/history output or fallback to another display.
Same-ID replacement with identical geometry is not distinguishable through the
current session-scoped backend IDs; physical topology/reconnect acceptance remains open.

Resource cost: one bounded ID string (at most 64 bytes) and scalar template,
optional scalar geometry on live source frames, one optional shortcut, one menu
item and two worker geometry readings for full-display sources. There is no new
pixel cache, persisted desktop preview, thread, dependency, timer, polling or undo
snapshot. Shortcut registration staging remains bounded to ten keys. The existing
capture/output/editor pipeline and selected output profile are reused.

Optimized core/UI checks cover legacy settings, geometry/overflow rejection,
absence of retained source pixels, derived-source exclusion and selection cancel.
The explicit native fixture compares direct and repeated 2×2 captures within an
owned opaque test window and rejects a mismatching scale. Native hotkey/tray checks
pass. Raw verification stays under ignored `target/repeat-region-verification/`.
Full physical hotkey, monitor hotplug/mixed-DPI, presentation and reference-machine
soak acceptance remain separate gates; fixture timings are not release acceptance.

### Code and verification entry points

- `crates/app-ui/src/workspace.rs`: Kit screen composition. `window.rs`: Base Root and typed content access. `lib.rs`: native controller/jobs. `redaction_editor.rs`: image interactions and Kit toolbar. `caption.rs`: bounded source-label input/rasterization. `theme.rs` / `assets.rs`: shared presentation.
- `crates/app-core/src/`: capture/pixel ownership, annotation export, storage and uploads. `README.md`: current user workflows/limits. Phase 4/5 notes below retain historical implementation evidence.
- Current checks and release-resource comparisons are recorded in [ui-redesign-verification.json](docs/ui-redesign-verification.json). The opt-in native Linux probe drives synthetic pixels and only this application's windows; the Windows tests below remain separate platform gates.
- Ordinary build: `cargo build --release -p screenshots --offline`. Show the control window: `cargo run -p screenshots -- --show`. Fully quit an older running tray instance before checking a newly built UI; a second launch otherwise forwards to that existing instance.
- Focused editor fixtures: `cargo test --release -p app-ui --offline redaction_editor::tests -- --test-threads=1`. Native opt-in probes are `control_pages_render_and_cancel_hidden_history` and `editor_toolbar_visual_probe` (same test command with `-- --ignored --test-threads=1 --nocapture`). They need Windows desktop access and capture only their own empty/synthetic app windows into ignored `target/ui-preview`; these local PNGs are disposable inspection artifacts, not reference designs.
- Performance evidence: `docs/performance-resident.md`, `docs/region-latency.md` and `docs/control-open-latency.md`. Historical measured runs and their hashes are separate from targets. The sidebar run's 48.03 ms p95 second-frame callback is a development-machine proxy, not physical presentation/input acceptance; CPU/GPU memory, idle/soak and reference-hardware gates remain open.
- At session start, inspect `git status` and the current code. Preserve existing work; do not assume the working tree is clean or that every prior edit still needs applying. No commit/push is required by this handoff.

## 1. Project Vision

Build a modern, open-source desktop application for screenshots, screen recording, image editing, and sharing.

The goal is a capable desktop capture tool with a clear user experience, clean architecture, and long-term extensibility.

The application should make powerful functionality accessible without overwhelming new users.

### Core principles

- **Balanced UX:** Simple for beginners, powerful for professionals.
- **Cross-platform:** Windows, Linux, and macOS from day one.
- **Performance:** Near-zero idle activity, small bounded memory use, and immediate visible input response. Performance is a feature and a release gate from Phase 0 onward, including all future extensions.
- **Open source:** Transparent, community-driven development.
- **Local-first:** Fully usable without an account or cloud service.
- **Extensible:** Modular architecture for future integrations and workflows.
- **Privacy:** Users retain control over their screenshots, recordings, and uploads.

### Platform strategy

All three desktop platforms are first-class targets from the beginning:

- Windows
- Linux (X11 and Wayland)
- macOS

Linux must be treated as multiple environments rather than one uniform platform. Different desktop environments and compositors can have different capture capabilities and restrictions.

First-class support means each platform is tested and ships the capture modes it can actually perform. It does not mean every mode exists on every compositor in the first release. Where a mode is missing, the interface hides it and reports the limitation. It does not offer a control that fails later.

---

## 2. Technology Stack

The application will primarily be written in Rust.

| Area | Technology | Purpose |
|---|---|---|
| Programming language | Rust | Main application language |
| Desktop UI | GPUI | Native desktop interface |
| Async runtime | GPUI executor + Tokio | UI on GPUI's executor; Tokio only for HTTP and similar I/O |
| HTTP client | Reqwest | Uploads and HTTP communication |
| Serialization | Serde | Data serialization |
| Database | SQLite | Local metadata and history |
| Database access | **rusqlite** | Lightweight, synchronous SQLite access |
| Database migrations | rusqlite_migration | Schema versioning and migrations |
| Image processing | image | Decode, encode, crop, and pixel filters such as blur and pixelate |
| 2D rendering | tiny-skia | Vector annotations such as arrows, shapes, and text |
| Video processing | FFmpeg | Recording and encoding after the MVP. Not an MVP dependency |
| Clipboard | arboard / native APIs | Clipboard integration |
| Global hotkeys | global-hotkey / native APIs | Keyboard shortcuts |
| File dialogs | rfd / native APIs | Native file selection |
| Logging | tracing | Structured logging |
| Error handling | thiserror + anyhow | Application and library errors |
| Configuration | TOML | User settings |
| IDs | UUID | Unique identifiers |
| Time | Chrono | Date and time handling |
| Build system | Cargo | Rust workspace and builds |
| CI/CD | GitHub Actions | Cross-platform builds and testing |

Dependencies are initial candidates, not unconditional commitments. Check maintenance status, licenses, platform support, and actual performance before adopting them.

### Stack constraints

- Pin an exact GPUI source in Phase 0, either a Zed revision or a reviewed crates.io snapshot. The UI uses `gpui-kit` 0.9.1 and its pinned `gpui-pre-*` 0.3.12 family, replacing the earlier 1.27.0 packages. Cargo.lock freezes the resolved application graph. Record source provenance, packaging/platform patches, licenses and maintenance ownership; a version label is not proof of an unmodified or officially maintained upstream build. Forks that bundle tray/hotkeys/overlays are not adopted solely for those features.
- System tray, global hotkeys, clipboard, notifications, and file dialogs are platform crates. They are not GPUI features.
- GPUI's foreground executor is the UI runtime. Tokio is not. See the execution model below.
- Still-image codecs/CPU operations use the `image` crate. tiny-skia is a vector rasterization candidate; text requires a shaping/font pipeline. Blur and pixelate have an immediate bounded-resolution preview and are flattened in document order during export.
- FFmpeg stays out of the MVP. Before bundling it, review the exact build, enabled components and distribution obligations. A GPL-enabled build can impose GPL obligations on the combined distributed program; the MIT license on our own source does not remove those obligations. A separate executable is not a blanket exemption. Record a compatible distribution decision before integration.
- Keep default features and platform dependencies narrow. Load optional services only when used. Measure release profiles, LTO and dependency changes; do not assume Rust, a GPU renderer or more threads automatically make the application fast.

### Database decision: rusqlite

Use `rusqlite` instead of SQLx.

The database is primarily responsible for local metadata: captures, uploads, history, and application state. This does not justify the additional asynchronous database abstraction and infrastructure that SQLx introduces.

Reasons for choosing rusqlite:

- Lightweight and direct SQLite integration.
- No unnecessary async database layer.
- No connection pool required for the initial use case.
- Direct access to SQLite functionality.
- Simple and predictable database architecture.
- Less infrastructure and fewer moving parts.

Implementation guidelines:

- Enable SQLite WAL mode.
- Use prepared statements.
- Use transactions for related database changes.
- Add migrations with the first real database schema (Phase 5), not an otherwise unused database service during startup.
- Keep database access off the GPUI foreground executor. The render path is part of that executor.
- Execute blocking operations through a dedicated database worker or controlled blocking tasks.
- Start with a single database connection; introduce additional connections only when justified by actual workloads.

Suggested migration dependency: `rusqlite_migration`.

---

## 3. Architecture

Use a modular Rust workspace with clear boundaries between the UI, application logic, platform integrations, and infrastructure.

### Architectural principles

1. GPUI views contain presentation and interaction, not business logic.
2. The application core must not depend on GPUI.
3. Platform-specific implementations must be hidden behind common interfaces.
4. Database and filesystem operations must be isolated from UI rendering.
5. Upload providers must be interchangeable.
6. Features should be modular without introducing unnecessary abstraction.
7. Keep the architecture suitable for a single application rather than prematurely introducing microservices.
8. Capture, clipboard, hotkeys, tray, and notifications sit behind platform interfaces. The UI calls those interfaces and does not call native APIs itself.
9. The capture interface must be implementable by a fake backend, so UI and editor tests do not need a physical display.

### Proposed workspace structure

```text
project/
├── apps/
│   └── desktop/
│       └── src/
│           └── main.rs
│
├── crates/
│   ├── app-core/
│   ├── app-ui/
│   │
│   ├── capture/
│   ├── capture-platform/
│   ├── capture-windows/
│   ├── capture-linux/
│   ├── capture-macos/
│   │
│   ├── image-editor/
│   ├── media/
│   │
│   ├── clipboard/
│   ├── hotkeys/
│   ├── system-tray/
│   │
│   ├── storage/
│   ├── history/
│   │
│   ├── uploader/
│   ├── uploader-providers/
│   │
│   ├── settings/
│   ├── commands/
│   └── shared/
│
├── assets/
│   ├── icons/
│   ├── fonts/
│   └── themes/
│
├── packaging/
│   ├── windows/
│   ├── linux/
│   └── macos/
│
├── docs/
│
├── Cargo.toml
└── README.md
```

This is a responsibility map, not a list of crates to create. The existing four-crate workspace is the starting point; most responsibilities should initially remain modules.

Start with a smaller number of crates and split modules when their responsibilities and complexity justify it.

### Main architectural layers

**Application UI**
- GPUI components and views.
- Navigation and window management.
- User interaction and presentation.
- No direct platform or database implementation dependencies.

**Application Core**
- Application state.
- Commands and events.
- Feature orchestration.
- Capture, editing, storage, and upload workflows.

**Platform Layer**
- Native screenshot capture.
- Window and display enumeration.
- Global hotkeys.
- Clipboard.
- System tray.
- Platform-specific permissions and integrations.

**Infrastructure Layer**
- SQLite database.
- Filesystem storage.
- HTTP uploads.
- Configuration.
- Logging.

### Execution model

GPUI runs the interface on its own foreground executor. That executor is not a Tokio runtime. Do not start the process with `#[tokio::main]`; do not block it or poll futures that require a Tokio reactor there. Sending a request to an infrastructure-owned runtime is allowed.

Use three places for work:

- **Foreground:** GPUI state, rendering, short input handling, and native event registration/window calls that require this thread. No blocking enumeration, capture, database, filesystem, image encode/decode, filters, synchronous logging or HTTP here. Defer native work that can synchronously re-enter borrowed toolkit state.
- **Blocking workers:** SQLite, image encode and decode, thumbnail generation, and native capture calls that are synchronous. One database connection on one worker is enough to start.
- **Tokio runtime:** HTTP uploads and similar network I/O, owned by the infrastructure layer. Completed work is delivered back to the foreground executor.

Native capture APIs that must run on a specific thread stay on that thread. The core talks to them with a message. It does not pretend that every platform API is async.

### Performance contract — all versions and features

The application should feel absent when idle and immediate when used. Zero RAM use and zero physical latency are impossible; hidden work, unbounded caches and visibly stalled input are not acceptable substitutes. Do not lower budgets silently to make a benchmark pass. A failing backend or UI toolkit must be reduced, replaced, or explicitly reconsidered before its gate passes.

#### Reference workloads and initial budgets

Use a documented modest reference machine: four CPU cores, 8 GiB RAM, an integrated GPU, SSD, and a 60 Hz display. Record actual model, OS/compositor, power mode, display resolution/DPI and driver. Run on AC and repeat responsiveness checks in the normal battery power mode. Also exercise 120/144 Hz input/rendering where available. A high-end development GPU is not the release baseline.

The following are initial release acceptance targets. Establish actual baselines in Phase 0 and retain results for each supported platform/session; these numbers are not claimed to be achieved today.

| Scenario | Acceptance target |
|---|---|
| Resident tray, all windows hidden, no jobs | Mean process CPU <=0.1% of one logical core over 5 minutes after warmup; no application polling timers, periodic enumeration, idle redraw loop or idle disk/network traffic |
| Resident tray memory, no live image consumers | <=50 MiB process private committed/footprint memory and <=16 MiB app-owned GPU resources; also report resident memory and driver allocations separately, without hiding them by trimming the working set |
| Visible empty control/history shell | <=100 MiB private memory and <=32 MiB app-owned GPU resources; no frames when unchanged; first usable window <=150 ms p95 when resident |
| Cold launch | Tray/hotkeys usable <=1 s p95; database history, upload runtime and editor loading must not gate readiness |
| Input response | Visible acknowledgment <=50 ms p95 for clicks/commands; selection/drag feedback by the next presented frame, without waiting for workers |
| Foreground work during interaction | <=4 ms p95 application CPU work per frame; <=16.7 ms p95 presented-frame intervals at 60 Hz, <=8.3 ms at 120 Hz; no application-caused stall >50 ms in the reference run |
| Warm resident region capture | Hotkey received to correct frozen overlay accepting input <=150 ms p95 at 2560×1440, <=250 ms at 3840×2160, excluding explicit OS permission UI |
| Copy selected region / full display | Release of selection or receipt of capture command to clipboard ready <=300 ms p95 at 1440p, <=500 ms at 4K; local save completion measured separately, target <=1 s p95 on the reference SSD |
| Return to tray | Release session pixel buffers, editor/overlay textures and temporary image references within 5 s after their last consumer; after 30 s, private memory <=idle baseline +16 MiB |
| Repeated use | After cache warmup, 500 capture/cancel/open/close cycles show no persistent upward allocation trend; retained private memory <=warm baseline +10 MiB, with OS/driver caches reported separately |

CPU means accumulated process user+kernel CPU time divided by elapsed wall time, multiplied by 100; it is not divided again by logical processor count. On Windows report Private Bytes as the private-memory budget and private working set separately; on macOS report physical footprint and resident size; on Linux report private mappings and PSS/RSS. These metrics are not interchangeable. Keep per-platform baselines and explain driver/shared-memory costs instead of claiming cross-platform byte equivalence.

Active-image memory is budgeted by pixels, not by a fixed idle number. Let `S` be the total decoded source bytes for the current session (including row stride and every selected display) and `E` the bytes of the selected output. For 8-bit RGBA, one tightly packed 4K image is about 31.6 MiB. Start with a total CPU-image-buffer limit of `2*S + 2*E + 32 MiB` above the shell baseline, and a GPU image allocation limit of `S + E + 32 MiB`. Every pool/cache counts against these limits; HDR/float buffers use their real byte depth. OS-owned clipboard copies and backend/driver allocations must be reported as additional costs, not silently omitted. Set an admission ceiling of 256 MiB decoded source data per job initially; larger desktops/images need an explained limit or a separately measured bounded/tiled path before acceptance. Never allocate to exhaustion or partially capture a desktop without telling the user.

Clipboard ownership can be a live consumer even with every window hidden, particularly on Linux. Preserve paste semantics: never clear the clipboard just to meet an idle number. Retain at most the single required output representation, release the larger source/preview data, and report its actual retained bytes separately; the private-memory ceiling in that state is the 50 MiB idle base plus that representation. Avoid an extra decoded copy when the native clipboard can serve encoded data. The return-to-idle and soak checks must compare identical clipboard ownership states, not disguise a retained image as a leak or exclude a real allocation from the total.

#### Mandatory implementation rules

- Idle is event-driven. Deliver hotkey/tray/IPC events through native callbacks and an executor wakeup/channel. No 20 Hz event-receiver polling, permanent capture streams, busy waits or resident housekeeping timers. A bounded, operation-scoped visibility wait is allowed; long-lived idle polling is not.
- Render only on invalidation. Coalesce mouse moves to the latest position per frame; avoid one task per move. Hidden windows must not schedule frames or retain full-resolution image textures. Reuse a bounded set of native overlay shells only if images/textures are released and measurements justify keeping the shells.
- Keep a captured frame in memory through selection, crop, preview and clipboard. Do not PNG-encode to disk and decode it again to feed the next stage. Store source buffers with explicit ownership; reuse immutable data where possible and copy only when a consumer requires a different representation. Record unavoidable backend/clipboard copies. Do not promise zero-copy without measuring it.
- A canceled selection must not save a full-desktop PNG, create history or change the clipboard. Release its buffers. Full-desktop debug dumps are explicit opt-in diagnostics with short retention, never the production default.
- Copy from pixels and encode once per required export representation. Clipboard round-trip reads and whole-image pixel comparisons belong in tests/diagnostics, not every production copy. Reuse encoded output for saving/uploading when the representation matches.
- Allocate each image/texture once per required representation. No full-bitmap undo snapshots, full-frame re-upload for moving a selection rectangle, per-frame filesystem reads or whole-frame filters on the UI thread. Use cropped/dirty-region work and cancel obsolete results. A reusable buffer pool is bounded in bytes and released when idle; it is not an excuse for permanent peak RAM.
- Every worker queue, cache and undo history has a byte/item bound. Start with one active capture job and no unbounded capture backlog; duplicate triggers coalesce or receive a busy response. Allow at most two uploads and one CPU-heavy image job initially. Inactive workers sleep; optional network/editor services initialize on demand. Benchmark before increasing concurrency.
- Capture and interactive preview work take priority over optional thumbnails/indexing. Do not place a hotkey capture behind a queue of background image jobs. Use the existing executors with bounded admission and measured scheduling before introducing a separate thread pool per feature.
- Add cancellation, deadlines, job/session IDs and stale-result checks to every long operation. A late result must not reopen a canceled overlay, copy an old image, replace a newer document, or dereference a removed window. Blocking APIs that cannot be interrupted need a bounded lifecycle or an isolated fallback; spawning another thread after each timeout does not solve a hung call.
- Enumerate displays/windows on demand or OS topology notifications, off the foreground executor where permitted. Refresh only affected metadata. Never enumerate every window after every capture just to update diagnostics.
- History uses indexed, paged queries and a virtualized list/grid. Decode only visible thumbnails with bounded prefetch and a byte-limited cache. Search cancels obsolete queries. Loading 10,000 history rows must not create 10,000 views or decode their originals.
- Recording, OCR, uploads, plugins and later features follow the same idle/UI/memory contract. They consume resources only when requested; hidden background work, streaming capture and automatic network checks require an explicit product need and their own measured active-work budget.

#### Measurement and regression gate

- Measure optimized packaged/release builds, not `cargo run` debug timings. Keep a profiling build with symbols. Compare LTO/codegen/startup tradeoffs rather than adopting every optimization flag by default.
- Record cold launch separately from warm resident capture. Timestamp input receipt, capture start/end, hide/compositor wait, first correct presented overlay, crop, clipboard completion and durable save separately. Report end-to-end latency as well as stage timings; native capture alone is not the user-facing result.
- Run at least 100 warm captures per latency scenario and 20 cold launches, report p50/p95/p99 and max (cold-launch percentiles are provisional with only 20 samples). Use fixed image fixtures plus real 1440p/4K, mixed-DPI and permission/portal sessions. Exclude user decision time only when it is also reported separately. Consent/backend delays are not excuses to freeze input or hide end-to-end latency.
- Measure idle before and after captures, editing, history scrolling and uploads; include CPU, wakeups, disk/network activity, private/resident memory, allocation high-water marks and GPU usage. Do not keep CPU sampling, frame counters or full-resolution dumps enabled in ordinary production idle.
- Test input while encoding, uploading, paging history and exporting large images. Verify queue admission, cancellation, disk-full and worker-failure behavior under load. Unit tests do not replace hardware presentation/energy measurements.
- Run deterministic allocation/copy/queue tests in CI. Use a stable reference machine for latency/energy comparisons; noisy shared-runner timings are reports, not trustworthy hard latency gates. Any repeatable >10% p95 latency/private-memory regression on the same workload requires a recorded cause and remediation before merge, even if the absolute budget still passes. Report small/noisy differences as inconclusive.
- Every phase has performance evidence for its new path. New dependencies, caches, background services and high-resolution pipelines need a stated resource cost. Performance is not deferred to Phase 6.

#### Prototype debt that must not ship

The initial polling loops and file-based frame handoffs have been replaced with native events and shared in-memory pixels. Canceled previews no longer save PNGs, and clipboard read-back runs only in tests. Windows now keeps controller/tray/hotkeys alive independently of windows, attaches graphics on demand and detaches after closing its UI. Capture groups now admit at most 256 MiB of decoded RGBA source data before native capture and recheck actual returned dimensions; individual capture, crop and recopy share the image limit. XCap still controls its internal allocations, including size-change races; a hard native allocation bound, other-platform resident integration and unmeasured reference-hardware memory/latency budgets remain production debt. See [Windows resident evidence](docs/performance-resident.md) and [control frame-callback timings/teardown regression](docs/control-open-latency.md); these callback timings do not establish physical presentation or usable-input latency. The [1440p native-hotkey region probe](docs/region-latency.md) records 101 real capture/cancel cycles and stage timings; 4K, physical presentation/input and reference-hardware acceptance remain open. Passing the functional spike is not a performance result. See [Phase 0 evidence](docs/phase-0.md).

### Process lifecycle

Windows implementation: normal startup uses SID/session-scoped named kernel objects with Windows default token security. A second launch sends only a bounded/coalesced Show signal, then exits before GPUI/tray/hotkey initialization. The listener starts after event delivery is installed and waits natively without polling. Ownership survives until GPUI shutdown completes. Resource cost: one parked native waiter with a 256 KiB requested stack and a small fixed handle set; updated idle measurements are not claimed here. The kernel-object handoff/restart test and a real two-process release smoke check passed: the secondary exited and native window enumeration confirmed a visible control window in the original process. The smoke processes exited on their diagnostic deadlines with no stderr errors. Cross-platform IPC, foreground-activation policy and shutdown/session/elevation races remain verification work.


A capture tool should run as a single instance. A second launch forwards its request to the running instance and exits. Two instances would register the same hotkeys and write the same history.

The running instance owns the tray icon, the hotkeys, and the database worker. Closing the main window hides the window. Quitting is explicit.

Use authenticated/user-scoped local IPC with bounded messages; a second launch must never silently create another tray/hotkey owner when forwarding fails. Native registrations stay on their required event-loop thread. Tray availability is a capability: sessions without a tray get an accessible window/launcher alternative, never a hidden process the user cannot reopen. Hide-on-close requires such a working reopen path. On quit, stop accepting jobs, cancel or bound outstanding work, release registrations and close storage in order. Test restart, sleep/resume, display disconnect, device loss and session lock; no stale frames or callbacks after shutdown.

---

## 4. Screenshot Capture Engine

The capture engine is one of the most important and technically challenging parts of the application.

It should be designed around a common API with platform-specific implementations.

### Required capture functionality

Implemented convenience workflow: a configurable delayed region selection (default three seconds) with a cancel button/Escape in the control window. Closing the UI or quitting discards the countdown task; it adds no resident polling. The existing capture/selection/copy/save pipeline is reused after the delay.

Initial implementation (2026-11-12, persistence superseded below): Recent capture offers an explicit Apply delay action for
whole seconds 1–60, default 3. The active delay is labeled as session-only; invalid
input retains it. The same scalar controls region and Windows active-window
countdowns from the control/sidebar/tray. Quick capture and global immediate
commands are unchanged. The running countdown snapshots the duration, blocks
preference changes and retains existing cancellation/Quit behavior. UI and tray
labels no longer promise a fixed three seconds.

Resource cost is one resident byte, one two-digit page-local Kit input and one
countdown snapshot. The input is released on page exit/UI close. The existing
operation-scoped once-per-second timer now runs at most 60 ticks, with no timer
when idle; no new worker, runtime, queue, cache, image buffer or dependency is added.
Optimized headless checks cover range boundaries, invalid/empty values, explicit
button routing, active-window duration, snapshotting, blocked changes, cancellation
beyond the original deadline and input release. Native tray/keyboard/accessibility,
timing under suspend/load and resource acceptance remain open. No new latency or
process/GPU measurement is claimed for this bounded preference. Persistence and
subsecond/longer delays remain outside this slice.

Persistence follow-up (2026-11-12): Save delay now persists the 1–60-second choice
through the existing single-admission settings worker and commits the active
value only after successful durable publication. Validation/write failure keeps
the old active value. Legacy settings default to three seconds; invalid stored
values are rejected. Startup restores the scalar and refreshes the page-local
input. Folder/theme/hotkey/output-profile/region-template/JPEG-quality/export-format/
filename-template saves preserve delay, and delay saves preserve those preferences.
No capture starts while persistence is pending; the running countdown still uses
its original snapshot. Resource cost remains the scalar/input plus a small
settings-job snapshot and the existing bounded settings serialization/temporary
file; there is no new worker, runtime, timer, watcher, image or cache. Optimized
settings fixtures verify bounds, reload, legacy fallback, preference preservation
and rejected-write preservation. The headless UI test verifies pending/failure
rollback, button routing and countdown behavior using a blocked temporary folder,
without writing the user's configuration. Native packaged restart, recovery,
keyboard/accessibility and resource acceptance remain open; no new native latency
or process/GPU measurement is claimed for this settings-only follow-up.


- Fullscreen capture.
- Display capture.
- Window capture.
- Region capture.
- Multi-monitor support.
- Display and window enumeration.
- High-DPI support.
- Correct coordinate handling.
- Permission handling.
- Clipboard integration.

Define modes precisely: primary-display capture and explicitly selected-display capture return one image; all-displays capture returns one image per output in the MVP. Region selection is confined to one selected output initially. A stitched virtual desktop or a drag spanning mixed-DPI monitors is separate scope and must not be implied by the phrase multi-monitor support. Display/window IDs are session-scoped and can become stale; validate them and the topology at capture time and cancel selection on an incompatible topology change.

### Stream candidate for evaluation: Scap

Repository: https://github.com/CapSoftware/scap

The existing Windows still-image spike uses XCap. Evaluate Scap only where a stream API or missing backend capability justifies it, especially future recording/PipeWire work. Do not replace the measured still-image path just to use one library everywhere.

It advertises support for:

- Windows Graphics Capture.
- macOS ScreenCaptureKit.
- Linux PipeWire.

However, do not assume that every feature works identically across all platforms.

Scap is built around capture streams. A stream API is only a good fit for a still screenshot if the pinned revision can also return one frame cleanly. On Linux, a README line that says PipeWire is not evidence. Confirm that the revision builds against current PipeWire and returns a frame on each compositor you intend to support.

Test its actual capabilities before deciding whether to build additional native backends.

### Alternative: XCap

Repository: https://github.com/nashaofu/xcap

XCap 0.9.8 is the evaluated Windows still-image candidate. Measure its suitability on macOS and X11 before claiming those targets work. Pin accepted backend revisions and record the exact platform/session in the spike notes.

On Wayland, measure it before trusting it. Expect incomplete window lists, limited screen capture, and monitor positions that cannot be used to crop a region. A reported origin of `(0, 0)` for every output is a known class of failure. Do not build region selection on those coordinates.

### Wayland still capture

Also evaluate the XDG Desktop Portal directly, through a Rust portal client such as `ashpd`.

Use the portal as the Wayland still-image candidate on GNOME and other compositors with an appropriate backend. Introspect the available interface version and runtime capabilities; installed portal software is not proof that a particular capture mode works. It can be the Wayland backend while XCap handles Windows/macOS/X11.

Portal cancellation is a normal outcome, not an application error. Permission denial must be shown to the user with the next step, not swallowed as an empty image.

### Native platform implementations

**Windows**
- Windows.Graphics.Capture.
- Display and window enumeration.
- Multi-monitor support.
- DPI-aware coordinate handling.

**Linux – Wayland**
- XDG Desktop Portals.
- PipeWire.
- Permission-aware capture workflows.
- Compositor-dependent functionality.

**Linux – X11**
- Native X11 capture.
- Window and display enumeration.
- Multi-monitor support.

**macOS**
- ScreenCaptureKit.
- Native screen recording and capture permissions.
- Retina-aware coordinate handling.

### Common API

Expose a consistent interface to the application core.

Illustrative interface:

```rust
pub trait CaptureBackend: Send {
    fn capabilities(&self) -> CaptureCapabilities;

    fn list_displays(&self) -> Result<Vec<DisplayInfo>, CaptureError>;

    fn list_windows(&self) -> Result<Vec<WindowInfo>, CaptureError>;

    fn capture_display(&self, display_id: &str) -> Result<CapturedImage, CaptureError>;

    fn capture_window(&self, window_id: &str) -> Result<CapturedImage, CaptureError>;
}
```

`capabilities` is part of the contract and reflects the current session/API/permissions, not just `cfg!(target_os)`. Distinguish direct window enumeration/capture from portal-mediated user selection: a portal may offer a user-picked window without exposing a window list to the application. Unsupported actions stay absent or unavailable with an explanation. Display capture may be available only through a portal flow.

The example is synchronous on purpose. Native grabs may be synchronous or callback-based. The `Send` contract applies to a core-facing handle; thread-affine native objects stay on their owner thread behind that handle. Do not require `Sync`, transfer objects that are not transferable, or wrap an operation in an async trait just to look uniform.

Portal calls wait on D-Bus. Poll their runtime-dependent futures on the integration's executor and return a completion to GPUI. GPUI may await a compatible completion receiver without blocking; it must not block a worker for the entire time a user is deciding in a consent dialog. Portal requests support cancellation and do not occupy the image-processing queue while waiting.

The final type can be a trait or an enum. The application core depends on common result types either way. A production capture result owns or references validated in-memory pixels with dimensions, stride, channel/alpha format, color metadata and source identity; a filesystem path is an export result, not the transport between capture, overlay, crop and clipboard. Check byte-size arithmetic before allocation.

### Region selection

Region selection is not one portable workflow. Use the path the platform actually allows.

**Custom overlay.** Windows, macOS and X11 use validated native overlay windows. A Wayland custom overlay requires a compositor advertising the appropriate layer-shell protocol and a validated capture coordinate/input path. The GPUI snapshots expose layer-shell facilities; upstream also includes an example. The old claim that GPUI has no layer-shell support is incorrect. An API/example alone does not validate output targeting, exclusive keyboard input, fractional scaling or our capture path. Layer-shell does not grant screen-capture permission or a usable global image coordinate system. Until those checks pass for a session, use portal selection/editor crop.

1. Hide this application's own windows, or exclude them from the capture, so the overlay is not part of the image.
2. Capture the selected display when the platform provides a frame and a usable coordinate space.
3. Show a transparent, frameless, always-on-top selection surface on that display.
4. Convert the selection from logical coordinates into physical pixels.
5. Crop the captured frame.
6. Return the image.

**Portal selection.** Use this for Wayland sessions where our custom overlay/capture path is unavailable or unvalidated, including the planned GNOME path. Detect protocol availability on the running compositor rather than hardcoding support from a desktop name. An XWayland fullscreen window is not a substitute for a validated native Wayland overlay with correct keyboard access.

1. Inspect the portal version. With Screenshot interface version 3 or later, request a `target` only when advertised in `AvailableTargets`. On older backends, `interactive` is a customization hint, not a guaranteed area/window picker; validate the actual UX and provide editor crop where necessary.
2. Treat cancellation as a normal outcome.
3. Use the image the portal returns. Do not invent a global monitor origin the compositor did not provide.

**Editor crop.** If neither overlay nor interactive portal is available, capture a display and let the user crop in the editor.

A normal toplevel window is not an overlay. On Wayland, covering the screen requires layer-shell or the portal's own UI. There is often no global coordinate space, so a capture-then-crop overlay must not be the default Wayland design.

### Still-image format

Store still captures as PNG. Width and height in the database are physical pixels.

Keep channel order, stride, alpha representation and color space explicit at the capture boundary. MVP export targets SDR sRGB PNG with appropriate color metadata. Convert supported tagged sources into that space once; validate HDR/tone mapping and wide-gamut inputs per backend or mark the mode unsupported. Unknown color metadata is an explicit limitation, not proof of sRGB. Do not claim HDR preservation for an 8-bit RGBA backend. Budget conversion buffers. JPEG can be added later as an explicit export option.

### Important edge cases

- Multiple monitors.
- Negative monitor coordinates.
- Different monitor resolutions.
- Mixed DPI scaling.
- Retina displays.
- Monitor rotation.
- Wayland permission restrictions.
- Compositor-specific limitations.
- Window movement during capture.
- Display changes during application runtime.
- The application's own overlay, editor, or main window appearing in the shot.
- Portal cancellation and denied/not-yet-granted permission are typed outcomes. Empty or structurally invalid frames fail. All-black pixels alone are not a reliable failure detector: a valid selection can be black. Use backend status/permissions to distinguish protected or unavailable content; never fabricate a successful frame after a backend error.
- No global coordinate space, including Wayland outputs reported at `(0, 0)`.
- Windows.Graphics.Capture consent UI and capture border.
- macOS Screen Recording permission that behaves differently for `cargo run` than for a signed, stable bundle. The spike must include the packaged form, not only a debug binary.

The application must expose available capabilities rather than promising identical functionality on every platform.

---

## 5. Application Core

The application core coordinates the individual features.

Keep the command and event architecture simple. A large event-bus framework is unnecessary at this stage.

### Example commands

```text
CaptureFullscreen
CaptureRegion
CaptureWindow
CaptureDisplay

SaveCapture
CopyCapture
EditCapture

UploadCapture
DeleteCapture

OpenHistory
OpenSettings
```

### Example events

```text
CaptureStarted
CaptureCompleted(CaptureId)
CaptureCanceled
CaptureFailed

UploadStarted
UploadCompleted
UploadFailed

CaptureSaved
CaptureDeleted
```

The UI sends commands to the application core and receives state updates or events.

The application core handles the actual workflows and communicates with the required services.

Model a capture session explicitly: idle, preparing, awaiting permission, selecting, processing, completed, canceled or failed. A terminal state restores or hides UI according to the initiating context and releases ownership exactly once. Give each session an ID; topology changes, cancellation and shutdown invalidate late callbacks. Restore UI on every failure path without forcing focus away from another application for a background capture unless the workflow needs it.

---

## 6. Local Storage and Database

The application should work completely offline without requiring an account or external service.

### Storage strategy

Use SQLite for metadata and the filesystem for actual image and video files.

Do not store large image or video files as database BLOBs.

### Database tables

Initial schema:

**captures**

```text
id
file_path
thumbnail_path
mime_type
width
height
file_size
created_at
source
```

**uploads**

```text
id
capture_id
provider
url
status
created_at
```

Additional tables can be introduced when necessary for recordings, workflows, or other functionality.

### Database implementation

- SQLite.
- rusqlite.
- rusqlite_migration.
- WAL mode.
- Prepared statements.
- Explicit transactions.
- Dedicated database worker or controlled blocking execution.
- No database access from the GPUI foreground executor, including during render.

When the first database schema is introduced in Phase 5, include migrations, foreign keys and indexes from its first version. Keep the database on a local application-data filesystem: SQLite WAL is not a network-filesystem design. The user-configurable export directory is separate. Bound busy waits and checkpoint work on the database worker; document the chosen synchronous/durability policy and test power-loss behavior. Do not disable durability merely to improve benchmark numbers. Back up before destructive migrations, test upgrades from every released schema, and open newer unknown schemas without destructive changes (or explain incompatibility).

### Filesystem

Use platform-appropriate application data directories.

Keep the storage implementation independent of the UI and database implementation.

Support:
- Original captures.
- Thumbnails.
- Recordings.
- Temporary files.
- Exported images.

The user should be able to configure the storage location.

File saving and SQLite are not one atomic transaction. Write to a uniquely named temporary file on the destination filesystem, finish/flush according to the stated durability policy, atomically publish without unintended overwrite, then commit ready metadata. Track pending operations and reconcile interrupted saves/deletes with bounded recovery work. Mark thumbnails as derived data that can be rebuilt lazily. A disk-full, permission or rename failure must not create a success notification or a history entry pointing at a missing file. Report Copy and Save independently when only one succeeds.

Retention is explicit and bounded for history, thumbnails, temporary files and logs. Cleanup may delete only app-owned files under validated storage roots; it must not traverse symlinks or delete arbitrary user exports. Changing folders, unplugging an export drive, and deleting records/files are tested workflows. Export deletion and metadata deletion have a documented order and recoverable partial-failure state. Do not keep sensitive full-desktop preview files after a canceled selection.

MVP history search covers metadata only: file name, date, capture source, and upload URL. Searching the pixels waits for OCR.

### Secrets

Upload tokens, passwords, and authorization headers do not belong in the TOML settings file. Store them in the operating system secret store: Windows Credential Manager, macOS Keychain, or the Linux Secret Service.

The TOML file may store a non-secret profile: URL, method, form field, and which secret to read. If no secret store is available, refuse to save the credential. Do not fall back to writing it into the settings file. Logging must not print secrets or authorization headers.

---

## 7. Upload and Sharing System

Sharing should be modular and independent of any particular hosting provider.

### UploadProvider abstraction

Create a common provider interface.

Illustrative concept:

```rust
pub trait UploadProvider {
    async fn upload(&self, file: UploadFile) -> Result<UploadResult, UploadError>;
}
```

The final implementation should support provider-specific configuration and authentication without exposing provider internals to the UI. The upload call runs on the Tokio runtime, not on the GPUI foreground executor. The `async` here is real network I/O. It is not a model for the capture interface.

### First implementation: Custom HTTP uploader

The first upload provider should be a generic HTTP uploader.

Configuration options:

- Request URL.
- HTTP method.
- Multipart form field.
- Custom headers.
- Authentication, read from the secret store rather than from the settings file.
- Request parameters.
- Response parsing.
- URL extraction.

URL extraction stays declarative: HTTP status, a response header, or a JSON pointer. The MVP does not embed a scripting language in the uploader. That would be a plugin runtime under another name.

This makes it possible to integrate many existing upload services without implementing a dedicated provider for every service.

Uploads are explicit opt-in actions, disabled until a destination is configured; capture and editing never upload implicitly. Default to HTTPS. Support user-configured local/self-hosted destinations deliberately, not through URLs taken from images or untrusted responses. Do not forward credentials or replay captured content to a different host through redirects; disable redirects by default. Stream files rather than loading a second full file into RAM. Start with bounded connect/transfer timeouts (5 s / 60 s), a 1 MiB response-body limit and the concurrency limits in the performance contract. Expose cancel and progress without per-packet UI updates. Retry with a finite policy only when safe; a POST is not automatically idempotent. Never auto-open extracted URLs; validate http/https before offering them, and treat returned text/HTML as data.

### Future providers

- S3-compatible storage.
- Cloudflare R2.
- Custom self-hosted upload servers.
- Community-maintained providers.
- Additional image and file hosting services.

Do not implement a large plugin runtime in the MVP.

Keep the provider architecture extensible so that a plugin system can be introduced later if it becomes useful.

---

## 8. Image Editor

The editor should be powerful but approachable.

All screenshot editing tools, including future tools, must also be available inside the frozen capture overlay before final output. Share document state, geometry, undo and export behavior with the standalone editor rather than building a second editing implementation. Selection and annotation do not publish a screenshot: only an explicit copy/save action changes the clipboard, files or history. Sessions without a validated custom overlay use the supported portal/editor fallback described above.

A major architectural decision is to use non-destructive editing.

Instead of modifying the original image whenever the user adds an annotation, store annotations as individual objects and render them during export.

Crop changes the document canvas and is stored as a crop rectangle so it can be undone. Arrow, rectangle and ellipse are vector objects; tiny-skia is a rasterization candidate, not a text layout engine. Choose and test text shaping, font fallback and glyph rendering explicitly, with consistent preview/export layout. Blur and pixelate are ordered image effects: preview them during editing and flatten them into the exported pixels. Draw and apply effects in document order, not in separate passes that silently reorder overlapping objects. Immutable original pixels remain available in memory for undo until the document closes.

Undo and redo record document changes. They do not snapshot the full bitmap on every edit.

Render an immediate bounded-resolution preview while dragging effects; perform expensive refinement/export off-thread with stale-result checks. Limit undo by bytes and commands. Use the same crop/transforms, text layout, alpha handling and layer order in preview and export, verified by deterministic image fixtures. Zoom/pan/selection must not rerasterize the whole full-resolution document on each mouse move.

Only a finalized, flattened document may be copied, saved or uploaded after edits. Never upload the original as a shortcut when redaction/export is still processing or has failed. Raw unredacted originals, undo data and thumbnails must not survive in app-managed history or temporary files by default after a sanitized export; retaining originals is an explicit choice with a clear privacy implication. Previously exported user-owned originals are not silently deleted; explain that sanitizing a new export does not sanitize those earlier copies. Blur/pixelation are visual obscuring tools, not a guarantee of irreversible removal: offer an opaque redaction rectangle and explain the distinction when choosing the tool. Do not promise forensic secure deletion on SSDs or external filesystems.

### MVP editing tools

- Crop.
- Arrow.
- Rectangle (outline and filled).
- Ellipse (outline and filled).
- Text.
- Blur.
- Pixelate.
- Opaque redaction rectangle.
- Undo.
- Redo.

### Later editing tools

- Freehand pen (initial bounded vector tool implemented).
- Numbering (initial sequential text-marker tool implemented).
- Spotlight (initial ordered rectangular focus tool implemented).
- Highlighting (initial translucent rectangle tool implemented).
- Additional shapes.
- More advanced text options.
- Advanced image transformations.

### Editing model

Represent each annotation as an independent object with its own properties.

Examples:

```text
Arrow
Rectangle
Ellipse
Text
Blur
Pixelate
```

The editor should support:

- Selection.
- Movement.
- Resizing.
- Deletion.
- Duplication (all current mark types; one undo step and shared assets).
- Undo and redo.
- Exporting the final rendered image.

Initially, annotations can remain in memory during editing.

A persistent editable project format can be introduced later.

---

## 9. User Interface and UX

The UI should feel like a modern desktop application rather than a collection of disconnected tools.

GPUI is the selected UI framework.

The application core must remain independent of GPUI. That boundary keeps a later UI replacement possible. It is not a promise that overlay behavior written for GPUI will move unchanged to another toolkit.

### Main navigation

Implemented control-window sidebar destinations:

```text
Screenshots
Capture history
Displays & windows
Uploads
Settings
```

Recordings appears when recording exists. Workflows appears when the workflow engine exists. An empty Home page is not part of the shell.

Not every section needs to be fully implemented in the MVP. Uploads can ship as soon as the HTTP uploader exists; until then the navigation does not show an empty section.

The control window follows a fixed-sidebar/large-workspace desktop layout. Capture commands stay directly accessible on the left; screenshot preview, capture history, display/window selection, uploads and settings have separate workspaces. An empty screenshot workspace offers explicit Capture & annotate and Quick region actions. Folder/quit remain in navigation. Menu entries use small native vector icons, neutral surfaces, a blue selected state, hover/pressed feedback, native GPUI focus traversal/keyboard activation and visible focus borders. Page changes use existing history cancellation/invalidation and preview release; active output/upload jobs continue. Closing a history view resets navigation to Screenshots to avoid a blank retired-history workspace on reopening. The shared active-tool styling remains in the overlay and standalone editor. Final visual design, complete context menus, capture-gallery layout, light theme, narrow-window/DPI interaction and assistive-technology acceptance remain open.

Resource cost is one small page enum, a bounded set of sidebar/focus/accessibility nodes and tiny vector icon paths on invalidated renders; hidden page trees are not built. No image/texture cache, dependency, worker, timer or idle task is added. Native optimized testing opens all five destinations, verifies history cancellation on switching and renderer detach on close, and captures only its own empty window for layout inspection. A 101-cycle control-window probe observed 48.03 ms p95 to the second frame callback, versus 43.86 ms in the preceding header-navigation run. Separate development runs do not establish causality or a speedup; these are callback proxies, not physical presentation/input or complete CPU/RAM/GPU acceptance. See [sidebar measurement](docs/control-open-latency.md#sidebar-layout-follow-up).

### Quick Access Overlay

Editor UI follow-up (2026-11-09): the shared frozen-overlay/standalone toolbar now groups 14 compact native vector buttons into Select/Crop, drawing, labels/highlighting and privacy effects. Tooltips include shortcut names and buttons retain focus borders/accessibility labels. Copy/Save/Save & copy are separated from drawing; color controls use real swatches; selection actions appear only for an existing selected mark. Keyboard and pointer tool changes share the same state cleanup/effect-preparation path. The bounded floating panel measures its children and defers one invalidation when its height changes, replacing the previous guessed workspace height. Its scroll height is capped at 60% of the viewport minus outer spacing; the existing top/bottom overlay placement remains available.

The design references include [Flameshot's in-place editing interface](https://flameshot.org/) and [Shottr's explicit export actions](https://shottr.cc/kb/startguide). These inform grouping/discoverability; no external artwork, UI framework or service is added. Incremental resources are up to 14 tiny vector paths per invalidated render, transient tooltip/focus metadata and one shared float cell per editor for measured height. There is no new image buffer/texture, worker, queue, permanent timer or idle work. Native optimized inspection uses a synthetic image and PrintWindow for only that editor HWND at 1000×760 and 760×500, preserving user pixels/clipboard; normal capture deliberately excludes the app's own windows. Release editor fixtures and the native overlay cancellation regression cover behavior. Final toolbar ergonomics, keyboard/screen-reader interaction, mixed DPI, light theme and complete CPU/RAM/GPU/input-latency acceptance remain open; the earlier control-window timing is not a measurement of this editor change.

After taking a screenshot, display a small floating interface with the most important actions:

- Copy.
- Save.
- Edit.
- Upload.
- Delete.
- Open in History.

The overlay should be quick, unobtrusive, and easy to use.

### UX principles

Visual direction: use a minimal neutral Dark appearance by default, with Oldschool as the compact alternative. Main window, editor, overlay toolbar and text input share semantic palettes in `crates/app-ui/src/theme.rs`; image/annotation colors remain document data. Palette changes add no runtime service, dependency, cache or idle work.

English is the default application language at launch, independently of the OS locale. All app-owned labels, menus, prompts, status text and errors must start in English. Localization may be added later; native OS dialog chrome and system error messages follow the OS language.


- Clean and modern interface.
- Minimal unnecessary UI elements.
- Clear visual hierarchy.
- Consistent design system.
- Keyboard-first workflows for power users.
- Discoverable functionality for beginners.
- Native platform behavior where appropriate.
- Dark and light themes.
- Immediate pressed/selection feedback; no debounce on direct manipulation. Search may coalesce input and cancel stale requests without blocking typing.
- No layout-blocking spinners, ornamental perpetual animations or simulated loading delays. Load fonts/assets on demand; respect reduced motion.
- Keyboard navigation, visible focus, scalable text and accessible labels. Verify actual assistive-technology behavior on each target; GPU rendering does not establish accessibility by itself.

### GPUI technical evaluation

Before building the full application, verify the pinned GPUI source itself:

- Transparent windows.
- Frameless windows.
- Always-on-top windows.
- Multiple windows.
- Placing a window on a specific monitor, including a monitor with a negative origin or a scale factor other than 1.
- Platform-specific window behavior.

Verify these separately. They are not GPUI features:

- Global hotkeys.
- System tray.
- Clipboard, including placing a PNG on the clipboard.
- Native file dialogs.
- Notifications.

If the pinned GPUI source cannot show a frameless, transparent, always-on-top surface on the monitor being captured, stop and decide on the UI again before Phase 1. On Wayland, also check whether that source can create a layer-shell surface. A transparent toplevel does not count. Do not build the design system on a toolkit that cannot host the region overlay, unless the product decision is to use the portal's selection UI for every session where the overlay is impossible.

Do not assume that every feature is equally mature on every operating system.

---

## 10. Desktop Integration

Native desktop integration is essential for an application of this type.

### Required functionality

- Global keyboard shortcuts.
- System tray integration.
- Clipboard access.
- Native file dialogs.
- Notifications.
- Floating quick-access overlay.
- Multi-window support.
- Startup behavior and application lifecycle handling.

Each platform should have its own implementation where necessary.

Avoid forcing one generic implementation onto platforms with fundamentally different APIs.

Global hotkeys are one of those splits:

- Windows: `RegisterHotKey`.
- macOS: a Carbon or Cocoa global hotkey, which needs an Accessibility permission depending on the API.
- X11: an X grab while the real X11 session is in use. An X grab inside XWayland does not deliver keys when a native Wayland window is focused.
- Wayland: the GlobalShortcuts portal, and only where that compositor's portal backend implements it. There is no general Wayland equivalent of `RegisterHotKey`.

The settings screen must be able to say that a global hotkey is unavailable here, and how the user can bind one through the desktop. A silent registration failure is not acceptable.

Clipboard images have the same kind of split. Writing image bytes that work on Windows does not mean the Wayland clipboard will offer them as `image/png`. Test the round trip on each session type.

The quick-access overlay is a small window, not the full-screen region overlay. It still has to position itself on the monitor that was captured, including mixed DPI and negative origins. On Wayland it only needs to be a normal window, so it must not depend on layer-shell.

---

## 11. MVP Scope

The MVP should already be useful as a daily screenshot and sharing application.

Advanced features should be introduced in focused, verified stages.

A platform that cannot list windows, register a global hotkey, or show a custom region overlay still meets this scope when the modes it does support work and the missing ones stay hidden.

### Capture

- Fullscreen screenshots.
- Display screenshots.
- Window screenshots.
- Region selection.
- Multi-monitor support.
- Global hotkeys.

### Editor

- Crop.
- Arrow.
- Rectangle (outline and filled).
- Ellipse (outline and filled).
- Text.
- Blur.
- Pixelate.
- Undo and redo.

### Sharing

- Copy to clipboard.
- Save locally.
- Generic HTTP uploader.
- Automatic URL copying.
- Upload history.

### History

- Capture history.
- Thumbnails.
- Metadata search by file name, date, capture source, and upload URL.
- Metadata.
- Delete captures.
- Open captures.

### Desktop integration

- System tray.
- Quick-access overlay.
- Global hotkeys.
- Native dialogs.
- Notifications.

### Settings

- Theme.
- Hotkeys.
- Capture defaults.
- Storage location.
- Upload configuration, with credentials in the OS secret store.

---

## 12. Development Roadmap

The following timeline is a rough estimate, not a production deadline. Phase 0 is unscheduled; platform, correctness and performance evidence determine the gates. The weeks below start only after Phase 0 has passed. A functional Windows demo does not establish cross-platform readiness or the resource budgets.

If the spike shows that a platform needs its own capture backend, Phase 2 takes as long as that backend takes. Do not drop DPI handling, permissions, or Wayland behavior to protect the two-week estimate.

### Phase 0 – Technical Spike

**Goal: Validate the riskiest technical decisions before committing to the architecture.**

The prototype may be thrown away. Phase 1 starts the application workspace only after the gate below passes. If the chosen GPUI pin is accepted, Phase 1 may keep that crate setup.

Tasks:

- Pin a GPUI source and record the exact revision.
- Build a minimal GPUI window on Windows, Linux X11, Linux Wayland, and macOS.
- Test transparent, frameless, and always-on-top windows on the pinned source.
- Test placing that window on a non-primary monitor, including a negative origin and a scale factor other than 1.
- Evaluate Scap where needed to resolve stream/PipeWire questions; a redundant backend is not a gate requirement when an accepted still-image path already meets the target.
- Evaluate XCap for a still image on Windows, macOS, and X11, and measure its Wayland coordinates and window list.
- Evaluate an XDG Desktop Portal still capture, including interactive selection and user cancellation.
- Decide the region-selection path per session type: custom overlay, portal UI, or editor crop.
- Put a PNG on the clipboard and paste it back on each session type.
- Register a global hotkey on Windows, macOS, and X11, and record the Wayland result through the GlobalShortcuts portal.
- Show a system tray icon and a menu on each platform.
- On macOS, repeat the capture check from a stable signed bundle, not only from `cargo run`.
- Measure release-mode idle, startup, UI/input latency, end-to-end capture and CPU/GPU memory against the performance contract on modest reference hardware. Test the idle cost of the pinned GPUI renderer before building on it.
- Compare an in-memory still-image path against the spike's disk/PNG handoff and count allocations/copies. Prove bounded ownership and cancellation; record any capability or budget failure.

**Exit gate:** The notes must name and demonstrate a capture approach and region UI on each declared target/session, distinguish unavailable modes, and include measured performance results or an explicitly unresolved failure. A candidate name or documented failure is not a passing result. Measure the implemented slices (empty resident shell, reference capture and overlay), not future history/editor features. Replace prototype event polling before accepting its idle results. The UI/renderer must meet the idle/input budgets and the capture design must demonstrate a feasible bounded in-memory path; functional correctness alone does not pass the gate. If GPUI cannot support the required overlays or performance, reconsider it before Phase 1. If the only gap is Wayland layer-shell, use validated portal selection/editor crop. Changing supported release targets or relaxing budgets requires an explicit product decision, not an unchecked box silently removed.

**Deliverable:** A short spike note with the pinned revisions, what worked, what failed, and the capture decision.

### Phase 1 – Foundation

Implementation progress (Windows-first, not a completed cross-platform phase gate): the usable capture home now groups region/display/window actions, last-output controls and optional diagnostics. Native folder selection persists the capture destination with worker-side bounded settings IO. Remaining phase/release gates stay open; feature work proceeds without repeating exploratory performance runs for every UI change.

UI direction: the current capture home is a functional placeholder. Its layout, typography, visual hierarchy and component styling are not the final design; a deliberate, polished UI pass remains required. Functional progress should continue without treating this temporary appearance as a design commitment. One-off PNG export now uses a native Save As dialog, preserving the default capture folder and existing files.



**Estimated duration: Weeks 1–2**

Tasks:

- Create the Cargo workspace.
- Set up the desktop application.
- Integrate GPUI.
- Build the main window.
- Implement basic navigation for the sections that exist.
- Establish a small design system: color, type, and spacing. Leave the editor's visual design until Phase 4.
- Create the application core and the foreground, worker, and Tokio split.
- Replace spike polling with event-driven hotkey/tray/IPC delivery and lazy service startup. Implement bounded queues, session IDs, cancellation and a repeatable performance harness now.
- Implement bounded asynchronous logging in the application-data directory; rotate/cap files, redact secrets and avoid logging paths/window titles/content by default. Crash reports/full-frame diagnostics require explicit opt-in. Error logging must not become a render-path lock or idle disk writer.
- Add settings infrastructure without a secret store yet.
- Set up CI for all three platforms. macOS hosted runners are a cost; record that choice instead of assuming they are free.
- Record shell idle/private-memory and cold/warm startup baselines; fail this phase if the framework alone exceeds the contract without an agreed replacement/rework.

**Deliverable:** A functioning cross-platform application shell.

### Phase 2 – Capture Engine

**Estimated duration: Weeks 3–4, longer when a platform needs a native backend**

Tasks:

- Implement the capture interface, including capability flags and a fake backend for tests.
- Add display enumeration where the platform provides it.
- Add window enumeration where the platform provides it.
- Implement fullscreen and display capture.
- Implement window capture where the capability exists.
- Implement region selection with the path chosen in Phase 0. Hide the application's own UI before a custom-overlay capture.
- Add multi-monitor support, including negative origins and mixed scale factors.
- Handle DPI and coordinate conversion with tests that do not need a display.
- Preserve source metadata, implement the declared SDR sRGB conversion/export policy, and explicitly reject unsupported HDR/wide-gamut paths.
- Implement permission handling, including denial and portal cancellation.
- Test X11 and Wayland as separate sessions.
- Replace file-based frame handoffs with the bounded in-memory ownership model. Test valid all-black images, malformed/oversized frames, late callbacks, device loss and monitor hotplug during selection.
- Meet overlay/clipboard latency and active-memory budgets; do not encode a canceled preview to disk or retain peak-resolution textures in idle overlay caches.

**Deliverable:** A working screenshot engine whose unsupported modes are explicit.

### Phase 3 – Desktop Integration

**Estimated duration: Week 5**

Tasks:

- Single-instance lifecycle, tray ownership, and hide-on-close.
- Global hotkeys, with an explained unavailable state on Wayland when the portal cannot bind them.
- System tray.
- Native clipboard representations from the in-memory frame, including PNG where supported. Run RGBA/PNG round-trip checks in tests rather than reading/comparing the clipboard after every production copy.
- Save the PNG into the configured folder. This path does not need SQLite yet.
- Quick-access overlay on the monitor that was captured.
- Native file dialogs.
- Notifications.
- Multi-window behavior.
- Validate hotkey conflict handling, hide/restore, no-tray fallback, IPC/single-instance races, sleep/resume and clean shutdown. Quitting must release registrations and not lose an already acknowledged save.
- Verify end-to-end latency and return-to-tray memory after repeated use; no polling loops or background capture streams may remain.

**Deliverable:** A screenshot workflow that feels like a native desktop application.

### Phase 4 – Image Editor

Direct capture editing now opens a frozen fullscreen annotation overlay through Capture and annotate, tray and independently registered Ctrl+Shift+E on Windows. Crop, redact, frame, arrow, ellipse, undo/redo and zoom operate before final output; the toolbar can move between top and bottom. One monitor under the pointer is captured per edit session (primary fallback), avoiding unused display buffers. No unedited PNG/history/clipboard output is generated. Explicit Copy only, Save only and Save and copy modes separate clipboard and file/history effects; without a crop, explicit output uses the whole display. Copy-only keeps one bounded last edited document in RAM for subsequent actions; this retained image is reported separately from the empty resident base. Cancellation leaves previous capture state intact. Editing existing files retains their originals. A short real-desktop test starts without a control window and checks overlay cancellation against a dedicated output/database directory and clipboard sequence, with windows removed and graphics detached afterward. Mixed-DPI interaction, multi-monitor topology changes and optimized latency/memory acceptance remain open; functional checks do not establish performance budgets.


Initial implemented editing slice: last-capture opaque redaction in a separate basic editor, up to 100 combined redactions/colored frames/arrows/ellipses and 100 bounded metadata-only undo/redo states including crop, Undo/Redo/Reset and scoped keyboard shortcuts, fit-to-window preview and worker-side flattened PNG/clipboard export. The editor can also open selected history entries, including imported PNGs, independently of the last capture. This explicit Edit a copy workflow keeps existing original files/history entries and says so in the editor. It does not claim original-file sanitization or deletion. Closing removes the editor image atlas entry and releases its document; production driver-memory/physical-input checks remain open. Editor crop now outlines the export area, shades excluded pixels and reports output dimensions; Full image restores the uncropped selection. Crop and redaction are rendered into one output buffer on the worker, without intermediate full-image snapshots. Colored rectangular frames now support red/yellow/green, shared selected-width preview/export border geometry, crop clipping and redaction-last layering. Arrows now support direction-preserving drag, shared preview/export vertices, RGB palette, crop clipping and metadata-only undo. Export uses one bounded output-sized RGBA overlay in addition to the output; opaque redactions are painted last. Zoom/pan now reuse the existing image texture with clipped workspace, bounded panning, DPI-aware 100% and Fit controls; document/export coordinates remain in source pixels. Elliptical outlines now share the palette, bounded undo history and source-space geometry; preview/export use matching bounded contours and the existing vector raster overlay. Select now picks existing redactions/frames/arrows/ellipses in visible paint order, shows a non-exported outline, previews bounded in-image movement using metadata and commits one undo step on release. Delete/button/Backspace removal is undoable. Hollow shapes are picked at their outlines. The shared overlay/standalone editor keeps the same 100-item/100-state limits; it adds no pixel buffers, worker queues or idle tasks. Selected redactions/frames/ellipses now resize through four corner handles; arrows expose start/end handles. Opposite anchors remain fixed, corners clamp to the image with a one-pixel minimum, and arrow endpoints cannot collapse. Preview and commit share the same source-coordinate transform, with one undo step per release. Handles use at most four small metadata entries and add no image buffers or idle work. Overlay and standalone editor now share V/X/C/R/A/E tool shortcuts and source-pixel arrow-key nudges (one pixel, ten with Shift), clamped to the image and recorded in the existing bounded undo history. Select mode exposes the current frame/arrow/ellipse color in the shared palette; recoloring is undoable, ignores no-op/invalid colors and never recolors opaque redactions. Modifier combinations remain available to existing commands. These commands add no image buffers, background jobs or idle work. An optimized metadata-only diagnostic with 100 marks and 100 undo states measured nudge-command p95 at 300 ns and recolor-command p95 at 100 ns (2,000 samples per command, development machine and timer-resolution limited); this excludes rendering/presentation and is not a reference-hardware acceptance result. Pixelate is now available in overlay/standalone editor (P), with fixed 16-source-pixel blocks, shared selection/move/resize/delete and bounded metadata-only undo. Its immutable alpha-weighted block grid is prepared lazily on a worker, cooperatively canceled on close and reused for full-resolution export. The single optional BGRA preview is bounded to 2048 pixels per side (16 MiB); the source grid and preview together fit the existing 32 MiB active-buffer allowance even for skinny admitted images. Preview has at most 100 clipped image draws and performs no image work on drag. The atlas image is dropped on close. Pixelate currently affects original source pixels below vector annotations/redactions; arbitrary effect/document ordering remains a Phase 4 release gate. Preview downsampling can soften edges; full-resolution image fixtures verify block values, alpha, crop, overlaps and redaction priority. A short release-mode 4K fixture measured preparation 7.63 ms, preview 2.96 ms and export of 100 overlapping fullscreen regions 8.32 ms; retained grid 129,600 bytes and preview 9,437,184 bytes. These single-run worker measurements are development evidence, not input/presentation or reference-hardware acceptance. Adjustable effect strengths and final visual design remain pending. A short optimized metadata-only diagnostic on the development machine measured pick/preview/handles p95 at 200 ns for both movement and resize with one and 100 marks (2,000 samples per case, timer-resolution limited); the earlier pick/preview-only run was 100/200 ns and did not include handle work. This is not a physical input/presentation or reference-hardware acceptance result.


Blur is now available in overlay/standalone editor (B): source-aligned alpha-weighted 17x17 box blur, with shared selection/move/resize/delete/nudges and bounded metadata-only undo. A worker lazily prepares one BGRA preview, bounded to 1024 pixels per side (4 MiB), by sampling the full-resolution kernel. The streaming separable filter uses a 294,912-byte ring/column buffer, not a full-resolution intermediate image. The source-effect preview/grid/scratch costs fit the existing active-image allowance; no additional image snapshots are stored in undo. Preparation is serialized with Pixelate, with one latest pending tool request and cancellation on close. Both optional atlas images are dropped at teardown. Export processes the union of clipped Blur regions against immutable source pixels (including the required neighborhood outside crop boundaries). Current fixed source-layer order is Blur, Pixelate, vector marks, opaque redactions; arbitrary document/effect ordering remains a release gate. A short optimized 4K fixture measured preview preparation 87.79 ms and export of 100 overlapping fullscreen regions 60.99 ms, with a 2,359,296-byte preview and 294,912-byte scratch. These are single-run worker measurements on the development machine, not physical responsiveness or reference-hardware acceptance. Deterministic fixtures cover edge windows, tile boundaries, alpha, preview sampling, crop seams, overlap and redaction priority. The real-desktop cancel regression now prepares both effects, verifies serialized admission, no output/clipboard changes and removed windows/graphics. Manual mouse/DPI checks and effect/presentation performance acceptance remain open.

Text is now available in overlay/standalone editor (T): click to place, native single-line Unicode input, 16/24/36 source-pixel type sizes, RGB ink palette, Apply/Enter, draft cancellation and Select → Edit text. Shared metadata transforms support move/resize/delete/nudges and undo. The native GPUI platform font system/fallbacks supplies shaping and monochrome glyph coverage on a worker; preview and export share immutable masks and source coordinates. Emoji use the selected monochrome ink. The first native test exposed an unsafe worker call into Windows color-glyph rasterization (shared D3D immediate context); the implementation now requests coverage only, with no GPU operations on workers. Three consecutive optimized native cycles passed after that correction, including Unicode/emoji, text replacement, undo/redo, source effects and output-free teardown. Warm native fixture shaping/raster was about 0.37 ms for the mixed-script label (9,376-byte mask), and 0.06–0.07 ms for the emoji-only replacement (1,056-byte mask). These are worker timings, not physical input/presentation or reference-hardware acceptance.

Text admission is bounded to 512 UTF-8 bytes total across up to four lines, a 1024×128 glyph mask and 200 retained assets / 4 MiB combined masks plus BGRA previews per document. Unreachable assets are pruned before admission; active/undo assets share pixels, and Reset/close removes atlas entries. The corresponding GPU text representation is at most 3.2 MiB, counted alongside source/effect textures. No additional font runtime or permanent timer is introduced; the caret redraws only on invalidation. The already-installed unicode-segmentation dependency is used directly for grapheme-safe input. Text preparation shares effect-job admission and uses cancellation/job identity so late jobs cannot apply canceled drafts or touch closed windows. Export remains flattened, paints text in the shared drawing order below opaque redactions, and stores no editable text in PNG/history. Mask fixtures cover clipping, alpha, resize and redaction priority; UTF-16/grapheme mapping has a deterministic test. Manual keyboard/IME, accessibility, mixed-DPI and complete presentation/memory acceptance remain release gates. Arbitrary document/effect ordering, font-family controls and adjustable effect strengths remain pending; multiline text and basic Bold/Italic styling are implemented below.

Frames/arrows/ellipses/text now have a shared creation-order stack in overlay and standalone editor, with Select → Bring forward / Send backward and undoable one-layer moves. Picking, transient previews and flattened export consume that order. Text replacement keeps its existing position in the stack; deletion adjusts references to remaining marks, and undo/redo restores order with the document. Redactions are deliberately pinned above drawings; Blur/Pixelate remain fixed source-effect stages below them. Arbitrary source-effect/document ordering is still unresolved and must not be treated as a passed release gate.

The change adds at most one order entry per drawing mark in each already-bounded metadata snapshot, with no idle tasks or image buffers for reordering. Export lazily allocates at most one vector-sized E buffer, reuses it across ordered shapes, and flushes only the regions touched by direct frame/text operations before compositing the remaining vectors once. It does not allocate or scan a full intermediate image per mark. Existing straight-alpha text compositing and untouched source pixels are preserved. A short optimized 4K comparison with 100 marks (median of three runs per case) measured 17.02 ms for grouped layers and 16.49 ms for alternating ellipse/frame layers, both using one 33,177,600-byte vector layer; this is a development worker comparison, not a claim of speedup or physical/reference-hardware acceptance. Fixtures verify order-dependent overlap, partial flush, crop, alpha, redaction priority, selection, deletion/reindexing and undo/redo. The native desktop regression draws reordered text/frame/ellipse layers and checks output-free teardown. Manual layer-control interaction, mixed DPI and complete presentation/resource acceptance remain open.

Highlight (H) is now available in overlay and standalone editor as a filled rectangle with current RGB palette color and adjustable opacity: 25%, 33%, 50%, 75% or 100% (alpha 64/85/128/191/255; default 85/255). The controls apply to new marks and selected highlights, with metadata-only Undo/Redo and the same alpha in GPU preview and export. It shares the drawing-order stack, selection, move/resize/delete, source-pixel nudges, recolor and bounded Undo/Redo. New marks inherit the current drawing color and highlight opacity. Changing opacity schedules no image work or new pixel buffers. The preview adds one GPU quad per highlight, no decoded image/texture/cache, worker or idle activity. Export composites the crop-clipped area directly into the final image with the text path's shared straight-alpha ink blend. If an earlier vector layer overlaps the area, it is flushed before blending; no image-sized highlight scratch is allocated. Opaque redaction remains the last layer. Translucent highlighting leaves content readable; use the dedicated opaque Redact tool for sanitization.

Optimized fixtures cover opaque/transparent backgrounds, untouched pixels, crop, invalid geometry/color, layer-dependent tint, vector flush, redaction priority, recolor, resize, deletion/reindexing and combined admission/undo bounds. The native regression draws a highlight, moves it below text and verifies output-free teardown. A short 4K comparison (median of three runs per case on the development machine) measured full-image copy export at 6.64 ms and full-image highlight export at 34.11 ms. This measures added worker compositing, not physical drag/presentation latency or reference-hardware acceptance. The release build and affected checks pass; manual pointer/DPI/opacity appearance and full resource/performance acceptance remain open. Adjustable opacity is implemented. Additional optimized fixtures cover all five alpha presets on opaque, transparent and partial-alpha sources, zero-opacity rejection and selected-opacity Undo/Redo including unchanged/invalid no-ops. The native regression also checks opacity Undo/Redo before output-free teardown. A serial 4K comparison (three runs per case, development machine) measured copy export at 7.93 ms and highlight export at 35.38 ms (33%), 35.29 ms (50%) and 35.31 ms (100%), with no extra image-sized scratch. These worker measurements do not establish reference-hardware, physical presentation or full memory/idle-budget acceptance; manual opacity-control interaction remains open.

Pen (D) is now available in overlay/standalone editor: drag to draw, click for a square dot, release for one undo step and Escape to cancel an unfinished gesture. It uses the current RGB palette and adjustable 1/3/6/12-source-pixel ink width (default 3) with flat caps/beveled joins. Width controls work for new strokes and selected strokes in both overlay and standalone editor; changes are metadata-only and undoable. Preview path/dot geometry, picking and export use the chosen width. Width is validated to 1..=12 at export admission; resizing retains it. Selection tests proximity to the centerline, and the shared model supports recolor, move/resize, deletion, nudges, layer order and metadata-only Undo/Redo. Resizing changes the path geometry, not the ink width. Preview and raster export consume the same source-pixel centerline/stroke parameters; opaque redactions remain the final layer.

Admission is bounded to 1,024 sampled points per gesture, 8,192 visible pen points per document and the existing 100 combined marks. Sampling suppresses duplicate/sub-two-pixel moves; release includes the endpoint if capacity remains. Reaching a cap shows an explicit status and stops accepting points; it does not queue work or silently extend storage. Accepted stroke point lists are immutable Arc slices shared by transforms and undo. At most the current 8,192 points plus 100 older 1,024-point assets can be retained by bounded history (884,736 coordinate bytes before small metadata/Arc headers); the active trace adds at most 8 KiB. There are no stroke bitmap snapshots, new textures, caches, dependencies, threads or idle tasks. Export reuses the existing single E-sized vector layer. Stroke data is released with the document and is not exported as editable PNG/history metadata.

Optimized development checks measured preview tessellation p95 at 0.165 ms for 1,024 points and 0.986 ms for 8,192 points (50 samples per case), and 4K export median at 9.47/14.77 ms respectively (three runs per case). These cover geometry/worker costs, not whole-frame input/presentation or reference-hardware acceptance. Fixtures cover centerline mapping/hit testing, crop, dots at image edges, alpha/source preservation, redaction, invalid points, admission limits and shared undo assets. The native desktop regression commits one gesture, exceeds the sampling cap in another, cancels it without changing the document and verifies output-free teardown. Manual long-stroke behavior, pointer capture across windows/monitors, DPI, visual join parity, accessibility and complete resource/frame-time acceptance remain open. Adjustable pen width is implemented; smoothing/pressure and unbounded strokes remain outside this initial tool. Width changes add one bounded scalar per pen/history entry and no image buffers, texture, worker, runtime or idle task. Point assets remain shared through Undo/Redo. Additional optimized fixtures cover line/dot widths, antialiased edges, crop parity, image-edge clipping, width-aware picking, invalid widths and unchanged/invalid no-ops. The native desktop regression draws at 12 px, changes to 6 px and checks Undo/Redo before output-free teardown. A serial optimized development comparison measured 3/12 px preview tessellation p95 at 0.168/0.212 ms for 1,024 points and 0.890/1.414 ms for 8,192 points (50 samples per case). Corresponding 4K export medians were 9.83/9.72 ms and 14.52/14.02 ms (three runs per case), using the existing single output-sized vector layer. These are geometry/worker measurements, not a reference-hardware acceptance or speedup claim. Manual width-control interaction, physical input/presentation, mixed-DPI and full resource acceptance remain open.

Number (N) now places sequential plain-text step markers (`1.`, `2.`, ...) in the frozen overlay and standalone editor, with the shared RGB palette and 16/24/36-source-pixel text sizes. The counter advances only on successful admission/placement; one metadata-only undo step restores both mark and counter. Restart numbering is undoable and leaves existing labels intact. Deleting or editing a label never silently renumbers others; Reset clears the sequence. Numbers remain ordinary editable text objects for selection, transforms, layering and flattened export. The checked 16-bit counter reports exhaustion rather than wrapping. Circular badge styling is not part of this slice.

The implementation reuses native monochrome text shaping/rasterization, the existing serialized optional image job, cancellation/session identity, and bounded shared text assets (200 assets / 4 MiB, combined 100 marks / 100 undo states). It adds only a counter in document snapshots and no optional runtime, worker, queue, cache or idle activity. Preparation immediately sets a visible busy state; rapid extra clicks while preparation is active are not queued, and source-effect preparation reports a retry message. Deterministic optimized fixtures cover sequence Undo/Redo, restart, delete/edit behavior, shared asset identity, combined admission and checked counter exhaustion. The native desktop cancel regression places two numbers, verifies counter Undo/Redo and bottom/right edge clamping, then tears down without files/history/clipboard changes. In that development run, native shaping/rasterization took 0.039 and 0.191 ms for the two labels, each with a 608-byte mask and 2,432-byte BGRA preview. These individual worker observations are not physical input/presentation, idle/memory or reference-hardware acceptance. Manual click/size/color controls, mixed-DPI, accessibility and full performance acceptance remain open.

Pen, Frame, Ellipse and Arrow now share Width controls (1/3/6/12 source pixels, default 3) in the overlay and standalone editor. New marks inherit the current drawing width; selected marks can change width with bounded metadata-only Undo/Redo. Width persists through transforms, recolor and deletion/restore. Frame segments, ellipse contours and arrow polygon geometry are shared by preview/export; arrow heads scale with width and shrink for short arrows. Picking uses the chosen frame/ellipse inset border and the actual arrow polygon with tolerance, including broad heads. Export rejects zero/out-of-range widths. No image job, pixel snapshot, cache, runtime, thread or idle activity is added for width changes.

Width/crop fixtures exposed a pre-existing tiny-skia antialiasing seam when vectors touched crop edges. Export now keeps at most a one-pixel raster gutter on the existing shared vector layer, clamped to original source bounds, and composites only the requested output. It allocates no second vector buffer or full-desktop intermediate for this correction. Gutter bytes are capped at 4 MiB and count within the existing 32 MiB active scratch allowance; extreme long/narrow vector crops exceeding that cap receive a recoverable error before vector allocation. Full-image vector-layer size is unchanged. Fixtures compare exact full-image/cropped pixels for all width presets, including one-pixel crops and source edges, and verify bounded padding admission for skinny images without allocating them. Existing partial vector flush, alpha, text/highlight ordering and opaque-redaction checks remain in place.

A serial optimized development comparison with 100 marks measured 4K export medians at 16.31 ms for grouped 3 px shapes, 16.14 ms for interleaved 3 px shapes and 16.43 ms for interleaved 12 px shapes (three runs each, one 33,177,600-byte vector layer for full-image output). Width-change metadata p95 with 100 marks/100 undo states was 300 ns across 2,000 commands, timer-resolution limited and excluding presentation. These are development worker/metadata observations, not reference-hardware acceptance or a speedup claim. The native cancel regression changes frame/ellipse/arrow widths, checks Undo/Redo and tears down without output side effects. Manual width controls, broad-head/short-arrow appearance, mixed-DPI, accessibility, physical presentation and full memory/idle acceptance remain open.

Select now supports Duplicate via button and scoped Ctrl+D (Cmd+D binding on macOS) in the frozen overlay and standalone editor. Every current mark type is copied, selected and shifted by ten source pixels where space permits, with inward/clamped placement near image edges and direction-preserving arrow translation. Drawing copies go to the front; source effects/redactions keep their fixed stages. Duplication is one bounded metadata undo step and preserves style, shared text/pen asset identity and the numbering counter. It creates no image buffers, textures, workers, queues or idle work. Admission enforces the combined 100-mark and 8,192-pen-point limits and the existing cumulative 4 MiB export-mask limit, counting repeated text references even though their assets share pixels. New/replaced text and numbered-label admission now checks that same export limit before recording success, so later edits cannot invalidate a duplicated document; rejected operations leave history/counter unchanged. Controls explain resource limits and reject duplication during drags, text drafts or text preparation.

Optimized fixtures cover all mark types, style/asset/order preservation, reversed arrows, edge/full-image placement, invalid selection/image bounds, single-step undo/redo/deletion restore, combined capacity, pen/text limits and text replacement/number admission after duplication. The native desktop cancel regression duplicates each type and verifies selection/Undo/Redo before output-free teardown. A short development metadata comparison (2,000 samples each) measured duplicate-command p95 at 300 ns with one mark and 600 ns with 99 marks, timer-resolution limited and excluding rendering/presentation. These are command observations, not full memory/idle, physical latency or reference-hardware acceptance. Manual shortcut/button, mixed-DPI, accessibility and macOS/Linux platform acceptance remain open.

Shift constraints are implemented for new drags in the frozen overlay and standalone editor: square rectangle-based regions (Frame/Highlight/Redact/Blur/Pixelate/Crop), circles for Ellipse and 45-degree steps for Arrow. Geometry is computed in source pixels with the initial corner/tail anchored and bounds clamping that preserves the constraint; unconstrained helper calls retain their prior behavior. Preview and release use the same core geometry functions. Native modifier-change events invalidate only a running constrained-tool drag, allowing Shift to toggle without pointer motion; release uses the mouse event modifier state. No constraint state is stored in undo: the resulting geometry uses the existing single-step mark/crop history. Existing selection transforms and Pen are unchanged. The path adds no heap/image allocation, worker, cache, timer or idle activity.

Optimized fixtures cover positive/reverse/axis-aligned drags, source bounds, skinny/degenerate images, fractional/anisotropic coordinate mapping, exact square dimensions, arrow axis/diagonal direction, invalid inputs and unchanged unconstrained results. A short development comparison of square plus arrow geometry (2,000 samples per case, 4K source at a 1920x1080 viewport) observed p95 at 100 ns both with and without constraints, timer-resolution limited and excluding presentation. The existing native output-free overlay cancel regression is retained; it does not simulate Shift/mouse combinations. Manual modifier press/release during drag, mixed-DPI, accessibility, physical input/presentation and full platform/resource acceptance remain open.

Spotlight (S) now adds a rectangular focus area with dimmed/tinted surroundings in overlay and standalone editor. It reuses the highlight annotation with an outside-fill flag, default Black at 191/255 opacity, separate small creation-style values, shared opacity presets and a Black palette choice. Focus selection, transforms, recolor, opacity, duplication, drawing order and Undo/Redo use the existing shared document path and combined 100-mark limit. Shift constrains the focus to a source-pixel square. Spotlights dim earlier drawing marks, later marks remain above them and multiple spotlights compose in order; opaque redactions remain last. This visual emphasis tool must not be presented as sanitization.

A shared source-coordinate helper returns at most four clipped, non-overlapping rectangles around the focus. GPUI paints those quads in the existing drawing pass; export flushes only earlier vector pixels they cover, then blends directly into the final output. There is no spotlight image/texture/mask, preparatory worker, cache, queue, thread, runtime or idle activity. The added document flag/style scalars and four stack rectangles are bounded metadata; up to 100 spotlights would draw at most 400 quads. Fixtures verify exact outside coverage without corner double-blending, inside/outside/partial crops, untouched focus/alpha, ordered vector flush, stacked spotlights, redaction priority and the shared selection/opacity/recolor/resize/reorder/duplicate/delete/admission behavior. The native cancel regression includes a spotlight and opacity Undo/Redo plus duplication before output-free teardown.

A serial optimized development comparison (three runs per case) measured 4K copy export median at 6.60 ms, full-image 33% highlight at 34.21 ms and a 75% Black spotlight with a centered 1920x1080 focus at 27.30 ms; the spotlight adds no image-sized scratch. These worker observations do not establish reference-hardware or presentation acceptance. Dense multi-spotlight export/frame-time, manual control/appearance, mixed-DPI, accessibility and full memory/idle/platform acceptance remain open.

The shared straight-alpha ink compositor used by Text, Highlight and Spotlight now has exact fast paths for opaque ink and fully transparent/opaque destinations. General partial-alpha blending remains the same. Fully transparent ink still preserves hidden source RGB. The change reduces worker arithmetic/divisions and adds no buffer, allocation, cache, thread, runtime, queue or idle work; GPU preview/input behavior and all resource budgets remain unchanged. An optimized reference fixture checks all 65,536 source/destination alpha pairs with four color/background combinations against the general equation (262,144 exact RGBA comparisons), alongside existing crop, layer, vector-flush and redaction fixtures.

A serial optimized development before/after comparison used the same 4K fixture and three samples per case (medians, including output copy):

| Worker export case | Before | After |
|---|---:|---:|
| Copy only | 7.03 ms | 6.78 ms |
| Full-image 33% highlight | 35.80 ms | 22.75 ms |
| Full-image 50% highlight | 34.28 ms | 22.62 ms |
| Full-image opaque highlight | 34.34 ms | 13.56 ms |
| 75% Black spotlight, centered 1920x1080 focus | 27.51 ms | 18.63 ms |

These are short development worker observations, not end-to-end latency, physical presentation, reference-hardware acceptance or a claim of lower idle CPU. Source/output/scratch memory representations are unchanged. The native output-free overlay regression remains part of verification. Dense multi-effect/frame-time, complete reference-machine resource/latency and platform acceptance remain open.

Frame and Ellipse now share Outline/Filled controls for new marks and selected shapes in overlay and standalone editor. The fill flag uses the existing bounded annotation/history metadata. Toggle is a single undo step, unchanged/invalid requests do nothing, and style survives transforms, recolor, layer moves, duplication and deletion/restore. The stored outline width is retained and its controls are disabled while filled. Selection follows the filled rectangle/ellipse interior or the outline as appropriate. Shift constraints and existing combined 100-mark/100-state limits remain unchanged. Preview/export use the shared filled rectangle segments or outer ellipse contour; opaque redactions remain last. Filled drawings do not replace the dedicated Redact workflow.

This adds one creation-style flag and one annotation flag, no image/texture/mask, runtime, thread, worker, cache, queue or idle task. Filled rectangles paint one GPU quad and export directly; ellipses reuse the existing shared vector layer and only their outer contour. Optimized fixtures cover interior coverage/picking, untouched source alpha/pixels, exact crop comparison, layer order, redaction priority, outline-width preservation, shared editing/undo and invalid requests. The native cancel regression toggles both shapes with Undo/Redo before existing duplication and output-free teardown. A serial 4K comparison with 100 shapes (three samples each) measured interleaved 3 px outline export median at 17.14 ms and filled-shape export at 18.17 ms, using the same 33,177,600-byte vector layer for full-image output. Fill-toggle metadata p95 was 300 ns across 2,000 commands with 100 marks/100 undo states, timer-resolution limited and excluding presentation. These are development observations, not physical input/presentation or reference-hardware acceptance. Manual controls/appearance, mixed-DPI, accessibility and full performance/platform acceptance remain open.

Text and Number now expose combinable Bold/Italic controls beside the existing size presets in overlay and standalone editor. Select -> Edit text restores the stored style flags, and applying a change prepares a new native asset with one document undo step. Undo/Redo, duplication and transforms preserve the style metadata and shared glyph assets. Regular remains the default; default-family weight/style resolution uses the existing native font system (normal/bold x normal/italic), without another font engine or a dependency. The typing widget remains an input field; the chosen screenshot style is applied when text is committed. The actual glyph mask/preview carries the style into the existing flattened export path; no editable text/style metadata is embedded in output.

Preparation remains serialized with text/effect work, with existing cancellation/session checks, 512-byte input, 1024x128 mask and 200-asset/4 MiB image-asset bounds plus the cumulative export-mask limit. This adds two creation flags and two flags per retained asset; requested native font variants initialize on demand and reuse the existing native caches. It adds no thread, queue, timer or additional image representation. Native font-cache memory still requires reference-machine measurement. The optimized native regression shapes the same ASCII fixture with all four combinations and verifies distinct masks, dimensions and asset metadata on the worker; it also checks regular-to-bold-italic text edit Undo/Redo and styled numbers before output-free overlay teardown. A deterministic fixture checks styled-asset sharing through Undo/Redo, duplication and resize.

In one optimized development run, individual native worker observations for Regular/Bold/Italic/Bold Italic on the fixed 24 px fixture were 1.06/4.74/3.57/4.48 ms, with 3,104/3,328/3,040/3,296-byte masks and BGRA previews four times those sizes. These are individual worker observations, not cold-start, percentile, physical input/presentation or reference-hardware acceptance. Manual style controls, IME, fallback appearance, mixed-DPI, accessibility and complete resource/performance acceptance remain open. Multiline input is implemented below; font-family selection remains pending.

Text now supports up to four explicit lines and 512 UTF-8 bytes total in overlay and standalone editor. Shift+Enter inserts a newline; Enter/Apply commits the block. Blank/trailing lines and left alignment are retained. A worker shapes each line through the existing native font system and assembles one mask/preview with native per-line metrics. The existing 1024x128-pixel mask bound is unchanged: overly tall blocks (including some large-font/four-line combinations) fail recoverably, leaving the draft available for correction. No automatic wrapping, extra image representation, font engine, job queue, timer or idle activity is introduced. Text/effect serialization, cancellation/stale-job checks, asset/export limits and metadata-only document undo remain in force.

The input holds at most four shaped lines plus one cached <=512-byte text value, uses a bounded 24-pixel row layout (maximum field height 104 logical pixels), and notifies the parent only when row count changes so the image workspace follows the field height. Navigation, selection/marked-text painting, pointer hit testing and native IME bounds now use row-local byte indices with UTF-16 conversion at the native boundary. Home/End navigate within a row, Ctrl/Cmd+Home/End across the block, and Up/Down use shaped horizontal positions with a grapheme-safe fallback when layout is stale. Clipboard/composition CRLF/CR normalizes to LF; corresponding UTF-16 selections are normalized too. Oversized paste is rejected before normalization allocation, and excess lines are rejected before text/history mutation.

Optimized deterministic checks cover UTF-16/newline ranges, empty/trailing rows and CRLF offset mapping. The native worker fixture compares identical row glyph pixels across blank/trailing-line blocks and checks height/line admission. The existing desktop cancel regression now paints a three-row mixed-script input including a blank row, exercises Home/Up/Down/End, Shift+Enter and input undo, rejects a fifth line, verifies CRLF composition selections and row-aware pointer/IME bounds, edits/undoes the multiline label and still tears down without output changes. Individual native worker observations in one optimized development run were 0.035 ms for the one-row fixture, 0.050 ms for three rows with an empty middle row, and 0.024 ms for a trailing-empty-row block; masks were 682/2,046/1,364 bytes. The three-row mixed-script label took 0.211 ms with a 19,104-byte mask. These are worker observations, not frame-time/input percentiles or reference-hardware acceptance. Manual keyboard/IME/clipboard workflows, RTL/fallback appearance, mixed-DPI, accessibility and full resource/presentation acceptance remain open.

**Estimated duration: Weeks 6–7**

Tasks:

- Build the editor interface.
- Implement the annotation model.
- Add crop.
- Add arrows.
- Add rectangles and ellipses.
- Add text.
- Add blur and pixelate.
- Implement undo and redo.
- Implement image export.
- Add opaque redaction and consistent text shaping/export. Test layer ordering, preview/export parity, sanitized clipboard/upload/history paths, and bounded undo memory.
- Meet drag/frame-time budgets while background export/filter work is running; release document buffers/textures on close.

**Deliverable:** A functional screenshot editor.

The shared overlay/standalone editor now includes Line (L), a straight stroke built from exactly two PenAnnotation points. Preview and release use the same source-coordinate/Shift snapping helper as Arrow, preserving horizontal/vertical/45-degree directions and clamping endpoints to the image. Lines inherit the pen renderer/exporter, flat caps, color palette, 1/3/6/12 widths, creation/reorder stack, shared point assets, selection/hit-testing/bounding-box transforms, duplicate/delete and bounded metadata undo. Zero-length drags create no mark; failed admission shows the shared limit state. There is no second line raster backend or document variant. Endpoint handles are implemented below; separate numeric-angle controls remain outside this slice.

Each committed line adds a 16-byte two-point payload plus existing Arc/annotation metadata shared by undo, within the combined 100-mark/8,192-point limits. The live preview builds only one tiny stroke path on invalidation. There is one extra toolbar button, no image allocation, new texture, worker, queue, dependency, timer or idle work. Optimized tests cover snapping/reverse/clamped geometry, no arrowhead in export, redaction precedence, unchanged source pixels, hit-testing, color/width undo, shared duplication, deletion and mark admission. The real desktop cancel regression now paints a line, exercises undo/redo, and still produces no file/history/clipboard changes. One 100-sample optimized geometry/path probe observed 0.9 microseconds p95 for Line and the same for the existing two-point Pen baseline. This excludes actual input/presentation and does not establish reference-hardware UI or full memory acceptance. Manual Line pointer/Shift interaction, mixed DPI, accessibility and complete platform/resource gates remain open.

Two-point strokes (including Line) now expose start/end grips in Select in both overlay and standalone editor. Endpoint dragging uses the existing source-coordinate hit-testing, live metadata preview and one-checkpoint-on-release flow; longer pen strokes/dots retain their bounding-box handles. Moving an endpoint preserves color, width and layer order, clamps to the image and rejects coincident endpoints. Geometry is snapped to source pixels; a legacy fractionally resized two-point stroke is normalized to that grid when its endpoints are edited. Undo retains the original shared point asset and redo restores the edited asset. Arrow endpoint clamping also uses saturating arithmetic so extreme finite deltas cannot wrap.

The incremental cost is two grip descriptors instead of four corners and a new two-point (16-byte payload) immutable asset per changed preview/commit, shared with bounded undo. No bitmap snapshots, image/texture buffers, cache, worker, dependency, timer or idle work are added. Optimized fixtures verify grip picking, fixed opposite endpoints, metadata-only preview, a single undo checkpoint, asset restoration, style/order preservation, collapse rejection, extreme/nonfinite/empty-image cases and unchanged multi-point handles. The native cancel regression edits a line endpoint and undo/redo before closing without output/history/clipboard changes. Manual endpoint pointer/zoom/DPI interaction, accessibility and full platform/resource gates remain open.

An optimized 100-sample metadata-preview/grip probe observed 0.4 microseconds p95 for Line and 0.3 microseconds for the existing Arrow baseline on the development machine. It excludes actual pointer input, rendering/presentation and process/GPU memory; it is not reference-hardware acceptance evidence.

### Phase 5 – History and Sharing

Implemented initial slice: new successful PNG saves and Save As exports enter a migrated local SQLite/WAL metadata store. Capture history loads lazily in bounded pages (20 visible rows plus one lookahead), with Open/Show file, bounded on-demand Copy, and metadata-only Remove entry. Listing performs no image decoding or eager startup query. Removing an entry keeps its image file; image deletion and retention are separate pending workflows. Metadata writes follow completed file publication and report failures independently. Word-prefix metadata search and source filtering are implemented below. Automatic viewport thumbnails, arbitrary substring search, automatic missing-file reconciliation, deletion/retention, crash recovery and the full phase gate remain open. Source classification currently uses capture for app outputs and import for manually added PNGs. Manual PNG import is limited to 100 explicit selections, validates bounded image metadata, skips duplicate exact paths, reports file failures and commits references without modifying original files.


History now supports All/Captures/Imports source filters with keyset pagination on the worker. Captures means app-saved outputs (including Save As); Imports means manually added references. Latest/Older retain the chosen filter, and a filter change resets to its latest page. Schema version 2 adds only the non-destructive captures(source,id DESC) index, preserving version-1 rows/files and the existing unfiltered API. Filtered reads use that index and retain the 21-row result limit (20 visible plus lookahead); no image file/header/thumbnail decoding or folder enumeration occurs during listing.

The UI keeps at most one active history fetch and one coalesced latest refresh. Filter changes invalidate the request generation and set its operation-local cancellation flag; cancellation is checked before storage work, after opening/migration and while collecting rows. An executing SQLite call/migration may finish before cancellation is observed; generations also prevent its late rows/errors from affecting a later filter or hidden history. History writes/import selection disable filter changes. Pending refresh state is consumed when a new fetch starts. Hiding history or retiring the control window cancels requests, drops loaded metadata and clears pending fetches without resetting the chosen filter. There is no startup/idle fetch, timer, extra worker/runtime, query backlog, thumbnail cache or pixel buffer. Existing 250 ms SQLite busy timeout, FULL durability/WAL and 512 KiB page-cache setting are unchanged; the page-cache setting is not a total process-memory measurement.

Optimized fixtures verify version-1 upgrade preservation, mixed-source filtering, lookahead/older-page boundaries without gaps/duplicates, empty pages, actual source-index query plans without a temporary ordering tree, and pre-open/mid-read cancellation with no partial page. Synthetic entries point at nonexistent images, confirming metadata-only listing. A native headless executor test rapidly switches filters, checks that only the latest result appears, cancels while hiding, reopens on the retained filter and checks write-time guards. It opens no windows, changes no clipboard data and preserves synthetic PNG contents.

A short optimized development run with 10,000 synthetic rows observed source-index migration at 7.95 ms and 180,224 bytes of additional logical database pages (not total WAL/disk footprint). Across 25 page fetches per case, p95 including connection/open/schema checks and collection was 1.08 ms for All, 1.02 ms for Captures and 0.97 ms for Imports; each returned at most 21 metadata rows. These are warm development storage observations, not reference-hardware, presented-input or complete memory/idle acceptance. Manual filter controls, concurrent-write/error recovery, large-history migration interruption, accessibility and full platform/resource acceptance remain open.

History entries now offer Relink PNG… through a single-file native picker. The explicit selection validates PNG dimensions and the existing decoded-size admission limit on a worker, then atomically replaces only the path/dimensions/file-size reference and invalidates thumbnail metadata. Entry ID, original creation time and source classification remain unchanged; classification describes the original entry, not a content/provenance check on the chosen replacement. No image files are moved, overwritten, copied or decoded into full frames. Exact paths already referenced by another entry are rejected. An optimistic ID/path/creation-time check rejects stale or removed entries rather than silently applying an old dialog result.

Relink uses the existing single history-write admission and SQLite durability/cache settings. A canceled picker performs no relink validation or database write; hiding history invalidates a still-open picker's result, clears action text, and does not let late completion reopen UI or apply stale status. A committed worker operation may finish after history closes; completion refreshes only the current visible history. No scans, watchers, timers, new threads, dependencies or pixel caches are added. Optimized synthetic tests cover moved-file recovery, duplicate-path/header failures, stale/removed references, updated dimensions, retained source/date/ID and unchanged image bytes. A development observation for header validation plus durable relink was 3.38 ms; this single sample is not a reference-hardware performance result. The headless native regression checks stale/missing/busy picker guards. Actual native-picker interaction, unavailable/network storage and full DPI/accessibility/resource acceptance remain manual gates. Automatic reconciliation, image deletion and retention remain pending.
History now has live word-prefix metadata search over file paths (including names), stored dates and source. All entered words must match; punctuation separates words and FTS operators are treated as literal words. Queries accept at most 512 UTF-8 bytes and 16 words. All/Captures/Imports, Latest/Older, multi-selection and relink keep working with search results. Upload URLs remain outside this slice because uploads are not implemented. Arbitrary substring/OCR search remains pending.

Schema version 3 adds an external-content FTS5 index, backfills existing metadata and maintains it with transactional insert/update/delete triggers. Tests check upgrades from version 2, update/delete/rollback consistency, literal query handling, Unicode/case-insensitive prefixes, source/date combinations, stable keyset pages, and actual FTS plans without a temporary ordering tree. No source images are read while searching. The existing unfiltered and source-filtered APIs remain available. Index maintenance adds local disk/write work to metadata transactions. The rusqlite hooks feature enables an operation-local SQLite progress callback: searched queries check cancellation every 1,000 VM instructions and stop after a 250 ms execution deadline. Opening/migration and blocking filesystem/SQLite busy waits are outside that execution deadline; large migrations and unavailable storage still need their production gates.

The UI reuses the native annotation text field in single-line mode, retaining UTF-16/IME, selection, clipboard editing and bounded input undo without a second widget implementation. It creates the field only when history is shown, limits text to 512 bytes and 100 input-undo strings (at most 51,200 text payload bytes, plus metadata/layout), and releases the field/query when history closes. Escape clears search. Deferred input callbacks check field identity, so a discarded field cannot alter a reopened history. Search changes cancel obsolete work, clear selection, and coalesce behind the single active fetch/write; there is no polling/debounce timer, new worker/thread/runtime, eager query, pixel buffer or thumbnail cache. The native regression covers rapid query/filter changes and latest-result application, query/field cleanup, and single-line/Unicode/byte/undo admission; the annotation regression still passes after sharing the input. Actual search-field keyboard/IME interaction, DPI/accessibility and full memory/latency acceptance remain manual gates.

One optimized development run with 10,000 synthetic entries observed the combined version-1-to-3 source/search-index migration at 22.57 ms with 1,196,032 additional logical database bytes (not total WAL/disk footprint). Across 25 reads per case, unsearched page p95 was 0.88–0.90 ms; searched page p95 was 1.17 ms for a common prefix, 2.16 ms for source/date words, and 0.96 ms for no matches. Results retained at most 21 metadata rows. SQLite's 512 KiB page-cache setting is unchanged, but does not bound all internal FTS allocations or establish total process memory. These are development observations, not reference-hardware, physical-input or production resource acceptance.
History offers an explicit Preview/Hide preview action per displayed entry. It prepares at most one image on the existing background executor and retains only one aspect-preserving thumbnail, at most 400×240 BGRA pixels (384,000 CPU pixel bytes and the corresponding uploaded image, plus toolkit/driver overhead). Small PNGs are not enlarged. The original PNG, clipboard, database and last capture remain unchanged. Missing/corrupt/oversized images show a preview error and remain available for explicit relinking/removal. No thumbnail file, persistent cache, folder scan, automatic decode, timer, thread or dependency is added. Automatic viewport thumbnails/prefetch remain a separate pending slice.

Preview decoding shares the existing image-job admission via the capturing flag, so it cannot overlap capture/editor image preparation, recopy or saving; duplicate requests receive busy admission rather than queuing. The existing PNG decoder and 256 MiB decoded-size admission are reused. It temporarily decodes the full original on the worker, then releases it before publishing only the thumbnail. Cancellation is checked before opening, after decoding and after resizing; decoding/blocking filesystem calls are not interruptible mid-stage, and unavailable storage/peak decoder allocations remain production gates. Closing the control window may retire graphics while this read-only worker finishes. Search/filter/page changes, hiding history, replacing/hiding a preview and quitting discard previews and mark obsolete work canceled. Callbacks check both page generation and cancellation, so canceled work cannot repopulate a later page. Uploaded images are explicitly dropped on the control window; no preview survives return to tray.

Optimized fixtures verify landscape, portrait, extreme aspect ratio, small-image/alpha preservation, the thumbnail byte cap, pre-open/post-decode/post-resize cancellation, invalid PNG failure and unchanged files. The native headless history regression checks a canceled preview across filter/hide, successful publication, BGRA pixels, source dimensions, single-job admission and image release on Hide preview, with no windows or clipboard changes. A five-sample warm synthetic 4K development comparison observed median decode-only 62.29 ms versus decode+thumbnail 67.71 ms. The 33,177,600-byte RGBA source is temporary; the 400×225 retained result is 360,000 bytes. This is not a latency distribution, physical presentation, process/GPU memory or reference-hardware acceptance result. Visible preview layout, GPU reclamation, DPI/accessibility and unavailable-storage behavior remain manual gates.
Selected history entries can be exported with Export selected… to one explicit native-picked folder. The action snapshots up to 20 current-page paths before the dialog; canceled/stale dialog results write nothing. Once copying starts, navigation may continue while the bounded snapshot finishes. Cancel export remains outside the history panel, and quitting requests cancellation before waiting for the worker to clean up. Results distinguish exported, failed and not-exported counts, show at most three file errors, and preserve completed copies after later failure/cancellation. A per-file error does not discard earlier successful copies. Identical basenames/existing targets are reported as conflicts, never overwritten or silently renamed. Exports are local byte copies, including existing PNG metadata; they do not sanitize/re-encode imported files, alter originals/clipboard/last capture, add duplicate history records or start uploads.

The existing saved-PNG export/publication path now supports operation-local cancellation with one 64 KiB streaming buffer. It retains PNG signature/encoded-size admission and writes to a unique temporary file, syncs it, checks cancellation, then publishes without replacement. Checkpoints run before opening, between chunks and immediately before publication after sync. Saved-PNG exports (including Save As) also have a 60-second per-file deadline checked at these checkpoints; blocking native filesystem calls are not interruptible mid-call, so this is not a hard I/O timeout. A canceled in-progress file is not published; best-effort temporary cleanup retains the existing warning on cleanup failure. Export uses the existing single saving-job admission and parked worker executor. There is no pixel decoding, per-file task queue, timer, runtime, cache, new dependency or idle work; at most 20 source paths and 20 failure records are retained.

Optimized tests cover admission/cancel-before-storage, partial success, missing/invalid PNGs, existing-file preservation, original bytes, cancellation between files and after streaming has started, and cancellation immediately before publication with no leaked part files. The native headless fixture checks worker completion, truthful status, released job state, exactly the chosen fixture copy, and unchanged history/last capture/clipboard. One sequential development diagnostic copied twenty 886,733-byte PNGs: individual calls took 173.60 ms, followed by the selected-export wrapper at 77.49 ms. Different cache/directory warmup and single samples prevent claiming a speedup or latency distribution. The relevant bounded cost is one 64 KiB application copy buffer without decoded images; process memory, disk-full/permission errors, interrupted cleanup, slow/network filesystems, native folder-picker interaction and full platform/resource acceptance remain production gates.
An initial anonymous HTTP uploader (2026-11-09) is available from the control window: Load destination…, Upload saved PNG, Cancel upload, Forget destination and explicit Copy link. A strict JSON configuration (at most 16 KiB) supplies a URL (at most 2,048 bytes), multipart field (1–64 safe ASCII characters), JSON pointer (at most 256 bytes) and explicit allow_http flag. Default requests require HTTPS. Credentials/userinfo, URL queries/fragments, control characters, invalid pointer escapes and unknown fields are rejected; no secrets are stored. Loading another profile initially changes the session only and clears the old destination/link before validation; explicit persistence is implemented below. Authentication beyond Windows Bearer/X-API-Key/Basic credentials, macOS/Linux secret storage, HTTP methods beyond POST/PUT, custom headers/parameters, proxy integration remains pending; response-header/plain-text extraction and upload history are implemented below; this does not complete the generic uploader or Phase 5 gate.

Only a manual action uploads the current saved-PNG path snapshot; capture, selection, annotation and opening history never upload. The UI identifies the source filename and destination origin, keeps one active upload, and does not alter originals, image history, last capture or clipboard. Link copying is separate and updates clipboard status. A later capture may proceed while an upload is active. The source streams as a size-limited PNG with the generic multipart filename screenshot.png; bytes/metadata are preserved, so this is not an imported-file metadata sanitizer. Redirects, automatic retries, decompression and idle HTTP connection pooling are disabled. HTTP/1 is used in this first slice. Explicitly configured anonymous HTTP endpoints can serve local/self-hosted installations; no endpoint is inferred from file contents or server responses. The returned link must be an absolute HTTP(S) URL without embedded credentials, is limited to 2,048 bytes and is never automatically opened/copied. A missing/bad link is distinguished from transfer success; failed/canceled requests can already have reached the service and do not promise remote deletion.

Network work runs on a GPUI background worker with a per-operation Tokio current-thread runtime and at most one blocking filesystem helper thread. Runtime, client, response and stream are dropped after completion; no optional runtime/network traffic starts on launch and no upload timer/client remains idle. Request file buffering and ReaderStream capacity are each 64 KiB; streaming is capped at the opened file length and existing encoded-size admission. Connect timeout is 5 seconds, overall operation deadline 60 seconds, and response accumulation at most 1 MiB with decompression disabled. JSON parsing and native HTTP/TLS buffers add bounded input-related memory but their full process footprint is not measured. Native filesystem/DNS/TLS initialization calls are not interruptible mid-call; runtime teardown may wait for an outstanding blocking operation. Unavailable filesystems therefore remain a lifecycle gate rather than claiming a hard 60-second I/O bound. Cancellation uses a native asynchronous notification, including cancel-before-start; there is no application polling, unbounded upload queue or per-packet UI update. Quitting requests cancellation and waits for operation cleanup. Config/file work is off the foreground executor and stale callbacks never open windows. The new direct tokio-util I/O dependency reuses the reqwest streaming dependency; stream/fs features introduce no permanently running service. Bounded event-driven byte progress is implemented below; server-side processing/acknowledgment progress remains outside this client.

Optimized tests use only synthetic temporary PNGs and loopback HTTP servers. They verify strict configuration/secret rejection, multipart field/content/filename, unchanged source, returned URL validation, missing/invalid JSON links, declared/chunked responses above the 1 MiB cap, blocked redirects without contacting their target, cancellation while awaiting a response and cancellation before work starts. A native headless integration verifies disabled states, duplicate-upload admission, successful UI link publication, unchanged capture/clipboard, and no graphics/windows. A single local observation including runtime/client setup and teardown was 1.41 ms for a tiny PNG; it excludes real HTTPS/WAN/DNS costs and is not a latency/resource acceptance result. TLS/service interoperability, native configuration picker and link-copy interaction, timeout/slow-filesystem behavior, post-upload CPU/memory return, accessibility and full supported-platform acceptance remain manual gates. No real screenshot or external service was used for verification.
Upload destinations now support explicit Remember destination and Forget destination. Only the validated, credential-free profile is serialized into upload-destination.json beside the existing app settings; imported source files and other settings are untouched. Saving uses a unique same-directory temporary file, sync and replacement rename, with cleanup on failure. Validation failure preserves the old profile. Restore reuses strict 16 KiB configuration admission, rejects malformed/unknown/secret-bearing fields and never uploads. A missing profile is normal. Forget disables this session immediately and removes only the saved profile; failure explicitly warns that it could return after restart and permits retry.

Restore runs once on the background executor at the first control-window opening (or initial windowed shell), independently of capture settings/hotkey readiness. Resident tray startup does not read the upload profile. Profile read/write/remove uses the existing single configuration-operation admission; controls cannot race replacement/forget or an active upload. Tests bypass user-profile restoration. There is no network runtime, filesystem watcher, polling, new thread/cache/dependency or repeated work on reopening the control window. The bounded resource cost is a profile read of at most 16 KiB, a few KiB of validated session strings and one short requested storage job. Optimized isolated fixtures verify first save, replacement/restart, validation failure preserving bytes, malformed restore, repeated forget, no temporary-file leak, blocked storage and preservation of other settings. Native button/picker integration, permission/crash/disk-full cases, actual storage latency and complete resource/platform acceptance remain open.
HTTP upload response extraction now supports the existing JSON-pointer format plus response: {type: text} and response: {type: header, name: Location}. Existing profiles retain JSON behavior, including root JSON strings with an empty pointer. New modes omit json_pointer; conflicting choices, unknown fields and malformed/oversized header names are rejected before a request. Text responses use strict UTF-8 and trim surrounding whitespace. Header lookup is case-insensitive, rejects missing/repeated/non-text values, requires a successful HTTP status and returns without consuming the body. Declared body lengths above the existing cap are still rejected. Header extraction does not follow 3xx responses.

All modes share the same bounded HTTP(S) link validation (2,048 bytes, no controls/embedded credentials), explicit Copy link, cancellation and deadlines. Text reuses the 1 MiB body cap; header mode avoids response-body buffering/JSON parsing entirely. No runtime, dependency, queue, timer, idle work or extra image buffers are added; profiles add only an optional response selector and at most 64 header-name bytes. Tests cover legacy configuration, persistence of both new modes, ambiguous settings, header-name validation, shared unsafe-link rejection and real loopback extraction for whitespace-padded text, case-insensitive headers, absent/duplicate headers and invalid links. Existing redirect, cancellation and oversized/chunked response tests still pass in optimized builds. These synthetic checks do not establish real-service compatibility, WAN latency, process memory or complete platform acceptance. One optimized synthetic loopback observation including runtime setup/teardown was 1.50 ms for text and 1.45 ms for headers, versus 1.40 ms for JSON in the same development session; single local observations do not establish a speed difference or latency distribution. Example configurations for JSON, text and header extraction are supplied in docs.
The HTTP uploader now supports method PUT for raw PNG bodies, alongside the backward-compatible default POST multipart request. Missing method defaults to POST; missing field defaults to file. Method names are restricted to uppercase POST/PUT, and the field setting only applies to POST. PUT sends image/png and the opened file's known Content-Length, streams exactly that bounded length and includes If-None-Match: * as a creation-only precondition. Conforming servers reject existing resources with 412; actual server enforcement remains an interoperability gate, and no remote overwrite/automatic retry option is provided. UI progress identifies the request method. JSON/text/header response extraction, explicit link copying, file/response limits, deadline/cancellation, redirect blocking and credential restrictions are shared unchanged. A PUT example profile is supplied in docs.

Optimized isolated tests verify default/unsupported methods, remembered PUT profile round trips, the exact raw PNG request body, MIME type, creation precondition, unchanged source bytes and truthful 412 failure. Existing POST, response-mode, cancellation and redirect tests still pass against loopback servers only. In one development observation for a tiny synthetic PNG, PUT including runtime teardown took 1.89 ms versus POST at 1.77 ms; single local observations do not establish a performance difference or WAN latency. Both paths use the same bounded file stream/runtime with no extra image buffer, task, dependency, timer or idle work; PUT avoids multipart framing. Real HTTPS/PUT service compatibility, precondition enforcement and complete resource/platform acceptance remain open.
Successful HTTP uploads now record the validated returned URL, source filename (not full local path), destination origin and timestamp in the local history database. Schema version 4 adds a separate uploads table with bounded metadata and monotonic AUTOINCREMENT IDs; capture rows/search indexes are preserved. Upload history is independent of local image availability and destination profiles. Returned URLs can include signed access links and are retained locally as such; forgetting a profile does not delete upload records. A failed metadata write never converts a successful upload into a failed transfer or discards its link. Canceled/failed transfers without a validated result do not create successful-upload entries. There is no remote deletion action.

The control window offers lazy Upload history, Latest/Older, explicit Copy link and metadata-only Remove entry. Queries use primary-key keyset pagination, at most 21 metadata rows (20 visible plus lookahead), without image/file enumeration, decoding, DNS or networking. Each history panel admits one query and coalesces stale reopen/refresh work through generation checks; hiding or returning to tray cancels/discards its page. Removal shares existing serialized history-mutation admission, preserves files/remote data and rejects stale row events. Capture-history pending refreshes are drained on mutation completion. At most 20 upload rows remain in the UI; SQLite keeps its existing 512 KiB per-connection page-cache setting. The two visible history panels may each have one bounded fetch; this adds no runtime, thread, timer, image buffers or idle query. Disk history retention remains a pending workflow, not an unbounded in-memory cache.

Optimized fixtures verify version-3 upgrade, preserved capture metadata, successful-upload persistence, indexed page boundaries/lookahead without duplicates, validation/cancel-before-storage, local-only removal and IDs that are not reused after deletion. Native headless testing performs a synthetic loopback upload, checks persisted link publication, hides/reopens while fetching, removes its record and preserves the source, last capture and clipboard. No real screenshot or external service is used. One development observation for 25 individually durable upload records was 91.78 ms total, including the upgrade/open/commit work; it is not a per-record latency distribution. A separate optimized fixture with 10,000 upload rows observed 1.09 ms page p95 over 25 reads, each returning at most 21 URL-validated metadata rows. This is a development storage observation, not physical UI latency or reference-hardware acceptance. Manual link-copy/remove controls, storage denial/crash recovery, link retention/privacy controls, full memory/idle measurements and complete platform acceptance remain open.
Capture history now offers an explicit Upload PNG action on each displayed entry, enabled only with a loaded destination and no active conflicting image/save/configuration/upload work. Both app-saved and imported PNG references are supported; the panel states that stored bytes and existing metadata are sent unchanged. Users must explicitly edit/save a copy before uploading revised content. This is not an imported-file metadata sanitizer. The click checks page generation, current membership and busy state, then snapshots that entry's path. Later page changes, removals/relinks or last-capture changes cannot redirect an already confirmed upload. There is no automatic upload on selection, preview or browsing.

The home-screen and history actions share one upload implementation, the existing request/response limits, cancellation/runtime lifecycle and successful-upload logging. Neither action replaces the last capture/last saved path or changes the clipboard. No extra runtime, task queue, dependency, timer, database query, image decoding/buffer or idle work is added; the incremental state is one cloned bounded source path for the same single active job. The optimized native loopback integration deliberately keeps a different last-saved path, uploads a selected synthetic history image, rejects stale/missing/loading/mutating rows and duplicate admission, verifies the returned source filename/link/history, and preserves last-capture/clipboard/source bytes. Manual row interaction and real-service/resource/platform gates remain open; no real screenshot or external service is used for verification.
Uploads now expose PNG request-preparation progress through a bounded latest-value watch channel. The existing streaming reader counts only actual PNG bytes read (excluding multipart framing); initial/final notifications and at most four intermediate notifications per second are generated by reads themselves. No periodic timer, polling, per-chunk task, growing queue or extra thread/runtime/dependency is added. The channel retains one pair of 64-bit counters; the reader retains counters/timestamp and the UI one bounded filename copy plus its existing status string. Backpressure and the existing 64 KiB buffers are unchanged. Callers without progress retain the original API and skip counter/timestamp work per read.

The UI distinguishes request bytes prepared from a valid server response: complete byte preparation changes status to Awaiting server response, never to success. Generation and cancellation checks prevent stale notifications from overriding a later upload, cancellation or completed result. Producer closure wakes the receiver to end its operation-scoped task; no progress task remains idle after completion/cancellation. This measures client-side file consumption, not socket acknowledgment or remote storage progress. Optimized tests verify that 1,000 updates coalesce to one latest value, sender closure terminates the receiver, stream bytes are unchanged and counters match consumed data. Loopback transfer tests exercise the progress API through actual multipart streaming and validate its final byte count/channel closure. The native headless upload regression still publishes the correct final link/status and preserves capture/clipboard state. A single sequential optimized loopback observation for a tiny synthetic PNG was 1.77 ms without progress versus 2.02 ms with progress, including runtime setup/teardown; this is not a latency distribution or a reliable overhead estimate. Slow real services, user-visible progress/cancel races, platform scheduling and full CPU/memory/latency gates remain manual acceptance work.
An initial Windows Bearer-authentication slice now reads Generic Credentials from Windows Credential Manager only during an explicit upload. A profile may store a non-secret credential reference (1–64 safe ASCII characters); the target is Screenshots/upload/{canonical HTTPS origin}/{reference}. Domain/port/scheme changes cannot select the old origin's credential, and authenticated HTTP is rejected even with allow_http enabled. An unsupported platform rejects authenticated profiles rather than falling back to anonymous requests. Anonymous profiles retain their existing platform behavior. The app displays/copies only the target identifier; users create/rotate/delete the secret through the native Credential Manager as documented in docs/upload-authentication.md. Forget destination deletes the app profile, not the OS secret. App-side secret entry, other authentication methods and macOS/Linux backends remain pending.

Lookup, UTF-16 decoding, token validation and header construction run on the existing upload worker before sending. Generic credential blobs are bounded by the native 2,560-byte limit; token/control/header injection is rejected. Native credential allocations and temporary UTF-16/UTF-8/header-construction strings are wiped on drop using zeroize; the resulting Authorization header is marked sensitive. HTTP/TLS request buffers are library-owned and are not claimed to be comprehensively wiped. Redirects/retries stay disabled, and the client never serializes the credential token into profiles, destination URLs, history fields or logs. Provider-returned sharing links are separate server data and may themselves carry access parameters as already documented. Native credential calls, like other blocking platform work, are not interruptible mid-call. The Windows API feature and direct zeroize dependency reuse existing OS/library components; no thread, watcher, credential cache, background service or idle lookup is added. Resource cost is one bounded credential allocation and short-lived copies per authenticated job, plus the transport header; full process/OS-service memory remains unmeasured.

Optimized tests validate HTTPS-only/reference/origin scoping, token injection/size rules and sensitive header marking. An explicit Windows fixture writes one uniquely named synthetic session credential, reads it, verifies another origin cannot find it, and deletes that exact entry with cleanup protection. It never accesses existing credentials or sends a real secret. A separate missing-reference test verifies that no connection is made even to a loopback listener. One optimized synthetic native lookup plus Bearer construction took 0.15 ms; this single observation is not a latency distribution or resource acceptance result. Anonymous loopback regressions still pass. Native Credential Manager UI entry, authenticated HTTPS interoperability, credential denial/revocation, native-call latency and complete platform/resource acceptance remain manual gates.
Windows upload credentials now also support authentication: api_key, emitting the stored secret verbatim in the sensitive X-API-Key header. Existing credential profiles without a mode remain Bearer; explicit bearer is supported. An explicit mode without a credential reference is rejected before work, and unknown modes remain schema errors. API keys accept bounded printable ASCII without spaces/control characters; Bearer retains its stricter token alphabet. Header names are fixed by the mode, so configuration cannot inject or replace arbitrary transport headers. HTTPS/origin/reference scoping, native blob bounds, token-copy wiping, no secret JSON/history fields, cancellation and redirect/retry policy are shared unchanged. Example/setup documentation is supplied; other authentication headers/platform secret stores remain pending.

This adds only an optional small profile enum and header-format selection; no new dependency, thread/runtime, cache, timer, lookup at idle or extra file/image buffers are introduced. The same OS credential lookup occurs once per authenticated job. Optimized tests cover raw API-key formatting/sensitivity, punctuation versus Bearer rules, injection rejection, required references, saved-profile round trips and legacy default selection. The native synthetic credential fixture reads both formats from one unique session entry and removes it; the missing-reference transport test checks both modes without any connection. Real X-API-Key service interoperability, native credential-entry UI and complete resource/platform acceptance remain manual gates; no real secrets or external service are used for verification.
Windows credential profiles now support authentication: basic. The stored Generic Credential username is decoded only for Basic; Bearer/API-key continue to ignore it. The native username scan is bounded by CRED_MAX_USERNAME_LENGTH, password blobs retain the native 2,560-byte cap, and combined UTF-8 username:password is limited to 4,096 bytes. Basic requires nonempty values, rejects control characters and colons in usernames, permits spaces/colons in passwords, and uses UTF-8 plus standard Base64. Legacy server encodings, Digest/NTLM and challenge negotiation are not implemented. The explicit mode still requires an origin-scoped OS reference and HTTPS; missing credentials stop before connecting. Existing profiles remain Bearer by default, and Basic can be combined with POST/PUT and existing response modes.

The Authorization header is marked sensitive. Password/token and decoded username copies plus Basic plaintext/Base64/header-format strings use zeroize; library transport-buffer limits remain as documented. Profiles/history contain no application-generated username/password fields. The direct base64 dependency reuses reqwest's existing 0.23 implementation; no new service, thread/runtime, cache, timer, idle lookup, image buffers or file decoding is introduced. Resource cost is one bounded username string and at most a few KiB of temporary Basic formatting in the existing authenticated job. Optimized tests cover Unicode/password punctuation, exact decoding, size/injection rules, sensitive marking, profile persistence and HTTPS/ref admission. The unique synthetic Windows credential fixture now reads all three modes and cleans up, and the missing-reference transport test covers Basic without connecting. One native development observation was 0.094 ms for lookup plus Basic construction (versus 0.157 ms for Bearer immediately before it); single local observations do not establish a speed difference or latency distribution. Real authenticated HTTPS service compatibility, native account-entry UI, credential denial/revocation and complete platform/resource gates remain open. Example/setup documentation is supplied; no real account or external service is used for tests.
PUT destinations can now explicitly select response: {type: request_url} for services that confirm storage without returning a link/body. This mode is restricted to PUT with no JSON pointer, and prevalidates the canonical configured URL against the existing 2,048-byte HTTP(S) link limit before any work starts. Only HTTP 200/201/204 confirms a resource result; 202/other pending/partial responses, redirects and failures do not create a completed link/history entry. The result is exactly the configured resource address; response Location/body values cannot override it. The response body is not consumed and no extra GET/HEAD/public-access verification is performed. A resource URL may still require authentication and is not claimed to be a public sharing link. Existing JSON/text/header modes remain unchanged; a sample profile is supplied.

The mode retains creation-only If-None-Match, single upload admission, byte/file/declared-response limits, progress, cancellation, credentials and explicit copy/history behavior. Incremental state is at most one 2 KiB canonical URL string in the existing job; there are no new dependencies, threads, timers, caches, decoded frames or idle actions. Optimized tests verify configuration/persistence, POST/conflicting-pointer rejection, canonical URL expansion beyond the cap, confirmed statuses, refusal of 202/206/307/412, unchanged source bytes and ignoring unrelated response links. Existing upload regressions pass against loopback only. One tiny synthetic observation was 1.38 ms for status-only PUT versus 1.42 ms for JSON PUT in the same run, including setup/teardown; single local observations do not establish a performance difference or latency distribution. Actual PUT/WebDAV service behavior, public/private URL semantics and full platform/resource acceptance remain manual gates.
**Estimated duration: Weeks 8–9**

History supports page-local multi-selection: Select/Selected per row, Select all, Clear selection and Remove selected. Selection retains at most 20 integer IDs and is cleared when a fetch starts, the filter changes or history closes. Page generations reject obsolete selection/removal events. Bulk removal validates 1–20 positive IDs before opening storage, reuses one prepared DELETE statement in a single worker-side SQLite transaction, reports the actual number removed, and never deletes image files. A storage error rolls back the whole batch. Writes remain single-admission; no new timers, threads, dependencies, image buffers or caches are added. Completion also drains coalesced refreshes after failed/canceled imports.

Optimized tests verify batch limits before storage creation, duplicate/missing IDs, unchanged PNG bytes, and rollback after an injected second-row deletion failure. The headless native regression covers selection/deselection, stale events, selected-only removal, refresh coalescing and canceled/failed-write completion, while preserving an unselected import and the clipboard. One development observation for 20 durable metadata removals was 3.73 ms; this is not a latency distribution or reference-hardware acceptance result. Manual history button interaction/accessibility and complete resource gates remain open. Image-file deletion, retention and arbitrary substring search remain pending.

Tasks:

- Integrate SQLite and rusqlite on the database worker.
- Implement database migrations.
- Move the Phase 3 save path onto the shared filesystem API if that path was temporary.
- Add thumbnails, generated off the foreground executor.
- Build capture history.
- Add metadata search.
- Implement generic HTTP uploads.
- Add URL extraction and automatic copying.
- Add upload history.
- Add paged/indexed history with a virtualized viewport, bounded thumbnail decoding/cache, and tests with at least 10,000 records.
- Test interrupted saves, migrations, partial delete recovery, disk-full, unavailable storage and retention. Test upload timeout/cancel, redirects, response limits and safe retry behavior.
- Meet idle and input budgets while history/search/uploads are active; no eager history load or network-service initialization on resident startup. A lazily initialized runtime may remain parked only if its retained threads/memory meet the idle budgets.

**Deliverable:** A complete local capture and sharing workflow.

### Phase 6 – Stabilization and Release

**Estimated duration: Weeks 10–12**

Tasks:

- Verify the accumulated performance budgets and remedy remaining regressions; the architecture and measurements have been required in every preceding phase.
- Run the memory/cancellation soak and resource-admission tests, not just a fast-path demo.
- Multi-monitor testing.
- DPI testing.
- Wayland testing.
- macOS permission testing on a signed and notarized build. Screen Recording permission is part of that package, not a last-minute check on an unsigned binary.
- Error handling improvements.
- UI polish.
- Installer creation.
- Windows code signing, or an explicit decision to ship an unsigned first release and document the SmartScreen warning.
- Linux packaging as a normal binary and a `.desktop` file, or distro packages. A Flatpak is a separate decision because the sandbox changes portals, the filesystem, and the secret store. Do not add it in the same step as the first native build.
- Documentation.
- Release preparation.

**Deliverable:** A usable first public release.

### Production verification and release engineering

- Pin the Rust toolchain and release dependency resolution. Cargo.lock is now versioned and CI uses `--locked`. Keep the exact resolved graph with release inputs and update the Kit/GPUI snapshot family together. Exact GPUI/XCap pins alone do not freeze transitive dependencies. Document repeatable dependency/toolchain builds separately from any unproven bit-for-bit build claim.
- CI runs formatting, linting, deterministic unit/component tests, migration/recovery tests, dependency vulnerability/license checks and builds for each declared target. Isolate hardware tests as explicit jobs; skipped hardware tests must not count as platform validation. Package/install/upgrade/uninstall smoke tests use the actual artifacts, with published checksums, version and dependency/license notices. Audit the real dependency graph, including GPUI packaging, not just top-level crate names.
- Treat imported images, clipboard images, portal-returned files, IPC messages and uploader responses as untrusted boundaries. Enforce dimension/decoded-byte/stride/path/response limits before allocation or use. Fuzz coordinate/size parsing and uploader response extraction; maintain deterministic regression fixtures. No shell execution from user-provided filenames, URLs or upload response text.
- Test cancellation and failures at every asynchronous boundary with fake backends and controlled faults: permission revocation, hung/failed capture, renderer/device loss, stale callbacks, full disk, database busy/corruption and shutdown while exporting/uploading. Keep UI available and ownership bounded. Recovery must preserve user-owned files and explain what was or was not saved.
- Test packaged apps as standard users. Document supported OS/architecture and Linux session/portal versions, signed macOS permissions, Windows signing/SmartScreen status, accessibility and startup/tray fallback. An update mechanism is out of MVP unless explicitly scoped; any later auto-updater must authenticate artifacts, handle failed installs/rollback and meet the same idle/privacy contract.
- Final release evidence includes the platform capability matrix, hardware/performance reports, storage/migration recovery results, security/privacy review and known limitations. A document or green compile alone does not establish production readiness.

---

## 13. Future Development

After the MVP, gradually expand the application. Every future feature inherits the performance contract, bounded-resource/cancellation rules and production gates. Add an explicit active-work budget for recording, OCR, plugins or synchronization before implementation; no feature may add a permanent idle cost by default.

### V1.1 – Recording

- Screen recording.
- GIF recording.
- Audio recording.
- An FFmpeg integration only after the license choice in the stack constraints is written down.
- Advanced capture options.
- Additional editor tools.

### V1.2 – Advanced Capture and Integrations

- OCR.
- Scrolling screenshots.
- QR code recognition.
- S3-compatible storage.
- Cloudflare R2.
- Additional upload providers.

### V2 – Automation and Extensibility

- Workflow engine.
- Custom actions.
- Plugin system.
- CLI.
- Scripting.
- Custom sharing server.

### V3 – Optional Cloud Ecosystem

- Optional user accounts.
- Synchronization.
- Team sharing.
- Self-hosted infrastructure.
- Public API.
- Community plugin ecosystem.

The local-first application must remain fully usable without any of these services.

---

## 14. Definition of Done

The MVP is ready for its first public release when the following requirements are met:

### Cross-platform

- [ ] Windows release builds use the documented frozen dependency/toolchain inputs.
- [ ] Linux release builds use the documented frozen dependency/toolchain inputs.
- [ ] macOS release builds use the documented frozen dependency/toolchain inputs.
- [ ] Linux X11 tested independently.
- [ ] Linux Wayland tested independently.
- [ ] Platform-specific permissions handled correctly.

### Capture

- [ ] Fullscreen capture works.
- [ ] Display capture works.
- [ ] Window capture works where the platform can list and capture windows.
- [ ] Where window capture is unavailable, the action is absent.
- [ ] Region capture uses the custom overlay where that overlay can cover the captured display.
- [ ] Wayland sessions that cannot host that overlay use portal selection or editor crop.
- [ ] Denied permission/canceled requests produce typed outcomes and no fabricated image, history entry or clipboard change; a legitimate all-black image is accepted.
- [ ] The application's own overlay and windows are not part of the captured image.
- [ ] Multi-monitor support works, including a monitor with a negative origin.
- [ ] DPI and coordinate handling are correct on mixed scale factors.

### Editing

- [ ] Crop works.
- [ ] Arrows work.
- [ ] Shapes work.
- [ ] Text works.
- [ ] Blur and pixelate work.
- [ ] Undo and redo work.
- [ ] Export and preview agree on text, alpha, layer order and crop/effect coordinates.
- [ ] Sanitized copy/save/upload/history do not leak the unredacted source; original retention is explicit.

### Sharing and storage

- [ ] Copying a capture places a PNG on the clipboard, and paste works on each supported session.
- [ ] Local saving works.
- [ ] Generic HTTP upload works.
- [ ] Uploaded URLs can be copied automatically.
- [ ] Capture history survives application restarts.
- [ ] Database migrations work.
- [ ] Interrupted save/delete/migration, disk-full and unavailable storage recovery are tested; success is acknowledged only for completed stages.
- [ ] Upload timeouts, cancellation, response limits, redirect policy and credential handling pass tests.

### Desktop integration

- [ ] Global hotkeys work where the platform supports them.
- [ ] Where a global hotkey cannot be registered, the settings screen says so.
- [ ] Tray works where available; other sessions retain an accessible reopen/quit path.
- [ ] Quick-access overlay works.
- [ ] Native dialogs work.
- [ ] Notifications work.
- [ ] Single-instance forwarding, hotkey conflicts, hide/restore, sleep/resume and shutdown are exercised.

### Quality

- [ ] No blocking enumeration/capture/database/filesystem/image decode/encode/filter/network work runs on the foreground executor; required native UI-thread callbacks stay short and reentrancy-safe.
- [ ] Coordinate conversion and annotation export are tested without a physical display.
- [ ] No unnecessary background infrastructure.
- [ ] Errors are handled gracefully.
- [ ] Upload credentials are not stored in the TOML file and do not appear in logs.
- [ ] The performance contract passes on documented reference hardware/session types; results include idle CPU/wakeups, private/resident/GPU memory, input/presentation, startup and end-to-end capture latency.
- [ ] No idle polling, hidden redraw loop, production clipboard read-back verification or PNG/disk round trip in the hot path remains.
- [ ] Queues, caches, decoded images, undo and concurrency are bounded; the 500-cycle test returns to the measured idle budget.
- [ ] Canceled sessions create no persisted desktop previews and stale results cannot mutate later sessions.
- [ ] Untrusted input limits, dependency/license checks, recovery tests and packaged install/upgrade/uninstall checks pass.
- [ ] Installers or documented packages are available.
- [ ] macOS builds that request Screen Recording are signed and notarized.
- [ ] Basic documentation is complete, including the Wayland limitations.

---

## 15. Important Architectural Decisions

These decisions should guide development throughout the project.

| Decision | Direction |
|---|---|
| Main language | Rust |
| UI framework | Pinned, reviewed GPUI source/snapshot; packaging provenance and platform patches recorded |
| Performance | Mandatory idle/latency/memory budgets, measured from Phase 0 and inherited by every future feature |
| UI runtime | GPUI foreground executor |
| Network I/O | Tokio, off the UI executor |
| Application architecture | Modular Cargo workspace |
| Database | SQLite |
| Database access | rusqlite |
| Migrations | rusqlite_migration |
| Image storage | Filesystem, PNG, physical pixel size |
| Hot path | In-memory frames with bounded ownership; encoding/storage only for required outputs |
| Scheduling | Native events, invalidation-driven rendering, bounded queues and cancelable jobs |
| Secrets | OS secret store, never the TOML file |
| Capture architecture | Common API with native backends and capability flags |
| Region selection | Custom overlay, portal UI, or editor crop, per session |
| Linux support | X11 and Wayland tested as separate sessions |
| Wayland hotkeys | GlobalShortcuts portal where it exists; explained fallback otherwise |
| Upload architecture | Provider abstraction, declarative HTTP uploader first |
| Editing | Non-destructive annotations; crop is a document change |
| Execution | Single-instance native desktop application |
| Cloud dependency | None required |
| FFmpeg | After the MVP, and only after a license decision |
| Plugin system | Later, not in MVP |

### Avoid premature complexity

Do not introduce:

- Microservices.
- Electron or a WebView-based UI.
- An unnecessarily complex event bus.
- An ORM-heavy database architecture.
- A large plugin runtime in the MVP, including a scripting language inside the HTTP uploader.
- A GPUI fork adopted only because it bundles tray, hotkeys, or overlay windows.
- Unnecessary C/C++ dependencies.
- Multiple implementations of the same functionality without a technical reason.

Rust should remain the primary implementation language.

C/C++ may be used selectively for native integrations when there is a demonstrable technical advantage, but the entire application should not become a mixed-language project without a clear reason.

---

## 16. First Development Steps

This is the historical bootstrap sequence, retained to explain the technical gates. The current continuation task is the UI/UX work in the session handoff at the top; do not repeat these steps from scratch or stop that authorized UI work because release gates remain open.

Original starting order:

1. Keep this repository. The plan, license, and README are already here.
2. Run Phase 0 in a small prototype, with a pinned GPUI revision.
3. Build and run that prototype on Windows, Linux X11, Linux Wayland, and macOS.
4. Test transparent, frameless, always-on-top, and multi-monitor window placement before writing application screens.
5. Validate XCap/portal still-capture paths; evaluate Scap only for unresolved stream/backend requirements.
6. Implement a bounded in-memory screenshot path and export its PNG off-thread.
7. Validate clipboard, hotkeys, and the system tray, and write down where each one is unavailable.
8. Measure release-mode idle/startup/input/capture/memory behavior and pass or fail the Phase 0 gate with evidence.
9. Only then create the application workspace and the shell described in Phase 1.

The original instruction to validate capture/desktop integration before building the full UI described project bootstrap. Capture/editor/UI slices now exist; their unresolved platform and performance checks remain release requirements alongside continued UI development.

---

## Final Direction

The goal is not to build another bloated screenshot utility.

The goal is a fast, reliable, extensible desktop capture platform with powerful tools and a clear user experience.

**Rust + GPUI + native platform integrations + SQLite/rusqlite** is the chosen starting direction.

Keep the architecture clean, the implementation pragmatic, and the application useful even without cloud services or an account.

## Technical references for the production review

These sources inform the contracts above; benchmark budgets are project requirements, not numbers taken from these documents.

- [Cargo: manifests versus lockfiles](https://doc.rust-lang.org/cargo/guide/cargo-toml-vs-cargo-lock.html) — freezing resolved dependencies for release inputs.
- [XDG Screenshot portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Screenshot.html) — versioned target capabilities and the older interactive hint.
- [Upstream GPUI layer-shell example](https://github.com/zed-industries/zed/blob/main/crates/gpui/examples/layer_shell.rs) — confirms that a blanket claim of no GPUI layer-shell support is obsolete; the pinned snapshots expose the corresponding API. Runtime validation remains necessary. Do not copy the example's timer-driven clock redraw into an idle capture application.
- [SQLite WAL](https://www.sqlite.org/wal.html) and [atomic commit](https://www.sqlite.org/atomiccommit.html) — local-filesystem constraints and database durability; not a transaction across external PNG files.
- [tiny-skia API](https://docs.rs/tiny-skia/0.12.0/tiny_skia/) — low-level rasterization; text layout must be designed separately.
- [FFmpeg license and legal considerations](https://ffmpeg.org/legal.html) — obligations depend on the build and distribution.
