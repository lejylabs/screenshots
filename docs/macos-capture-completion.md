# macOS native capture, targeted scrolling and finite recording

Author: NewtTheWolf
Date: 2026-11-16
Status: approved contract; native implementation and focused verification in progress.
Actual ARM/Intel SDK typechecking is separate from native TCC/physical acceptance.

## Pre-implementation inventory and exact scope

Read the PLAN performance contract (including active-image/concurrency/deadline
rules), current `capture_native_macos.rs`, native call/worker/protocol lifecycle,
`scroll_capture.rs` and its X11/Windows adapters, `record.rs`, `audio_record.rs`,
and the capture defaults, native isolation, scrolling, recording and audio docs.

At the initial inventory, the macOS still adapter provided persistent display UUIDs and a checked
nondefault window CGImage decoder with sRGB/alpha/stride checks. Cursor inclusion
and client-only framing were explicitly unsupported. The default window/display
path used XCap's `CGWindowListCreateImage`; it did not receive those same
checks. XCap reports display/window bounds in logical screen points while its
image can contain physical Retina pixels. Its window focus predicate checks the
frontmost **process**, not a particular window of that process. Those facts make
simply enabling the existing generic scroll implementation unsafe.

Live scrolling then had X11 and Windows adapters only. The assembler,
viewport/output byte limits, fuzzy overlap matching, fixed-header trims,
cancellation, repeated-end detection and truthful `complete` result already
exist and must be reused. No new matching implementation or application UI is
needed.

The optional Unix recorder accepts local X11/test video; macOS AVFoundation audio
already has an explicit `none:AUDIO_DEVICE` selector, fixed PCM sample count,
bounded diagnostics, exact probe verification and no-replace publication.
There was no native macOS screen-video source. The Unix recorder's process group
at the initial inventory had no macOS parent-loss lifeline; adding a source must not inherit
that missing parent-loss guarantee or blindly signal a group after its leader
has been reaped.

This stage adds guarded macOS scrolling, correct explicit still cursor handling,
modern still capture/policy routing and camera-safe finite screen/GIF/combined
audio recording. Existing X11/Windows commands, codecs, bounds, task meanings and
GUI design remain unchanged. Physical monitor/hardware, signed TCC, HDR and
reference-hardware performance acceptance remain separate verified gates.

## Source and permission contract

All native work executes only in the existing operation-owned helper. Check
`CGPreflightScreenCaptureAccess` before capture/record work and
`AXIsProcessTrusted` before accessibility scrolling. Do not call a permission
request API, activate an arbitrary app, install a global event tap or post a
global scroll event. Denied/not-determined permission fails before capture or
control actions. Microphone use remains an explicit audio option and requires
an authorized audio preflight; tests do not initialize native devices when that
precondition is absent.

The normal application installs metadata/function pointers only. Native
frameworks, observers, sessions, dispatch queues and frame pipes are operation
scoped. The helper main thread services required AppKit/ScreenCaptureKit and
short native event operations. Blocking AX messages have a positive per-object
timeout (initially 150 ms) and are additionally bounded by process isolation;
no abandoned timeout thread replaces an unresponsive target.

Retain concrete source identity: CGWindowID and owner PID plus process-start
identity for windows; CGDirectDisplayID plus UUID for displays. Snapshot logical
rectangles, native pixel scale, rotation and participating display UUIDs before
work. Revalidate after capture and before/after every control action. Nonfinite,
ambiguous, absent, rotated/unsupported or changing geometry fails rather than
falling back to the primary display or resizing pixels to fit a prediction.

Use public AX focused-application/window, position/size and parent attributes.
Never use `_AXUIElementGetWindow`, private WindowServer/CGS calls or an invented
AX window-number attribute. Match the focused AX window to the explicitly
selected CG window using owner identity and exact bounds, rejecting ambiguity
among equal candidate windows. The selected control must remain a descendant
of that retained focused-window AX object. A focus/window/process change stops
the operation, even if the process still owns another frontmost window.

## Targeted scrolling

Keep one helper for the **whole finite scroll operation**. It retains the same
AX window and scroll-control references rather than attempting to serialize AX
object identity between fresh helpers. No output path or task actions enter the
native request. The parent continues to own delay/preflight, cancellation,
final PNG publication and truthful result output.

Proposed typed request `MacScrollRequest` contains the explicit window ID,
physical-pixel region, steps/wheel/settle and existing `ScrollMatchOptions`.
Validate the existing 1–128 step, 1–10 action, 20–2000-ms settle, 100-second
requested-wait, 16-MiB viewport, 32-MiB source-window and 256-MiB assembled-image
limits before native work. The existing overall 120-second deadline and parent
owned-pending cleanup/admission rules apply. The explicit initial delay remains
in the parent and counts against the overall operation deadline.

Resolve the element at the requested region center using the target app's AX
object; traverse at most 16 parents and only bounded child/attribute arrays.
Require a public vertical scroll-bar object associated with the retained window
and an advertised `AXIncrement` action (or a documented settable scrollbar
value/range if that is the precise supported control). Reject an absent or
ambiguous controllable scrollbar. Apply the bounded number of increments to
that specific object; never synthesize a global wheel as a fallback. Apps that
do not expose the required control fail explicitly, like X11/Windows apps that
ignore their targeted native wheel message.

Convert physical region pixels to logical AX hit-test points using admitted
native scale. Keep source rectangles and final pixels at native resolution;
Retina is not fixed by dividing an image's dimensions or resampling it. Require
the selected window to fit a single admitted unrotated display for this first
implementation, preserving the existing scrolling topology restriction.

Capture a shadow-free native window image before/after the action, with exact
window/process/focus/topology checks. Crop only after validating the actual
pixel geometry. Reuse the existing assembler inside the helper so only the
assembled image crosses the authenticated raw channel. Require two unchanged
frames and an actual prior advance before reporting completion; preserve
`step_limit`, `no_scroll`, trims and fuzzy-match behavior. Stale native control
references, a hung app, failed matching or cancellation publishes no output.

The proposed native call `MacScroll { request }` returns one bounded assembled
frame with the selected window ID and bounded result metadata (counts, advances,
stop reason/complete). It is not an arbitrary input-automation API.

## Still cursor, framing, shadow and transparency

Use public ScreenCaptureKit on versions where the needed API is available:
`SCScreenshotManager` on macOS 14+, with the required public single-CGImage
legacy path on 12–13. `SCStreamConfiguration.showsCursor` supplies compositor
cursor shape/hotspot/scale and avoids using the helper app's current cursor.
Do not substitute `NSCursor.currentCursor`, an arrow image or a private cursor
query for the user's actual system cursor. A legacy fallback, if retained for
older supported systems, must use public system-cursor image/hotspot APIs on
the helper main thread and explicitly validated pixel scale; inability to do
so remains an explicit unsupported result.

Select the exact SC display/window corresponding to the admitted CG identity.
On macOS 14+, use the filter's `contentRect` and `pointPixelScale`, not XCap's
logical sizes as pixel sizes. Legacy geometry must independently reconcile
display mode pixels and logical bounds; ambiguity fails. Preserve no-prompt
screen permission checks, fixed source identity and before/after guards for
default as well as nondefault capture options.

Request packed SDR BGRA8/sRGB, validate the returned bitmap/sample format,
stride, provider extent and alpha mode before copying to the existing straight
RGBA8 frame. Source/native OS buffers count as additional active resources.
Do not label this genuine high-bit HDR capture. Native cursor inclusion must
not cause a second manual cursor composition pass.

Full native/decorated frame modes remain distinct from client-only capture.
Map shadow controls to public `ignoreShadowsSingleWindow` where supplied;
accept returned native image bounds without a guessed padding/resize. Native
shadow extent and cursor behavior require the owned-window pixel fixture before
being described as accepted. A policy whose exact public bounds/behavior cannot
be established fails rather than silently clipping a shadow. Preserve existing
legacy behavior only on a version where the public legacy API remains usable;
do not hide a version-dependent unsupported mode.

Request nonopaque native output for transparency preservation where the API
supplies alpha, unpremultiply only when its metadata says premultiplied and use
the existing explicit opaque-background compositing path for opaque mode.
Client-only framing remains unsupported unless a public, unambiguous native
content rectangle is available; do not guess it from a title-bar height.

## Camera-safe recording source

Passing `Capture screen N` to arbitrary FFmpeg is not a sufficient guarantee.
The current upstream AVFoundation input matches camera/muxed localized names
before that screen-name fallback; numeric video indices also depend on camera
count. A name collision or device topology change must not start a camera.

Create the screen input natively with public
`AVCaptureScreenInput.initWithDisplayID` and a validated fixed display UUID/ID.
No `AVCaptureDeviceInput` for video, numeric FFmpeg video index, camera discovery
or default-video selector is used. FFmpeg receives **only raw video pixels** as
its video input. Its existing optional AVFoundation audio selector is
`none:EXPLICIT_AUDIO`, so it cannot create a video-camera input. Native microphone
authorization and external-tool/signed TCC behavior remain explicit platform
checks, not a claim based on a Linux argv fixture.

New mode proposal: `record avfoundation OUTPUT.mkv|OUTPUT.gif --display ID` with
the existing required seconds/fps/width/height, optional paired x/y and cursor
show/hide, and existing explicit audio options. Only macOS accepts this video
mode. Width/height/x/y describe a native pixel rectangle on that display; they
do not select a camera or silently scale an entire display. Preserve current
MKV limits (60 seconds/30 FPS/1920×1080), GIF limits (30 seconds/15 FPS/1280×720),
FFV1/GIF codec policy, 256-MiB encoded-output ceiling and exact ffprobe checks.

Actual macOS SDK `AVCaptureInput.h` declares `cropRect` in display-local
**points**, bottom-left. This conflicts with the current web page calling it
pixels; the real macOS15.5 SDK excerpt is preserved in
`target/verification/macos-screen-input-sdk-arm.log`. Convert the admitted
physical rectangle using `x/scale`,
`(display_pixel_height-y-height)/scale`, `width/scale`, `height/scale`, and set
`scaleFactor=scale` to retain the requested physical pixels. Require actual
frames to match that extent. Set capturesCursor explicitly and capturesMouseClicks
false. The duplicate-removal setter is obsolete/no longer supported and is not
called; omitted/late source frames fail count/PTS/deadline checks rather than
being silently duplicated. Reject an inactive/changed UUID, geometry, scale or
rotation.
A native reconfiguration observer cancels the active source on topology change;
it is registered before source admission and removed at teardown.

Configure AVCaptureVideoDataOutput as packed BGRA8 and late-frame discard.
Validate CVPixelBuffer type/extent/color/stride and lock it read-only only for
the bounded copy. Use a serial active delegate queue and **one reserved frame
slot**: reserve before allocating/copying and keep it reserved until its raw
pipe write is complete. A busy slot, reported source drop, interruption,
malformed frame or encoder backpressure marks failure; do not queue frames,
silently duplicate them or pad a shorter capture to pass verification.

Accept exactly `seconds * fps` frames. Source timestamps must be valid,
strictly increasing and within the admitted cadence/drift tolerance; record
first/last native PTS and reject a discontinuity rather than concealing it in
fixed-rate rawvideo timestamps. Initially require a positive CMTime timescale,
checked rational nanosecond conversion, no negative/indefinite timestamp, a
per-frame gap no greater than two requested frame periods plus 20 ms and prefix
drift from `(accepted_index / fps)` no greater than two periods plus 50 ms.
These are named pure-model constants, tested at their boundaries and reported
as provisional until native evidence confirms them; do not increase them merely
to turn a failed native fixture green. The first frame has at most five seconds
of startup wait, still within the unchanged whole-operation deadline.
Normalize the encoded video timeline to the existing explicit CFR output.
Combined audio retains exact 48-kHz sample-count checks. Independently started
audio/video sources are not advertised as subframe-synchronized; startup/clock
and missing-source failures remain visible.

## Stream/process ownership seam

Reuse the current operation-owned native controller/admission, anonymous
authentication handshake and parent-death lifeline, not a second process
supervision framework. A `MacVideo` request has bounded source/options/count
metadata and a private inherited raw sink capability. No output pathname or
arbitrary command enters the native request. Reject stdio/wrong-type/stale sink
FDs and set close-on-exec immediately. Do not place pixels/secrets in argv or
environment. Raw writes use finite cancellation/deadline-aware chunks.

The helper owns a reserved process-group identity while recording. Start the
fixed, parent-validated FFmpeg encoder in that same existing group with only
the raw read end as stdin. The helper's macOS lifeline kills its own group on
parent loss, including the encoder. Retain the leader until all signaling is
done; no numeric group kill after leader reaping. Encoding/probe failures must
cancel the source, close writes and join/reap both sides before admission is
released. Kernel-pending termination keeps truthful owned pending state.

After the exact source frame count, stop/detach/drain native capture callbacks,
close the raw writer and report bounded source completion metadata. The helper
waits for a parent finish acknowledgement, retaining group identity while
FFmpeg flushes and the parent verifies/reaps it. Only then may the helper exit.
This prevents normal leader exit from killing an encoder mid-flush. The Unix
engine needs an explicit borrowed-group/stdin seam; a successful encoder must
not kill the helper's shared group in its current generic cleanup method.

The parent owns private staging, codec/probe verification, sync, no-replace
publication and truthful post-publication errors. Preserve seconds-plus-ten
deadline semantics and existing count/output bounds. Ordinary still replies
retain their 32-frame/256-MiB aggregate admission; video is a separate **finite
stream**, never accumulated into a frame Vec. Per-frame dimensions/count/total
raw-byte arithmetic are admitted before starting, with at most one app-owned
source copy, one bounded pipe and the external encoder's separately reported
buffers. A finished/canceled job leaves no optional native runtime or stream.

## Files and order

New-file ownership is the portable core `capture_macos_model.rs`, native
`capture_macos_ax.rs`/`capture_macos_sck.rs` and scrolling runner, plus desktop
`scroll_capture_macos.rs`, `record_macos.rs` and `record_macos_source.rs`.
Corresponding fixture targets are `tests/capture_macos.rs` and
`tests/record_macos.rs`; pure model/decoder tests can live beside their source.
The exact native module split may be smaller when an existing adapter owns the
same resource; do not create a second model/decoder solely for these filenames.

Shared seams, applied only after root review and the frozen checks finish:

- Core native call: `MacScroll { request: MacScrollRequest }`, one assembled
  image plus bounded `MacScrollSummary`; deadline 120 seconds. Existing request
  variants and frame identity/byte admission remain intact.
- Desktop native controller: a finite `MacVideoSession` reserve/start/source-end/
  finish-ack lifecycle sharing current admission/transport/owned cleanup. The
  regular still executor refuses a streaming request rather than collecting
  its frames. A raw pipe FD is an inherited authenticated capability, never an
  output path.
- Recorder parser: optional `display: u32` only for the new macOS video mode;
  reject it in existing modes. Root coordinates CLI help, task/seal config
  snapshots and platform-owned declarations.
- Unix encoder engine: narrowly permit explicit stdin and a borrowed native
  source group, keeping the source leader alive until encoder/probe reaping.
  Preserve normal X11/test behavior and independently fix any unsafe post-reap
  group signaling exposed by the shared lifecycle change.
- Core capture routing: default/nondefault Mac native pixels and built-in
  cursor inclusion use the guarded Mac path; no second cursor application.
  Physical-pixel metadata must not be mislabeled as logical geometry.

1. New portable core macOS geometry/permission/scroll/record models and exact
   byte/PTS/argv regressions; this document. No shared execution path changes
   while the root's frozen matrix runs.
2. New guarded macOS native modules plus targeted scroll runner. Coordinate
   `MacScroll` call/result metadata and the narrow CLI dispatch with root;
   preserve existing X11/Windows `scroll_capture` code paths.
3. Modern still adapter/cursor routing and physical-pixel guards in
   `capture_native_macos.rs` plus a narrowly coordinated core capture seam.
   Prefer maintained objc2 framework bindings. Existing objc2/AVFoundation/
   CoreMedia/CoreVideo/Foundation packages are already locked. Preserve older
   macOS startup compatibility: do not add an unconditional strong link to a
   framework absent on an otherwise supported system. Load optional
   ScreenCaptureKit only during the native operation using public NSBundle
   loading/class availability checks (or a demonstrated weak-link binding),
   with narrowly declared public Objective-C interfaces where necessary.
   Additional dependency edges require root review/lock coordination.
4. New desktop macOS recorder/producer modules, private inherited raw pipe and
   borrowed-group lifecycle integration with the existing Unix engine/native
   controller. Root owns shared CLI/main/Task/digest wiring. Coordinate every
   Cargo/lock mutation; do not overwrite audio's Windows child/feature work.
5. Focused optimized synthetic/native-fixture checks, current SDK CI compilation,
   source review, measured active cost and docs/status freeze before root commits.

## Verification and open native gates

Portable checks cover points↔pixels including Retina/negative display origins,
crop Y conversion, geometry/UUID/PID/start-time changes, equal-window ambiguity,
AX parent/role/action bounds, permission-denied-before-action, premultiplied
alpha/component orders/cursor clipping, unsupported policies before work,
timestamp/count/backpressure boundaries and raw sink FD admission. Tests must
prove that video argv cannot select a camera, even with names colliding with
FFmpeg's screen label or changing camera count.

Actual optional FFmpeg fixtures use generated raw frames and synthetic audio on
Linux/private processes: verify FFV1/GIF pixels, exact audio samples, cancellation,
encoder error/stall, early EOF, source failure, group ownership during flush,
parent loss and no-publish/cleanup behavior. These exercise shared plumbing and
codec checks; they do not prove macOS capture, permissions or AX delivery.

An explicit macOS owned fixture creates only its own window/scroll view and
painted pixels, records its CG/AX identity, checks content advances and captures
cursor/frame/shadow/alpha at admitted scale. No physical keyboard/mouse input,
real user window or permission-prompt request is used. Native tests require
pre-existing authorization; denied-state checks can run without permission.
If a native opt-in gate lacks required authorization or a signed stable bundle,
it must report that gate as unpassed rather than count a mock as completion.

This Linux host has only Linux and GNU Windows Rust targets installed and no
macOS SDK/osxcross. Use real macOS SDK CI for compilation and authorized native
fixtures; no fake framework stubs or Linux-mock native acceptance claims. Keep
physical footprint/RSS separate and measure active native/frame/encoder costs
in optimized builds. The PLAN budgets remain unchanged; repeatable >10%
regressions require the same recorded cause/remediation decision.

## Primary API evidence

- Apple [AXUIElement APIs](https://developer.apple.com/documentation/applicationservices/axuielement_h),
  [messaging timeout](https://developer.apple.com/documentation/applicationservices/1459345-axuielementsetmessagingtimeout),
  and [documented AX actions](https://developer.apple.com/documentation/applicationservices/axactionconstants_h/miscellaneous_defines).
- Apple [screen permission preflight](https://developer.apple.com/documentation/coregraphics/cgpreflightscreencaptureaccess()),
  [ScreenCaptureKit configuration](https://developer.apple.com/documentation/screencapturekit/scstreamconfiguration),
  [content rectangle](https://developer.apple.com/documentation/screencapturekit/sccontentfilter/contentrect)
  and [pixel scale](https://developer.apple.com/documentation/screencapturekit/sccontentfilter/pointpixelscale).
- Apple [fixed-display AVCaptureScreenInput](https://developer.apple.com/documentation/avfoundation/avcapturescreeninput/init(displayid:)),
  [pixel crop rectangle](https://developer.apple.com/documentation/avfoundation/avcapturescreeninput/croprect)
  and [screen input controls](https://developer.apple.com/documentation/avfoundation/avcapturescreeninput).
- FFmpeg [AVFoundation device contract](https://ffmpeg.org/ffmpeg-devices.html#avfoundation)
  and [upstream input selection implementation](https://github.com/FFmpeg/FFmpeg/blob/master/libavdevice/avfoundation.m).


## Current implementation checkpoint

Portable geometry/identity/clock/slot models passed six initial optimized tests;
subsequent packed-pixel, completion and display point/pixel calibration checks
bring the focused core set to thirteen after the profile-admission regression.
`target/macos-video-worker-final.log` records 40 passing optimized worker/protocol/
private generated-video tests with two separately invoked cases ignored. They
include retention of the source leader through encoder/probe flush and parent
loss during both encoding and probing. `target/macos-record-tests.log` records
12 passing recorder tests including raw-video argv with no camera selector.
These are portable/private plumbing evidence, not macOS capture acceptance.

The actual SCK/AX/default-window core compiled on both real macOS ARM and Intel
SDK runners after correcting CF binding types. Later AVFoundation probes exposed
feature-gated public crop/scale and audio authorization APIs; those use their
real existing binding features, never guessed selectors or SDK stubs. The
newest display/cursor and parent recorder paths still require the next complete
SDK/app check. Native prompt-free permission, cursor/shadow/Retina pixel and
recording timing gates remain explicit until their owned native fixtures pass.

The separately invoked generated-source optional-runtime test passed FFV1,
48-kHz synthetic PCM audio and GIF through actual installed FFmpeg/ffprobe,
including an independent decoded-pixel oracle. FFV1 preserves the generated
opaque primary colors exactly. GIF uses the existing fixed RGB8 palette, so its
oracle compares the nearest encoded palette entries rather than claiming
lossless source RGB. The log is `target/macos-video-codec-e2e.log`.

`scripts/test-macos-capture.py --self-test` passed its independent FFmpeg
PNG/FFV1/GIF oracles, including rejection of a modified middle video frame. Its explicit
`--owned-session` native gate compiles `tools/MacCaptureFixture/main.swift`,
opens only that fixture's generated window/content, checks exact physical
window/display ROI rows, retained AX scrolling and finite camera-free FFV1/GIF
recording with optional generated 48-kHz audio. It uses existing authorization only and exits 77 if the native
platform/session or Screen Recording authorization is unavailable. Cursor
visual acceptance requires a fully contained, unchanged native pointer footprint.
The fixture paints an explicit half-alpha patch and verifies native preservation
plus the chosen opaque-background blend. Full-shadow extent and microphone
acceptance remain reported as unpassed, rather than treated as established by
invoking the source path. The
owned native gate has not run on this Linux host.

Apple documents `SCDisplay.width`/`height` in points. Display capture therefore
binds `SCDisplay.frame` to the admitted logical geometry and independently
derives physical output from `SCContentFilter.contentRect × pointPixelScale`,
requiring agreement with the native display-mode dimensions. The application
does not guess or resize a mismatched frame. An explicit queue depth of one
would contradict the stream configuration's documented minimum, so the
screenshot path leaves that property at its native default. No `SCStream` is
created; `SCScreenshotManager` documents one returned image, but does not publish
its internal compositor-buffer count. Stream queue-depth requirements alone
do not establish that the still API allocates three frames. Only one returned
app-owned image is retained; native/compositor buffers are additional active memory,
released with the helper and not measured as reference-hardware acceptance.
See Apple's [SCDisplay dimensions](https://developer.apple.com/documentation/screencapturekit/scdisplay/width)
and [queueDepth contract](https://developer.apple.com/documentation/screencapturekit/scstreamconfiguration/queuedepth).

The CPU check admits known simultaneous CGImage/provider/decoded bytes against
`2*S + 2*E + 32 MiB`. `E` is the selected region's actual byte count, rather than
assuming a full-display output. Small crops from a 4K source fit this check;
much larger source/small-output combinations can be refused before native
pixels. A refused job reports active-image admission, never silently resizes
or drops displays. macOS26's screenshot configuration has no stream
queue setting; display capture uses that API with exact calibrated physical
dimensions. The earlier public one-CGImage path remains for older systems.
Tight-buffer arithmetic is conservative admission of known representations;
native row-stride, IOSurface/driver metadata and whole-process peaks still need
actual native measurement. The 256-MiB general ceiling and PLAN budgets remain
unchanged. The GPU formula `S + E + 32 MiB` remains a native measurement gate;
neither an undocumented still-buffer count nor compile success establishes it.

Native window preparation now retains the actual SCK filter, PID/start-time and
display proof set through admission and capture. Aggregate preflight reserves
physical `contentRect × pointPixelScale` bytes, rather than XCap's logical
`CGWindowBounds` width/height. A shadow-including request whose full native
extent is unavailable before capture reserves the entire existing 256-MiB job
ceiling as bytes. It cannot share a batch with another nonempty target; no fake
geometry or guessed shadow margin is used. Decode/provider/stride limits still
apply to the returned native bitmap. The legacy path reconciles one containing
display's native pixel scale; ambiguous geometry fails explicitly. The prepared
source is operation/main-thread owned, with no retained source/filter at idle.

Still batches and whole AX scrolling operations also use the existing native
CG display-reconfiguration observer once per helper operation. Endpoint proof
checks and the event flag must both pass, including after pixel work, so a
transient topology change cannot be accepted merely because geometry later
returns to its original values. This adds only operation-scoped callback/runloop
metadata; no polling, repeating timer or resident observer is installed.
The raw Rust-main-thread helper also encloses Mac native operations in explicit
Objective-C autorelease pools; retained native sources are dropped within the
operation and only Rust pixels/summary data leave those scopes.

The focused aggregate-byte regression passed after this preparation change
(`target/macos-physical-admission-tests.log`), including an unknown native extent
before/after another frame, arithmetic overflow and an unchanged budget on
failure. GNU Windows core/tests metadata also passed after the shared private
budget change (`target/macos-window-admission-windows-check.log`), preserving
the non-Mac source arms. Configured all-target core Clippy passed
(`target/macos-core-clippy.log`); remaining warnings are not treated as native
runtime or performance evidence. The newest prepared source/topology/pool
changes still require the next actual macOS SDK snapshot and owned runtime
gate; this Linux check does not compile Mac-only branches.

## Fresh source-review corrections

The first review found that the macOS parent-loss watcher read from a cloned
protocol socket. That could consume valid `VIDEO_START`/`VIDEO_FINISH` bytes and
terminate an otherwise valid recording. It now watches the authenticated direct
parent with `EVFILT_PROC/NOTE_EXIT`, checking parentage before/after registration,
and cancels/joins with an independent `EVFILT_USER/NOTE_TRIGGER`. It has no
reference to the control transport, performs no protocol reads/peeks and has no
periodic timeout. The existing 128-KiB watcher thread remains operation scoped;
one close-on-exec kqueue FD replaces the cloned protocol descriptors. An
eight-run Mac-only regression exercises normal START/FINISH with the production
watcher enabled. Parent-loss fixtures cover both encoding and probe flush. See
Apple's [kqueue process contract](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/kevent.2.html)
and public [event constants](https://github.com/apple-oss-distributions/xnu/blob/main/bsd/sys/event.h).

The AV producer now registers three exact-session Foundation notifications
before source start: interrupted, runtime error and unexpectedly stopped.
Callbacks synchronously mark the existing state terminal and wake its native
loop; an immediately ended interruption cannot recover within the PTS slack.
The explicit own-stop flag permits the normal stop notification. After native
stop and the sample callback barrier, observer removal closes/drains a short
active gate before delegate/writer/session teardown. Late copied blocks retain
safe Arc metadata and cannot mutate retired state. There is no new runtime,
notification queue, polling or idle observer. Two Mac-only permission-free tests
use a private notification center and empty, never-started sessions: owner
filtering/terminal failure, expected stop and removal. Their real CF loop runs on
the Rust test thread without doing GUI/capture work; production main-thread
guards remain unchanged. The existing Foundation binding enables only its
`NSOperation`/`block2` features, with no package/version addition. Apple's
[interruption](https://developer.apple.com/documentation/avfoundation/avcapturesession/wasinterruptednotification)
and [runtime-error](https://developer.apple.com/documentation/avfoundation/avcapturesession/runtimeerrornotification)
notifications are treated as failure, not source-timing heuristics.

The owned native recorder gate now independently decodes every frame, rather
than trusting summary/nonempty-file checks. The fixture paints nine-bit row
barcodes using opaque RGB primaries. FFV1 pixels must match the exact admitted
physical ROI (the existing two-code-value native sRGB tolerance); GIF pixels
must match the corresponding declared RGB8 palette levels, allowing one
adjacent level for the encoder's error diffusion/reserved transparency entry.
Primary bits remain distinct, and alpha must stay opaque. The bounded decoder
admits the expected frame count plus one to detect extra frames. The portable
self-test encodes/decodes actual FFV1/GIF and deliberately corrupts the middle
frame in each oracle; it passed. It does not establish that the native gate ran.

All three changes still require a fresh source recheck and real Mac SDK/native
permission-free test execution. Native TCC, physical ROI/cursor/AX/AV capture,
microphone, shadow and full CPU/GPU budgets remain unaccepted on this Linux host.

Round two confirmed those corrections and found one remaining color defect:
missing source metadata could still select an sRGB fallback. The fallback is
removed from both the native sample admission and the raw BGRA converter.
Every frame now requires its real three-component RGB `CGColorSpace`, a copied
native ICC profile (132 bytes through 4 MiB, exact declared length, RGB/ICC
signature), and a non-HDR native space. A transfer label, including `sRGB`, is
never sufficient evidence for primaries/profile. Missing transfer metadata is
accepted only when the complete native profile supplies the interpretation;
present HDR/unknown transfer labels are refused. CoreGraphics retains
responsibility for valid native profile conversion into the explicit sRGB
destination. No color space is substituted for an untagged frame.

The portable admission test covers no profile with absent/sRGB/linear labels,
empty/short/oversized/malformed profile metadata, valid RGB metadata and
HDR/unknown transfers. Actual Mac permission-free bitmap tests additionally
require null/unprofiled device-space rejection, unchanged tagged sRGB pixels
and a real tagged Display-P3→sRGB conversion. Only the portable test ran here;
the native cases require the next actual Mac run. Accepted ICC metadata adds at
most 4 MiB per active frame and is released before frame handoff, with no cache
or idle allocation. The native copy call can allocate before its returned size
is checked; its native/driver cost remains part of the measured resource gate.
Apple documents [ICC profile copying](https://developer.apple.com/documentation/coregraphics/cgcolorspace/copyiccdata())
and [native HDR identification](https://developer.apple.com/documentation/coregraphics/cgcolorspace/ishdr());
both APIs predate the supported macOS12 baseline (10.12 and 10.15 respectively).

## Shared coordinate consumer integration

The [coordinate consumer contract](macos-coordinate-consumers.md) now covers the
actual headless region/template/repeat/analysis/global-point and Desktop paths.
Ordinary listing remains permission-free integer Quartz hints; explicit native
geometry admission retains UUID, full point bounds, oriented raster and filter
scale. Complete 1–32-display enumeration and independent native inventory checks
prevent a stable 16-display subset from becoming partial Desktop success.
Legacy ambiguous templates return static typed rebind error 17; permissions
13–15 and colour policy 16 retain their meanings. Crops and transformed rasters
discard the original desktop grid metadata. These source changes do not inherit
earlier SDK results: the next frozen-source macOS SDK and hardware gates remain
required, alongside the recorded actual Linux workspace/private checks.
