# Offline image workflows

Run an explicit saved recipe without opening the desktop application:

```sh
screenshots workflow recipe.json input.png output.webp
```

All three paths are native command-line paths. Relative paths resolve from the
invoking directory, including when the recipe lives in another directory.
Recipes contain operations only, so moving or sharing one does not embed input
paths, output paths, credentials or executable commands.

```json
{
  "version": 1,
  "steps": [
    { "op": "crop", "x": 20, "y": 10, "width": 640, "height": 480 },
    { "op": "redact", "x": 10, "y": 10, "width": 120, "height": 30 },
    { "op": "blur", "x": 200, "y": 20, "width": 80, "height": 60 },
    { "op": "pixelate", "x": 320, "y": 20, "width": 80, "height": 60 },
    { "op": "resize", "width": 320, "height": 240 },
    { "op": "canvas", "padding": 8, "background": "FFFFFF" }
  ]
}
```

Version 1 requires between 1 and 16 steps. JSON objects reject unknown and
duplicate fields; missing fields, unsupported operations and other versions
are errors. The recipe must be a regular file containing at most 64 KiB of JSON,
including whitespace. There are no includes, variables or implicit defaults.
The opened handle is checked again after path validation; on Unix it is opened
nonblocking so a FIFO substituted between those checks cannot wait for a writer.

| Operation | Required fields besides `op` | Behavior |
| --- | --- | --- |
| `crop` | `x`, `y`, `width`, `height` | Keep an in-bounds rectangle. |
| `rotate` | `degrees` | Rotate clockwise by 90, 180 or 270 degrees. |
| `flip` | `axis` | Mirror `horizontal` or `vertical`. |
| `remove_strip` | `axis`, `start`, `length` | Remove complete rows (`horizontal`) or columns (`vertical`) and join the retained pixels without resampling. |
| `resize` | `width`, `height` | Exact center-sampled nearest-neighbour resize; permits upscaling. |
| `canvas` | `padding`, `background` | Add equal padding on all four sides and composite over the specified background. |
| `redact` | `x`, `y`, `width`, `height` | Replace an in-bounds rectangle with opaque black pixels. |
| `blur` | `x`, `y`, `width`, `height` | Apply the existing alpha-weighted 17×17 box blur inside an in-bounds rectangle. |
| `pixelate` | `x`, `y`, `width`, `height` | Apply the existing alpha-weighted, source-aligned 16-pixel block grid inside an in-bounds rectangle. |

Coordinates and dimensions are nonnegative whole numbers; widths and heights
must be positive. Rectangle coordinates refer to the current image after all
earlier steps, with `(0, 0)` at its top-left corner. Backgrounds use six or eight
hexadecimal digits, `RRGGBB` or `RRGGBBAA`, without a `#` prefix; six digits mean
opaque. Canvas compositing matches `image compose`, including alpha blending.
Resize matches `image resize`; it preserves exact selected RGBA pixels and does
not interpolate. Image input decoding applies EXIF orientation before the first
step, matching the existing image commands.

Blur and pixelation have fixed parameters, with no radius, block-size or preset
fields. They sample the current full image and change only the selected rectangle;
blur neighborhoods and pixelation blocks can include pixels outside that rectangle.
Blur clips its 8-pixel-radius neighborhood at image edges. Pixelation anchors its
16×16 blocks to the current image's top-left corner, so an earlier crop changes
the grid origin. Fully transparent sample colors have no weight; RGB and alpha
are averaged using the existing editor/export implementation. Blur and pixelation
obscure appearance but are not substitutes for opaque redaction of secrets.

Steps run sequentially on flattened pixels. A redaction removes the original
pixels from that rectangle before later steps run, so later effects, crop, rotation,
resize or canvas operations cannot recover those pixels. Effects after redaction
can blend the replacement black with its neighbors; they never sample the
removed original pixels. This is an offline raster recipe, not editable document
layers or arbitrary editor effect ordering.
Redaction does not infer what content is sensitive; its coordinates must cover
the intended content in the image state at that step.

Inputs are PNG, JPEG, WebP, [supported truecolor BMP](bmp-input.md) or
[supported classic TIFF](tiff-input.md), detected from file contents, with the
existing 32 MiB encoded and 256 MiB decoded limits. TIFF accepts one uncompressed
RGB8 or explicit straight-RGBA8 image in top-left chunky strips; profiles,
compression, other orientations and multiple pages are rejected. Its immutable
snapshot and temporary decoded raster follow the TIFF input memory contract.
Every output and intermediate image must have a nonzero size and fit 256 MiB
as RGBA; dimension arithmetic and
rectangles are checked before allocating their result. This byte limit also
bounds each dimension, without imposing a different workflow-only side limit.
One current image and at most one replacement image are held during each step,
then the previous image is dropped. The worst-case transform therefore holds
512 MiB of RGBA buffers, plus bounded recipe data and existing decoder/encoder
scratch. Blur uses less than 300 KiB of scratch. Pixelation prepares one grid of
`ceil(width / 16) × ceil(height / 16) × 4` bytes from the current full image;
under the RGBA admission limit this is at most 16 MiB, including extreme skinny
images. Each grid or blur scratch allocation is released with its step. It keeps
no undo history or retained original image. Existing export encoder limits and
scratch costs still apply; this is an allocation contract,
not a measured total-process memory result.

Only the final image is exported, through the existing atomic no-replace export
path. The destination extension selects PNG, JPEG or lossless WebP; JPEG uses
the application's default quality and rejects remaining transparency. Existing
files, including the input and recipe, are never replaced. A parse, validation
or transform failure produces no output. An export error retains the existing
temporary-file cleanup behavior. Successful stdout contains one JSON object:

```json
{"version":1,"steps":6,"format":"webp","width":336,"height":256}
```

The command performs no uploads, subprocesses, shell evaluation, clipboard
operations, history writes or configuration changes. It has no resident tasks,
timers, network runtime or added worker threads; work runs only in the explicit
headless process and all image state is released when it finishes.

Cancellation retains the existing headless process semantics: Ctrl+C or process
termination stops the command. The CLI supplies Unix cancellation and a five-minute processing budget, with
checks between steps and during supported image/publication work. Synchronous
codec and native filesystem calls remain noninterruptible boundaries; the
budget does not promise a hard wall-clock deadline through those calls. Interruption before export
writes no image; interruption during export can leave an unpublished UUID
`.part` file, which the explicit storage inspection command can report. A final
file already published before termination remains a completed output. Native
filesystem stalls and full platform resource acceptance remain open; no timeout
or hardware-performance acceptance is claimed here.

The executable fixtures in `apps/desktop/tests/workflow.rs` cover ordered exact
pixels for the original eight operations; `apps/desktop/tests/strip.rs` covers
the ninth, `remove_strip`, including redaction ordering. Other fixtures include independent alpha-weighted blur/pixelation
oracles across composed steps, opaque redaction followed by resizing and effects,
alpha,
invocation-relative paths, recipe and step boundaries, strict schema rejection,
oversized input/transform rejection, source preservation, existing outputs and
output-free failures. These are test definitions; passing-run and optimized
measurement evidence belong in `docs/non-ui-delivery.md` after verification.

An unchanged-result recipe (same-size resize, whole-image crop or cancelling
flips) still requires a fresh output and re-encodes decoded pixels. It does not
copy source bytes or retain source metadata. Extensions are case-insensitive;
`.jpg` and `.jpeg` both report `"jpg"`. JPEG quality defaults to 90. Missing
output parent directories are created by the shared exporter. Errors produce
stderr with exit 1 and no success JSON.

A short Linux optimized process observation (release-fast, five warm samples)
used a solid 3840×2160 RGBA input. Convert-only median was 8.06 ms and a recipe
with two cancelling horizontal flips was 14.33 ms; peak RSS observations were
45.17 MiB and 44.97 MiB respectively. These include process startup and export,
are not controlled reference-hardware latency measurements, and do not prove
a memory improvement. The recipe does additional requested work; no idle cost
or GPU resources were added.
