# One in-memory Wayland portal frame

Author: NewtTheWolf
Date: 2026-11-16
Status: implemented; optimized pure/protocol checks and private D-Bus/PipeWire
generated-pixel execution pass. Fresh source review, native compositor permission
acceptance and reference-hardware performance remain separate gates.

## Required behavior

Provide an explicit portal-selected display/window capture source through
ScreenCast and PipeWire. Capture one admitted frame in RAM, stop the stream and
close the session, then return pixels to the existing recipe/output pipeline.
No Screenshot/ScreenshotArea call, PNG staging, screenshot URI, temporary image,
automatic upload, clipboard change, permission persistence or idle stream is
allowed. The existing worker isolation does not fix those concerns by itself.

The previous xcap 0.9.10 Wayland still implementation tried GNOME and
portal Screenshot PNG files under a global D-Bus mutex before wlroots. Those
still-image disk fallbacks and their unused PNG/URI helpers are now removed. Its
Wayland video recorder is not a safe shortcut: it starts a long-lived thread,
uses an unbounded frame channel, connects to the ambient PipeWire server instead
of the portal's restricted remote FD, and does not provide this still-session
lifecycle/stride/admission contract.

The explicit headless source is
`capture-portal display|window OUTPUT [--cursor exclude|include] [--delay-ms 0..3600000] [--recipe FILE]`.
PNG/JPEG/WebP follow the output extension. It uses the existing image exporter
and bounded recipe implementation. A typed Task source is also available:

```json
{"version":1,"source":{"type":"portal","source":"window","cursor":"exclude"},"actions":[{"type":"save","path":"capture.png"}]}
```

The Task/host/sealed execution path shares the same enum and cancellation-aware
core API; it does not pre-initialize a portal when validating a task. Reuse existing
recipe/export/upload actions rather than adding a second task engine. Delay and
all output/profile validation happen in the parent before worker startup or
permission access. Common cursor exclusion/inclusion maps to portal Hidden/
Embedded capability; unsupported modes fail explicitly. Native frame/shadow/
transparency policies that ScreenCast cannot request must not be silently
ignored or advertised as native window controls.

OS permission/source selection is required behavior, not application UI design.
Cancellation is normal; denial returns a useful fixed permission/source error
without an empty-image success. The unattended tests never display that dialog
or touch the user's permission store.

## Portal/session and source identity

Create the maintained ashpd proxy only inside the operation-owned Linux worker.
The pinned 0.14.0 package has one documented local serde field-name correction
for the specification's `pipewire-serial` key; no underscore alias is accepted.
Original package source/license and the patch rationale are in `vendor/ashpd`.
Inspect interface version, AvailableSourceTypes and AvailableCursorModes before
requesting sources. Create one session, select exactly one source of the requested
type, set `multiple=false` and `PersistMode::DoNot`, and start it with no app
window parent. Retain proxy/session/response ownership until frame capture and
explicit Close finish. Do not save or restore permission tokens.

Obtain the FD from **OpenPipeWireRemote** and pass that FD to PipeWire's
`connect_fd_rc`; connecting to a default server or matching a global desktop
stream is forbidden. Require one returned stream and reject conflicting returned
source_type metadata. Portal position/size describe compositor coordinates and
are not physical pixel coordinates. The output dimensions come only from the
negotiated video format; no existing monitor ID, stable physical binding or
cross-monitor crop is inferred from those properties.

Use the v6 `pipewire-serial` property with `PW_KEY_TARGET_OBJECT` when available;
never keep using a reusable node ID after a known serial is available. For v3–5
compatibility, resolve the authorized node's object serial from the restricted
remote registry, bind that serial and abort if the source disappears or changes
before the admitted frame. Missing/ambiguous targets fail rather than selecting
another node. A v6 response missing/mismatching its advertised serial must fail.
This is one portal-selected source, not a claim of globally atomic topology or
saved physical-monitor identity; older Start-to-registry observation gaps remain
explicit native compatibility checks.

The [ScreenCast specification](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html)
defines the session/remote-FD flow, logical coordinate caveat and v6 serial-based
targeting. Minimum supported interface/cursor/source compatibility must be
verified in focused mocks and native compositors before a broad support claim.

## PipeWire admission and first frame

Run the native PipeWire main loop on the worker thread handling the call. Its
native event FD is integrated with the active async wait, session-close signal
and deadline; there is no repeating poll timer. Create
no permanent receiver/runtime in the parent. Negotiate CPU-readable packed
RGBx/BGRx/RGBA/BGRA8 only, with positive admitted dimensions and the existing
256 MiB final RGBA ceiling. Request at most two stream buffers and fail if the
actual admitted buffer count/extent exceeds its bound. Reject DMA-only buffers
that cannot be mapped safely; do not add an eager EGL/GStreamer fallback.

Parse the actual negotiated format. Validate data type, plane count, mapped
extent, chunk corruption flags, offset, size and stride before making a slice or
copying rows. Require enough bytes for `(height-1)*stride + width*4`, checked
addition/multiplication and an admitted total mapped extent. Unsupported negative
stride/planar/modifier layouts fail explicitly. Handle BGR swapping and RGBx
opacity; inspect premultiplied-alpha flags rather than guessing alpha encoding.
Explicit HDR/PQ/HLG/wide-gamut or high-bit formats are rejected until a genuine
color pipeline exists. Unreported RGB8 color remains the existing documented
assumption, not new HDR or color-correctness acceptance.

Buffer metadata is deliberately strict: a valid video crop must cover the full
negotiated frame at origin, and video transform must be identity. Header flags
may contain only DISCONT and MARKER; delta, corrupt, gap and unknown flag bits
are refused. Format changes before the accepted frame also fail. The adapter
does not silently crop, rotate or present an incomplete delta as a still image.

Keep one output raster, no frame queue. A first valid buffer is copied once,
requeued/released, and causes stream deactivation/main-loop exit. Later callbacks
cannot overwrite that result. Node/core/stream errors, target removal and
session closure before completion return failure. A single operation-owned timer
bounds frame waiting at 20 seconds; it is removed with the loop. Close the portal
session before delivering pixels to the parent. Cleanup failure cannot be called
a fully completed capture; helper exit/parent-death containment still applies.

Add a separately explicit **120-second overall portal deadline**, including
permission selection, to the existing capture-worker policy. A frame wait after
permission has at most 20 seconds and cannot extend that overall deadline. User
cancel/close kills the owned helper and its PipeWire/D-Bus connections; no late
frame can reach a new task/session. This policy does not promise unlimited human
permission-input time under the ordinary 20-second still-call limit.

Peak active representations are at most two admitted native stream buffers plus
one worker RGBA raster; native stream/session resources are released before the
parent transfer. The existing authenticated raw channel then holds the worker
and parent raster during transfer. These costs count against PLAN's active-image
contract; OS/driver/GPU allocations are additional. No image/process/runtime is
retained in normal idle.

## Eliminate the old disk path without changing target meaning

Remove the GNOME/Screenshot PNG fallback from the vendored still path. Preserve
the already in-memory wlroots path where its session/coordinate admission is
valid. An existing explicit display/window ID must not be silently replaced by
a fresh portal picker choice. If that legacy source cannot meet its requested
identity/coordinate contract, return an explicit unsupported/source message
pointing to the new portal-selected source. Update capability/help/matrix text
for that deliberate behavior change; do not hide it as an implementation detail
or call unsupported older compositor paths accepted.

Named monitor templates, exact physical desktop regions, native scrolling input
and resident integration remain separate Wayland obligations. This source closes
the actual single-frame memory path, not every Wayland feature family.

## Implemented files and resource cost

`crates/app-core/src/portal_capture.rs` defines the source/cursor enums and
cancellation-aware executor entry. `native_capture_job.rs` validates the typed
Portal call and fixed source identifiers without inventing physical geometry.
The desktop worker dispatch/deadline and strict raw protocol carry that call.
`apps/desktop/src/capture_portal_linux.rs` owns ashpd/PipeWire/session lifetime;
`capture_portal_pixels.rs` admits/decodes packed CPU frames. The CLI helper is
`capture_portal.rs`; Task source preparation/sealing uses the same core API.
The coordinated main/CLI wiring adds no application UI controls.

Direct Linux dependency edges expose already-locked ashpd 0.14.0 and PipeWire
0.10.1 plus async-io 2.6.0/futures-lite 2.6.1 for finite event/deadline waits.
They initialize only in the operation-owned helper. The ashpd source patch
changes one serde name, preserves its original license and records provenance.
No unrelated package version was resolved.

At admission limits the two native buffer extents can total 512 MiB, with at
most one 256-MiB worker output. A 4K RGBA frame is 33,177,600 bytes: two native
buffers plus output are 99,532,800 bytes of admitted representations. Once the
native context/stream is released, raw transfer holds the worker and parent
rasters. These are representation limits, not process RSS or guarantees about
allocations inside PipeWire/the OS before its add-buffer callback. Native driver,
codec, allocator and GPU allocations remain additional. No aggregate
active-memory acceptance or reference-hardware budget is established here.
Normal idle retains no portal worker, session, stream or optional runtime.

## Verification

Pure decoder tests cover every packed format, padded rows, alpha encoding,
offset/size/stride overflow, exact pixel/alpha preservation and admission edges;
malformed/unsupported layouts never allocate an oversized output or publish it.
Protocol/session tests cover source/cursor capability refusal, cancel/deny,
multiple/mismatched streams, missing/reused serials, node removal, invalid FD,
missing frames, deadlines and session cleanup. Source inspection must prove
Screenshot/ScreenshotArea and image-file deletion are absent from this path.

Run actual end-to-end capture against **two private services**: a fresh D-Bus
daemon with a mock ScreenCast portal, and an isolated PipeWire daemon/publisher
producing only generated pixels. The mock returns a real FD connected to that
private server. Set the ambient PipeWire remote to an unavailable name so using
it accidentally fails. Test v6 serial selection and the stated older-interface
compatibility, exactly one finalized image, independent pixel decoding, no
temporary screenshot files/permission persistence, and complete helper/session/
stream teardown. Force timeout and parent loss while the private stream is
stalled; no replacement or late result is allowed.

The fixture never connects to user PipeWire/audio, requests user desktop input,
registers OS services/timers or captures a real display. Native GNOME/KDE/wlroots
permission/color/DPI/compositor acceptance remains a separate explicit gate.
Measure optimized active latency/memory with generated frames, keep targets
unchanged, and record any repeated >10% regression before merge.


## Completed focused evidence

Optimized worker/protocol/packed decoder checks passed (31 regular tests; one
unrelated X11 timing case was skipped in `target/portal-unit-checks.log`). Six
subsequent focused portal checks in `target/portal-decoder-final.log` distinguish
a generic portal failure from a denial and verify that accepting the FD sets close-on-exec even
when the inherited flag was cleared. Core source/cancellation checks passed
(two tests, `target/portal-core-tests.log`). Two regular executable/fixture tests
passed with the actual native case separately invoked
(`target/portal-cli-checks.log`). GNU Windows application/tests metadata compiled
(`target/portal-windows-check.log`); this is compile evidence, not native Windows
or macOS execution.

`target/portal-private-e2e-final.log` records actual isolated native execution:
v6 display/window and v3 registry-serial captures match independently decoded
RGBA pixels, including hidden RGB/alpha. Logical size/position do not replace
pixel dimensions. The independent libdbus mock encodes `pipewire-serial` using
the standard hyphen; the underscore spelling, missing/wrong serial, multiple or
wrong sources, source/cursor capability refusals, denial, invalid FD, closed
session and Close failure publish no image. A stalled real stream exercises the
20-second frame deadline. Killing the CLI proves loss of the helper's unique
D-Bus name/session with no late output. Disconnecting the admitted source before
its first frame fails promptly. A typed Portal Task applies a crop before its
single final output; named XWayland capture never opens a replacement picker.
The mock is on a fresh bus with no activation directories, and its PipeWire
server has only software/native-protocol modules. Generated test processes have
parent-death guards and bounded outer waits. No user service or permission store
is used.

The serial optimized 4K report is
`target/verification/portal-4k-performance-final.json`. Three cases each use three
warmups and 30 exact-pixel captures on private software services. Binary/test
images are pinned by hard links; process samples check start time, comm and
executable identity before/after each smaps read. Application process sums are
separate from the source publisher, private daemons and PNG pixel oracle.

| Metric (median of three case results) | Observation |
| --- | ---: |
| Native portal setup/first-frame p50 | 21 ms |
| Native portal setup/first-frame p95 | 50 ms |
| Complete CLI including transfer/final PNG p95 | 91.327 ms |
| Maximum observed application RSS/PSS sums | 106,004 / 88,386 KiB |
| Maximum observed entire private-fixture RSS/PSS sums | 195,932 / 146,553 KiB |

The worker image SHA256 was
`b11eb067169adff7bfdea0ad5add4f0cacf973d9f04b9c111a80b505ebb6382c`;
test image SHA256 was
`e0a310646d19bcd896f63f32d4f6df65e0a9acd76fd3bed847067416cf4afe80`.
The preceding `target/verification/portal-4k-performance.json` report measured
50/54-ms native p50/p95 and 96.393-ms CLI p95. Its maximum observed application
RSS/PSS was 107,144/88,909 KiB and whole-fixture sums 229,832/163,507 KiB. Both
reports preserve all cases; the software source has a 33-ms frame cadence, and
these differing samples are not a claim of a production optimization. Native
elapsed values have whole-millisecond precision. Complete CLI observations use
finite 5-ms exit checks and can include their scheduling overshoot.

Every case observed 70 processes over its lifetime; all were gone at completion.
No swap was observed in these cases, which does not establish general zero
swap. Sums are sequential near-simultaneous samples, can miss peaks and count
shared RSS in multiple processes; PSS/RSS are not private-footprint guarantees.
The whole fixture includes its source and output decoding, not just the app.

The host was Ryzen 7 9800X3D, 16 logical CPUs, approximately 30.4 GiB RAM/swap,
Linux 7.3/CachyOS and performance governor. Mock consent is immediate; this is
not native GNOME/KDE/wlroots permission latency. This is a new source with no
equivalent prior in-memory portal baseline; comparing it to the removed PNG
fallback would not verify this contract. The 250-ms target is unchanged and
these observations do not pass hardware/end-to-end or aggregate memory gates.
