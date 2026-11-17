# Native HDR capture and declared SDR output

The capture/editor format remains straight RGBA8 sRGB. These adapters acquire
higher precision and perform one explicit SDR rendition; they do not promise
HDR file preservation, mastering reconstruction or physical display matching.

macOS modern screenshots request tagged extended sRGB. ColorSync converts into
a bounded linear float context with native tone mapping explicitly disabled.
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
Windows retains one GPU frame and bounded staging/output. The current Windows
slice refuses output larger than 32 MiB pending the separately prepared tiled
readback extension; it never partially captures it. Incompatible per-display
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

The separate Linux adapter extends proven SPA color metadata and preserves
legacy RGB8 negotiation; its integration and native results must be recorded
separately. Exact current source/checkpoint evidence lives under
`target/verification/hdr-integration/` and the independently reviewed frozen
packets under `target/verification/hdr-capture-delivery-held/`.
