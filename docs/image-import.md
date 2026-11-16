# Encoded image import

Date: 2026-11-16
Status: shared core byte admission and explicit URL CLI/Task input implemented;
final source review/current-tree integration and native paste/raw-drop consumers
remain in progress.

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


## Explicit URL input

`image-url URL OUTPUT [--allow-http] [--recipe FILE]` downloads a selected image
without writing an intermediate image. Task source
`{"type":"image_url","url":"https://example.invalid/image.png","allow_http":false}`
feeds the same recipe/action pipeline. A URL source is dynamic image data,
not a stable physical monitor or window binding. It adds no implicit upload,
history entry, clipboard mutation, startup fetch or idle runtime. The scheduler
freezes its URL/policy settings but may fetch current content at each due run.

Admission requires a nonempty HTTP(S) URL of at most 4096 bytes, no embedded
credentials/control characters/fragment, and HTTPS unless explicitly opted into
HTTP. URL values are omitted from Debug output and native errors. All action,
recipe, destination and format admission runs before network work. Redirects,
cookies and ambient browser state are not imported. Content is sniffed by the
shared bounded decoder, independent of server-provided Content-Type.

The existing operation-owned raw-frame worker isolates DNS, HTTP and decoder
lifetime. It initializes one current-thread HTTP runtime only while importing,
uses HTTP/1 with at most 64 headers, a five-second connect and 15-second whole
transfer timeout, and a 32 MiB actual streamed-body ceiling, including responses
without Content-Length. The parent applies its 20-second operation deadline and
owned-family cleanup/pending-admission rules even if a native resolver/codec
cannot finish. This is not a promise to interrupt every kernel operation.

Only one decoded RGBA frame crosses the authenticated private channel, with
fixed `image-url` identity and no invented physical geometry/window ID. PNG/JPEG
share borrowed encoded bytes; WebP/BMP/TIFF use the documented bounded extra
snapshot. Native process overhead and the 32 MiB encoded buffer are active costs,
not zero-copy or whole-process/reference-hardware acceptance. Color follows the
existing editor's documented 8-bit/sRGB assumption; this adds no HDR conversion.

Initial optimized tests passed core admission/redacted Debug/frame identity and
four actual headless executable cases: exact hidden RGB/alpha import, explicit
HTTP and output preflight, invalid/oversized/redirect responses, streamed size
rejection, owned-worker cancellation during a stalled body, and Task save with
existing-output refusal before a second GET. User directories stay untouched;
fixtures use unique localhost ports and generated PNG pixels. Current proof logs
are `target/image-url-{admission,expanded}-tests.log`. Final focused repeat and
source review remain pending; these are not real network/provider or hardware
acceptance.
