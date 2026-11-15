# Cooperative cancellation during history thumbnail reduction

Author: NewtTheWolf
Date: 2026-11-15
Status: implemented; focused core/lifecycle checks and measurements passed.
Final workspace and artifact evidence is recorded in the delivery log.

## Smallest complete slice

Make the existing history thumbnail reduction observe its existing cancellation
callback during pixel work. `load_png_thumbnail` currently checks before PNG
loading, after loading and after `DynamicImage::thumbnail`, but that last call
can scan the entire admitted image without observing cancellation. Leaving
Library, changing pages or starting a higher-priority capture therefore stops
publication but does not promptly stop obsolete thumbnail processing.

Both existing UI callers already supply the needed signal:
`CaptureWorkspace::request_history_thumbs` passes `history_thumb_cancel`, and
`preview_history_image` passes `history_preview_cancel`. Their revision/identity
checks reject stale results. Reuse those paths with the control-state repair
below; no rendering, design, additional worker, timer or framework is added.

`clear_history_thumbs` currently forgets the active token and loading marker
while the worker still runs. Retain both until matching completion, while
clearing visible cached images immediately. The existing callback must clear
admission, suppress canceled/old-revision results and request the latest page.
Both initial `close_application` busy checks must include the thumbnail loading
marker and cancel that job, so Quit waits for the same completion guard as
`finish_quit_if_idle`. Do not treat revision changes as identity-stale early
returns that would prevent the next page from starting.

This slice does not claim general image-job cancellation. Blur previews and
pixelation preparation already poll their effect cancellation callback. Final
editor rendering, export downscaling, PNG decode/encode, workflow operations,
native I/O and several copy loops need separate work. `export_redaction` closes
the editor before rendering and has no equivalent reusable processing token;
Save/Copy shutdown also deliberately drains acknowledged output. Do not wire
the editor's close flag into that output path and cancel every requested save.

## Compatibility contract

Keep the public API and result/error type of
`load_png_thumbnail(path, cancelled)` unchanged. It remains a PNG-only worker
operation, with no output files, clipboard/history writes or source mutation.
Keep its existing pre-load and post-load checks, then replace only the
uncancellable thumbnail reduction. Cancellation uses the existing fixed
`Preview canceled` error; callers continue to discard it as they do today.

Create private core helper:

```rust
pub(crate) fn history_thumbnail_rgba(
    source: &image::RgbaImage,
    cancelled: impl Fn() -> bool,
) -> anyhow::Result<image::RgbaImage>;
```

It accepts nonzero source dimensions whose RGBA bytes fit the existing 256 MiB
admission, with backing length exactly `width * height * 4`. Validate before
allocation and reject malformed backing. The result remains at most 400×240
RGBA, with no upscaling, and must be byte-identical to the pinned image 0.25.10
thumbnail operation used by the existing loader.

Preserve that operation's dimension calculation: first set bounding dimensions
`bw = min(source_width, 400)` and `bh = min(source_height, 240)`. Compute in f64
`r = min(bw / source_width, bh / source_height)`, then round each scaled source
dimension to the nearest integer and clamp its minimum to one. Preserve the
existing rounding, including highly asymmetric images; do not replace it with
flooring, a new nearest-neighbour policy or the export resampling kernel.

Preserve the pinned downscale sample windows as well. In f32 calculate
`xr = source_width / output_width` and `yr = source_height / output_height`.
For output `(x,y)`, set `left = ceil(x*xr)` and
`right = ceil(x*xr+xr)`, clamping left to `[0,source_width-1]` and right to
`[left,source_width]`; calculate bottom/top equivalently from y and yr. Retain
this addition order and floating-point precision. Average every channel
independently over `[left,right) × [bottom,top)` with integer rounding
`(sum + count/2) / count`. The RGBA downscale-only dimensions make these windows
nonempty; assert this in tests and fail explicitly if violated in production.

This is ordinary per-channel averaging, including alpha and hidden RGB. Do not
switch to alpha-weighted averaging, zero RGB when alpha is zero, linear-light
resampling, or a different filter in a cancellation change. At equal dimensions
the same loop copies exact RGBA values; tiny and one-pixel axes retain their
existing behavior. Wider checked sums are acceptable only when parity fixtures
show identical results for every admitted sample size.

Use a small local specialization because upstream's generic thumbnail API has
no per-pixel cancellation callback, and splitting the image into independent
tiles changes sampling origins/rounding. This is not a second general resizing
backend: do not expose arbitrary output sizes or use it for export/workflows.
Document its compatibility pin to image 0.25.10 beside the helper and retain an
oracle against the installed upstream implementation. On future image upgrades,
review that oracle and the pinned behavior deliberately.

The algorithm is derived from image 0.25.10
`src/imageops/sample.rs::thumbnail`/`thumbnail_sample_block` and
`src/math/utils.rs::resize_dimensions`. Retain the upstream attribution and
license notice in the helper; use the upstream MIT option and include its full
MIT text in `THIRD_PARTY_NOTICES.md`. The dependency's packaged license files
continue to ship through the existing packaging process. Do not copy unrelated
generic interpolation code or remove license/provenance information.

## Cancellation and resources

Check cancellation after validation and before allocation, at the start of
each output row, before the first source sample, after every at most 4096 source
pixels read across all windows, and once after processing before returning.
Return immediately when observed. Callback invocation counts are not public
API; callers must treat the callback as a cheap, side-effect-free flag read.
The result is private until full success, so cancellation drops partial pixels
and returns no thumbnail. Source ownership remains with the caller and is
released normally when `load_png_thumbnail` unwinds.

The contract bounds work between checks, not wall-clock time: at most 4096
source pixels plus fixed local arithmetic once sampling starts. Allocation,
PNG decoding and native file operations cannot be interrupted by this helper.
Keep no hard millisecond deadline or eager watchdog thread. Ordinary idle work
remains zero and cancellation adds only callback/loop-counter overhead to an
already active reduction.

Memory stays one admitted decoded source, one result of at most 384,000 bytes,
four channel accumulators and scalar loop/check state. No source clone, integral
image, row cache, second thumbnail, extra encoded copy or GPU resource. Validate
actual optimized peak memory and timings rather than claiming the allocation
model is measured evidence. The [PLAN performance contract](../PLAN.md#performance-contract--all-versions-and-features)
and thumbnail priority/admission rules stay unchanged.

The existing `load_saved_png` path still relies on its installed PNG decoder and
its existing image limits; this work does not repair all file-open, encoded-byte,
metadata-allocation or native-stall concerns in that older loader. Do not expand
format support or quietly claim the new helper bounds decoding itself.

## Files and verification

Create `crates/app-core/src/history_thumbnail.rs` with the private helper and
focused tests; register it in core `lib.rs`. Modify only the reduction call and
related cancellation fixtures in `crates/app-core/src/clipboard.rs`. Preserve
the public wrapper's PNG loading and before/after checks. No rendering, CLI,
workflow, dependency or feature change is required. Update
`THIRD_PARTY_NOTICES.md` for the derived code and the relevant thumbnail
description in PLAN/README plus delivery/status evidence after actual checks.
Also modify `crates/app-ui/src/lib.rs` only for the admission/Quit control-state
repair above and `crates/app-ui/src/ui_tests.rs` for deterministic held-job tests.
These are operational lifecycle changes, not a UI redesign or new controls.

Focused tests must cover:

- Exact upstream-oracle dimensions and RGBA for every width/height pair 1–32,
  odd sizes, 400/401 and 240/241 edges, 4K, panorama/portrait/one-pixel axes,
  equal-size images, and 32 randomized cases using seed 0x54_48_55_4D and
  dimensions 1–1024. Include
  alpha 0/128/255, unrelated hidden RGB and a hand-computed rounding case.
- Maximum 256 MiB source admission and one-byte-over/invalid backing rejection.
  Exercise very skinny dimensions and source-window arithmetic without
  introducing an unbounded fixture set. For the maximum source, use one
  operation-scoped fixture rather than retaining both production/oracle peaks.
- Immediate cancellation before allocation, cancellation inside a single huge
  output sample (so an output-row-only check fails), and cancellation just
  before return. A test-only sample observer or loop seam may count source work
  deterministically; do not add production instrumentation or timing-dependent
  sleeps. Prove no more than 4096 source samples occur between flag observation
  opportunities and no partial result escapes. Preserve source bytes.
- The existing `thumbnails_are_bounded_cancellable_and_keep_pngs_unchanged`
  fixture and the existing headless history thumbnail/preview tests. Update
  brittle tests that assume exactly three callback calls to assert behavior
  rather than preserve that accidental count. Verify canceled/stale jobs do
  not cache a thumbnail or strand the one-job admission slot.

Run targeted optimized tests, formatting, Clippy and Windows compilation; then
the affected workspace regression. Extend or reuse the opt-in synthetic
`history_preview_4k_cost` measurement to compare upstream reduction and the new
helper over identical already-decoded pixels, as well as end-to-end PNG preview.
Report median/p95 and retained/peak memory separately. Measure cancellation with
deterministic work counters; a few elapsed-time observations are not a hard
cancellation deadline. No desktop capture, clipboard change, real history,
network account, new UI control or broad hardware benchmark is needed.

Hold an actual job at a test-only barrier without sleeps, change page/hide/reopen
repeatedly, and assert that a second decode is not admitted. Release it and
verify the current page starts and the obsolete result is discarded. Test Quit
while held: cancellation is set and the app waits until completion.

Use two warmups and ten measured interleaved 4K reductions per implementation;
report median/p95 with the ten raw samples, and GNU time peak RSS on three
separate end-to-end synthetic processes. Keep reports under
`target/verification/thumbnail-*.json` and summarize completed evidence here.

The first atomic-callback measurement exposed an 81% slowdown in a per-pixel
control loop. Bounded row chunks reduced the measured overhead to 2.03% median
and 2.60% p95: ten interleaved samples after two warmups measured upstream/new
4.443/4.533 ms median and 4.495/4.612 ms p95 for already-decoded 4K input, with
byte-identical output. Cancellation still observes at most 4096 sampled pixels
between checks. Three separate end-to-end fixture processes measured peak RSS
75,056/75,536/75,300 KiB; each process includes fixture generation and five
iterations of decode, thumbnail and editor input. These are development
observations, not private-memory or reference-hardware acceptance. Raw samples,
commands and source/output sizes are in `target/verification/thumbnail-performance.json`.
