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

Coordinates describe pixel boundaries, with sampling at pixel centers. Default
base placement and nearest sampling retain exact RGBA bytes, including RGB in
transparent pixels. Source-over uses the existing integer straight-alpha rule.
Nontrivial image interpolation uses premultiplied samples and returns straight
RGBA; it never mixes hidden RGB from transparent samples into visible pixels.
Transforms support translation, scale, rotation, reflection and shear with an
explicit pivot. Canvas resize changes extent/anchor; document scale/rotation
transforms the base, marks, privacy masks, crop and sampling source mappings
together. Moving a clone object alone leaves its chosen source mapping fixed.

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
- Prepared rich text retaining literal text/style/font identity plus immutable
  shared glyph-mask assets. Text supports explicit family/face selection,
  size/weight/italic, multiline alignment/wrapping, underline/strike and RGBA.
- Speech-bubble geometry with bounded rounded corners and a tail, plus a
  convenience insertion transaction for bubble and text as one undo operation.
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
| Source plus distinct retained raster/mask assets | 256 MiB actual backing bytes, including assets retained by undo/redo |
| Layers | 100 combined draw/effect/privacy instances |
| Undo/redo | 100 combined states and 4 MiB counted metadata; prune oldest complete states |
| Geometry | 8,192 shared pen/path points per document; finite bounded affine values |
| Raster assets | 64 unique admitted immutable assets; deduplicate shared backing identity |
| Font data | 8 MiB total, 2 MiB per face source, explicit bytes/index only |
| Text | 4,096 UTF-8 bytes per object, 32 KiB document text, 64 lines; no controls except tab/newline |
| Prepared text mask | 1 MiB per object; every retained mask charged in the raster ledger |
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
or budget-exhausted shaping returns an error rather than guessed glyphs.

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
   admission, shaping/layout/outline raster preparation and mask assets.
5. `crates/app-core/src/editor_document_tests.rs`: independent pixel/privacy,
   state/asset, transform/order/alpha/crop/cancel and performance fixtures.
6. `crates/app-core/src/editor_document_text_tests.rs`: real font fixtures and
   malformed/resource/cancellation cases using existing licensed repository fonts.
7. `crates/app-core/src/editor_document_legacy.rs`: validated programmatic legacy
   objects, annotation-only stamps and asset-bank-bound text-mask leases.

New consumers/tests:

8. `apps/desktop/src/editor_document.rs`: a bounded strict-spec headless
   consumer, explicit existing image/font input admission, current cancellation,
   shared core render and create-new final save. It does not duplicate tools.
9. `apps/desktop/tests/editor_document.rs` and
   `scripts/test-editor-document.py`: actual executable fixtures, independent
   FFmpeg/Python pixel checks and failure/no-publication checks.
10. `crates/app-core/src/file_identity.rs`: stable native object comparison for
    source alias refusal; existing version bytes/equality remain unchanged.
11. This contract records exact proof, resource cost, review and remaining native
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
