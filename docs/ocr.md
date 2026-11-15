# Explicit OCR

`screenshots ocr INPUT [--language LANG]` recognizes a PNG/JPEG/WebP,
[supported truecolor BMP](bmp-input.md) or [supported classic TIFF](tiff-input.md)
file through an optional, already-installed `tesseract` executable. The default language is
`eng`; a bounded name such as `deu+eng` selects installed language data. The app
does not install an engine, download trained data or contact a network service.
Missing engines or language data fail explicitly. The Unix launcher uses a
dedicated process group; the [Windows launcher](windows-ocr.md) uses a
kill-on-close Job Object. Native Windows execution remains a separate acceptance
gate.

The command bypasses GPUI, preserves the source and writes exactly one JSON value
with `text` and `engine` to stdout after successful engine completion. It never
creates image/output files, modifies history/configuration, copies the clipboard,
opens recognized URLs or uploads anything. Recognized content appears only in
the explicit result; engine diagnostics are counted and discarded, not logged.
Errors use fixed engine messages rather than echoing captured text or paths from
external diagnostics. The file utility does not expose an editor action.

`screenshots ocr-region DISPLAY X Y WIDTH HEIGHT [--language LANG]` instead
captures an explicitly selected display-local rectangle and sends its owned
pixels through the same processor. It creates no screenshot file. The
[region analysis contract](region-analysis.md) describes session, scale,
coordinate and source limits; Wayland, rotated displays and scale other than
100% are rejected for this path. Native acceptance is tracked separately.

Input is limited to 32 MiB encoded and 16 MiB decoded RGBA. TIFF input is one
uncompressed RGB8 or explicit straight-RGBA8 image in top-left chunky strips;
profiles, compression, other orientations and multiple pages are rejected.
The existing decoder applies admission before pixel decode and EXIF
orientation for applicable formats. TIFF's immutable snapshot and temporary
decoder raster follow its [input memory contract](tiff-input.md#resource-cost-and-cancellation).
Pixels are encoded once to an in-memory PNG, at most 32 MiB, and released before the engine starts;
there is no image-file handoff. One invocation owns at most three pipe workers
with requested 256 KiB stacks, one three-message channel, 256 KiB recognized
output and an 8 KiB read chunk per reader. Stderr is capped at 16 KiB and never
retained. The optional engine receives `OMP_THREAD_LIMIT=2`.

Engine work has a 30-second operation deadline. Output overflow, failure and
deadline expiry kill its dedicated Unix process group (or close the Windows job)
and reap the direct child
before joining pipe workers, preventing descendants that stay in that group from
retaining pipes after a timeout. Waiting
uses a 25 ms operation-scoped check; there is no resident timer, OCR thread,
runtime, cache or eager engine load. Native process creation, filesystem and
pipe calls still depend on the operating system. The engine is a trusted local
executable; its separate memory/OS allocations are not hard-capped by this app.
Their reference-hardware memory/latency acceptance remains open.

On Unix, scoped signal-safe flags handle SIGINT/Ctrl+C and SIGTERM while the engine is
active. The operation observes the flag at its next bounded wait, kills the whole
engine group and reaps the direct child before returning an error. Signal
cancellation registrations are removed afterward. Two small conditional-shutdown
actions remain for this headless process's lifetime, so SIGINT/SIGTERM still
terminate a blocked final stdout write after the engine has been cleaned up.
The command exits instead of entering the resident UI; no signal worker thread
is created. Engine descendants that deliberately escape the group are outside
this trusted-engine contract. Windows uses OS handle cleanup on process exit;
its different console and pre-assignment crash limits are documented separately.

Languages accept at most 64 ASCII letters/digits/underscores, with `+` between
nonempty names. Arguments are passed directly through `Command`, never a shell.
The segmentation mode is fixed to Tesseract mode 6 (one text block). OCR accuracy
and layout reconstruction depend on the engine/language; empty text is a valid
successful result. This does not promise handwriting or document-layout support.

Successful JSON contains exactly `text` (a UTF-8 string preserving engine
whitespace and trailing newlines) and `engine` (the literal `tesseract`). Invalid
UTF-8 engine output fails; it is never replaced or decoded lossily.

Regular tests use fake engine fixtures and synthetic images to check byte limits,
invalid arguments, missing engines, JSON output, discarded diagnostics, source
preservation and absent user-state changes. The ignored
`real_tesseract_recognizes_synthetic_printed_text_without_user_state` test runs
the installed engine with `--language deu` on a generated bitmap reading
`HELLO 123`; invoke it
explicitly for real engine evidence. A skipped fixture is not native acceptance.
The development host has `deu` data but no `eng`; the default is not silently
changed to another language when a requested model is unavailable.
