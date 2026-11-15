# Native Open With entry

`screenshots --open FILE` opens one bounded image in the existing standalone
editor. The path is admitted before desktop startup; absolute resolution and
worker-side PNG/JPEG/WebP/BMP/TIFF decoding retain the existing limits. Source
bytes, history and clipboard remain unchanged until explicit output.

The request waits for settings and existing capture/output work; it cannot
replace an active editor. Only one pending path is retained. Closing or quitting
keeps the existing stale-job and teardown rules. This file-editor process does
not register another tray or global-hotkey owner. Native associations invoke
this entry through the stable installed executable.

macOS also receives local `file:` URLs through GPUI's native open callback,
including an early event before the controller exists. Remote authorities,
malformed escapes, null bytes, raw query/fragment and oversized URLs are refused.

Optimized headless tests cover settings/output admission, exact pixels, source
preservation and URL admission. The maintained workspace run passed 597 tests
(56 ignored); it includes the Open With fixtures. Real Finder/Explorer/file
manager negotiation, native IME/accessibility and reference-hardware budgets
remain separate acceptance checks. No application layout was redesigned.
