# Explicit QR commands

`screenshots qr generate TEXT OUTPUT_PNG` encodes 1–2048 UTF-8 bytes as one
normal QR code with medium error correction. It writes an opaque black/white PNG
with a four-module white quiet zone and four pixels per module. It supports
normal QR versions 1–40; Micro QR is not advertised. Existing output files are
never replaced. Generation prints `format`, `width`, `height` and
`error_correction:"M"` as JSON, without echoing
the supplied text to logs.

`screenshots link qr OUTPUT.png` provides a separate explicit
[upload-result input](upload-results.md): it reads one JSON object of at most
8 KiB from nonterminal stdin, validates its `url` and optional
`thumbnail`/`deletion` links, then encodes the canonical primary URL with the
same QR settings. It reports the same PNG metadata fields and never replaces
an existing output. Batch NDJSON and nested `capture-task` results are not
accepted. It neither uploads nor contacts, shortens, opens or copies any URL;
the formatted text variants are `link format plain|markdown|html`.

`screenshots qr scan INPUT` reads PNG, JPEG, WebP,
[supported truecolor BMP](bmp-input.md) or [supported classic TIFF](tiff-input.md)
through the shared bounded image loader. It prints one JSON object containing
`codes`, with `text` and
`version` for each decoded code. These are inert strings: a URL, command or Wi-Fi
payload is never opened, executed, copied or acted upon. Non-UTF-8 payloads,
unsupported/corrupt codes and images without a detected code fail explicitly;
there is no partial-success output. Alpha is composited onto white before
recognition, so hidden RGB cannot produce an invisible QR code.

TIFF scanning accepts one uncompressed RGB8 or explicit straight-RGBA8 image
in top-left chunky strips. Profiles, compression, other orientations and
multiple pages are rejected before decoding. The 32 MiB encoded limit and
the scan-specific decoded limits below still apply. The TIFF input contract
also accounts for its immutable snapshot and temporary decoder raster;
generation remains PNG-only.

`screenshots qr-region DISPLAY X Y WIDTH HEIGHT` captures one display-local
rectangle directly into the same processor, without a screenshot file. See the
[region analysis contract](region-analysis.md) for geometry/session restrictions
and native verification limits. This does not add an editor action.

Scanning admits at most 16 MiB of decoded RGBA source, 4 MiB of grayscale
bytes, and 4096 pixels per side. Source RGBA is released before detector buffers
are allocated. At most seven result strings of 8192 bytes each are retained. The
selected detector's own source limits region tracking to 65,534 entries and
capstones to 32; reaching either limit fails with a request to crop to fewer
codes instead of returning a silently incomplete list. The detector silently
stops at eight grids, so eight detected grids also cause failure, before JSON
publication. This conservative rule can reject a dense image instead of claiming
an incomplete scan was complete. The upstream geometry
and decoding work is synchronous and bounded by these admissions; a hard
wall-clock cancellation deadline is not claimed. Ctrl+C uses the existing
headless process-termination behavior.

Generation uses [qrcode 0.14.1](https://docs.rs/qrcode/0.14.1/qrcode/) with default
features disabled. Detection uses [quircs 0.10.3](https://docs.rs/quircs/0.10.3/quircs/),
a pure Rust port of quirc. rqrr was considered but its public detector does not
expose a candidate cap before combinatorial grouping and image cloning. The
selected dependency has source-enforced detector caps. These libraries add no
resident service, runtime, thread, timer, camera, stream, disk cache or network
action. The grayscale allocation is at most 4 MiB; the detector's u16 pixel
buffer is at most 8 MiB plus bounded region/candidate metadata and temporary
flood-fill storage. Decoder scratch and library-internal allocations count
toward active work; no GPU resources are used by these headless commands.

Real-executable fixtures cover Unicode/max-length generation-to-scan round
trips, exact opaque pixels, alpha visibility, malformed/no-code/oversized input,
candidate saturation, existing-output preservation and absent user-state writes.
Commands/results are recorded in the delivery report after execution. No native
camera, screen capture, OCR engine, UI workflow or full OCR/QR milestone acceptance
is established by this QR-only slice.
