# Color-managed image input

Status (2026-11-16): optimized compilation, independent numerical fixtures and
private executable checks pass. Fresh source reviews five and six of the
corrected checkpoint are clean, including the exact vendor/oracle inventories.
Native and reference-hardware acceptance remain separate below.
This is the explicit image import backend. Native capture HDR/wide-gamut capability,
Windows WGC/Wayland pipelines, display presentation and gamma-correct resampling
remain separate work. An unknown XCap/native pixel color space is not established
as sRGB by this implementation.

## Working pixels and entry points

Opened editor files and encoded clipboard/drop/URL image bytes share
`managed_color::decode`. A file is read once from the retained, identity-checked
handle into its bounded snapshot. Byte input is borrowed. The existing 32 MiB
encoded input cap and caller's decoded-source cap (at most 256 MiB) remain.

The result is tightly packed **8-bit, straight-alpha SDR sRGB RGBA**. Supported
embedded colors are converted once with relative colorimetric intent, then clipped
to the sRGB gamut and quantized to nearest 8-bit values. This is deliberate SDR
gamut clipping, not HDR preservation or tone mapping. Untagged input explicitly
assumes ordinary SDR sRGB; absence of metadata does not establish its true color.

RGB is converted independently of alpha, including hidden RGB under alpha zero.
There is no premultiply/unpremultiply step or matte. Every 8-bit alpha value is
preserved exactly; 16-bit alpha is independently rounded to nearest 8-bit by
`(value + 128) / 257`. Source RGB16 stays at 16-bit precision until normalization
for the float transform; it is not reduced to RGB8 before color conversion.

EXIF orientations 1–8 are applied once while writing the output raster. Expected
`source_size` is checked against oriented header dimensions before decoding pixels.
Malformed/unrecognized EXIF orientation keeps the existing no-transform fallback.

`load_saved_png`, thumbnails, the PNG-only metadata export loader, original library
imports/relinks and byte-preserving library exports retain their existing contracts.
They do not acquire the editor's new orientation or color conversion policy.
Native capture stays in memory and never goes through an encoded import round trip.

## Admitted and refused color descriptions

| Input | Behavior |
| --- | --- |
| PNG iCCP; JPEG APP2 ICC_PROFILE fragments; WebP ICCP | Convert the admitted RGB matrix/TRC profile into SDR sRGB. |
| PNG cICP `[1,13,0,1]` | Explicit full-range RGB sRGB, no color transform needed. |
| PNG cICP `[12,13,0,1]` | Full-range RGB Display P3 with sRGB transfer; convert into sRGB. |
| PNG sRGB | Explicit sRGB; validate rendering-intent byte. |
| PNG legacy gAMA/cHRM | Only canonical sRGB fallback values alongside an admitted ICC/CICP/sRGB description are accepted. Legacy gamma/chromaticity alone is refused, since gamma 2.2 is not exactly the sRGB transfer function. |
| PNG simultaneous ICC/CICP/sRGB | Refuse duplicates and simultaneous descriptions instead of choosing an ambiguous source. Canonical lower-priority gAMA/cHRM may accompany one description. |
| HDR CICP/PQ/HLG, limited-range/non-RGB CICP, cLLI/mDCV | Refuse with the payload-free `ImageColorPolicy` category and fixed guidance to convert to SDR sRGB. |
| CMYK JPEG/profile, grayscale ICC, device-link/output/abstract profile, Lab PCS, CLUT/multi-process profile, ICC cICP combination | Refuse; decoder substitutions are not called color conversion. |
| Existing narrow truecolor BMP and uncompressed RGB8/straight-RGBA8 TIFF | Keep their existing structural admission and explicit color-profile/calibrated-color refusal. Their untagged RGB follows the stated SDR sRGB assumption. Embedded-profile BMP/TIFF support is still open. |

Supported ICC is version 2 or 4, RGB input/display class, XYZ PCS, with all three XYZ
colorants, RGB TRCs and white point. Curves may be bounded monotonic `curv` tables
or continuous SDR `para` functions 0–4. Parametric admission checks each complete
active branch analytically: power-domain endpoints, increasing branch direction,
output range and the branch junction. It does not use sampled points to establish
validity between samples. A junction/range allowance of eight s15Fixed16 units
accommodates coefficient rounding; a negative active power base has no tolerance.
Coefficients must be represented exactly by the CMS's f32 parameter storage, and
a derived f32 threshold must not introduce a negative-base interval. Unsupported
precision fails explicitly. Missing, duplicate, out-of-range, partially
overlapping, singular or unsupported color data fails instead of silently falling
back to untagged pixels. Exact tag-data sharing, as used by common shared TRCs,
is allowed. ICC rendering-intent header hints do not change the fixed import policy.

PNG color/EXIF metadata CRCs are checked before use. Color chunks after pixel data,
invalid zlib data/trailers and oversized metadata fail. PNG eXIf and WebP EXIF are
bounded to 64 KiB and duplicates fail. JPEG ICC fragment numbers must be unique,
complete and agree on the fragment count; out-of-order complete fragments are
assembled by their declared sequence. CMYK/unsupported JPEG precision is refused
from the frame header before constructing the JPEG decoder. WebP ICC flag/profile
presence must agree; the existing bounded still/default-frame validator remains.

## Allocation and cancellation contract

Before ICC parsing or decompression output growth:

- Decompressed/assembled ICC is capped at 256 KiB; JPEG fragment counts are at most
  255 and their total bytes are checked before allocation.
- ICC has at most 64 tags and 4096 samples per TRC. Header, table and payload
  extents and overlaps are checked first. LUT/CLUT and ICC cICP tags are refused
  before the CMS sees them.
- A bounded copy of at most 256 KiB contains only validated matrix/TRC/white-point
  tag entries for the CMS parser. Untrusted ancillary `mluc`/`desc` counters never
  reach a metadata allocator. This copy and the expanded profile coexist briefly.

PNG/JPEG decoder input uses a borrowed Read/BufRead/Seek segment view. Color metadata
and text/EXIF chunks are removed from the view after extracting required color and
orientation, so PNG cannot decompress ICC/text a second time and JPEG does not
reassemble ICC again. The source bytes are not copied merely to remove chunks.
The index's actual Vec capacity is counted; its fixed initial allocation is
4097 entries × 24 bytes on 64-bit targets (98,328 bytes). Retained segments are
bounded. Compressed PNG ICC is itself read through the cancellation-aware view.

Native decoder byte depth and RGBA output bytes must each fit the caller's source
budget before allocating a raster. An RGB/RGBA16 PNG therefore consumes its true
6/8-byte source depth. Codec/header allocation limits use the separately admitted
working allowance, not the caller's pixel ceiling: a valid 60-byte TIFF still
needs bounded IFD/intermediate space. Constructor admission reserves at most RGBA8
header precision (RGBA16 for 16-bit PNG), then uses the decoder's actual native
byte count before pixel decoding. Header-output admission failures keep the static
`Decoded image exceeds the memory limit` diagnostic. A decoder-native precision
or RGBA-output rejection reports `Decoded image native precision or RGBA output
exceeds the memory limit`. Neither rejection decodes a raster first.
The application owns at most one decoded source and one
RGBA8 output; untagged/already-sRGB RGBA8 without orientation transfers its existing
raster directly. Conversion uses two 512-pixel RGB float buffers, 12,288 bytes in
total, with alpha read separately from the source. There is no full-size RGBA16
expansion just for alpha handling, full-image float conversion buffer, image undo
snapshot, cache, worker, background service or polling timer.

The active-image check uses the PLAN allowance `2*S + 2*E + 32 MiB`. It charges the
actual encoded owner's backing capacity, source/output rasters, actual segment index capacity,
a conservative 4 MiB color/profile/table reserve and up to twice the filtered JPEG
length for the JPEG decoder's growable `read_to_end` Vec. Oversized encoded input
with a tiny raster may therefore fail even below the 32 MiB file cap. File snapshots,
URL and native clipboard Vecs and GUI encoded Arc<Vec> owners report `capacity()`,
including unused growth capacity. Borrowed fixed headers supply dimensions for an
initial working-set check before allocating a segment index, profile or decoder.
A 20 MiB tiny-image input retained in a 32 MiB Vec is refused at that boundary.
There is no production shrink/reallocate workaround that hides the overlapping peak.

The compatible slice-only APIs cannot discover spare capacity outside the borrowed
extent. Their caller must account for that backing; internal owners use
`decode_editor_image_bytes_with_backing_and_cancel` and supply the real capacity.
Managed file snapshots allocate their retained extent once.
JPEG dimensions, component depth, oriented-size checks and this working-set
admission are read from the immutable SOF header before constructing the decoder;
its constructor is not allowed to allocate the encoded duplicate before refusal.

Reader/copy/CRC traversal is split into at most 64 KiB pieces. ICC inflation reads
8 KiB output batches; matrix conversion works in 512-pixel batches and charges
alpha/orientation work in that same bounded loop. The explicit cancellable byte
entry points check cancellation before preflight, native decode, output allocation
and after conversion as well. Existing editor File/Encoded input, explicit URL,
Task file and editor-document source/raster consumers pass their operation token
through snapshot and color work; compatible no-token wrappers remain available.
URL response copies and file snapshots check between 64 KiB pieces. Saved-PNG
loaders retain their prior policy. Native codec decode calls remain individually
non-interruptible; existing worker/job lifecycle limits still apply.

These checks bound application-owned admission and color work. They are **not a
measured proof of all codec-private allocations**: the image WebP adapter does not
implement a strict private-scratch allocation ceiling, and decoder/private memory
and native blocking cancellation remain separate backend acceptance work. Runtime
limits, noisy timing reports and performance targets must not be reported as
measured peak-memory guarantees.

## Exact dependency and independent oracle provenance

The implementation uses the versions already in Cargo.lock: image 0.25.10,
moxcms 0.8.1 and flate2 1.1.11. Adding direct dependencies does not introduce a
new crate version, external process, runtime or native CMS dependency. Primary
API verification used those exact cached crate sources: `ImageDecoder::total_bytes`,
PNG decoder metadata behavior, JPEG read-to-end behavior, `ParsingOptions`,
`ColorProfile::new_from_slice_with_options`, `create_transform_f32` and
`TransformExecutor::transform`. image 0.25.9 is not the API reference.

`crates/app-core/tests/fixtures/color/` contains generated ICC profiles, an RGB16
Display P3 PNG and the independent expected sRGB8 RGBA bytes. The generator is
`scripts/generate-color-oracle.py`; it uses the installed **LittleCMS 2.19** C API
through ctypes, D65 Display P3 primaries and the sRGB parametric transfer curve.
It calls `cmsCreateTransform` from RGB16 to RGB8 with relative colorimetric intent,
`NOOPTIMIZE | NOCACHE`, then computes alpha separately. No moxcms function or result
is used to generate expected RGB values. The fixture README records source tuples.
ICC header creation timestamps can differ on regeneration; expected pixel bytes
should remain stable. LittleCMS is a verification-only local tool, not a production
dependency. The CMYK JPEG refusal fixture was generated with Pillow 13.0.0.

The numerical test allows at most two 8-bit RGB levels of difference against
LittleCMS, while requiring exact alpha. It also exercises P3 CICP against the same
independent oracle, every orientation, RGB16 native-byte admission, hidden RGB,
untagged/explicit sRGB, JPEG fragmentation, WebP ICC, metadata conflicts, malformed
extents/curves/CRCs, profile bombs, CMYK/HDR refusal and cancellation. Additional
regressions cover an invalid parametric interval between the former sample points,
valid zero-domain/junction-rounding boundaries, a 24 MiB padded tiny JPEG refused
before any decoder input read, a capacity-sensitive 20 MiB PNG refused before any
managed index allocation, signaled snapshot cancellation and URL cancellation.

Independent per-channel ICC profiles remain admitted. The exact moxcms 0.8.1
source had two common-TRC predicates that accepted equality of only two neighboring
channels; its optimized path then reused red's transfer curve for every channel.
`vendor/moxcms` changes exactly those two predicates from OR to AND. The ordinary
per-channel engine is retained, with all-equal optimized paths unchanged. Its
three source/destination pairs of 65536-entry f32 tables total 1.5 MiB, versus
512 KiB for the all-equal path. The additional 1 MiB fits the existing 4 MiB
reserve. No version, runtime,
cache, image buffer or profile restriction is added. Exact archive checksum,
VCS revision, licenses and original byte inventory are in
`vendor/moxcms/LOCAL-PATCH.md` and `UPSTREAM-SHA256.txt`.

`scripts/generate-color-channel-oracle.py` independently uses LittleCMS 2.19 to
write and reopen parametric function0 and 257-entry `curv` profiles with sRGB
primaries. The ten cases cover `r=g!=b`, `r!=g=b`, `r=b!=g` and all-equal gamma1/
gamma2 controls, using RGBA16 input and exact alpha. For RGB128 with red/green
gamma1 and blue gamma2, the oracle is `[188,188,137]`; reusing red incorrectly
produces blue188. Application tests compare all source tuples and all eight
orientations for selected unequal-channel cases. Their files and source tuples
are recorded in `tests/fixtures/color/channels/oracle.json` beneath app-core.

The static color-policy category survives URL decode, clipboard decode and native
worker error transport (code 16). It contains no ICC/frame bytes, URL, source name
or untrusted string. Cancellation is checked before classifying a decode failure.
Other malformed/unsupported source errors may retain the generic URL/clipboard
transport diagnostic. A generated private CICP `[9,16,0,1]` fixture exercises the
loopback URL, private paste-decode classification and worker error round trip;
it does not read or change the user's clipboard or access an external provider.

The vendor-adjusted coordinated `release-fast` workspace passed 953 tests, zero
failed, with 69 explicit opt-ins excluded across 59 outer Cargo suites. Its 17
regular managed-color fixtures include the independent per-channel regressions.
Final formatting, all-target optimized Clippy, default-policy advisory checks
and all-workspace/all-target Windows GNU metadata checking pass. The GNU check
is type/API checking and executes no Windows binary. Exact logs, commands and
the unchanged 417-file source/provenance inventory are recorded in
`target/verification/current-three-final-checks.json`.

Fourteen actual headless CLI calls pass with isolated user-state directories.
Eleven source ICC profiles are reopened in LittleCMS 2.19 and their recomputed
RGB16-to-sRGB8 expectations match the committed oracle bytes exactly. Actual
`image convert` outputs stay within two RGB levels and preserve alpha exactly.
File and private-loopback URL HDR refusals retain SDR-sRGB guidance without
publishing an output; an existing output sentinel is preserved. Source bytes
and the complete isolated state tree stay unchanged. The executable SHA256 is
`d2bc869a6ea132bc00e5f3da81a22c0ebe6d59ffe712bdf958975d36e1fc90a9`;
`target/verification/managed-color-current-e2e.json` records every case/hash.
No user's clipboard, GUI, real provider or application data is accessed.

The explicit 15-sample 3840×2160 worker report on this Linux development machine
(Ryzen 7 9800X3D, Rust 1.100.0) observed median/max 2.228/2.994 ms for legacy
untagged PNG, 2.331/2.491 ms for managed untagged PNG and 116.601/120.040 ms for
managed P3. All use the same synthetic 33,177,600-byte RGBA raster; encoded
untagged input is 166,443 bytes. The entire test harness peaked at 76,576 KiB RSS,
which includes fixtures and is not an incremental production allocation bound.
Raw reports and hashes are in `target/verification/current-three-performance.json`.
These short development observations establish no capture latency distribution,
full-process private-memory, presentation, GPU, idle or reference-hardware gate.
The P3 transform is additional requested work; the source bounds and independent
numerical correctness checks remain separate from these timings.
