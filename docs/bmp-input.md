# Bounded truecolor BMP input

Date: 2026-11-15
Author: NewtTheWolf
Status: implemented and locally verified; broader BMP variants remain unsupported.

Allow BMP in the shared explicit image loader after immutable-snapshot preflight.
This lets converted BMP images reenter image tools, file OCR and QR scanning.
It adds no UI control or default and does not add BMP to Library import.
Current Library originals are PNG/JPEG/WebP; PNG-specific compatibility APIs
remain narrow. See [format policy](capture-library-formats.md).
All shared-loader callers gain this support, including workflows, image batches,
image collection tools and the existing Open image worker. No new picker filter,
button or other UI control is introduced.
Read at most 32 MiB encoded and apply the caller's decoded byte budget plus the
existing 256 MiB ceiling before invoking the installed BMP decoder.

Accept 40-byte BITMAPINFOHEADER with uncompressed 24-bit BGR or 32-bit BGRX.
BGRX's fourth byte is ignored and the result is opaque. Accept 56/108/124-byte
headers with 32-bit BI_BITFIELDS only when masks are exactly R=00ff0000,
G=0000ff00, B=000000ff, A=ff000000. The fourth channel is then straight alpha.
For V4/V5 require sRGB and reject embedded/linked profiles or calibrated color.
Reject indexed/palettized, 16-bit, RLE, JPEG/PNG compression and arbitrary masks.

Check file signature/reserved fields, exact declared file size, header length,
planes=1, nonzero signed dimensions, width positive and absolute height <=65535.
Reject i32::MIN height and dimensions above the installed decoder's limits.
Compute padded row stride and byte extent with checked arithmetic; require
pixel offset immediately after the admitted header and raster end at EOF.
Image-size field must be zero or match the raster. Palette counts must be zero.
Snapshot preflight and decoding use the same bytes, preventing pathname/header
changes from bypassing admission. This is a deliberately bounded subset.
Concurrent in-place writes can affect the snapshot while it is read, but the
resulting same immutable bytes must pass validation before decoding. Native
filesystem calls retain their existing blocking limits; no 30-second read or
decode deadline is promised here.

Create core bmp_input.rs for preflight and tests, modify clipboard.rs's shared
loader to snapshot BMP just as it snapshots WebP, and update imports/docs/tests.
No new package or resident work. The snapshot costs at most 32 MiB; 24-bit/BGRX
decode can temporarily own RGB plus converted RGBA buffers (about seven bytes
per pixel), while alpha bitfields decode directly to RGBA. Retain existing
orientation/resource checks and no image disk round-trip.

Test own BMP-export round trips including alpha and hidden RGB, padded 24-bit
rows, top-down orientation, opaque BGRX, invalid masks/lengths/offsets/profiles,
extreme signed dimensions and smaller OCR/QR budgets. Use real executable
conversion and independent FFmpeg pixel comparison with synthetic images only.

Three preflight tests and three executable tests passed. A separate loader
fixture admits exactly 24 decoded bytes and rejects the same image with a
23-byte budget. The standalone `scripts/test-bmp-input.py` generates synthetic
PNG, converts it to BMP and back, verifies exact RGBA with FFmpeg and scans an
inert QR payload from BMP. The optimized executable passed that check without
source or user-state writes. Clippy and Windows compilation passed.
