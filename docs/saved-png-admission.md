# Bounded saved-PNG loading

Author: NewtTheWolf
Date: 2026-11-15
Status: implemented; focused compatibility/admission, release and packaging
checks passed. Full regression results are recorded in the delivery log.

## Problem and scope

Harden `load_saved_png` without changing its historical pixel interpretation.
Before this follow-up it opened a raw `File`, constructed a PNG `ImageReader`,
set a 256 MiB allocation limit, decoded, and only then validated final RGBA
dimensions.
Unlike the explicit editor loader, that original path had no regular-file
admission or encoded extent cap. A FIFO, including a pathname replaced after preflight, can block at
open. Grayscale or indexed pixels can fit the decoder's native output budget
while their later RGBA representation exceeds the application budget.

The existing path already passes limits into
`PngDecoder::with_limits` before reading PNG headers: image 0.25.10
`ImageReader::make_decoder` does this for PNG. Do not describe header parsing as
previously unlimited. The change makes this construction explicit, adds bounded
file admission and checks both native decoded bytes and RGBA bytes before pixel
decoding. `PngDecoder::set_limits` alone is insufficient because the pinned
wrapper does not propagate later changes to the underlying png reader.

Keep the public signature:

```rust
pub fn load_saved_png(path: &std::path::Path) -> anyhow::Result<image::RgbaImage>;
```

The following records the original PNG-specific consumer map. It is a
compatibility checkpoint, not the current generic UI consumer map:

- `ImageClipboard::copy_saved_png`, used for explicit saved-image recopy.
- `load_png_thumbnail`, used by history preview and the visible-page gallery.
- `EditorImage::SavedPng` in the existing editor-opening worker.
- `png_without_metadata`, used for explicit saved-image upload sanitization.
- `export_saved_png_as` when decoding a saved PNG for another format or size.

Current UI saved-image recopy, editor loading, thumbnails and explicit export
use the generic [format pipeline](capture-library-formats.md): PNG keeps the
legacy saved-PNG decode, JPEG/WebP use managed color/orientation, and generic
sanitization/conversion uses managed decode for all three. The narrow APIs above
retain their distinct contracts; import/relink and Preserve remain header/byte
operations rather than pixel decoding.

The direct editor loader keeps its separate 32 MiB encoded limit and managed
orientation/color policy. The narrow `load_export_png` compatibility loader
retains its prior 32 MiB and EXIF behavior; it is not the current generic CLI
sanitizer. Import/relink admission, Preserve byte copies, raw upload admission
and PNG structure inventory do not become pixel-decode operations through this
compatibility loader change.

## File admission and readable extent

Use a whole-file ceiling of
`MAX_SAVED_PNG_FILE_BYTES = MAX_CAPTURE_BYTES + 16 * 1024 * 1024`
(**272 MiB**, 285,212,672 bytes). This deliberately permits application-produced
large PNGs that exceed the editor's 32 MiB limit. It aligns with the existing
encoded PNG upload/buffer envelope, but is a new explicit bound on this former
uncapped saved-file reader. The original narrow byte-copy wrapper counted 272 MiB after its signature.
Current generic Preserve checks the entire file against the per-kind ceiling
(272 MiB PNG, 32 MiB JPEG/WebP); that is separate from this compatibility loader.

Admission sequence:

1. Read pathname metadata and require a regular file, with length between 8 and
   272 MiB inclusive. Preserve symlinks to regular files; reject directories,
   FIFOs/devices and broken links before opening.
2. Open for reading. On Unix use `OpenOptionsExt::custom_flags(O_NONBLOCK)`,
   as the existing editor-input path does. Do not add `O_NOFOLLOW`, which would
   unnecessarily remove supported regular-file symlinks.
3. Validate the opened handle's metadata independently: require a regular file
   and the same byte limits. Use its length as the authoritative readable
   extent; a regular-file replacement between pathname check and open is still
   allowed if the actual opened file passes admission.
4. Wrap that one handle in the existing extent-limited `EditorFile` reader and
   an explicit 64 KiB `BufReader`. Reuse the reader's `Read`/`Seek` behavior; no
   full-file allocation, reopening, map, temporary file or second PNG pass.
   A small private file-open helper is acceptable if it avoids duplicated
   admission code, but do not refactor unrelated format paths in this slice.

Every read is clamped to the handle's initial length and EOF seeking uses that
extent, so later growth cannot expand accessible input. A shrink that removes
required bytes fails through the decoder. There is no promise to detect a
shrink confined to unread trailing data or all in-place edits. The same opened
file is used throughout; this is an extent bound, not an immutable snapshot.
An in-place writer can still alter bytes the decoder has not read. Callers that
need a content snapshot must avoid modifying the file while it is loaded.

Unix nonblocking open prevents a FIFO substituted between metadata and open
from waiting for a writer; post-open validation rejects the FIFO handle. This
does not make remote filesystems, Windows native opens, metadata or disk reads
interruptible. Do not add retry loops or claim a hard native I/O deadline.

## Decoder and pixel admission

Construct `image::codecs::png::PngDecoder::with_limits(buffered_reader, limits)`
with `limits.max_alloc = Some(MAX_CAPTURE_BYTES)` before any header parsing.
Keep other image limits at their existing defaults: no new arbitrary dimension
limit or PNG format restriction. The pinned wrapper uses `png::Limits` during
header/metadata parsing and enables its existing EXPAND transformation. Keep
that behavior; do not switch to `PngDecoder::new` then call `set_limits`.

After construction and before `DynamicImage::from_decoder`:

- Read `ImageDecoder::dimensions` and apply `validate_image_size(width, height)`
  with checked arithmetic. This requires positive dimensions and
  `width * height * 4 <= 256 MiB`.
- Require `ImageDecoder::total_bytes() <= MAX_CAPTURE_BYTES` independently.
  This protects 16-bit RGB/RGBA decoder buffers, which can be larger than the
  final RGBA8 image. Grayscale and indexed expansion cannot use a smaller native
  byte count to bypass final-RGBA admission.
- Retain the exact admitted dimensions/byte count; decode once with
  `DynamicImage::from_decoder`, then call `into_rgba8`. Do **not** use
  `decode_editor_pixels_with_limit` with its default orientation argument,
  because that would newly apply EXIF orientation to saved images. A final
  defensive dimension/backing-length check may validate the returned image;
  it must not substitute for either predecode check.

If a small private `decode_saved_png` helper is introduced for testing, it must
take an already bounded `BufRead + Seek` and an explicit testable decoded budget;
the public entry always passes 256 MiB. Reject invalid budgets before decoder
construction. Keep the helper private rather than adding a new public resource
configuration API.

## Compatibility and failure behavior

Accept the PNG variants the pinned decoder already handles, including
grayscale, grayscale-alpha, indexed/palette and tRNS, 8/16-bit samples and
interlaced PNGs. Preserve existing conversion into RGBA8 and exact 16-to-8-bit
rounding through the installed image library. Preserve hidden RGB and alpha as
that library returns them; no new matte, color conversion or metadata policy.

PNG is selected explicitly and checked by its signature, independent of the
filename. Keep EXIF orientation **ignored** on this saved-image path; the direct
editor path's orientation handling remains distinct. APNG continues to return
the decoder's default still image from IDAT, including when that default is not
one of the animation frames. Do not enumerate frames, allocate a frame collection,
or promise complete APNG validation. Existing decoder behavior for trailing data,
ancillary chunks, CRCs and metadata remains unchanged within the new byte cap.
Do not force an EOF scan or call the animation/metadata APIs just for admission.

Oversized encoded files and oversized native/RGBA decoded representations fail
explicitly with stable saved-PNG admission errors. OS/read/codec failures remain
errors, never an empty or all-black replacement image. Do not print chunk
payloads, source pixels or secrets in diagnostics. Source files and existing
outputs remain unchanged. Recopy fails before `copy_image` opens/updates the
clipboard; editor failure does not create a new editor document; thumbnail
failure follows the existing page/revision guard. Pixel export/upload wrappers
must not publish or upload on load failure. Their existing prior-success and
partial-result semantics remain unchanged.

Compatibility is intentionally reduced only for inputs outside the new limits
or nonregular sources. An old PNG with more than 272 MiB of metadata/trailing
data is now rejected even if its pixels are small. A 16-bit PNG must fit both
its native decoded representation and RGBA8 output. Do not claim every possible
PNG written by an unlimited external producer is accepted, or reduce the limit
to 32 MiB to reuse editor admission unchanged.

## Resource cost and limits

There is one file handle, one 64 KiB read buffer and small extent/decoder state.
No 272 MiB encoded snapshot is allocated. RGBA8 decode can transfer its existing
pixel vector directly; RGB/grayscale/16-bit conversion temporarily retains the
native raster and the RGBA8 result, each independently at most 256 MiB. Thus up
to 512 MiB of pixel representations can coexist during conversion, in addition
to decoder/metadata scratch. They are released when their consumers finish.

The pinned PNG limit is a library allocation control, not a measured strict
aggregate process-memory ceiling. It does not include every allocator capacity,
buffer clone, row/unfilter buffer or external caller representation; ancillary
metadata handling also has library-specific accounting. Retaining the existing
256 MiB setting does not establish that all PNG scratch stays inside PLAN's
32 MiB allowance. Record that residual memory acceptance gap; do not silently
relax the active-image budget or present this admission fix as its closure.

No worker, thread, runtime, queue, cache, timer or GPU allocation is introduced.
All current callers continue to run loading off the foreground executor.
Thumbnail cancellation retains its existing checks before/after PNG decode and
inside reduction; this slice adds no mid-decode cancellation guarantee.
The [PLAN contract](../PLAN.md#performance-contract--all-versions-and-features)
still governs user-visible latency, memory and release acceptance.

## Files and focused verification

Modify `crates/app-core/src/clipboard.rs` for the private bound/opener/decoder
helper and public loader implementation. Reuse `EditorFile`; add focused tests
in `crates/app-core/src/saved_png_tests.rs`, included as a private test child of
the clipboard module so it can exercise the private helpers.
No dependency, Cargo feature, production UI or CLI schema change is required.
Call sites keep their signatures. Update the saved-image/thumbnail descriptions
in PLAN/README and the delivery/status records after verified behavior; link
this contract from `docs/sanitized-export.md` without rewriting the separate
32 MiB sanitized-history contract. Exact documentation write list: `PLAN.md`,
`README.md`, `docs/non-ui-delivery.md`, `docs/non-ui-status.md`,
`docs/goldfish-review-report.md`, `docs/saved-png-admission.md` and
`docs/sanitized-export.md`.

First implement the private opener/decoder and unchanged public entry point;
then add/run the focused core fixtures. After they pass, run the existing
thumbnail/saved-export regressions and the full optimized workspace plus
Clippy/Windows checks. Freeze source, measure the optimized path, package the
release, and update documentation with completed evidence before commit.

Required optimized fixtures:

- Exact expected pixels for RGBA8 with partial alpha/hidden RGB; grayscale,
  grayscale-alpha, RGB8, indexed+tRNS, 16-bit grayscale/RGB/RGBA, and interlace.
  Include misleading filename extensions, EXIF orientation that remains ignored,
  and APNG with a distinct nonanimated default image. Compare supported inputs
  with the pinned former loader/oracles, never a new handwritten color converter.
- Valid generated PNG larger than 32 MiB but within 272 MiB, representing a
  large application output; exact whole-file cap acceptance and one-byte-over
  rejection through file/extent tests. Sparse oversized files must fail before
  decoder construction or image allocation.
- Header/required-palette/IDAT-header grayscale/indexed input whose native byte count fits but RGBA
  count exceeds the budget, plus 16-bit input exceeding the native budget.
  Fail on admission before invalid or missing compressed IDAT data is decoded.
  An IHDR-only stream can fail during decoder construction before dimensions
  become available; do not claim admission precedes that error. Use smaller
  injected limits to prove exact-boundary behavior without allocating huge
  decoded images; production tests still check the actual constants.
- Directory/device/FIFO rejection, regular-file symlink acceptance, broken-link
  refusal and deterministic metadata-to-open FIFO substitution on Unix. The
  substitution test must run in a killable subprocess so a blocking regression
  does not hang the suite. Do not change the user's clipboard or filesystem.
- Extent reader tests for zero/end/relative seeks, attempted growth, short reads
  and no reopening of a replaced path. Verify no read passes the captured extent;
  do not falsely require detection of all same-length content edits.
- Existing history-thumbnail pixel/cancellation tests, saved editor/recopy
  admission, metadata-stripping and saved-export regression fixtures. Use a
  synthetic `ImageClipboard` failure case to prove its clipboard field remains
  uninitialized on load refusal. All file-output tests preserve existing paths
  and originals and verify no partial final output on loading failure.

Run formatting, targeted optimized saved-PNG/thumbnail/export tests, Clippy,
Windows GNU compilation and the affected workspace regression. Compare optimized
load/thumbnail time and peak memory on identical synthetic 4K RGBA8 and 16-bit
files before/after; report the new 64 KiB buffer and peak conversion costs.
Native UI/clipboard, arbitrary filesystems, physical presentation and full
reference-hardware budgets remain separate. No real desktop capture, account,
network request or broad benchmark is necessary for this scoped change.

The optimized 4K comparison used two warmups and ten alternating old/new runs.
RGBA8 old/new medians were 3.559/3.265 ms, p95 3.783/3.430 ms; RGBA16 medians
13.759/12.974 ms, p95 13.962/13.605 ms. Exact output pixels matched throughout.
Encoded inputs were 33,182,358 and 66,362,488 bytes, demonstrating that this path
does not inherit the editor's 32 MiB cap. These warm synthetic observations show
no regression here, not universal latency improvement or hardware acceptance.
Three isolated executions of the complete comparison fixture observed median
peak RSS 312,536 KiB, including encoding and overlapping old/new decoder results;
that number must not be presented as the production loader's peak memory.
Exact commands and raw samples are in `target/verification/saved-png-performance.json`.
