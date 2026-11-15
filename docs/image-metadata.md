# Explicit PNG, JPEG and WebP structure inventory

Author: NewtTheWolf
Date: 2026-11-15
Status: implemented, final workspace/release checks in progress.

Extend `screenshots image metadata INPUT` to identify PNG, JPEG or WebP by file
signature, not extension. Preserve the existing PNG JSON exactly. JPEG/WebP
results include an explicit `format`, byte size, dimensions, basic encoding or
animation flags, a bounded segment/chunk inventory and metadata presence.
Never decode pixels or print metadata payloads, comments, ICC, EXIF, GPS, XMP or
arbitrary application bytes. This inspects structure, not semantic metadata
values or complete compressed pixel validity.

Reuse the current PNG command's regular-file admission, one opened handle,
32 MiB extent-limited snapshot, 64 KiB reads, modification recheck and 30-second
cooperative deadline. Add two parser modules with 4096 segment/chunk caps,
checked offsets/lengths and the existing 256 MiB theoretical RGBA size limit.
JPEG walks markers including scan byte-stuffing/restarts and supports ordinary
baseline/progressive still headers; APP/COM payloads remain opaque. WebP checks
RIFF extent/padding and VP8/VP8L/VP8X dimensions/animation headers, retaining
unknown printable FOURCC names without treating them as instructions.

Create `metadata_jpeg.rs`, `metadata_webp.rs`, executable metadata tests and
update `png_metadata.rs` only for shared admission/dispatch. Add module declarations
in main.rs. Update this document, PNG metadata wording and delivery/gap records.
Test genuine encoder output, synthetic sensitive metadata suppression, malformed
lengths, truncated scans/chunks, bad dimensions, count limits and unchanged
sources/state. PNG regression behavior stays unchanged. No UI, format default,
network, clipboard, persistent cache, dependency or resident worker is added.

JPEG JSON contains `format: "jpeg"`, `source_file_bytes`, stored `width`/
`height`, `bit_depth: 8`, `components`, `metadata_present`, `progressive` and
`segments`. Each segment has a fixed marker name and payload `bytes`, excluding
the marker/length fields; standalone markers have zero payload bytes. Presence
means an APP0–APP15 or COM segment was found, not that sensitive values were
identified. Only 8-bit SOF0/SOF1/SOF2 with 1, 3 or 4 components are supported;
deferred-height DNL and other frame types are rejected. Dimensions are stored
dimensions, without EXIF orientation. Entropy, coding tables and progressive
scan completeness are not validated; a successful inventory is not a decode
or security verdict on the image.

WebP JSON contains `format: "webp"`, `source_file_bytes`, stored `width`/
`height`, `metadata_present`, `animated` and top-level `chunks`, each with its
FourCC `type` and payload `bytes` excluding headers/padding. Presence means an
ICCP, EXIF or XMP chunk was seen; it does not include unknown chunks. Unknown
chunks, including chunks nested in animation frames, can carry private data.
Nested chunks count toward the global 4096 limit but are not listed. VP8X feature
flags are not required to exactly match metadata chunks; reserved bits, selected
known-chunk ordering, frame bounds and minimum image headers are checked, not compressed
entropy or full format conformance. `metadata_present: false` never means the
file is free of private data. Use a pixel-only re-encode for sanitization.

Eight focused parser tests passed, including a real baseline JPEG, a real
10-scan progressive JPEG, generated WebP, malformed lengths and chunk limits.
One executable JPEG/WebP fixture verified signature dispatch, metadata payload
suppression and source preservation; all five existing PNG executable fixtures
also passed. Clippy and Windows GNU compilation passed before final validation.
