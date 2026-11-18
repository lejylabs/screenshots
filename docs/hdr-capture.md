# Native HDR capture and declared SDR output

The capture/editor format remains straight RGBA8 sRGB. These adapters acquire
higher precision and perform one explicit SDR rendition; they do not promise
HDR file preservation, mastering reconstruction or physical display matching.

macOS modern screenshots request tagged extended sRGB. ColorSync converts into
a bounded linear float context with native tone mapping explicitly disabled.
The context sets EDR target headroom to zero (unknown), then uses ordinary
`CGContextDrawImage`; its setter is resolved dynamically and must succeed for
HDR. This preserves the extended linear values for the single application SDR
rendition. `CGToneMappingNone` is not used: the macOS 15 SDK describes SDR
clipping for that mode, and actual native tests rejected unknown source headroom.
Native region cropping precedes conversion while the complete original backing
and actual stride remain charged. Unchanged SDR RGB8 retains its byte path;
SDR P3/profile input retains native profile conversion. Unknown/device profiles
and unsupported raw transfers fail explicitly.

Windows uses one finite WGC RGBA16-float frame only for a proven Advanced Color
source. Matching all-SDR mirrored targets retain GDI; ambiguous Advanced Color
mirrors fail. Modern/unknown metadata failures cannot silently select GDI.
Legacy fallback needs both an observed pre-HDR version and absence of the modern
User32 API. Source color, reference white, geometry and ROI are frozen in each
prepared capture. The whole native-source group is admitted before pixels;
normalized earlier outputs never discount those immutable native charges.
Proofs are checked again before acquisition and publication.

Reference white becomes linear 1. An extended container alone does not activate
HDR compression: content must exceed relative luminance 1 + 1/1024. Uniformly
for the admitted output frame, luminance Y = .2126R + .7152G + .0722B stays
unchanged through .75. Above .75 use .75 + (Y-.75)/(1+4(Y-.75)), scaling RGB by
the luminance ratio. Then clip linear gamut, apply the IEC sRGB encoding curve
and round half up. Alpha is unpremultiplied and rounded independently; transparent
RGB becomes zero. Nonfinite values or invalid alpha fail without output.
Different independently captured extents may activate different renditions if
only the larger extent contains a highlight. A later editor crop reuses its
already converted frozen source.

Actual source stride/depth counts toward the 256 MiB native-source ceiling.
macOS source S, float context F and output E coexist only when
S + F + E <= 2S + 2E + 32 MiB. Shadow extents and returned stride are rechecked.
Windows retains one GPU frame, one reusable FP16 staging texture (at most 4 MiB
logical, 8 MiB actual mapped pitch) and the final RGBA8 output. The tiled path
removes the former 32 MiB output refusal while preserving source admission.
HDR analysis and conversion read the same frozen frame; a late highlight affects
every tile uniformly. Borrowed row spans exclude pitch padding and uncopied
edge texels. Incompatible per-display
reference whites cannot be guessed. Native driver/scratch allocation and GPU
accounting remain measured gates, not exclusions from PLAN.

All work uses existing operation-owned capture helpers, cancellation, topology,
deadline and reap ownership. Synchronous native calls remain bounded by their
outer helper lifecycle. Portable loops check cancellation every 64 KiB. No
new idle thread/runtime, timer, image cache, capture stream at startup, PNG/disk
handoff, automatic upload or sensitive-frame log is introduced.

Local development evidence: optimized Linux all-target and real Windows GNU
workspace/all-target metadata pass. All 89 selected optimized capture controls
pass (four explicit opt-ins ignored), including fixed SDR/HDR numeric vectors,
all 63,490 non-NaN binary16 values, padded rows, mirrored SDR, legacy refusal,
source-proof transitions and native-charge accounting. Optimized all-target
Clippy passes. Actual source reviews are recorded separately for macOS R2,
Windows R2 and the later Linux packet. Metadata and portable math do not prove
native acquisition. Native macOS CG fixture/SDK/link, WGC device/fault/runtime,
HDR/P3/SDR/mirror hardware, mixed DPI/topology/permission/cursor, cancellation,
CPU/private/GPU memory, latency and idle/soak gates remain open.

Actual ARM macOS snapshot `98c9bff` failed the original native float fixture;
that failure is retained. A separate owned C/CoreGraphics diagnostic on both
macOS 15 ARM and Intel confirms zero target headroom preserves every synthetic
float component, including 12.5, for source headroom zero and 16. Target headroom
one demonstrably maps the tagged highlights instead. The corrected production
Rust fixture additionally checks source headrooms zero, two and 16, full images
and native crops. The corrected ARM macOS job for snapshot `5d12c475` passes,
including all 32 selected native capture controls and the exact float fixture.
Its source matches the fix committed as `a3652ed`. Intel verification remains
pending. The diagnostic uses
only constructed images and does not request screen/clipboard access. Exact
logs and SDK header contracts are retained under the HDR integration artifacts.

The later Windows tiled extension has two clean independent source reviews.
Its four optimized portable controls pass, covering independent numeric vectors,
single/multiple padded tiles, late highlights, cancellation, exact source ceilings
and overflow/edge geometry. Instrumented Windows GNU workspace/all-target checking,
project-policy Clippy and formatting pass. Native WARP texture/cancellation
acceptance is queued separately; real WGC/HDR hardware, 4K before/after latency,
private/GPU peaks and the PLAN regression budget remain unverified. Two-pass
readback is a resource tradeoff, not a claimed performance improvement.

The integrated Linux adapter extends proven SPA color metadata while preserving
legacy RGB8 negotiation. Portable RGB, installed-SPA filtering and real private
PipeWire color/refusal/dual-format cases pass; exact evidence and producer limits
are recorded in [finite Wayland capture](wayland-finite-capture.md). Exact current source/checkpoint evidence lives under
`target/verification/hdr-integration/` and the independently reviewed frozen
packets under `target/verification/hdr-capture-delivery-held/`.


Legacy X11 GetImage capture now verifies the returned window visual against its
setup screen/depth and requires TrueColor RGB masks. It uses native image byte
order, pixmap bits per pixel and padded scanlines, with checked protocol extents,
source/wire lengths and RGBA admission before indexing. This accepts proven
RGB565, packed RGB888, XRGB8888 and depth-30 RGB101010 packing; other byte-aligned
TrueColor layouts use their validated masks. Missing/non-TrueColor/invalid format
proof fails. The existing Composite alpha-preservation route is unchanged;
legacy output remains opaque. Integer channel rescaling into RGBA8 is SDR
quantization. Neither depth 30 nor a TrueColor visual proves sRGB, PQ, HLG, HDR
tone mapping, a wide gamut, ICC conversion or physical display color matching.

The exact vendored pure decoder is also included through an app-core test seam.
The owned authenticated `scripts/test-x11-native-pixels.py` fixture paints literal
native colors on separate depth-16/24/30 Xvfb servers and checks odd-width root
regions plus window capture against independent RGBA literals. Its source exists;
compilation, pure tests and the native fixture have not been run for this change.
Optimized before/after latency/private-memory comparison and real X11 color/
hardware/cancellation acceptance remain open. Existing capture-process deadline,
cancel, stale-result and reap ownership applies; the legacy XCap API exposes no
row cancellation callback, so this decoder adds no in-process cancellation claim.
