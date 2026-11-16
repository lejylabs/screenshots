# Encoded image import

Date: 2026-11-16
Status: shared core byte admission implemented; native paste, raw drop and URL
consumers and their integration acceptance remain in progress.

`decode_editor_image_bytes` decodes explicit in-memory clipboard/drop/URL bytes
on a blocking worker. It uses the same decoding path as opened editor files:
PNG, JPEG, validated WebP, narrow truecolor BMP and uncompressed TIFF, with the
same orientation, alpha and 256 MiB decoded-source admission. Encoded input must
be nonempty and at most 32 MiB. Unsupported formats and malformed structural metadata fail; unreadable optional
WebP EXIF keeps the existing no-transform fallback.
No filename, temporary image, history entry or clipboard mutation is needed.

The existing file loader still freezes its opened regular-file extent before
JPEG reads and validates the same captured bytes for WebP/BMP/TIFF. Generalizing
the reader does not add a file-to-memory copy to PNG/JPEG. Byte inputs for
WebP/BMP/TIFF currently retain the original encoded input plus one validated
snapshot (at most 32 MiB each) while decoding; this is bounded active scratch,
not zero-copy or whole-process memory acceptance. There is no idle service.

Consumers must validate their own transfer/type/length boundary before obtaining
the bytes, then release encoded input after decoding. This API cannot make a
platform clipboard provider's earlier unbounded allocation safe by itself.
Native adapters must also bound transfer size, operation lifetime and stale
callbacks. Do not invoke toolkit clipboard calls from a wrong thread or put
image decoding on GPUI's foreground executor.

Initial optimized core fixtures pass exact PNG/WebP hidden RGB/alpha, empty and
unsupported formats, encoded-size rejection and valid-CRC oversized PNG header
rejection. Native transfer and end-to-end consumer checks remain open.
