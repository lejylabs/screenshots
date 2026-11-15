# Explicit BMP conversion output

Author: NewtTheWolf
Date: 2026-11-15
Status: implemented, final workspace/release verification in progress.

Extend only `screenshots image convert INPUT OUTPUT.bmp` with exact RGBA8 BMP
output. Existing PNG/JPEG/WebP input admission, rotate/flip/downscale ordering,
size limits, no-overwrite publication and JSON summary stay in use. Keep the
editor's selectable/persisted format enum unchanged; this is a CLI conversion
output, not BMP import or a new default for captures, history or workflows.

Use the installed image crate's BMP encoder with RGBA8 and BITMAPV4 alpha masks.
No new dependency or full-image color conversion is required. Keep the 256 MiB
pixel limit, 64 KiB output buffer and existing atomic no-replace writer. Exact
width/height/backing-size checks occur before encoding. Partial alpha and hidden
RGB are preserved; independent decoder compatibility needs explicit verification.
This preservation describes encoding finalized pixels without resizing; the
existing alpha-weighted downscale can change colors/alpha and clear hidden RGB
when output alpha rounds to zero. BMP is uncompressed, so the encoded size is the admitted RGBA size plus bounded
header overhead. No palette/quantization, background matte, timer, extra thread,
GPU allocation or image-file round trip is added.

Create core `bmp_export.rs` with a narrow public `save_bmp` function that accepts
pixels and an output path; expose through lib.rs and reuse capture's private
atomic writer. Modify desktop conversion parsing only to select BMP by extension
and use the same downscale function as existing exports. Reject JPEG/WebP quality
flags for BMP. Leave ImageExportFormat::ALL and its UI consumers unchanged.
Create executable BMP fixtures, update CLI help, README/image-tool docs and the
delivery record. Test exact alpha/hidden RGB, odd rows, rotation/downscale,
malformed/oversized input, existing/broken-symlink destination, unchanged sources,
absence of user state and independent FFmpeg decode of actual CLI output.

Three core tests and two executable tests passed, including downscale and
malformed-source rejection. `scripts/test-bmp-export.py` independently decoded
the actual optimized CLI's 3×5 and rotated/flipped 5×3 files with FFmpeg, checking
exact RGBA including hidden RGB and alpha 0/128/255. Both were 182-byte BMPs;
source/state and pre-existing destinations stayed unchanged. Run it with an
explicit built executable:

```sh
python3 scripts/test-bmp-export.py --binary target/release-fast/screenshots
```

TIFF and GIF still output are separate work: verify TIFF alpha tags first, and
define GIF palette/partial-alpha loss and encoder resource limits explicitly.
