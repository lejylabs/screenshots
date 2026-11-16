# macOS capture coordinate consumers

Status: bounded design and portable model implementation, 2026-11-16. Shared
execution-path integration is held until the current frozen verification run
finishes. This document does not establish native SDK, TCC or hardware acceptance.

## Scope and invariants

All existing headless display/region, repeat/exact-region, monitor-bound template,
OCR/QR-region and screen-color consumers must use the same admitted native pixel
extent. Their per-display regions remain top-left, display-local native pixels.
The native display UUID and ID, complete Quartz point bounds, native display-mode
extent, public filter scale and already oriented raster extent travel together.
Before/after proof equality and the existing operation-wide topology witness
reject changed or restored topology. No second pixel rotation is performed.

`MacDisplayCoordinates` stores a fixed 16-byte UUID and is `Copy`, so the optional
metadata can extend existing `DisplayInfo`/`DisplayGeometry` without changing their
ownership model. Wire fields default to absent and omit absent values; existing
Windows/Linux metadata retains its current coordinate calibration and wire form.
An optional proof is validated against display ID, origin, scale, rotation and
raster dimensions before use, and rejected on platforms where it is inapplicable.
Mac arithmetic uses the typed full point rectangle. Legacy integer origin fields
are accepted only when the native origin is exactly representable; native point
origins are never multiplied by a display-specific scale.

ScreenCaptureKit admission independently compares the selected SCDisplay frame
with CG bounds, then derives oriented pixels from the filter's `contentRect` and
`pointPixelScale`. Its complete point extent must agree with the selected display.
The native mode's dimensions must agree directly, or at a quarter turn agree in
the opposite axis order. The filter and actual image establish the raster axis
order: rotation metadata alone cannot select an orientation. The legacy source
retains its unrotated restriction when independent oriented geometry is absent.
Recording and targeted scrolling retain their separately documented restrictions.

Missing legacy template proof may be upgraded only after independently observing
an unrotated 1:1 native raster with identical legacy geometry and the template's
bound UUID. Ambiguous legacy data fails with an explicit stale/rebind result.
Current templates include the full native geometry; moving a stable monitor may
retain its display-local crop only through explicit binding resolution and fresh
proof admission. Exact/repeat regions retain the admitted UUID and point layout.

Ordinary `list displays` and source inventory remain permission-free CG/XCap
metadata observations. On macOS their absent typed proof explicitly means
unverified Quartz point dimensions, and JSON labels origin/size space and native
verification status. Those legacy integer fields are explicitly informational
point hints, including when native bounds have fractional components; they never
drive Mac pixel arithmetic. `list_capture_displays_with_cancel` uses a separate typed
`PixelDisplays` helper request for consumers that require native pixel geometry.
It performs no-prompt Screen Recording preflight and owns the existing finite
capture deadline and topology witness. Template binding, region analysis and
global point sampling use this explicit admission; ordinary listing never
initializes ScreenCaptureKit or requests permission.

## Global tools

The existing `screen-color --global` option continues to mean physical desktop
pixels. macOS cannot invent that coordinate plane from mixed-density Quartz
layout and rejects the unsupported meaning. Explicit `--global-points` accepts
Quartz global points on macOS, labels them `quartz_global_points`, and maps them
through the selected display's typed rectangle to a local native pixel cell.
The selected cell is `floor((point-origin)*pixels_per_point)` on each axis, with
half-open display edges. Local screen-color coordinates remain native pixels.
No coordinate origin is scaled before subtraction, and the operation does not
silently convert a physical-unit option into a point-unit option.

## Derived desktop grid

macOS desktop output uses the explicit `quartz_max_native_scale` policy. The
layout retains the Quartz arrangement, normalizes its top-left point, and uses
the maximum admitted native pixels-per-point density as one common output grid.
Every output edge must map to an integral grid boundary within 1/1024 pixel;
unsupported fractional placement fails before pixel allocation. Overlapping or
mirrored Quartz rectangles fail, as do duplicate display IDs or UUIDs.

Sources at that density copy their exact RGBA rows, including alpha. Sources at
lower density expand with the same center-aligned nearest-neighbor rule as the
existing raster resize operation. The sampler writes directly into the final
image, avoiding a temporary resized-display image. This policy preserves the
highest-density native raster and deliberately repeats lower-density pixels.
The output is a derived Quartz layout grid; it is never described as an OS-global
physical desktop raster. Transparent holes remain transparent. Sequential display
acquisition does not claim atomic cross-display content.

The isolated child returns the actual policy, grid density, Quartz origin,
admitted display proofs and output placements from the same operation snapshot.
The CLI describes this metadata without enumerating a second parent snapshot.
No frame is published until all captures, actual-image checks, final native
identity/geometry comparison and the topology witness pass. The existing typed
Desktop request remains one isolated operation with parent deadline/cancellation.

## Resources and verification

Admission remains 1–32 displays, at most 256 MiB aggregate decoded native source
pixels and at most 256 MiB final output. Sparse output bounds and oversized native
sources fail before image allocation. Layout metadata is bounded by 32 proofs and
placements; the compact proof avoids per-region UUID string allocations. The
compositor retains the supplied immutable sources and allocates one final image.
Native OS source buffers remain additional measured resources under the existing
active-image contract. No dependency, cache, queue, thread, resident framework,
timer, optional stream, PNG round trip or disk handoff is introduced.

Existing native still capture currently decodes the full source before a local
crop. Its CGImage/provider/decode admission additionally requires source RGBA
bytes minus selected RGBA bytes to be at most 32 MiB. A full display can fit the
256 MiB source ceiling while a tiny region on that display fails this stricter
active-memory check. The coordinate change preserves that limit; it does not
claim efficient native subregion acquisition on large 5K/8K sources.

Copying and resampling share the existing 64 KiB work/cancellation budget across
short rows. Cancellation discards the incomplete in-memory output. Existing helper
admission, raw frame count/aggregate bytes, authenticated transport, parent loss
and process-family teardown remain required. Metadata is validated before raw
pixel allocation and cannot weaken the 256 MiB protocol admission.

Portable fixtures vary native mode dimensions, logical point bounds, filter
extent/scale and actual pixel arrays independently. They cover Retina local/global
mapping, negative origins, quarter-turn orientation without pixel rotation,
mixed-density layout without false overlaps, exact source pixels/alpha, transparent
holes, malformed frames, sparse/fractional/overlapping/duplicate layouts, topology
proof changes and cancellation across copy/resampling rows. These are model/source
checks, not evidence of physical capture or native SDK linking.

Native Intel/ARM SDK checks, signed permission denial/authorization, physical
Retina/mixed-density rotation, reconnect/sleep/restored topology, native image
orientation and optimized footprint/latency acceptance remain separate gates.
The current GitHub permission failure prevents scheduling new macOS SDK CI; a
Linux portable harness cannot turn that gate green.

Primary API contracts: Apple's [filter point rectangle](https://developer.apple.com/documentation/screencapturekit/sccontentfilter/contentrect)
and [point-to-pixel scale](https://developer.apple.com/documentation/screencapturekit/sccontentfilter/pointpixelscale).
The exact cached objc2 CoreGraphics 0.3.2 binding declares `CGDisplayBounds` as
`CGRect`, mode pixel dimensions as `usize`, and rotation as `c_double`; these
signatures establish ABI shape, not axis-order acceptance on rotated hardware.
