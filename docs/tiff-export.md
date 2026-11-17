# Explicit TIFF conversion output

Author: NewtTheWolf
Date: 2026-11-15
Status: implemented and locally verified; native product acceptance remains separate.

Extend only `image convert INPUT OUTPUT.tif|OUTPUT.tiff`, matching extensions
case-insensitively and reporting JSON `format: "tiff"`. Keep shared image
input (including [supported BMP](bmp-input.md)), existing transforms/downscale,
defaults and the editor format enum.

The separate [bounded TIFF input contract](tiff-input.md) admits this output
for shared-loader operations only while it fits the 32 MiB encoded input limit.
It accepts one uncompressed RGB8 or explicit straight-RGBA8 image in top-left
chunky strips, with strict directory/range checks; compression, profiles, other
orientations and multiple pages remain unsupported. Input acceptance does not
add TIFF to editor/capture output defaults or Library history import, whose
current originals are PNG/JPEG/WebP. Narrow PNG compatibility APIs stay separate. The
output verification recorded below is not evidence for the new input path.

Use the already locked `tiff 0.11.3` package directly to write one uncompressed
RGBA8 image with `ExtraSamples=[2]` (unassociated alpha). The image wrapper's
default RGBA encoder omits that explicit alpha tag; a synthetic source decoded
correctly through image, Pillow and FFmpeg, but this path must encode its meaning
explicitly. Do not call `extra_samples()` on RGBA8, which would add a fifth sample.

Reuse the 256 MiB image/exact-backing checks and existing atomic no-replace writer
through a 64 KiB BufWriter. The TIFF encoder borrows existing pixels for
uncompressed strips; its target strip size is about 1 MB, but a wide row can
exceed that and no strict 1 MB scratch cap is claimed. Bounded strip metadata and
encoder bookkeeping are additional to the source, without a full-image clone.
No compression, multi-page output, EXIF/ICC metadata, orientation tag, UI setting,
resident work or new package is added. Straight RGBA encoding preserves finalized
alpha and hidden RGB; existing resizing retains its alpha-weighted semantics.

Create `crates/app-core/src/tiff_export.rs` with
`save_tiff(&RgbaImage, &Path) -> Result<(), CaptureError>`, add the direct
locked dependency and export in lib.rs. Modify only the desktop conversion
branch to choose TIFF and reject JPEG/WebP quality flags, with its existing
transform/resample flow. Add executable fixtures and a reproducible independent
decoder script; update help, README, image-tools, gap and delivery records.
Core tests verify four samples, tag 338=2, uncompressed strips, exact RGBA,
nonzero/bounded backing, no overwrite and cleanup. Executable tests verify
case-insensitive extensions, rotate/flip/downscale, quality rejection before
input access, malformed input, unchanged source/state. Independently decode
actual output with FFmpeg and inspect TIFF tags without trusting the writer.

The source limit bounds the classic-TIFF output well below its 32-bit offsets.
The writer syncs file content and publishes atomically using existing behavior;
there is no new parent-directory crash-durability claim or hard native I/O timeout.

Two executable tests passed for transforms/downscale, explicit format selection,
quality errors, malformed input, source preservation and no-overwrite. The
release binary passed `scripts/test-tiff-export.py`: independent TIFF IFD parsing
confirmed four 8-bit samples, no compression, a single image and unassociated
alpha; FFmpeg decoded original and rotated/flipped fixtures exactly, including
hidden RGB and partial alpha. Both synthetic TIFFs were 286 bytes. Clippy and
Windows GNU app/test compilation passed. Native application/file associations
and editor integration are not claimed.
