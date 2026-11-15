# Bounded truecolor TIFF input

Author: NewtTheWolf
Date: 2026-11-15
Status: implemented; focused executable and independent release checks passed.
Complete workspace verification is recorded in the delivery log.

## Scope and existing behavior

Allow the [TIFF conversion output](tiff-export.md) to reenter the shared explicit
image loader. Add a small, documented subset of classic TIFF: one uncompressed
RGB8 or straight-RGBA8 image stored as chunky strips, in either byte order.
Detect the format from contents, independent of the filename extension.

All existing shared-loader callers gain this input support: conversion, image
information and pixel sampling, image collections, recipes, GIF assembly,
scroll-image assembly, file OCR/QR, image uploads and the existing Open image
worker. Their own smaller pixel budgets and output rules remain in force. Add
no picker filter, UI control, output default, history import format or resident
work. PNG-only history and stored-PNG paths stay PNG-only. `image metadata`
continues to accept its existing PNG/JPEG/WebP subset; it is not the shared pixel
loader.

No new dependency is needed. Reuse the already locked direct `tiff 0.11.3`
package and `image 0.25.10`. Do not enable all TIFF variants merely by adding
`ImageFormat::Tiff` to the existing format match.

## Immutable admission before decoder construction

The installed `tiff::decoder::Decoder::new` parses the first image directory,
tag values and strip arrays before `with_limits` can be called. The installed
`image` wrapper also constructs that decoder before applying image limits.
Therefore validate the same immutable byte snapshot that will be decoded.
Header checks on a pathname followed by a second unrestricted read are not
sufficient.

Reuse the existing regular-file check, nonblocking Unix open, handle metadata,
32 MiB encoded admission and frozen readable extent in `clipboard.rs`. Once
TIFF is sniffed, seek the bounded reader to zero and read exactly its admitted
length into one owned byte vector. Validate that vector, then construct the
installed `image::codecs::tiff::TiffDecoder` over a `Cursor` owning it. A file
shrinking during the read fails. Growth cannot extend the frozen extent;
in-place changes can influence the initial snapshot, but all later validation
and decoding see identical bytes.

The preflight API is private to core:

```rust
pub(crate) fn validate_tiff(bytes: &[u8], max_decoded_bytes: u64) -> anyhow::Result<()>;
```

It succeeds only for the complete subset below. Use checked arithmetic and
checked slices throughout; malformed offsets, counts and types return errors,
never panics. Validate the caller budget is nonzero and at most
`MAX_CAPTURE_BYTES` (256 MiB). Admission checks both `width * height * 4` against
that budget and the existing `validate_image_size` contract before invoking a
pixel decoder. The physical uncompressed raster and the encoded-file limit
also constrain the actual largest admitted TIFF.

## Exact admitted TIFF subset

Require an 8-byte classic header: `II` or `MM`, magic 42 and an in-bounds first
IFD offset of at least 8. Reject BigTIFF magic 43. Every external value offset
must be at least 8. Require one IFD with 1–32 entries, strictly increasing tag
IDs and no duplicate tags. Its full count/entry/next-pointer region must fit in
the snapshot. The next-IFD pointer must be zero: no page chain, cycle, thumbnail,
SubIFD, EXIF or GPS directory is followed.

Use the header byte order for every integer. A classic IFD begins with a
2-byte entry count, followed by 12-byte entries and a 4-byte next-IFD pointer.
Each entry has a 2-byte tag, 2-byte type, 4-byte element count and 4-byte
value-or-offset field. Accepted element widths are SHORT=2, LONG=4 and
RATIONAL=8 bytes. Check count times element width before slicing. Values of
at most four bytes are stored inline at the start of that field, including
big-endian SHORTs; larger values use that field as an absolute file offset.
Ignore unused inline padding bytes. Read rationals as two ordered u32 values.

The following is the complete tag allowlist. `SHORT` is TIFF type 3, `LONG`
type 4 and `RATIONAL` type 5. Reject unknown tags and incorrect field types or
counts instead of passing them to the decoder. Scalar means count 1. A missing
required tag is an error.

| Tag | Contract |
| --- | --- |
| ImageWidth 256; ImageLength 257 | Required SHORT/LONG scalars, both nonzero; checked RGBA size within the caller budget. |
| BitsPerSample 258 | Required SHORT array, exactly SamplesPerPixel elements, all 8. |
| Compression 259 | Required SHORT scalar, exactly 1 (uncompressed). |
| PhotometricInterpretation 262 | Required SHORT scalar, exactly 2 (RGB). |
| StripOffsets 273 | Required SHORT/LONG array with exactly the computed strip count. |
| Orientation 274 | Optional SHORT scalar, exactly 1 (top-left); absent means 1. Other orientations are rejected in this slice. |
| SamplesPerPixel 277 | Required SHORT scalar, exactly 3 or 4. |
| RowsPerStrip 278 | Required SHORT/LONG scalar, nonzero; values larger than the image height are allowed. |
| StripByteCounts 279 | Required SHORT/LONG array with exactly the computed strip count. |
| XResolution 282; YResolution 283 | Optional RATIONAL scalars with nonzero denominators; accepted but ignored. |
| PlanarConfiguration 284 | Optional SHORT scalar, exactly 1 (chunky); absent means 1. |
| ResolutionUnit 296 | Optional SHORT scalar, one of 1, 2 or 3; ignored. |
| Predictor 317 | Optional SHORT scalar, exactly 1; absent means 1. |
| ExtraSamples 338 | For four samples, required SHORT scalar exactly 2 (unassociated alpha). For three samples, must be absent. |
| SampleFormat 339 | Optional SHORT array, exactly SamplesPerPixel elements, all 1 (unsigned integer); absent means unsigned. |

The strip count is `ceil(height / rows_per_strip)` and must be 1–4096. For strip
`i`, derive its start row and remaining row count using checked arithmetic.
Require its declared byte count to equal
`width * samples_per_pixel * min(rows_per_strip, remaining_rows)`. There is no
row padding, compression, planar conversion or extra sample to infer. Require
every complete strip to fit in the snapshot, with offsets increasing in strip
order and without gaps in the decoded row sequence.

Treat `[0, 8)`, the entire IFD region, every out-of-line tag-value region and
every raster strip as distinct occupied file ranges. Inline tag values remain
inside their own IFD entry and do not create another range. Reject overlapping
or duplicated occupied ranges, including overlapping strip arrays, repeated
raster offsets and metadata pointing into pixels. Check the entire declared
range, not only the bytes read by a particular decoder. The sum of IFD bytes
and out-of-line tag-value bytes must be at most 256 KiB. Gaps, alignment padding
and trailing unreferenced bytes are allowed within the 32 MiB file cap and are
ignored, never retained in output metadata. This is not a claim that an input
file has no hidden data.

Reject all tags outside the allowlist, including ICC profiles, transfer/color
tables, white-point/primary calibration, XMP, comments, EXIF/GPS pointers,
SubIFDs and tile tags. Reject associated or unspecified alpha, grayscale,
palette, CMYK, YCbCr, float/signed/16-bit samples, compression and predictors.
Accepted RGB values are treated as ordinary 8-bit RGB without a color-managed
conversion. Accepted RGBA values retain straight alpha, including hidden RGB
under alpha zero. RGB gains alpha 255. Resolution tags do not rescale pixels.

## Decode, ownership and failures

After successful preflight, pass the owned snapshot through the installed TIFF
decoder and existing `decode_editor_pixels_with_limit` helper with
`Orientation::NoTransforms`. Retain its decoded-byte checks and exact RGBA
return type; do not duplicate pixel decoding in the preflight module. Preflight
already rejected orientations requiring transforms. Decoder errors return the
normal shared-loader failure; no partial image is returned. Do not print tag
payloads or raw image bytes in errors or logs.

The input remains unchanged. A failed decode does not publish output, write
history, change the clipboard or initiate an upload. Existing caller ordering
governs later errors, including previously completed files in explicit batches;
this format addition does not introduce rollback of earlier results. No
changes to the public `load_editor_image` or `load_editor_image_with_limit`
signatures or CLI result schemas are needed.

## Resource cost and cancellation

The operation owns at most one 32 MiB encoded snapshot. Preflight retains at
most 32 tag descriptors, 4096 strip ranges and 34 structural/tag ranges; bound its combined
temporary bookkeeping to 256 KiB. The metadata-byte cap is an admission bound
on file regions, not a reason to allocate a second metadata snapshot.

The installed image TIFF wrapper allocates its own decoded raster and the
caller output concurrently. Account for that explicitly: up to two RGBA-sized
buffers during RGBA decode. RGB decode first holds two RGB buffers and later
the surviving RGB buffer plus RGBA conversion, bounded by two RGBA-sized
buffers. Decoder tag/strip bookkeeping is additional but bounded by the
preflight counts. The snapshot is released with the decoder before returning
the final pixels. Uncompressed chunky 8-bit strips use the installed direct
read path; no decompressor workspace or full-image staging copy is added by
this feature. Confirm these assumptions against the pinned source while
implementing and record measured peak memory separately from these bounds.

No thread, queue, cache, runtime, timer or GPU allocation is added. Native UI
callers keep their existing one-image admission and worker-side execution;
headless commands finish before GPUI initialization. There is no new hard
deadline for native file reads or decoder calls. Existing UI cancellation and
stale-result handling remain applicable; Ctrl+C terminates the headless
process. This feature does not claim reference-hardware performance acceptance.

## Files and focused verification

Create `crates/app-core/src/tiff_input.rs` for the preflight and its malformed
input tests; register the private module in core `lib.rs`. Modify
`crates/app-core/src/clipboard.rs` to add TIFF to the explicit-loader format
match and to snapshot/preflight TIFF before decoder construction. Keep the
existing PNG/WebP/BMP branches and PNG-only guard behavior. Reuse the existing
TIFF dependency; Cargo dependency/feature changes are not part of this slice.

Create `apps/desktop/tests/tiff_input.rs` and `scripts/test-tiff-input.py`.
Documentation write allowlist: `README.md`, `docs/image-tools.md`,
`docs/image-collection.md`, `docs/image-info.md`, `docs/qr.md`, `docs/ocr.md`,
`docs/workflows.md`, `docs/gif-assembly.md`, `docs/scroll-stitch.md`,
`docs/tiff-export.md`, `docs/tiff-input.md`, `docs/gap-analysis.md`,
`docs/non-ui-delivery.md` and `docs/goldfish-review-report.md`. Update format
lists only where present, and add the contract link to TIFF export. CLI help
may change in `apps/desktop/src/cli.rs` only if it enumerates input formats.
Do not modify UI source or controls.

Implement in three phases: first the private validator and focused malformed
fixtures; second the loader branch and exact shared-loader tests; third the
real executable fixtures, independent decoder script and documentation. Run
`cargo test --locked --profile release-fast -p app-core tiff_input` after the
first two phases and the named executable test after the third. Then run
format/Clippy checks, the complete optimized workspace regression and Windows
GNU compilation. Build the native release once code is frozen, run the
independent script against it, and smoke the packaged binary before commit.

Focused optimized tests must cover:

- Existing TIFF export to shared-loader round trips for odd-sized RGBA images
  with alpha 0/128/255 and hidden RGB, unchanged source bytes, and content sniffing
  under a misleading extension. Add independent little- and big-endian RGB and
  straight-RGBA fixtures, including multiple strips and partial final strips.
- Exact caller-budget acceptance and one-byte-smaller rejection, the 32 MiB
  encoded boundary, 32-entry and 4096-strip limits, nonzero dimensions and
  arithmetic overflow. Verify tight OCR/QR/GIF budgets remain effective.
- Truncation at header/IFD/tag-array/raster boundaries; BigTIFF; extra pages;
  cycles; duplicate or unordered tags; unknown types/tags; wrong tag counts;
  invalid strip sizes; offsets outside the snapshot; overlapping/duplicate
  ranges; unsupported compression/color/alpha/orientation/profile variants.
- Conversion, info and pixel commands through the real executable; TIFF QR
  input with an inert payload; no user config/data/cache state, no overwrite,
  no final output on invalid TIFF, and unchanged PNG-only history behavior.
- Independent FFmpeg decode of synthetic source TIFFs and resulting PNGs to
  compare exact RGBA. Generate fixtures independently of the preflight/parser,
  and include the existing app's TIFF export as a separate interoperability
  case. No real desktop capture, credentials or network upload is needed.

Run formatting, targeted core/executable tests and the existing input regression
suite, Clippy and platform compilation appropriate to the touched shared path.
Measure optimized worker decode and peak process memory on fixed synthetic
RGBA/RGB files, reporting image bytes and decoder/snapshot overhead; compare
with the existing BMP/shared-loader path on identical dimensions. Keep small
development observations distinct from release-hardware acceptance. An idle
or exhaustive capture benchmark is unnecessary because no idle/native capture
execution path changes.

## Development memory observation

Three interleaved release-process runs on synthetic 2048×2048 inputs (16 MiB
final RGBA) observed median peak RSS of 48,480 KiB for RGB TIFF and 60,760 KiB
for RGBA TIFF. Equivalent BMP inputs observed 39,672 and 43,652 KiB respectively.
The extra 8,808/17,108 KiB is consistent with the decoder's temporary raster;
the feature does not hide this overhead behind the decoded admission limit.
File sizes were about 12 MiB RGB and 16 MiB RGBA. GNU time rounded durations to
0.00–0.01 s, so these runs do not establish latency improvements or regressions.
RSS includes startup/shared mappings and is not private memory or reference
hardware acceptance. Raw samples and exact commands are stored locally in
`target/verification/tiff-performance.json`.
