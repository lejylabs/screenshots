# Finite portal capture, recording and manual scrolling

Linux CLI and typed Task sources use explicit ScreenCast portal selection and
its restricted PipeWire descriptor. No capture runs at startup. A selected node
is pinned by object serial, and format, exact raster, transform, data descriptors
and bounds are revalidated. Finite modes accept the negotiated raster itself;
they do not infer physical monitor coordinates or silently resize a selection.

```sh
screenshots record portal-display OUTPUT.mkv --seconds 2 --fps 5 --width 1920 --height 1080
screenshots record portal-window OUTPUT.gif --seconds 2 --fps 5 --width 1280 --height 720
screenshots scroll-capture-portal window OUTPUT.png --width 1280 --height 720 --frames 4 --interval-ms 500
```

The normal recorder's explicit audio options remain available; no microphone or
system audio is selected implicitly. Recording is finite (1–60 seconds, 1–30 FPS,
with the existing tighter GIF limits). Manual scrolling samples a finite source
sequence through the existing bounded seam assembler. The user scrolls the
selected source outside the app. It injects no session-wide pointer/keyboard
input and does not claim automatic target-bound scrolling: the standard portal
input interface does not provide the required atomic target/focus proof. Task
`portal_scroll` uses the same request, validation, sealing and publication path.

Encoder startup is an authenticated phase. Only after VIDEO_START does the
adapter establish its CLOCK_MONOTONIC cutoff, clear retained preflight pixels
and flush the consumer queue. Source Header metadata is mandatory; positive,
ordered, nonfuture PTS must cross that cutoff before pixels can establish the
fresh slot. This admits an explicit absolute-monotonic producer profile, not all
possible compositor timestamp conventions. Unknown or stale source timing fails
closed, with a five-second fresh-frame deadline. Callback/decode arrival alone
is never source-freshness evidence.

One active image job, two admitted CPU buffers and finite source-frame/encoded
queues retain the existing limits. Width/depth/stride and actual mapped size
are checked before slices or output allocation. Active sampling timers exist
only during the explicit operation. Ownership, cancellation, outer native
watchdog and process-family termination/reaping reuse the capture worker and
borrowed raw-video group. Caller deadlines do not promise release of a
kernel-stalled process: NativeCleanupPending retains its owner/admission.
Native source cleanup precedes final publication; later staging/directory
failure truthfully identifies an already-published no-clobber destination.

## Native color negotiation

Legacy RGB8 bytes retain their previous path. Managed FP16/FP32, packed 10-bit
orders and tagged wide-gamut RGB require full-range/RGB matrix plus supported
primaries and transfer fields. Those properties are MANDATORY on managed offers;
a separate legacy RGB8 offer defaults to UNKNOWN and does not invent tags.
Thus an untagged float/RGB8 producer can select the previous SDR format, a
float-only missing-proof producer is refused, and a fully tagged float source
can retain precision. D65 primaries, fixed PQ 203-nit normalization and declared
HLG reference policy feed the same explicit SDR rendition in [HDR capture](hdr-capture.md).
Unsupported/partial-alpha proof is refused; legacy behavior is unchanged.

No full float-image expansion is made. Finite FP32 permits 32 MiB per native
buffer; FP16/RGB8 retain 16 MiB. Two native buffers, retained/new RGBA and encoder
conversion must fit the actual-depth PLAN image budget; the 256 MiB source
ceiling and other limits are not raised. Driver/compositor allocations remain
additional measured costs, never hidden from resource reports.

## Executed development evidence

Optimized Linux all-target checking and Windows GNU workspace/all-target
metadata pass. Two portable RGB tests cover all 29 independent numeric cases;
15 Portal integration/preflight tests pass (three opt-ins separate). The actual
installed SPA C filter oracle passes in both directions and for omitted proof.
Private EXTERNAL-only D-Bus/PipeWire tests use generated pixels and synthetic
audio, never the user compositor, desktop, clipboard or microphone.

The base permission/FD/serial/error fixture passes. The finite fixture passes
real CLI/worker/encoder MKV plus synthetic audio, GIF, manual seams, Task routing,
old queued/quiet/replay/unknown/new-frame controls and failure/no-output cases.
Five native color sources, three dual/float-only negotiation scenarios and all
four individual missing-color-property refusals pass. These are fixture runs,
not a claim that each internal assertion is an independent harness test.

The separate 30-sample synthetic 3840×2160 run (three warmups) observes p95 46 ms
capture and 91.31 ms complete CLI including PNG. It verifies exact pixels,
authorized FD, serial and closed sessions. This software producer and mock consent
exclude real compositor/permission latency, HDR hardware, reference CPU/GPU/RAM,
long recording/audio synchronization, physical DPI/cursor, idle and soak budgets.
Named logs, executable/source identities and failed test-parameter attempts are
retained under `target/verification/wayland-native-capture-integration/` and
`target/verification/hdr-integration/linux-round2/`. Full final workspace/native
platform and resource acceptance remain separate.
