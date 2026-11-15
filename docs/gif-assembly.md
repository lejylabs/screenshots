# Offline GIF assembly

Date: 2026-11-15
Author: NewtTheWolf
Status: implemented; focused and independent release checks passed. Native
player and reference-hardware acceptance remain open.

Add `gif OUTPUT.gif --delay-ms N INPUT INPUT...` for 2–16 explicit image files.
Require equal dimensions, at most 1280×720 per frame and 64 MiB cumulative
decoded RGBA. Delay is 10–60000 milliseconds, divisible by 10, and is stored
exactly in centiseconds. Playback is once: omit a loop extension. This command
does not capture or sleep. Inputs use the shared bounded image loader:
PNG/JPEG/WebP, supported truecolor BMP and [supported classic TIFF](tiff-input.md).
TIFF accepts one uncompressed RGB8 or explicit straight-RGBA8 image in top-left
chunky strips; profiles, compression, other orientations and multiple pages
are rejected. GIF's smaller per-frame and cumulative budgets still apply.
The earlier GIF verification below does not establish TIFF input acceptance.

GIF supports only binary transparency: reject alpha values other than 0/255
instead of silently flattening. More than 256 colors are quantized, with fixed
speed 10 and a local palette per frame. Hidden RGB is not preserved. Each frame
fills the canvas and uses background disposal, so a transparent next frame does
not retain prior visible pixels. Native player disposal behavior still varies.
Opaque frames also reserve an unused transparent palette index so background
disposal clears to transparency in the tested decoders. If all 256 indices are
occupied, the current implementation rejects the frame rather than sacrifice
an opaque color or silently lose transparency. Consequently some complex-color
images cannot yet be assembled with this conservative encoder.
After quantization, verify that the selected transparent palette index matches
exactly the input's transparent pixels. Reject an unsafe index collision rather
than silently turning an opaque pixel transparent.

Preflight counts, delay, output collision and all file metadata before staging;
complete pixel validation happens one frame at a time inside an unpublished
atomic output. A bad later frame discards the temporary output; no partial final
GIF is published. Decode one frame, convert its owned mutable RGBA to GIF indices,
write it and drop it before the next decode. Use the already locked gif 0.14.2
directly so `Encoder::into_inner()` reports trailer-write failure. Cap encoded
output at 64 MiB before writes, then flush/sync/publish with existing no-replace
semantics. The counter is below the 64 KiB buffering layer. No retries or cleanup
of existing outputs; no UI/default/history/clipboard/network work.
Ordinary errors clean the known temporary file through the existing writer.
Forced termination or a crash can leave an unpublished `.part` file containing
pixels; explicit storage inspection can report it. No automatic janitor is added.

At most one 3.52 MiB RGBA frame, 0.88 MiB palette-index frame, encoder LZW buffer
and bounded quantizer/palette state are active, in addition to loader scratch
and its 32 MiB encoded admission. TIFF's immutable snapshot and temporary
decoded raster follow its [input memory contract](tiff-input.md#resource-cost-and-cancellation).
No full animation buffer or frame collection.
There is no hard mid-quantization cancellation deadline; Ctrl+C terminates the
headless process. This is not a reference-hardware timing or memory claim.

Create core gif_assembly.rs (atomic encode API and source metadata admission),
desktop gif_assembly.rs (strict CLI parsing), executable fixtures and independent
GIF frame/delay/disposal tests. Add the direct already-locked gif dependency,
core export, main/CLI routing, and documentation links. Test exact small palettes,
transparent frames after opaque ones, loss on larger palettes, delay bounds,
alpha/dimension/count/budget rejection, write-cap/trailer failures, no overwrite
and unchanged inputs. Recording remains separate; no optional FFmpeg is required
for producing this GIF, only for an independent decoder test if available.
Composition tests use the installed decoder's web-compatible transparency
behavior; independent FFmpeg checks supplement them, but native player behavior
is not a cross-platform acceptance claim.

The core API is `assemble_gif(&[PathBuf], &Path, delay_ms: u32) ->
Result<(), CaptureError>`. Successful CLI stdout is one JSON object with
`format: "gif"`, `frames` and `delay_ms`; it does not include dimensions or
input paths. Failure emits no success JSON and exits nonzero. A stdout failure
after publication explicitly says the GIF was saved. No replay/rollback follows.

An optimized Linux development observation used identical two-color 1280×720
frames, three interleaved runs each. Two frames had median wall time 0.02 s and
maximum resident set 16,896 KiB; sixteen frames had 0.13 s and 17,164 KiB. The
small peak-RSS difference supports releasing each frame before the next on this
fixture. These coarse process observations include startup, are not private
memory measurements, and do not establish reference-hardware or complex-palette
budgets. The reproducible command details and raw samples are recorded locally
in `target/verification/gif-performance.json`.
