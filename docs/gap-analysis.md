# Functional gap analysis: screenshots and ShareX

Implementation follow-up (2026-11-14, working tree): Destination JSON
`response.type` `xml` and ShareX `{xml:xpath}` extract a link from the
already-buffered UTF-8 body with a bounded XPath subset on `roxmltree`. DTD,
`//`, `*`, namespaces, `{xml}` without a path and `{xml:input|xpath}` stay
rejected. Secret-store platforms other than Windows remain missing. See PLAN.md's
XML result URLs slice.

Implementation follow-up (2026-11-14, working tree): Destination JSON
`response.type` `regex` and ShareX `{regex:pattern}` / `{regex:pattern|group}`
extract a link from the already-buffered UTF-8 body with the linear-time `regex`
crate. Lookaround, RegexList and `{regex:input|pattern|group}` stay
rejected. XML result URLs are implemented in the later 2026-11-14 follow-up.
Secret-store platforms other than Windows remain missing. See PLAN.md's
regex result URLs slice.

Implementation follow-up (2026-11-14, working tree): Destination JSON
`thumbnail`/`deletion` parsers and ShareX `ThumbnailURL`/`DeletionURL` reuse the
existing JSON-pointer, text or header subset, including prefix/suffix
composition. Results are session-only copy actions; history still stores the
primary URL. The app never fetches the thumbnail or calls the deletion URL.
PUT request-URL mode cannot combine with extras. Regex result URLs are
implemented in the later 2026-11-14 follow-up. XML result URLs are implemented
in the later 2026-11-14 follow-up.
Secret-store platforms other than Windows remain missing. See PLAN.md's
thumbnail and deletion result URLs slice.

Implementation follow-up (2026-11-14, working tree): Destination JSON
`link_prefix`/`link_suffix` and ShareX URL templates such as
`https://cdn.example/{json:id}.png` compose one existing parser onto a bounded
HTTPS prefix and suffix. JSON numbers become decimal text. Origin hijacks,
regex/XML and multi-token templates stay rejected. Thumbnail and deletion
result URLs are implemented in the later 2026-11-14 follow-up.
Secret-store platforms other than Windows remain missing. See PLAN.md's composed
result URLs slice.

Implementation follow-up (2026-11-14, working tree): Destination JSON `json` and
ShareX Body JSON Data string maps can carry up to 8 POST JSON object fields with
exactly one `{base64}` PNG field. PUT destinations, extra form fields, reserved
secret names and other templates stay rejected. Composed result URLs are
implemented in the later 2026-11-14 follow-up. Regex/XML parsers and
deletion/thumbnail URLs remain missing. Secret-store platforms other than
Windows remain missing. See PLAN.md's JSON request bodies slice.

Implementation follow-up (2026-11-14, working tree): Destination JSON `form` and
ShareX Arguments string maps can carry up to 8 extra POST multipart text fields.
PUT destinations, reserved secret names, templates and reuse of the file field
stay rejected. JSON bodies are implemented in the later 2026-11-14 follow-up.
Richer parsers and deletion/thumbnail URLs remain missing. Secret-store
platforms other than Windows remain missing. See PLAN.md's extra multipart form
fields slice.

Implementation follow-up (2026-11-14, working tree): Destination JSON `query` and
ShareX Parameters string maps can carry up to 8 query parameters. The URL field
still rejects embedded query strings. Reserved secret names, templates and
composed URLs above 2,048 bytes stay rejected. Extra multipart form fields are
implemented in the later 2026-11-14 follow-up. JSON bodies, richer parsers
and deletion/thumbnail URLs remain missing. Secret-store platforms other than
Windows remain missing. See PLAN.md's query parameters slice.

Implementation follow-up (2026-11-14, working tree): Destination JSON and ShareX
Headers string maps can carry up to 8 custom request headers. Reserved
auth/transport names, templates and secrets stay rejected. Query parameters are
implemented in the later 2026-11-14 follow-up. JSON/form bodies, richer parsers
and deletion/thumbnail URLs remain missing. Secret-store platforms other than
Windows remain missing. See PLAN.md's custom request headers slice.

Implementation follow-up (2026-11-14, working tree): Uploads can assign a named
destination to each of the six capture commands. Starting that capture applies
the name to this session only and never starts an upload. Empty or missing names
keep the current session destination. Settings backup stores the six names.
Custom request headers are implemented in the later 2026-11-14 follow-up.
Secret-store platforms other than Windows remain missing. See PLAN.md's per-task
destination defaults slice.

Implementation follow-up (2026-11-14, working tree): Settings can export and
import a preferences backup JSON (appearance, folder, shortcuts, output profile,
region presets, JPEG quality, Save as format, filename template, capture delay
and per-task destination names). A missing backup folder keeps the active save
folder. Upload destinations, history, images and OS credentials stay out of the
file. Per-task defaults are implemented in the later 2026-11-14 follow-up.
Custom request headers are implemented in the later 2026-11-14 follow-up.
Secret-store platforms other than Windows remain missing. See PLAN.md's full
settings backup slice.

Implementation follow-up (2026-11-14, working tree): Uploads can export the
session destination as a documented ShareX .sxcu subset. Secrets and credential
references are omitted; existing files are never overwritten. Full settings
backup is implemented in the later 2026-11-14 follow-up. Per-task defaults
are implemented in the later 2026-11-14 follow-up. Custom request headers are
implemented in the later 2026-11-14 follow-up. Secret-store platforms other
than Windows remain missing. See PLAN.md's ShareX custom-uploader export slice.

Implementation follow-up (2026-11-14, working tree): Uploads can import a
documented ShareX .sxcu subset as a session destination. Secrets,
query parameters and richer parsers are rejected. Bounded Headers string maps
are imported in the later custom-request-headers follow-up. Remember and Save
named stay explicit. ShareX export is implemented in the later 2026-11-14
follow-up. Per-task defaults, full settings backup and secret-store
platforms other than Windows remain missing. See PLAN.md's ShareX
custom-uploader import slice.

Implementation follow-up (2026-11-14, working tree): Uploads can export and
import a destination backup JSON of the session destination plus named catalog.
Secrets, settings.json, history and OS credentials stay out of the file.
ShareX profile import is implemented in the later 2026-11-14 follow-up.
Per-task defaults, full settings backup and secret-store platforms other than
Windows remain missing. See PLAN.md's destination configuration export/import
slice.

Implementation follow-up (2026-11-14, working tree): Uploads can save up to 16
named destinations, switch the session destination with Use, and Remove a name
from the catalog. The remembered current destination file stays separate.
Secrets stay out of JSON. Configuration export/import is implemented in the
later 2026-11-14 follow-up. Per-task defaults and secret-store platforms other
than Windows remain missing. See PLAN.md's named upload destinations slice.

Implementation follow-up (2026-11-14, working tree): Windows Uploads Remove
credential deletes the origin-scoped Generic Credential for the attached
reference and returns a credential-free session destination. Forget destination
still leaves the OS secret in place. Named destinations are implemented in the
later 2026-11-14 follow-up. Secret-store platforms other than Windows remain
missing. See PLAN.md's credential-removal slice.

Implementation follow-up (2026-11-14, working tree): Uploads Test destination
sends a throwaway 1×1 PNG through the existing upload path. It is explicit and
cancellable, does not record upload history, and does not change the last
capture. Named destinations are implemented in the later 2026-11-14 follow-up.
Secret-store platforms other than Windows remain missing. See PLAN.md's
destination connection-test slice.

Implementation follow-up (2026-11-13, working tree): Windows Uploads can save a
Bearer/API-key/Basic credential for the current HTTPS destination into Credential
Manager. The secret never enters destination JSON. Connection testing is
implemented in the 2026-11-14 follow-up. Named destinations are implemented
in the later 2026-11-14 follow-up. Secret-store platforms other than Windows
remain missing. See PLAN.md's credential-entry slice.

Implementation follow-up (2026-11-12, working tree): configurable Windows bindings
for the three existing capture commands are now implemented. See [scope and
verification](hotkeys.md). The original static assessment baseline below remains
historical; this follow-up does not establish complete platform/runtime acceptance.

Further implementation follow-up (2026-11-12): one saved global quick-capture
output choice now supports Copy only, Save only, Save and copy and Annotate first.
Per-shortcut task profiles/chains remain missing. See PLAN.md's output-profile
slice for scope, multi-image restrictions and verification limits.

Active-window follow-up (2026-11-12): Windows now exposes a configurable active-
window shortcut (disabled by default) and cancellable configurable menu/tray
commands. Native foreground-focus acceptance remains open; see PLAN.md.

Last-region follow-up (2026-11-12): Windows can repeat one session-only selected
region through UI/tray/configurable shortcut, with worker-side geometry checks.
Numeric-region entry and named rectangle templates are implemented in the
follow-ups below; automatic cross-session monitor binding remains absent.

Exact-region follow-up (2026-11-12): Displays & windows now accepts display-local
physical X/Y/Width/Height and dispatches a geometry-checked exact-region request
through existing output profiles. Successful capture becomes the session's last
region. Inputs are temporary and bounded. Optimized metadata/parser/UI validation
passes; native capture from the new controls, mixed DPI/topology, accessibility
and resource acceptance remain open.

Region-template follow-up (2026-11-12): up to 16 named rectangles now persist in
settings. Sources offers Save / replace preset, Use and Remove; Use only fills
coordinates, and capture requires explicitly selecting a display. Templates store
no monitor identity or image pixels. Bounded parsing, legacy defaults, persistence,
failure preservation and stale/UI behavior pass optimized checks. Native restart/
accessibility and full resource acceptance remain open; automatic monitor binding
and per-template hotkey/task profiles remain missing.

Open-image follow-up (2026-11-12): Recent capture and Library open PNG/JPEG/WebP directly into
the shared editor through a single-file native picker or Ctrl+O (Cmd+O on macOS).
Originals remain unchanged and output stays explicit. Optimized headless UI checks
pass; direct inputs have a 32 MiB encoded-file limit, predecode size checks and
worker-side EXIF orientation. Native dialog/accessibility and resource acceptance
remain open. Other formats and Explorer Open With remain missing.

Clipboard follow-up (2026-11-12): Windows now offers explicit Paste image through
Recent capture and Ctrl+Shift+V, using bounded PNG or 24/32-bit DIB input on a
worker. Outputs remain explicit. Synthetic format/size/alpha/row-order and UI
admission checks pass without accessing the real clipboard. Native cross-app
paste, delayed rendering/contention, resource and accessibility acceptance remain
open; non-Windows image paste remains missing. See PLAN.md for format limits.

File-drop follow-up (2026-11-12): the control window now accepts one external
PNG/JPEG/WebP file into the shared editor, using the existing worker/input limits.
Busy/multiple-file rejection and output-free synthetic WebP opening are verified
through GPUI file-drop events. Native file-manager integration, accessibility,
mixed DPI and resource acceptance remain open; URL/raw-image drops and Explorer
Open With remain missing.

Transform follow-up (2026-11-12): frozen and standalone editors now rotate 90
degrees clockwise and flip horizontally/vertically before cropping or adding
annotations. Undo/Redo/Reset retain orientation metadata and compute lossless
inverse/relative transforms on a cancellable worker; no bitmap undo snapshots
are retained. Exact pixels, history/UI and subsequent redaction/export are covered
by optimized tests. Native interaction/resource acceptance remains open. Existing
annotations cannot yet be transformed; export resampling is described below.

Export-format follow-up (2026-11-12): Save as now exports finalized pixels as PNG,
JPEG (default quality 90, opaque pixels required) or lossless RGBA WebP. JPEG/WebP
are explicit copies that preserve the current source and PNG-only library. New
formats share worker-side no-overwrite publication; signatures, transparency,
failure/cancel/source preservation pass optimized core/UI fixtures. Direct editor
Save/quick captures remain PNG and non-PNG library entries remain absent. The
default Save as format is implemented below; native picker/recovery/resource
acceptance remains open.

JPEG-quality follow-up (2026-11-12): Settings persists quality 1–100, defaulting
older files to 90. Save as snapshots the applied value for in-memory and saved-PNG
conversion. Validation/write failures retain the active value, and other settings
preserve it. Optimized settings/encoder/UI fixtures cover migration, bounds,
persistence/failure, quality effects and export wiring. Native interaction/restart/
accessibility and resource acceptance remain open; PNG/WebP behavior is unchanged.

Default-format follow-up (2026-11-12): Settings persists a default PNG/JPEG/WebP
Save as format. It controls the suggested name and missing-extension fallback;
explicit supported suffixes override it. Legacy settings default to PNG. Other
preferences and failed writes preserve the active choice. Optimized settings/UI
checks cover persistence, rollback and asynchronous export routing. Native picker
suggestions/restart/accessibility and resource acceptance remain open. Direct
editor/quick saves still use PNG; no automatic capture conversion is introduced.

Naming follow-up (2026-11-12): automatic PNG output now supports a saved bounded
filename template with literal text and {date}/{time} in UTC save time. A unique
suffix and .png are mandatory; traversal, unsupported tokens and extensions are
rejected. Edited/cropped outputs share the save path, while manual Save as retains
the chosen name. Optimized parser/settings/UI fixtures verify persistence/failure,
distinct synthetic outputs and matching last-saved/history paths. Native IME/
accessibility, path/recovery and full resource acceptance remain open. Rich macros,
folder templates and renaming existing files remain absent.

Resize-export follow-up (2026-11-12): Save as offers session-only Original/50%/25%
size. Reduced PNG/JPEG/WebP files are copies that preserve the current source and
PNG library. Worker-side box resampling weights RGB by alpha, keeps rounded pixel
bounds and uses one RGBA output without full-size float scratch. Optimized tests
cover pixel/alpha/dimension behavior, format/source preservation and UI snapshot/
history behavior. Native interaction/recovery/memory acceptance remains open;
Custom width and Custom height now allow a bounded physical-pixel dimension with
the other dimension following the aspect ratio, rounded up. Both reuse the same
page-local input and worker kernel; switching axis resets the input. Optimized
fixtures cover bounds, pixels/alpha, saved/live sources, format routing, modal
snapshotting and input release. Exact dimensions additionally allows independent
width/height with an explicit aspect-ratio warning, no upscaling and the same
bounded worker/copy semantics. Optimized fixtures verify both axis bounds,
pixels/alpha, format routing, snapshots and release of its extra page-local input.
Upscaling and canvas resize remain absent. Native interaction/recovery/memory
acceptance remains open.

Assessment date: 2026-11-12. Local baseline: `ed53567f5112818781b55af63fc6931f12d0cd36`
on `main`, including the GPUI Kit rewrite. Static inspection of production code,
UI entry points, configuration and existing verification evidence; no new build,
capture, upload, hardware benchmark or interactive acceptance run was performed.
Existing test results are historical evidence, not newly reproduced results.

The official ShareX changelog currently lists 22.4.0 (2026-11-08) as its newest
release. Its recent additions include thumbnail/list layouts, canvas expansion
and recording improvements. This assessment compares feature families against
the current documented product, not an older screenshot of ShareX.
[ShareX changelog](https://getsharex.com/changelog)

## Conclusion and scope

The app has a substantial still-capture/editor foundation. It is not yet a
ShareX replacement across everyday workflows, and is much further from complete
feature parity. Adopting GPUI Kit improves presentation and standard interaction;
it does not supply recording, OCR, capture backends, upload integrations or a
workflow engine.

Two milestones must remain distinct:

- **Everyday screenshot replacement:** dependable capture, configurable shortcuts
  and outputs, useful image import/export, convenient sharing and a distributable
  desktop application. This is an intermediate milestone, not full ShareX parity.
- **At least ShareX functional scope:** also includes advanced capture, recording,
  OCR, pinning, workflow/custom-uploader breadth and the additional tool families
  listed below. Calling those optional does not satisfy that scope.

Windows is the direct comparison baseline for ShareX's established desktop
workflow. Our Linux/macOS targets remain additional obligations; this comparison
does not drop platforms or lower the repository's release/performance targets.

Status terminology: **implemented** means a production code path exists;
**partial** means important user-facing scope is missing; **missing** means no
corresponding implementation was found in the inspected application; **unverified**
means the relevant runtime/release evidence remains open. Missing and unverified
are not interchangeable. This is a feature-family inventory, not an exhaustive
enumeration of every ShareX preference.

## What already exists

- Primary/chosen display, enumerated window and rectangular-region capture;
  all displays as separate images; a cancellable delayed region (1–60 seconds,
  saved preference, default three seconds).
- Frozen in-memory editing and a standalone editor with 14 tools: selection,
  crop, rectangle, ellipse, line, arrow, pen, text, numbering, highlight,
  spotlight, opaque redaction, pixelation and blur. Movement/resizing,
  duplication, drawing-layer reordering, zoom/pan and bounded metadata undo/redo.
- Explicit editor Copy, Save and Save & copy; PNG Save As and existing-file
  preservation. Several quick-capture paths use a fixed save-and-copy workflow.
- Windows resident tray, three global capture shortcuts, single-instance
  reopening and on-demand graphics attachment/detachment.
- Persistent SQLite capture/upload metadata, indexed prefix search, filters,
  bounded paging, PNG reference import/relink, on-demand preview and page-local
  multi-selection/export/removal.
- One configurable/remembered HTTP destination, POST multipart or raw PUT,
  JSON/text/header/request-URL results, cancellation and progress. Windows
  Credential Manager supports Bearer, fixed X-API-Key and Basic authentication.

Evidence: [capture backend](../crates/app-core/src/capture.rs),
[UI commands/jobs](../crates/app-ui/src/lib.rs),
[workspace](../crates/app-ui/src/workspace.rs),
[editor](../crates/app-ui/src/redaction_editor.rs),
[history](../crates/app-core/src/history.rs),
[uploader](../crates/app-core/src/upload.rs).

## 1. Everyday capture and task configuration

ShareX exposes repeat-region, custom-region, window/control selection, precise
selection adjustment and configurable after-capture tasks in its capture flow.
[Region capture reference](https://getsharex.com/docs/region-capture)

| Capability | Current status and gap | Priority |
| --- | --- | --- |
| User-defined global hotkeys | Partial: Windows settings now assign/disable/restore bindings for primary display, quick region, capture-and-annotate, active window and repeat-last-region. Arbitrary command assignment and per-shortcut task profiles remain missing; see the implementation follow-ups above for verification limits. | A |
| Configurable output workflow | Partial: one saved global choice provides copy-only, save-only, save-and-copy or annotate-first for quick captures. Editor outputs stay explicit. Per-shortcut saved task profiles, after-capture chains and sharing profiles remain missing. | A |
| Active-window/active-monitor command | Partial: Windows foreground-window and active-display capture each have an optional configurable immediate shortcut and menu/tray countdown. Active display selects the foreground window's largest-intersection monitor. Both reject own/invalid targets; native focus/topology/DPI acceptance and other-platform integration remain open. Cursor-based monitor targeting for Windows annotation also exists. | A |
| Last region and exact region | Partial: session-only repeat, geometry-checked numeric display-local X/Y/Width/Height and up to 16 saved named rectangles exist. Templates require explicit display choice; automatic cross-session monitor binding and per-template task/hotkey profiles remain missing. Native acceptance remains open. | A |
| Precision selection | Partial: crop and transforms exist; no equivalent complete magnifier, window/control snapping or keyboard-driven region-selection workflow. | A |
| Capture preferences | Partial: region and Windows active-window countdowns share a saved delay of 1–60 whole seconds, default three. Failed saves preserve the active value; legacy settings default to three. Missing configurable cursor inclusion, subsecond/longer delays, window-frame/shadow/transparency options and other persistent capture defaults. Native restart/countdown/resource acceptance remains open. | A/B |
| Multiple outputs | Partial: all displays are captured separately. No stitched desktop output, cross-monitor drag selection or multiple independent regions in one capture session. | B |
| Repeated capture | Missing operation-scoped interval capture, scheduled runs and saved repeat settings. Any implementation needs explicit activation and cancellation. | B |
| HDR/color correctness | Unverified/limited: XCap returns 8-bit RGBA; color metadata is not reported and code currently treats it as sRGB. No validated HDR conversion or wide-gamut workflow. | A for truthful limitations; B for support |

Evidence: [hard-coded hotkeys](../crates/app-core/src/hotkey.rs),
[settings schema](../crates/app-core/src/settings.rs),
[capture command and overlay routing](../crates/app-ui/src/lib.rs),
[pixel format/capabilities](../crates/app-core/src/capture.rs).
Do not bypass editing or upload without the user's configured, explicit consent.

## 2. Existing images, editing and output

ShareX's editor accepts files and clipboard images, exposes richer annotations,
cut-out/background editing and output actions integrated into tasks.
[Image editor reference](https://getsharex.com/docs/image-editor)

| Capability | Current status and gap | Priority |
| --- | --- | --- |
| Open/Paste/Drop into editor | Partial: direct Open image / Ctrl+O and a single external file drop support PNG/JPEG/WebP; Windows Paste image / Ctrl+Shift+V supports PNG and common DIB layouts. Last capture and PNG history editing exist. Other formats/platform paste, URL/raw-image drops and Explorer Open With remain missing. See the implementation follow-ups for verification limits. | A |
| Image formats | Partial: direct Open/Drop accepts PNG/JPEG/WebP; Save as has a saved default format, opaque JPEG quality 1–100 (default 90) and lossless RGBA WebP. The library and direct editor/quick saves remain PNG; capture/editor format preferences, lossy WebP and other formats remain pending. | A/B |
| Resize and basic transforms | Partial: Save as exports smaller 50%/25% copies, a custom width/height with aspect ratio kept, or independent exact dimensions without upscaling; lossless 90-degree rotation and horizontal/vertical flip are available before crop/annotations with metadata Undo/Redo. Upscaling, canvas resize and transforming existing annotations remain missing. View zoom does not resize the exported file. | A |
| Colors and typography | Partial: four drawing color swatches, discrete widths and text sizes; labels have four-line/512-byte limits. No general color/font selection, richer text/background/outline styling or reusable tool presets. | A/B |
| Effect order | Partial: drawing annotations reorder; blur/pixelation are source-effect stages and opaque redaction is last. Arbitrary effect/drawing order is unavailable. Make restrictions clear or implement a consistent ordered document model. Keep opaque-redaction safety explicit. | A/B |
| Advanced annotations | Missing speech balloons, inserted image/logo/clipboard objects, cursor stamps, emoji, magnify, clone/smart eraser, annotation rotation and richer arrow/shape options. | B/C |
| Canvas and presentation | Missing canvas expansion, cut-out strips, padding/background/shadow composition, watermark and reusable effect/export presets. | B |
| Editor sharing | Partial: final pixels can be copied/saved, but no direct finalized-document upload or pin action. Upload currently requires a saved PNG. | A/B |
| Other outputs | Partial: Recent capture and Library More can explicitly copy saved paths as text; Windows also offers Copy file as a single native file reference after worker-side source validation. Unsaved captures and stale library actions are rejected; failures are reported. Native Explorer/attachment/clipboard acceptance remains open. Print, other-platform file transfer and drag-out remain missing. | B |
| Editable projects | No persistent document/session format. Useful for longer edits, but not asserted here as a prerequisite merely because in-memory undo exists. | Product decision |

Evidence: [PNG decode and clipboard output](../crates/app-core/src/clipboard.rs),
[editor controls/tool set](../crates/app-ui/src/redaction_editor.rs),
[label input](../crates/app-ui/src/caption.rs),
[fixed effect/export stages](../crates/app-core/src/selection.rs).
Resource bounds are intentional safeguards, not defects; improve useful editing
within measured byte/item limits rather than replacing them with unbounded state.

## 3. Sharing and uploader compatibility

ShareX supports custom uploader import/export, several HTTP methods/body types,
parameters/headers and response expressions. Its command interface also accepts
files/URLs and configured tasks.
[Custom uploader](https://getsharex.com/docs/custom-uploader),
[command-line interface](https://getsharex.com/docs/command-line-arguments)

| Capability | Current status and gap | Priority |
| --- | --- | --- |
| Destination setup | Partial: Uploads offers native credential-free POST/PUT setup with plain-text/JSON link responses, bounded validation and explicit HTTP opt-in; Use is session-only and Remember persists it. Windows Save credential writes Bearer/API-key/Basic secrets to Credential Manager and attaches only the reference to the session destination. Remove credential deletes that OS secret and clears the session reference; Forget destination still leaves the OS secret in place. JSON profiles retain advanced settings, including up to 8 custom request headers, 8 query parameters, 8 extra POST form fields and 8 POST JSON body fields with one `{base64}` PNG field. Test destination sends a throwaway 1×1 PNG through the existing transfer path without recording history. Named destinations persist up to 16 profiles in a separate catalog. Per-task defaults apply a saved name when that capture starts, without uploading. Export/Import configuration copies that catalog and the current destination without secrets. Settings Export/Import copies preferences, including per-task destination names, without destination URLs, history, images or secrets. | A |
| Multiple destinations | Partial: a named catalog of up to 16 destinations can be saved, switched into the session and removed. Export/Import configuration copies the catalog with the current destination. Per-task defaults apply a saved name when that capture starts, without uploading. The remembered current destination remains one profile. | A/B |
| Existing ShareX profiles | Partial: Uploads can import and export a documented `.sxcu` subset (HTTPS ImageUploader POST multipart, POST JSON or PUT binary, `{json}`/`{header}`/`{response}`/`{responseurl}`/`{regex:pattern}`/`{regex:pattern\|group}`/`{xml:xpath}` with one optional HTTPS prefix/suffix, ThumbnailURL, DeletionURL, Headers, Parameters, Arguments and Body JSON Data string maps through the same validators). `{base64}` is the only JSON file placeholder; secrets, HTTP, `{input}`/`{filename}`, RegexList, lookaround, `{xml}` without a path, `{xml:input\|xpath}` and multi-token parsers are rejected. Full custom-uploader parity remains missing. Our profile JSON is not the ShareX format. | B |
| Generic HTTP breadth | Partial: POST multipart/PUT PNG/POST JSON with up to 8 custom request headers, 8 query parameters, 8 extra POST form fields, 8 POST JSON body fields with one `{base64}` PNG field, composed result URLs from a JSON string/number, text, header, bounded linear-time regex or bounded XML path plus prefix/suffix, and session-only thumbnail/deletion result URLs from the same parser subset. Compatibility must preserve origin-scoped secrets and response bounds. | B |
| Service integrations | No dedicated S3/R2, SFTP/FTP, OAuth/provider integrations or remote file management. Some simple services can already work through our generic POST/PUT path; test service by service. | B/C |
| Accepted input | PNG files only. No arbitrary file/folder/text/clipboard/URL uploads, batch upload UI, watched-folder workflow or capture-to-link chain. | B |
| Post-upload behavior | Explicit Copy link exists. Missing optional user-configured automatic copy, link templates/shortening/QR and follow-up tasks. No implicit network/browser action should be added. | A/B |
| Metadata handling | Edited exports flatten pixels. Existing imported PNG uploads and bulk exports preserve source bytes/metadata; no explicit metadata-inspection/sanitization option. | A before privacy claims; B for full tooling |
| Secret stores | Windows Credential Manager lookup, native Save credential entry and Remove credential deletion exist; macOS Keychain and Linux Secret Service remain missing. | A for relevant platform releases |

Evidence: [profile/method/response schema and validation](../crates/app-core/src/upload.rs),
[credential implementation](../crates/app-core/src/upload_credentials.rs),
[upload UI](../crates/app-ui/src/workspace.rs).
A built-in provider catalog is not required to support one chosen service, but
one successful generic upload is not provider/ShareX compatibility.

## 4. Missing major ShareX capabilities

These are necessary for a broad parity claim, even if the first usable screenshot
release precedes them. The present roadmap defers several of them beyond MVP;
the requested parity target therefore requires a larger scope than that MVP.

- **Scrolling capture:** capture/stitch long content, handle fixed headers and
  report imperfect matches; needs bounded image admission and cancellation.
  [ShareX scrolling capture](https://getsharex.com/docs/scrolling-screenshot)
- **OCR and QR:** no region/image text recognition, language handling, text
  result/copy workflow or QR scanning/generation.
  [ShareX OCR](https://getsharex.com/docs/ocr)
- **Recording:** no screen-video/GIF recorder, start/stop controls, audio capture,
  device/settings selection, encoding or recording history/export. Further
  parity includes camera/cursor/click overlays and media conversion/trimming.
  Current ShareX has native recording as well as FFmpeg-related workflows;
  FFmpeg is not inherently the only possible backend. Our integration still
  requires explicit codec/license/resource decisions.
  [ShareX recording changes](https://getsharex.com/changelog)
- **Pin to screen:** no floating reference-image windows, scale/opacity controls
  or pin-from-capture/clipboard/file workflows. A fullscreen capture overlay is
  a different feature.
  [ShareX pinning](https://getsharex.com/docs/pin-to-screen)
- **Workflow/actions:** no reusable after-capture/after-upload task graph,
  external-tool actions or real task CLI/IPC. Current launch arguments are Show
  and diagnostics, and secondary-instance forwarding only requests Show.
  [ShareX actions](https://getsharex.com/actions),
  [ShareX task CLI](https://getsharex.com/docs/command-line-arguments)
- **Utility families:** color picker, ruler/measure, image combine/compare/split,
  conversion/resizing/watermark/beautification/background removal, media helpers,
  metadata tools, hash checks, URL utilities, remote storage browser, clipboard
  viewer and system inspection tools have no equivalent exposed app workflows.
  Source-image blur and annotation labels are not substitutes for OCR or image
  conversion tools. These can follow the everyday capture priorities, but remain
  gaps for full product breadth.
  [ShareX feature inventory](https://getsharex.com/)

## 5. Library, desktop integration and release readiness

These gaps determine whether implemented features are convenient and dependable:

- Library has bounded metadata lists and an explicit single preview, not a
  thumbnail gallery. Add bounded visible-thumbnail decoding/cache, useful date
  filtering/sorting and missing-file handling. Search is metadata word-prefix
  search, not arbitrary substring, OCR content or integrated upload-link search.
- Current selection/export/removal is limited to one page. Cross-page batch
  workflows, rename/move and deliberate file deletion/retention are missing.
  Metadata-only removal correctly preserves user files and should remain explicit.
- No app-owned Windows autostart setting, actionable completion notifications,
  Explorer context/file associations or useful task CLI. Settings Export/Import
  copies preferences without destinations, history, images or secrets. The
  current tray is a small fixed command set.
- No installer/portable distribution pipeline, upgrade/uninstall verification,
  published release artifacts/signing workflow or app update mechanism was found
  in the inspected repository. A documented manual update path is sufficient
  for an intermediate release; full updater behavior needs a separate decision.
- After the Kit rewrite, evidence includes Linux native synthetic UI inspection
  and Windows compilation checks. Fresh interactive Windows capture/editor,
  clipboard, hotkey/tray, mixed DPI, topology changes, sleep/resume and device-loss
  acceptance remain open. Do not mark these implemented paths as absent, or mark
  them validated from compilation.
- Atomic PNG publication, transactions, rollback and selected failure fixtures
  already exist. Complete crash reconciliation across files/history, migration
  backup/recovery, disk-full and unavailable-drive acceptance remain incomplete.
- Existing optimized observations do not meet/prove all resident/shell memory,
  idle/wakeup, physical presentation and soak budgets. No new measurements were
  taken for this assessment. Historical comparisons belong to their exact build.

Evidence: [library implementation](../crates/app-core/src/history.rs),
[desktop launch/IPC routing](../crates/app-ui/src/lib.rs),
[settings](../crates/app-core/src/settings.rs),
[CI](../.github/workflows/rust.yml),
[current verification limits](ui-redesign.md),
[recorded measurements](ui-redesign-verification.json).

Our additional cross-platform gaps: non-Windows startup remains the windowed
spike; resident integration/hotkeys/single-instance modules are Windows-only.
Window listing/capture is disabled on Linux, including X11. Capabilities are
currently largely static OS flags rather than session/permission discovery.
A validated Wayland portal/selection fallback and signed macOS permission flow
are not implemented/accepted. Native Linux UI tests do not validate Linux capture.
English-only app UI is deliberate project scope; multilingual UI would be an
additional parity gap if the goal includes ShareX's localization breadth.

## Recommended implementation order and acceptance

1. **Dependable desktop baseline:** run post-Kit Windows native acceptance;
   stabilize DPI/topology/clipboard/cancellation/recovery and ship a reproducible
   installable or portable build. Keep explicit unsupported-mode explanations.
2. **Everyday capture package (A):** configurable global shortcuts and saved
   output profiles; active-window and last/exact region; Open/Paste/Drop;
   resize/rotate; PNG/JPEG/WebP; configurable names/folders and completion actions.
   Acceptance: configure a shortcut, capture in another application, annotate,
   produce only the requested outputs, and reopen/reuse a previous image without
   developer configuration or unintended source persistence.
3. **Sharing/library package (A/B):** destination/auth UI, named profiles,
   finalized-document sharing, explicit metadata policy and bounded gallery.
   Acceptance: configure and test a real HTTPS destination, share an edited image,
   copy its result and find it later; cancellation/failure must remain truthful.
4. **Distinctive capabilities (B):** scrolling capture, OCR/QR, pinning and
   video/GIF recording. These are separate substantial work items, not toolbar
   buttons. Each needs native fixtures, failure handling and active-work budgets.
5. **Advanced parity (B/C):** complete capture customization, richer editor and
   effect model, custom-uploader compatibility/providers, workflows/CLI/actions,
   media and utility families. Confirm a capability matrix against ShareX before
   claiming parity. A functioning prototype or matching menu layout is insufficient.

These priorities are recommendations, not changes to PLAN.md or its acceptance
targets. No percentages or calendar estimates are assigned: there is no measured
feature weighting or implementation estimate that would justify them.
