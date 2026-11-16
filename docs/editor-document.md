# Bounded editor document and renderer

Status: implemented backend, focused verification/source review in progress,
2026-11-16. The parent/Elephant approved this contract before implementation.
Scope: the remaining editor data/render backend from the user's list. New GPUI
controls and final product design are excluded. A headless consumer verifies the
shared backend; it does not establish that those controls exist in the editor.

## Existing behavior and problem

The editor's EditState stores crop, base orientation, independent marks, shared
text/pen assets and bounded metadata undo. Drawing objects have an order, while
blur/pixelation still sample original pixels in prepasses. Base orientation is
restricted once marks exist. Core TextAnnotation accepts a prepared immutable
mask; GPUI currently supplies native shaping with a fixed family/discrete sizes.
Legacy source-over, hidden RGB at alpha zero, source-pixel geometry, crop edges
and redaction-last behavior have established fixtures and must remain intact.

The shared core model represents ordered draw/effect objects, affine
transforms, rich styles, reusable presets and the missing tools. It must be
usable by an editor controller and by a deterministic executable fixture without
duplicating rendering logic. Source images remain immutable; undo stores
metadata/shared assets and never rendered full-frame snapshots. Only explicit
final render/export publishes pixels.

## Model and coordinate contract

EditorDocument owns an immutable CapturedImage, an admitted asset bank, current
DocumentState and bounded metadata undo/redo. State contains canvas dimensions,
an affine placement of the base, optional crop, independent ordered layer
instances and the chosen export preset. Layer IDs are stable bounded integers.
Each layer has local geometry, one local-to-canvas affine transform, explicit
straight RGBA styling and a typed payload. All numbers are finite and range
checked; invertibility and transformed bounds are checked before raster work.

EditorState owns the actual metadata transitions/history without requiring
pixels. EditorDocument delegates edits to it; the scene consumer uses that same
implementation with a declared AssetCatalog for sequential preflight. No dummy
image or second transition engine is used. Invalid IDs, ordering, crop,
dimensions, anchors and undo/redo-dependent sequences fail during inspection
and before source/font/raster decode.

Coordinates describe pixel boundaries, with sampling at pixel centers. Default
base placement and nearest sampling retain exact RGBA bytes, including RGB in
transparent pixels. Source-over uses the existing integer straight-alpha rule.
Nontrivial image interpolation uses premultiplied samples and returns straight
RGBA; it never mixes hidden RGB from transparent samples into visible pixels.
Transforms support translation, scale, rotation, reflection and shear with an
explicit pivot. Canvas resize changes extent/anchor; document scale/rotation
transforms the base, marks, privacy masks, crop and sampling source mappings
together. Moving a clone object alone leaves its chosen source mapping fixed.

Vector geometry is constructed relative to a geometry-derived local origin,
then that origin is folded into the affine translation in f64 before f32
rasterization. Explicit path/arrow origins come from their points, independently
of declared layer bounds; rectangle/ellipse/bubble origins come from their actual
shape rectangle. The affine linear part is retained, including reflected and
nonuniform stroke transforms. Text outlines remain in their prepared local
frame and share the same raster admission check. Vector tiles use a fixed canvas
grid and fixed dimensions; only the copy intersection depends on the layer
extent. Changing an explicit path's declared bounds cannot change tile phase.

The native f32 affine must remain nonsingular, and positive strokes must not
underflow to a zero-width native hairline. Before raster work, an operation-scoped
point scan bounds path/coefficient/product/sum and tile-translation rounding
using 32 f32 epsilon allowances on the sum of absolute coordinate terms,
including stroke width. This is conservative for the fixed construction and
affine arithmetic stages; admissible coordinate error is at most a quarter
pixel, inside the existing one-pixel traversal gutter. Tile translation uses the
actual first/last canvas tile origins, so a small mark at a large canvas position
is not rejected merely for its absolute position. Unstable spans/transforms
return an explicit rasterizer-precision error rather than successful clipped
output. This does not promise exact real-number antialiasing.


The old drawing APIs and existing GPUI controls retain their renderer. A
programmatic legacy import produces the equivalent document: original effects
first, existing drawing order next, active redactions final. Identity legacy
groups use the established ordered paint kernel. Affine legacy objects use
bounded annotation-only in-memory stamps, with shared text masks counted in the
asset ledger. No stamp contains sampled original pixels. New RGBA objects add
behavior without changing the old palette/width/crop interpretation.

## Ordered tools

The layer payload inventory is concrete:

- Rectangle, ellipse, arrow, pen/path, highlight and spotlight with arbitrary
  validated RGBA fill/stroke, stroke width and per-object affine placement.
  Arrowheads and singleton path markers use active stroke ink; an absent or
  zero-width stroke falls back to the fill.
- Rich text retains literal text/style/font identity. Each render prepares real
  glyph outlines into temporary RGBA and per-span coverage rasters; those rasters
  are dropped after composition and are not retained assets or undo snapshots.
  Text supports explicit family/face selection,
  size/weight/italic, multiline alignment/wrapping, underline/strike and RGBA.
- Speech-bubble geometry with bounded rounded corners and a tail. Bubble and
  text layers can be inserted in one metadata edit transaction/undo operation.
- Inserted raster objects for images, logos, clipboard pixels and emoji. The
  caller explicitly supplies their pixels and semantic label; no network,
  clipboard read, file open or emoji download happens inside the renderer.
  Color emoji are genuine RGBA objects, independent of monochrome font glyphs.
- Blur and pixelation at any point in the layer order default to the previous
  sanitized composite. The explicit ProtectedBase input preserves old
  original-source effect semantics during legacy import, including overlapping
  blur/pixelation. It samples only the protected base; failure never switches
  inputs. Radius/block size is in canvas pixels and remains fixed when an effect
  mask is transformed; edit its strength separately when desired.
- Clone and magnifier objects with explicit source/destination mappings and
  nearest or premultiplied interpolation. Magnifiers support an elliptical lens
  and styled outline; their zoom is actual resampling, not metadata alone.
- Eraser with explicit transparent-clear or restore-sanitized-base behavior.
  Clearing an annotation can restore the base without exposing protected pixels.
- Opaque redaction masks. These participate in selection/transform metadata but
  always enforce privacy independently of ordinary layer placement/order.

Tool/effect presets are typed data: named bounded styles and payload defaults.
Applying one changes metadata in one validated edit transaction. Export presets
hold format/quality preference, canvas padding, RGBA background, bounded alpha
drop shadow and an explicit raster watermark mapping. They reuse the same final
render/export path and do not encode paths, macros, credentials or implicit I/O.

## Format-specific output admission

The final metadata planner validates the dimensions after crop and export padding
against the same application policy used immediately before actual encoding.
The scene is refused before source/font/raster I/O when a known limit fails:

| Output | Known geometric/input limit |
| --- | --- |
| PNG | Positive axes and the existing 256 MiB RGBA ceiling |
| JPEG | Axes at most 65,535 and the existing 256 MiB RGBA ceiling |
| Lossless WebP | Axes at most 16,384 and the existing 256 MiB RGBA ceiling |
| Explicit lossy WebP | Axes at most 16,383 and the existing 64 MiB RGBA input ceiling |

JPEG axes derive from the encoder's u16 representation. The locked image-webp
0.2.4 encoder writes 14-bit dimensions minus one and exposes no public maximum;
the single application policy uses that limit and real thin-axis encoding
fixtures guard compatibility. Lossy admission reuses libwebp's exposed dimension
constant and the existing application input cap. The final planner delegates to
these shared validators instead of duplicating their constants. Existing native
encoder checks, output-byte limits, quality/transparency checks, cancellation and
create-new publication remain in place. Pixel/content-dependent encoding refusal
can still occur after metadata planning; inspection does not promise a successful
filesystem write or a particular compressed size.

## Privacy and sampling

Rendering starts with a protected source sampler. Every active canvas mask is
projected back through the base placement into a conservative source-pixel
footprint, using projected quadrilaterals and pixel intersections instead of
blackening entire bounding rectangles. Those source texels return opaque black
before interpolation. This prevents scaled/rotated base sampling from blending a protected original pixel
over a mask edge and later carrying it into a clone or lens elsewhere.

The current composite is made opaque black at active masks before any operation
that samples it and after the final layer. Clone, magnifier, blur, pixelation,
shadow and restore-base erasing never obtain a raw-original escape path. Effect
snapshots are temporary in-memory sanitized buffers. Ordinary later drawing or
an eraser cannot remove the final opaque masks; mask removal itself is an
explicit undoable edit, as in the existing editor.

The reserved base image cannot be named as an ordinary inserted-raster asset in
an untrusted spec. Explicit external images remain user-provided content; the
renderer cannot infer that an arbitrary external image was copied from an older
unsanitized file. Asset APIs carry known source provenance and do not turn an
internal original into a supposedly sanitized raster shortcut. No undo/source
pixels are written into history, helper temp files or diagnostic reports.

The executable consumer rejects the opened source object reused as a raster,
including hardlinks/symlinks and a hardlink created after a version check. Stable
native dev/inode or volume/128-bit-file-ID equality is distinct from full version
equality; the original handle stays open during comparison. Full version checks
remain in cache reuse and prepared-input validation.

## Bounds and lifecycle

All admission precedes large allocation or mutation. Initial limits:

| Resource | Limit |
| --- | --- |
| Source plus distinct retained raster/mask assets | 256 MiB actual backing allocation, including Vec capacity slack and assets retained by undo/redo |
| Layers | 100 combined draw/effect/privacy instances |
| Undo/redo | 100 combined states and 4 MiB counted metadata; prune oldest complete states |
| Geometry | 8,192 shared pen/path points per document; finite bounded affine values |
| Raster assets | 64 unique admitted immutable assets; deduplicate shared backing identity |
| Font data | 8 MiB total, 2 MiB per face source, explicit bytes/index only |
| Text | 4,096 UTF-8 bytes per object, 32 KiB document text, 64 lines; no controls except tab/newline |
| Rich-text render raster | 1 MiB RGBA output plus a 1 MiB temporary coverage raster, included in render scratch; no retained cache |
| Legacy text masks | Immutable shared alpha assets, charged in the retained raster ledger |
| Presets | 32 entries per family, 64-byte names, strict typed fields |
| External document spec | 256 KiB, strict version/fields/counts; asset IDs, not implicit file/network expressions |
| Blur/shadow | Radius at most 64; bounded scratch and declared work cost |
| Sampling | No recursive reference to the current/later layer; only prior sanitized composite |

S is the total admitted source/raster backing actually retained by the document;
E is the selected output's RGBA extent. Planned peak raster, metadata, font and
scratch ownership is checked against PLAN's `2*S + 2*E + 32 MiB` before render.
Every individual RGBA allocation also passes the existing source-size guard.
The first implementation may render the bounded canvas before exact crop; if
that work extent cannot fit this formula it fails explicitly before allocation.
It does not silently increase the budget, return a partial image, or allocate
an unbounded full-canvas cache. This is a deliberate bounded refusal for large
canvas/small-crop combinations; a tiled dependency renderer is not smuggled into
this change. Existing legacy crop remains on its existing direct-crop renderer.

There is at most one output canvas and one reusable stage snapshot during
ordered effects. Vector raster/alpha scratch are included in admission, reused
or dropped between stages. Export crop/padding/shadow planning accounts for
overlapping buffers rather than adding all stage maxima permanently. The asset
ledger covers current, undo and redo references; failed edits leave all three
unchanged. Render snapshots are immutable and can outlive a UI edit without
publishing stale state. Closing the document releases its original/assets.

Core image traversals share one ImageWork budget, including short rows, masks,
sample footprints, vector coverage/composition and effect kernels. Work is
canceled before/after bounded text-shaping runs and individual outline draws,
and charged throughout application-owned loops. Optional font/layout resources
are constructed only for explicit preparation and dropped afterward; no idle
font database scan, service, watcher, cache or worker thread is added. Existing
consumer admission provides the one-heavy-image-job slot and stale-result IDs.

## Real font preparation

Prefer the already-locked maintained HarfRust 0.15 and Skrifa 0.49 family instead
of a new native font runtime or unbounded Swash bitmap cache. The proposed path
uses explicit font bytes, Unicode bidi/script/grapheme runs, real shaping and
Skrifa outline drawing into bounded tiny-skia masks. Fallback uses only caller-
supplied admitted faces, with a truthful missing-glyph error; no system font
enumeration or filesystem fallback. Color/bitmap/SVG font tables are outside
this outline path; explicit RGBA emoji/image objects cover those insertions.

Font parsing checks face/table extents, face/glyph counts and outline support
before preparing shape caches. HarfRust's own preprocessing allowances derive
from the combined GSUB/GPOS/GDEF byte extent; their documented source formula is
charged as potential scratch, not assumed to be the tiny input font size. A
face whose potential scratch does not fit the operation budget is refused before
cache creation, without silently disabling its shaping features. Caches are
dropped before switching faces. Runs are capped at 128 Unicode scalars, and
glyph-buffer expansion/operation bounds are included in the plan. Unsupported
or budget-exhausted shaping returns an error rather than guessed glyphs. The
published 0.15 source skips exhausted lookups internally; its narrow local patch
exposes a sticky work/allocation-exhausted flag, including allocation arithmetic
overflow. The editor checks this flag after shaper construction and after every
shape before accepting glyphs. `vendor/harfrust/LOCAL-PATCH.md` records the exact
published revision, unchanged default behavior, MIT notice and source-derived
adversarial regression. The observer adds one atomic boolean and one optional
shared-budget reference to the existing font cache; no new runtime or idle work.

The editor additionally enables the buffer's opt-in runtime exhaustion refusal.
Legacy kern cannot force a nonadvancing state transition past an exhausted
operation allowance, and a zero-budget OpenType required recursion cannot be
silently skipped. Failure survives the engine's leave/reset of the counter and
is checked by the existing `allocation_successful()` admission before glyphs.
Natural advancing completion exactly at zero remains valid; exceeding it fails,
including the legacy fast path, and no later legacy subtable runs after failure.
The option adds one Buffer boolean and no allocations, and remains disabled for
other callers. Buffer clear retains the option while clearing failure; reset
also disables it. Unrelated `morx`/`mort`/`kerx` paths remain rejected at editor
font admission rather than silently substituting a different shaping mode.

Legacy `kern` uses a separate upstream AAT cache and therefore requires a
separate admission proof. The opt-in, allocation-free native-parser preflight
accepts formats 0/1/2/3 only when their conservative peak cache heap fits 4 MiB
and record/state/range-cell work fits 4 Mi visits. It includes actual
cache entry/ClassCache sizes, spare Vec/page capacity, state/class temporaries
and old/new reallocation overlap. The returned bytes are added to the OT scratch
charge, before any ShaperFont/cache construction. A missing bound refuses the
font; ordinary OT/Apple kerning within the limits remains real shaping.
`vendor/harfrust/KERN-BOUND.md` records the source-derived formulas and locked
private-layout/toolchain assumptions. The consumer checks cancellation before
and after the bounded preflight; no background runtime or implicit font read is
introduced.

Skrifa reports required outline scratch before drawing; the renderer supplies a checked buffer
of at most 1 MiB. A bounded outline sink caps path commands and finite geometry
and flattens curves to at most 8,192 line segments before tiny-skia allocation.
Cancellation is checked between glyphs and throughout the sink. There
is no full glyph-image cache. Glyph positioning, fallback and mask preparation
are shared by preview/export callers. Tests must include ligatures, combining
marks, RTL/mixed bidi, explicit fallback and malformed/oversized font refusal.
Measured scratch/private-memory evidence remains separate from these planned
ownership ceilings and library allowance estimates.

## Files, phases and coordination

New core files, owned by this implementation:

1. `crates/app-core/src/editor_document.rs`: public state/transactions, asset
   ledger, bounded undo/redo, validation and render snapshot API.
2. `crates/app-core/src/editor_document_types.rs`: strict spec, affine/geometry,
   RGBA, typed tool/effect/export presets and transformations.
3. `crates/app-core/src/editor_document_render.rs`: protected source/stage
   sampling, ordered drawing/effects, cloning/lens/eraser and final masks.
4. `crates/app-core/src/editor_document_text.rs`: explicit bounded font
   admission and per-render shaping/layout/outline raster preparation.
5. `crates/app-core/src/editor_document_tests.rs`: independent pixel/privacy,
   state/asset, transform/order/alpha/crop/cancel and performance fixtures.
6. `crates/app-core/src/editor_document_text_tests.rs`: real font fixtures and
   malformed/resource/cancellation cases using existing licensed repository fonts.
7. `crates/app-core/src/editor_document_legacy.rs`: validated programmatic legacy
   objects, annotation-only stamps and asset-bank-bound text-mask leases.
8. `crates/app-core/src/editor_document_state.rs`: shared metadata transitions,
   bounded history and declared asset-catalog validation, without pixel backing.

New consumers/tests:

9. `apps/desktop/src/editor_document.rs`: a bounded strict-spec headless
   consumer, explicit existing image/font input admission, current cancellation,
   shared core render and create-new final save. It does not duplicate tools.
10. `apps/desktop/tests/editor_document.rs` and
   `scripts/test-editor-document.py`: actual executable fixtures, independent
   FFmpeg/Python pixel checks and failure/no-publication checks.
11. `crates/app-core/src/file_identity.rs`: stable native object comparison for
    source alias refusal; existing version bytes/equality remain unchanged.
12. This contract records exact proof, resource cost, review and remaining native
   or UI acceptance as work completes.

Parent-coordinated changes only: core lib module/exports, narrow direct edges to
already-locked maintained font/Unicode packages, desktop main/CLI routing/help
and package evidence. No file-wide changes to selection/layers/text/pen or
redaction_editor, and no existing UI control rewrites are required. If a legacy
paint helper needs reusable visibility, propose the exact change first.

Phase order is model/admission/metadata transforms, privacy-aware raster/effect
ordering, remaining tools and presets, real text preparation, then executable
integration and optimized evidence. New model files can be written while the
parent's integrated checks run; compiled shared files remain frozen until
coordinated wiring. No commits are made by this agent.

An optional Recipe/Task step is not required for the backend's first consumer.
If desired, its seam is a frozen PreparedEditorDocument (admitted spec plus
explicit immutable assets) applied to the current finalized in-memory pixels
with the same cancellation, returning final pixels. It must not reread a mutable
profile, save a temporary PNG or recover the original capture after failure.
Task/Recipe owners approve and wire that seam separately.

## Executable consumer

`screenshots editor-document inspect SCENE.json` validates metadata and reports
counts without opening image/font assets or printing literal label text.
`screenshots editor-document render INPUT SCENE.json OUTPUT` uses the same
EditorDocument/EditorSnapshot backend. It creates a new explicit output, preserves
INPUT and performs no history, clipboard or upload action.

The strict scene wrapper is limited to 512 KiB and contains `version: 1`, a
`document` conforming to DocumentSpec, optional `rasters`/`fonts` path manifests,
an optional PresetBook, an optional named `export_preset` and up to 100 edits.
Relative asset paths are relative to the scene file. Asset references are
validated against the declared manifest before decoding. Repeated opened files
share their admitted raster/font backing. Images retain the existing input
codec/32 MiB encoded-file policy and share the remaining 256 MiB decoded budget;
font files share 8 MiB, with a 2 MiB per-source ceiling.

An optional `source_size: [width, height]` declares the input's post-orientation
dimensions and defaults to the initial document canvas. Declare it explicitly
when the initial canvas differs from the source. Reset uses those original
dimensions during metadata preflight. The actual shared decoder verifies them;
inspection validates the declaration and sequence without reading image assets.

For example, this scene adds a translucent green rectangle to an 800×600 input:

```json
{
  "version": 1,
  "document": {
    "version": 1,
    "canvas": [800, 600],
    "layers": [{
      "id": 1,
      "bounds": {"x": 20, "y": 20, "width": 120, "height": 60},
      "content": {
        "tool": "draw",
        "shape": {"shape": "rectangle"},
        "style": {"fill": [0, 255, 0, 128], "stroke": null, "width": 0}
      }
    }]
  }
}
```

Edits support insert/remove/duplicate/reorder, layer/document affine transforms,
canvas resize, scale, crop, named tool presets, undo/redo and reset. Final export
options are reapplied after the edit sequence and checked against OUTPUT before
source/asset decode or rendering; undo/reset cannot silently change the requested
file format. PNG/JPEG/lossless WebP and explicitly lossy WebP use the existing
real encoders and cancellation-aware create-new publication. JPEG retains its
existing transparency refusal unless the chosen background makes pixels opaque.

Unix SIGINT/SIGTERM and Windows console cancellation exist only for the command
lifetime. Cleanup restores termination before result output can block. The core
renderer checks cancellation during byte copies, source footprints, pixel loops,
effect kernels, bounded vector/eraser coverage tiles and outline preparation.
It returns no partial image and never substitutes an unedited original.

## Proof and acceptance

Pixel fixtures must establish real output for every tool and preset, order-
sensitive draw/effect overlaps, base and existing-object affine transforms,
rotation/scale pivots, exact source alpha/hidden RGB and full-versus-cropped
equivalence. Legacy import is compared with its established renderer. No test
merely checks that an enum or serialized property exists.

Privacy fixtures use a distinctive secret pattern in opaque and transparent
source texels. Clone, lens, reordered effects, restore-base erasing, rotation,
upscale and interpolation must not reproduce it elsewhere while masks are active.
Final masks stay black/255 even after transparent erasing or later drawing.
Asset provenance and error paths must not substitute the original on failure.

Memory/state tests count unique backing assets across 100 undo/redo operations,
reject quota/overflow/nonfinite/singular/invalid-reference specs before raster
allocation, preserve atomic state on refusal and release unused state/assets.
Cancellation must be exercised during short-wide/tall-narrow image loops,
effect sampling, vector paths, glyph preparation and final composition, with no
published partial output. A stale snapshot never alters the live document.

Actual executable E2E will prepare explicit raster/logo/emoji/font assets,
execute a strict scene containing all tools and presets, transform/reorder/
undo/redo, and export finalized pixels. Independent decoders verify geometry,
RGBA, redaction/sampling safety and source-file preservation. Invalid assets,
exhausted budgets, cancellation and existing output paths must refuse without
overwriting or publishing partial pixels.

Optimized synthetic 1440p/4K runs report render/text/effect latency distributions,
owned source/output/scratch/undo bytes and process RSS separately, plus retained
state after repeated edits/render/drop. They do not close physical presentation,
native font/IME/accessibility or reference-hardware acceptance. Core/CLI coverage
does not mark the excluded editor controls or final GPUI design complete.

## First source-review checkpoint evidence

The optimized core suite passed 20 regular editor tests, with the separate
performance observation explicitly ignored by the ordinary suite. This includes
byte-for-byte comparison with the old renderer for mixed drawing/text/pen and
overlapping original-source blur/pixelation, rotated legacy pixels, mask lease
provenance, masked clone sampling outside the export ROI, transformed source
footprints, RGBA ink, actual bubble/emoji/shadow/eraser output, presets, quota/
undo/cancellation and real Arabic/Hebrew/combining/variable-font preparation.
Two FileIdentity tests and one consumer policy test independently reproduce a
same-object/different-version hardlink interleaving; version validation still
refuses changed content while stable object comparison rejects reinsertion.
Four actual executable integration tests passed. Repository-configured Clippy
and Windows GNU core/desktop all-target metadata checks passed. These checks
do not establish native Windows/macOS runtime behavior.

The independent executable fixture completed eight renders and explicit refusal/
signal cases using the optimized binary with SHA256
`1d9915937063ff13c015fc7373e8d7ab1f65200ca3053182595a74d351a86b94`.
Python writes synthetic PNG inputs; independent FFmpeg decoding verifies actual
tool/preset/edit pixels and all four encoding modes. It checks source preservation,
original hardlink/symlink refusal, no overwrite, preset conflict before a missing
source, metadata-only inspection without font/asset reads or text disclosure,
SIGTERM cancellation without an output and termination while success stdout is
blocked. The final Linux cancellation observation was 3.699 ms after SIGTERM;
this is one development observation, not a responsiveness distribution.
No real desktop, microphone, clipboard, credentials, account or printer is used.

The optimized resource observation ran two warmups plus 100 warm iterations per
case; old/new identity copy order alternates. Mixed work includes a privacy mask,
RGBA drawing, blur, pixelation, clone and real styled text. Times exclude image
loading/encoding, UI and presentation:

| Synthetic case | p50 | p95 | p99 |
| --- | --- | --- | --- |
| 2560×1440 identity | 0.188 ms | 0.194 ms | 0.204 ms |
| 2560×1440 ordered effects/text | 12.438 ms | 13.456 ms | 14.301 ms |
| 3840×2160 identity | 0.488 ms | 0.596 ms | 0.757 ms |
| 3840×2160 ordered effects/text | 22.349 ms | 28.502 ms | 32.899 ms |

The old identity copy p95 values were 0.202/0.623 ms; these small differences are
inconclusive rather than a claimed performance improvement. Metadata edit p95
was approximately 1.2 µs across 150 edits. Raster/font leases remained bounded
through history and reached zero after the document's references were dropped.
The full test process reached 123,824 KiB RSS high water, including fixture,
font/layout and allocator costs. After fixture drops it still had 59,476 KiB RSS
versus 8,140 KiB initially. This retention is explicitly **not** a passing idle/
return-to-baseline measurement; ownership counters are not process memory.
Reference-hardware private/GPU memory, native UI/presentation, energy and long
soak acceptance remain open. No budget was relaxed to hide that distinction.

Evidence: `target/editor-document-core-tests.log`,
`target/editor-document-file-identity-tests.log`,
`target/editor-document-alias-race-test.log`, `target/editor-document-cli-tests.log`,
`target/editor-document-configured-clippy.log`,
`target/editor-document-windows-check.log`, `target/editor-document-e2e.log`,
`target/verification/editor-document-e2e.json`,
`target/editor-document-performance.log`, and
`target/verification/editor-document-performance.json`.

This is implementation/test evidence awaiting fresh EGM source review. It is not
a clean-review claim or completion of the excluded editor/overlay controls.

## First review fixes and second review checkpoint

The first fresh source review reported three technical Majors and one
documentation blocker. All four have focused fixes and regressions:

- Raster admission now charges the retained Vec capacity, including a 1×1 image
  with large unused backing, for original sources and inserted assets. Source
  metadata capacities are bounded; document Vec/String capacities are counted,
  and declared catalog strings are normalized before retention.
- Rich text stores literals/styles/font identity and prepares temporary RGBA/
  per-span coverage during each render. Only legacy alpha masks are retained
  shared mask assets. Both temporary rich-text rasters are included in the
  existing scratch allowance; no cache was added merely to match old wording.
- EditorState is the single metadata history/transition implementation used by
  EditorDocument and inspection/preflight. Invalid sequential IDs, order, crop,
  dimensions and anchors fail before any image/font decode. Declared oriented
  source_size makes reset deterministic. The shared existing decoder checks its
  oriented header before pixel allocation; default codec/PNG behavior is unchanged.
- New rotation/shear/scale-3 fixtures use a distinct secret source, bilinear
  interpolation, clone and lens. Output must equal an independently pre-sanitized
  source. Independent inverse equations select an unmasked-geometry pixel whose
  interpolation support would contain secret red; the unmasked control reproduces
  that red contribution, proving the fixture discriminates a missing protection.

The updated optimized editor suite passed 23 regular tests; nine shared-loader
regressions passed with four explicit opt-ins skipped. Six executable integration
tests passed, including inspection/render refusal of invalid edit sequences before
missing source/fonts. Configured Clippy and Windows GNU all-target metadata passed.
The expanded executable fixture passed 14 renders, four encoder modes, render-work
cancellation and no-overwrite/format-refusal cases. It did not prove cancellation
during file publication. Its pinned optimized binary SHA256 was
`79857d70c114f0794915bb8bd1fbeefaa573fe1599b302abdd0bbe51a5c74f36`;
the final Linux SIGTERM cleanup observation was 4.292 ms.

After two warmups, 100 alternating-order identity samples and 100 mixed samples
per resolution, identity p95 was 0.219/0.700 ms at 1440p/4K and mixed effects/text
p95 was 12.816/22.929 ms. Metadata edit p95 was approximately 1.2 µs. These are
development CPU observations, not a causal improvement claim or hardware/UI gate.
The complete test process reached 124,080 KiB RSS high water and retained
60,016 KiB RSS after fixture drops; the return-to-baseline/private-memory gate
remains open despite document-owned raster/font leases reaching zero.

Latest evidence: `target/editor-round2-core-tests.log`,
`target/editor-round2-loader-tests.log`, `target/editor-round2-cli-tests.log`,
`target/editor-round2-loader-check.log`, `target/editor-round2-clippy.log`,
`target/editor-round2-windows-check.log`, `target/editor-round2-e2e.log`,
`target/verification/editor-document-round2-e2e.json`,
`target/editor-round2-performance.log`, and
`target/verification/editor-document-round2-performance.json`.
These fixes/tests await a second fresh source review; no clean-review claim is made.


## Second review fixes and third review checkpoint

The second fresh source review found a shaping refusal blocker, an early-output
admission Major and a staging-file-check Nit. All three receive focused fixes:

- HarfRust's fixed 0.15 preprocessing budget now exposes sticky exhaustion across
  initial/lazy cache construction and variation instances. Exact-budget spending
  remains successful; a later underflow or allocation-multiplication overflow
  records exhaustion. Other callers keep upstream shaping defaults. The editor
  refuses before accepting glyphs when initial or lazy preprocessing exhausts.
- The font regression constructs valid aliased GPOS pair sets over the existing
  supplied OFL outline fixture. A one-subtable control shapes and renders; 512
  aliased subtables exhaust the real preprocessing budget while the glyph Buffer
  still reports allocation success. The status and editor refusal are checked
  independently. No altered font is distributed or system font read performed.
- The final prepared EditorState calls the same output_size planner used during
  rendering. Final crop, canvas edits and selected export-preset padding must fit
  the existing 256 MiB individual raster bound before any source/font/asset I/O.
  Inspection and render reject identically, even with missing input assets.
- The SIGTERM fixture checks the real `.screenshots-*.part` staging name. Its
  existing cancellation observation exercises heavy input/render work and proves
  no final output or staging remains. It does not establish interruption during
  publication; the report explicitly leaves that distinct case unexercised.

The optimized focused suite passed 25 editor tests (the explicit performance
observation remains excluded from ordinary runs), four standalone budget-source
tests and seven CLI integration tests. Configured Clippy passed with no new
editor/vendor warnings. Windows GNU all-target **dev-profile metadata** passed;
the extra release-profile Linux cross-host attempt failed at the unchanged GPUI
`shaders_bytes.rs` generation prerequisite and is not counted as passed. Native
Windows execution remains a separate CI gate. Cargo metadata confirms the four
original workspace members; HarfRust remains a dependency, not a workspace member.

Actual E2E passed 15 real renders, the adversarial-font refusal, oversized final
output refusal before missing assets, and all previous pixel/privacy/format/
provenance checks. It used pinned optimized binary SHA256
`cd770d5abc37680ae5cbf330fe37fefda803051bf49bfb0bd0d4bb8807005406`.
Heavy-work SIGTERM cleanup took 4.905 ms in that observation; the real staging
pattern was absent afterward. Publication-phase cancellation is explicitly not
claimed exercised.

Two direct optimized resource observations each used two warmups and 100 warm
samples per case. The first/repeat mixed-effects/text p95 values were
14.676/14.739 ms at 1440p and 26.170/19.349 ms at 4K. Identity p95 was
0.242/0.250 ms and 0.844/0.534 ms; unchanged legacy-copy p95 was 0.284/0.226 ms
and 0.838/0.552 ms. These uncontrolled development observations do not establish
a causal improvement or regression. The 1440p mixed p95 is about 15% above the
previous single observation and needs controlled baseline investigation before
performance acceptance; the copy/4K variation is retained rather than hidden.
No performance budget was relaxed. The complete process reached 124,508/123,856
KiB RSS high water and retained 60,228/59,620 KiB after fixture drops, while owned
raster/font leases reached zero. Native/UI/reference-hardware, private/GPU memory
and return-to-baseline gates remain open.

Evidence: `target/editor-round3-budget-tests.log`,
`target/editor-round3-core-tests.log`, `target/editor-round3-cli-tests.log`,
`target/editor-round3-clippy.log`, `target/editor-round3-windows-check.log`,
`target/editor-round3-e2e.log`, `target/verification/editor-document-round3-e2e.json`,
`target/editor-round3-performance.log`, `target/editor-round3-performance-repeat.log`,
`target/verification/editor-document-round3-performance.json`,
`target/verification/editor-document-round3-performance-repeat.json`, and
`target/verification/editor-document-round3-windows-release-check-status.json`.
Source hashes and the reading list reside in
`target/verification/editor-document-round3-checkpoint.json`.

All three second-review findings have focused fixes and evidence; this frozen
source awaits a third fresh review. It is not a clean-review claim.


## Third review fix and fourth review checkpoint

The third fresh source review found one blocker: valid legacy kern data could
build unaccounted caches outside the OpenType preprocessing allowance. The
new preflight uses the same engine parser, bounds all admitted legacy formats,
refuses malformed/progress/count errors and charges a proved conservative peak
plus finite work before constructing any cache. The font/source budgets were
not increased and default upstream shaping for other consumers is unchanged.

Core tests preserve OT/Apple AV kerning positions, exercise formats1/2/3 and
reject the real checksum-valid 80,000-subtable Apple font before retention or
cache creation. A tiny, separate high-column state-table example proves the
work ceiling independently of encoded/cache byte size. All 27 optimized editor
tests passed. The isolated allocator fixture passed and measured zero allocation/
reallocation calls on all four admitted formats, no-kern data, the large attack
and malformed input. Seven CLI tests, configured Clippy (no editor/vendor source
warnings) and Windows GNU all-target dev-profile metadata passed. Native Windows
execution remains separate CI acceptance.

Actual executable E2E passed 19 renders and the real 80,000-subtable refusal.
Both OT and Apple controls change AV pixels when their -120-unit kerning is
removed, and produce identical adjusted pixels across those two header formats.
All earlier geometry/privacy/format/state/refusal checks remain green. The
pinned optimized binary SHA256 was
`17334b3ebf56385cdb7f4c07414e4758298c207074b4838b6ef9424577f1274b`.
The heavy-work SIGTERM observation was 1.161 ms with no output or actual staging
file afterward; publication-phase interruption is still not claimed exercised.

Evidence: `target/editor-round4-core-tests.log`,
`target/editor-round4-allocation-test.log`, `target/editor-round4-cli-tests.log`,
`target/editor-round4-clippy.log`, `target/editor-round4-windows-check.log`,
`target/editor-round4-e2e.log` and
`target/verification/editor-document-round4-e2e.json`. Exact repository/vendor
and private-assumption source hashes are recorded in
`target/verification/editor-document-round4-checkpoint.json`.
The new path is operation-scoped font admission; the renderer and upstream
runtime shaping are unchanged. Earlier uncontrolled renderer timing differences,
process memory retention and native/UI/reference-hardware gates remain open.
This frozen checkpoint awaits a fourth fresh source review; it is not a clean
review or product/UI completion claim.


## Fourth review fix and fifth review checkpoint

The fourth fresh review found one Major: metadata planning applied the generic
raster bound but deferred known JPEG/WebP limits until after rendering. The final
planner now delegates to the shared format-specific admission above. The same
policy runs before native encoding and before export downscaling; native library
checks remain intact. Crop and padding are evaluated on the final prepared state,
so a larger working canvas may become valid after cropping.

Focused evidence includes both axes, the exact lossy byte ceiling, matching
inspection/render errors before missing source/fonts, final-preset padding and
PNG controls beyond JPEG/WebP axes. The optimized editor suite passed 28 tests;
eight existing shared encoder regressions passed, with their separate performance
observation excluded. Those regressions retain alpha, quality, cancellation,
callback-error and no-overwrite/source-preservation behavior. Eight executable
CLI integration tests passed.

Actual executable E2E passed 25 renders. Six additional files were really encoded
at horizontal/vertical axis boundaries: lossy WebP 16,383, lossless WebP 16,384 and
JPEG 65,535. FFmpeg independently verified their decoded dimensions. Exact 64 MiB
lossy input planning was accepted without allocating that large image during
inspection, and oversized cases refused before missing source/fonts were opened.
The pinned optimized binary SHA256 was
`dea0f8ac92f51e2bfdd4703d67348d585b7098ca5e01d639d1afe80b4a30b582`.
The heavy-work cancellation observation was 2.110 ms; publication-phase interruption
remains explicitly unexercised. Configured Clippy passed with no warnings in the
new editor/output-policy blocks; unrelated existing warnings remain. Windows GNU
all-target dev-profile metadata passed; native runtime execution remains separate.

Evidence: `target/editor-round5-core-tests.log`,
`target/editor-round5-encoder-tests.log`, `target/editor-round5-cli-tests.log`,
`target/editor-round5-clippy.log`, `target/editor-round5-windows-check.log`,
`target/editor-round5-e2e.log`,
`target/verification/editor-document-round5-e2e.json`, and the source/encoder
assumption hashes in `target/verification/editor-document-round5-checkpoint.json`.
No full-workspace, native UI, resource/return-to-baseline or reference-hardware
acceptance is inferred. This frozen fix awaits fifth-round fresh source review.


## Fifth review runtime finding and sixth review preparation

The fifth fresh source review reproduced one blocker against the frozen optimized
binary: a valid legacy format1 PUSH/DONT_ADVANCE self-loop consumed the engine's
runtime operation allowance but its forced-progress fallback still published an
apparently successful text image. The earlier cache admission remained valid;
it did not make this runtime fallback observable.

The editor now opts into strict runtime-budget refusal on its existing shaping
Buffer. Both legacy spending branches refuse negative allowance; the main branch
also refuses exactly zero when the only progress would be forced advancement.
Naturally advancing exact-zero completion remains valid. An unsuccessful strict
run does not enter later kern subtables. Required OpenType recursion skipped at
zero marks the same unsuccessful status. The existing post-shape check refuses
before any glyphs or final pixels are accepted. Published default behavior for
other clients remains unchanged; clear/reset semantics are explicit above.

Four optimized tests invoke the actual private kern and OpenType engines at
their exact counter boundaries and exercise failure through leave/clear/reset.
They passed in an ignored, source-identical harness using 848,389 bytes of actual
omitted fixtures from the pinned upstream revision. All 19 enabled package
identities/checksums match the root lock. The separate harness manifest excludes
unused Criterion/macOS test dependencies and defines a workspace; dormant optional
ICU lock entries differ but are neither enabled nor compiled. No placeholder font
corpus or extra production API was introduced. The remaining 331 upstream tests
were filtered, not counted as passed.

Evidence: `target/editor-round6-vendor-runtime-tests.log` and the exact source,
fixture URLs/hashes and active dependency identities in
`target/verification/harfrust-runtime-proof/provenance.json`.
Actual application, executable refusal, current Clippy and Windows metadata
verification are pending the coordinated rebuild; this is not a clean-review
or native/resource acceptance claim.


### Seventh-review source preparation

Fresh source review six found one additional renderer Major: minimum-size thin
arrowheads extended beyond an extent computed only from stroke width. The extent
and painter now share the exact three head vertices. Tight bounds, thin strokes
and fill-only arrows have a complete-raster comparison against roomy bounds and
explicit head-wing coverage checks. This adds no cache, worker or pixel snapshot.

The new glyph-position control initially failed to compile because the upstream
position type has no PartialEq; the fixture now compares its four public numeric
components. The corrected library suite passes. The optimized executable passed
27 render cases and real legacy runtime-budget refusal with no output/staging,
using binary `d65064c2c4c30d32ed0dbcc3bb1c1c21e645ddbcb96f1713f61c89c3c05f30df`.
That executable predates the arrow-extent correction. Its new regression/current
binary and full workspace, Clippy and Windows metadata checks are pending; no
new clean source round, publication-phase cancellation or native/hardware/resource
acceptance is inferred.


### Eighth-review source preparation

Fresh review seven found the same extent class in singleton paths: painting
uses a minimum half-pixel dot radius, which the stroke-only extent omitted
before affine scaling. Singleton extent and painting now share that exact dot
rectangle. The new regression compares every pixel for tight/roomy bounds with
zero and thin widths under scaling/shear, including an independent interior
pixel. It adds no retained buffer or geometry cache. The arrow regression has
passed; the new dot regression/current executable and complete post-fix checks
remain pending. Review seven was not a clean round.


### Ninth-review source preparation

Fresh source review eight found that an inactive zero-width stroke could still
choose the marker ink for singleton dots and arrowheads. Both now share the
active-stroke/fill fallback rule. Opaque and transparent zero-width stroke
controls compare all pixels with the no-stroke result and assert independent
green interior pixels. The singleton scale/shear regression has passed. Current
marker controls/full executable/Clippy/Windows metadata remain pending; review
eight was not clean and no convergence is claimed here.


### Tenth-review source preparation: shared vector numeric geometry

Fresh review nine found f64 extent versus f32 path/affine rounding disagreement
under valid large-origin cancellation shear. The old optimized executable
`d65064c2c4c30d32ed0dbcc3bb1c1c21e645ddbcb96f1713f61c89c3c05f30df`
reproduced 14 tight/roomy pixel differences; some occurred at area-dependent
vector tile boundaries. This old binary predates the latest geometry/marker
corrections. The private synthetic observation is recorded in
`target/verification/editor-vector-precision-before.json`.

Shape/arrow/dot/magnifier/eraser construction now uses the shared normalized
geometry, while coverage uses the shared precision admission and canonical
canvas tile grid. The existing tile maximum is unchanged: up to258×34 RGBA
pixels including its gutter. No new image, cache or persistent worker is added;
one path-point scan is charged to the operation's work/cancellation budget.
A small mark can rasterize a full existing256×32 tile rather than a narrow
partial tile. Optimized small-mark/mixed workload comparison is pending; this
extra work is not claimed free or a performance improvement.

New source fixtures compare full tight/roomy images with independent unit-square
references at positive/negative million-pixel origins, cancellation shear and
reflection; anisotropic reflected strokes are compared with ordinary-origin
controls. A separate explicit refusal covers an unstable span, a native-singular
rounded affine and stroke underflow. Executable fixtures independently decode
normalized outputs and require no final/staging file on precision refusal.
Current compilation, those new tests, executable rendering and performance are
pending the root's coordinated build. Review nine was not clean; no additional
source convergence or native/resource acceptance is implied.

### Reviewed source and optimized execution

Fresh EGM reviews 10 and 11 found no Blocker, Major or Nit; source review
converged. Both verified the frozen repository/vendor/proof hashes and actual
runtime-harness fixture/dependency identities. The corrected source passed all
38 editor core cases in the integrated run; that run stopped at an unrelated
legacy test still expecting history schema 9 instead of the new 10. The stale
expectation is corrected; the next full workspace run remains separate.

The optimized executable passed 37 successful render cases plus refusal,
no-overwrite, input-provenance and cancellation checks, including normalized
large/negative-origin geometry, bounds-independent pixels, reflection and
unstable-precision refusal. Binary SHA256:
`442d41f29aa29ee45e8b165257d46a8e554a6657a39f7c64b5c38e240bfd4ebd`.
The result is in `target/verification/editor-document-round10-e2e.json`.
Publication-phase cancellation and native/UI/hardware acceptance remain open.

A combined-change comparison against the retained pre-geometry executable first
observed one mixed-workload p95 outlier of +52.6%; the other mixed repetition was
-1.0%. That observation is retained in `editor-vector-precision-performance.json`.
Three additional alternating 100-sample-per-side repetitions on one allowed CPU,
with no competing build and recorded child CPU/fault data, gave tiny-mark p95
changes of +1.1%, -0.6%, +1.0%, and mixed-render changes of +2.1%, -0.2%, -1.5%.
Independent decoded pixels were equal for both comparison workloads. The larger
outlier was not reproduced in this controlled series; it is not attributed to a
specific scheduler or code cause without evidence. The baseline predates several
geometry/marker fixes, so this does not isolate one change. The raw controlled
record is `editor-vector-precision-performance-affinity.json`. These synthetic
measurements do not pass reference-hardware idle, memory or presentation budgets.
