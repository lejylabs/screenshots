# Bounded saved-PNG loading

Author: NewtTheWolf
Date: 2026-11-15
Status: the original saved-PNG admission is implemented. The metadata-memory
continuation described below is staged for review and has not been compiled,
executed or measured. Earlier passing evidence applies to the original path.

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

Before decoder construction, the fixed-size streaming metadata reader reads and
replays the original 33-byte signature/IHDR. It checks positive dimensions,
RGBA admission and a necessary native-raster bound before any ancillary payload
can reach the codec. The codec still validates IHDR CRC and all actual PNG
syntax; this reader does not replace its validation. The later native check
still covers tRNS expansion. No file reopen, rewind or whole-file pass is added.

Construct `image::codecs::png::PngDecoder::with_limits(guarded_reader, limits)`
with parser allocation allowance `min(256 MiB, maximum_expanded_row + 1 + 8 MiB)`.
The maximum expanded row includes possible tRNS channels and preserved 16-bit
samples. This is separate from the unchanged 256 MiB native/RGBA raster admission:
the pinned png allocation limit charges metadata and transformed row storage,
not the full caller-owned raster. A fixed 8 MiB parser limit would wrongly reject
wide admitted rasters. Other image limits and EXPAND behavior stay unchanged.

Before exposing each subsequent original chunk header, the reader admits the
cumulative declared payload bytes of tEXt/zTXt/iTXt/eXIf/iCCP at at most 1 MiB
and their count at at most 4096. Charge before payload exposure, and latch refusal.
The count also bounds retained record/vector overhead for thousands of tiny text
chunks. The byte limit bounds raw chunk capacity, EXIF copies, compressed text
and Latin-1 to UTF-8 growth. The parser allowance additionally bounds ICC
inflation. Successful ICC inflation and the transformed row share this parser
allowance. A formerly accepted PNG can therefore be refused even when its
compressed ICC payload fits the 1 MiB encoded metadata cap: a 3,000,000×1 RGBA8
row plus a successfully inflated 16 MiB profile exceeds the combined allowance.
This is an explicit additional compatibility limit, covered against the former
256 MiB loader. png 0.18.1 already ignores failed ICC parsing, including inflation
allocation failure, so a still larger profile may instead be ignored and decode.
Acceptance is not monotonic in expanded ICC size. Saved-PNG pixels never applied
ICC; pixel equivalence is promised only when the bounded decoder admits the file.
Compressed zTXt/iTXt is not eagerly expanded by this path.

All encoded bytes are forwarded unchanged, including metadata, chunk lengths,
CRC and ordering. Unknown ancillary payloads and IDAT are streamed without a new
payload cap because the pinned decoder does not retain those whole chunks.
The existing 272 MiB whole-file admission remains unchanged. The adapter uses
one 33-byte replay array and scalar counters, reusing the existing 64 KiB input
buffer. The pinned still decoder does not seek; the adapter supports position
queries and refuses other seeks so a dependency change cannot reset accounting.

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
or promise complete APNG validation. Existing decoder behavior for trailing data, CRCs, critical ordering and
metadata parsing remains unchanged within the explicit metadata limits. An
oversized retained-metadata header that the codec visits fails admission even
if that still decoder would not read its payload. No EOF validation is added.
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

Compatibility is intentionally reduced for nonregular sources and inputs
outside the encoded, pixel or retained-metadata limits. An old PNG with more than 272 MiB of metadata/trailing
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

The earlier unconditional 256 MiB parser setting allowed a valid 1x1 image
with 64 MiB ASCII text to retain roughly 128 MiB of raw/text metadata before
pixel admission. The staged reader rejects that header before exposing payload,
and scales ICC inflation allowance to the admitted row. Its fixed ancillary
cost model fits below 32 MiB for tiny images: at most 1 MiB retained encoded
payload, bounded raw/copy/UTF-8 capacities and 4096 records, plus an ICC vector
whose capacity may approach twice the row-plus-8-MiB parser allowance. The
provided allocator probe must verify actual requested-live heap peaks; no
measurement has been run for this continuation.

This is not a measured aggregate process-memory ceiling. In particular, wide
rows, unfilter growth/reallocations, native-to-RGBA conversion and caller-owned
representations must still be measured against PLAN's pixel-scaled budget.
The pinned PNG limit alone does not establish all row scratch or process/RSS
acceptance. Keep that gate open and do not relax the active-image budget.

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
Add the opt-in isolated Rust allocator probe at
`crates/app-core/tests/saved_png_metadata_alloc.rs`. No dependency, Cargo feature,
production UI or CLI schema change is required.
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

## Staged metadata continuation verification

All new production logic and checks are Rust. The frozen source and contract are
under `target/verification/saved-png-metadata-held`; only formatting was run.
Fresh review, optimized fixtures, allocator/timing measurements, Clippy, Windows
compilation and the affected regressions remain unrun for this continuation.

Required added fixtures cover 64 MiB retained-metadata headers before payload
reads, the cumulative 1 MiB and 4096-record boundaries, Latin-1 expansion,
compressed 24 MiB ICC/text, malformed CRC/critical ordering/truncation parity,
sub-byte grayscale/indexed/tRNS and 16-bit alpha, row-scaled budgets, refusal
latching, and opt-in wide/tall rasters exceeding fixed scratch. The isolated
allocator fixture streams a valid 64 MiB text file without a whole-file fixture
allocation, compares the former/current loaders and checks exact ICC pixels.
It counts requested live Rust heap bytes, excluding allocator retention,
reallocation overlap, non-Rust/native allocations, RSS and GPU memory.

## Integrated metadata guard verification (2026-11-18)

Two independent source reviews of the corrected revision2 packet are clean.
The optimized saved-PNG suite passes19 regular cases; the separate allocator,
wide/tall raster and interleaved4K cost probes also pass. Project-policy Clippy
and workspace formatting pass; these are not claims of zero existing warnings.

The isolated synthetic64MiB tEXt case measures134,311,994 requested live Rust
heap bytes with the former decoder versus85,487 with the new admission refusal.
For24MiB expanded ICC, peaks are34,837,752 and8,885,496 bytes with identical
small-image pixels. These are allocator-requested bytes, not process RSS or GPU
memory. The wide/tall probe admits3,000,000×1 and1×3,000,000 rasters.

Ten interleaved optimized4K samples after two warmups show median3.62→3.43ms
for RGBA8 and13.24→12.21ms for RGBA16; observed p95 is4.39→4.47ms and
13.69→13.02ms respectively. These synthetic development observations establish
no reference-hardware or whole-app budget. The standalone process-memory, native
codec/clipboard and full resource gates remain separate. Exact commands, source
and logs are retained under `target/verification/saved-png-metadata-integration/`.
