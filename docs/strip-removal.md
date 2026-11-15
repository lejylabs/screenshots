# Exact strip removal

Date: 2026-11-15
Author: NewtTheWolf
Status: implemented, final regression/release verification in progress.

Add `image remove-strip INPUT OUTPUT horizontal|vertical START LENGTH` and a
recipe step `{"op":"remove_strip","axis":"horizontal","start":N,"length":N}`.
Horizontal removes complete rows; vertical removes complete columns. The
remaining sides join without resampling. Positions are zero-based. Reject zero
length, overflowing/out-of-bounds ranges and removal of the whole axis.
Recipe coordinates refer to the current image after preceding steps, including
crop, rotation or another strip removal.

Keep exact retained RGBA, including hidden RGB and partial alpha, and preserve
the source file. Reuse supported shared input and PNG/JPEG/WebP output; JPEG's
opaque-only rule still applies. Recipes publish only the final pixels. The
existing 256 MiB admission applies before one output allocation, with direct
row/segment copies and no full-image undo, extra runtime or GPU resource.
During copying, source and output coexist; the source is then released. Exact
retained pixel values describe the core result and lossless outputs; JPEG
encoding remains lossy.

Create core strip.rs, expose remove_image_strip and call it from existing image
utilities/workflow. Update CLI help, image/workflow docs and executable tests.
Test all axis boundaries, exact pixel order, invalid ranges, source preservation,
composition with redaction and one final output, no overwrite and no user-state
writes. This adds an offline tool, not editor selection or scrolling capture.

Three core and three actual-executable tests passed, covering both axes and
boundary positions, exact retained RGBA, invalid ranges, no overwrite, and
redaction followed by strip removal. A failing later recipe step leaves no
output and preserves the source. Clippy and Windows compilation also passed;
the complete combined-tree regression and release checks are recorded separately.
