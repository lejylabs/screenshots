# Explicit region OCR and QR

Author: NewtTheWolf
Date: 2026-11-15
Status: implemented; verification evidence follows after execution.

## Contract

Add `screenshots ocr-region DISPLAY X Y WIDTH HEIGHT [--language LANG]` and
`screenshots qr-region DISPLAY X Y WIDTH HEIGHT`. Coordinates are nonnegative,
display-local native coordinates. Both commands validate arguments before any
native access, capture once, transfer owned pixels to the existing processor,
and print its existing JSON schema. No image file, history entry, clipboard
change, automatic URL action, upload, UI or persistent worker is added.

Exactly five positional arguments are required. Display IDs contain 1–64 UTF-8
bytes. Coordinates and dimensions parse as `u32`, origins must also fit nonnegative
`i32`, dimensions must be nonzero and both endpoint additions must fit `u32`.
Only OCR accepts the trailing `--language LANG` pair; its default is `eng`.
Language names use at most 64 ASCII letters/digits/underscores with `+` between
nonempty names. Unknown, duplicate and misplaced options fail before capture.
The [OCR contract](ocr.md) defines the exact `text`/`engine` JSON and engine
failure/cancellation behavior. The [QR contract](qr.md) defines `codes` with
`text`/`version`, UTF-8/no-code failures, seven-code saturation handling and the
8192-byte payload limit. No partial JSON is published after an analysis error.

The source region must fit 16 MiB tightly packed RGBA; QR also limits each side
to 4096. Validate actual returned geometry, backing bytes and exclusive pixel
ownership before processing. OCR keeps its existing one in-memory PNG and
30-second trusted-engine lifecycle. QR keeps its bounded synchronous detector.
Capture enumeration and native calls have no hard deadline; process termination
remains the escape for a stuck OS call. This is not native acceptance.

## Native admission

Use the existing XCap backend and `RememberedRegion`/`ExactRegion` snapshot
validation, including pre/post capture geometry checks. X11 retains its 100%-scale,
unrotated policy. Windows uses calibrated physical geometry. macOS explicitly
admits native pixel geometry with the full UUID/Quartz/filter proof; the modern
source must independently establish orientation and actual raster dimensions.
Its legacy source remains unrotated. Native platform acceptance remains separate.
Windows sets a scoped per-monitor-v2 thread DPI context before enumeration and
restores it afterward. This adds an existing windows-sys feature, no dependency.
The inherited metadata preflight also requires the complete display's dimensions
to fit the core 256 MiB RGBA limit, even though only the region is captured.
Returned backing length must equal width × height × four, not just fit the cap.

Linux requires an admitted X11 context with nonempty UTF-8 DISPLAY and no
Wayland session/descriptor authority. The old disk-backed fallback was removed.
The separate [in-memory portal still source](wayland-still-capture.md) returns
one user-picked still; it does not supply this command's named physical display
binding and exact-region geometry. Native Wayland region analysis therefore
remains unsupported rather than falling back to another source.
Check X11 absolute origins fit signed 16-bit and dimensions fit unsigned 16-bit
before XCap narrows them. All platforms check native signed-origin addition.
XCap and OS internal allocations are additional to the application pixel limit;
the admitted region is not a hard process-RSS cap. Geometry races remain bounded
by the existing before/after snapshot checks, not atomic OS capture.

## Implementation and alternatives

- Create `apps/desktop/src/region_analysis.rs`: argument/session/geometry
  admission, native capture and an injected execution seam for deterministic
  fixtures. Modify `main.rs` and `cli.rs` for headless routing/help.
- Modify `ocr.rs` and `qr.rs` to expose owned-pixel processors and existing byte
  limits, preserving file-command behavior and result schemas.
- Modify `apps/desktop/Cargo.toml` to enable the Windows HiDpi bindings.
- Create `apps/desktop/tests/region_analysis.rs` for executable rejection/no-state
  tests. Unit fixtures inject capture/analysis for transfer and failure checks.
- Create `scripts/test-region-analysis.py` for explicit synthetic native testing
  in a private cookie-authenticated, TCP-disabled Xvfb server. It requires Python,
  Pillow, libX11, xauth, xdpyinfo, Xvfb, a selected font and installed Tesseract;
  it never installs them. All fixture files/state are temporary, QR is generated
  into a fixture file solely to draw the test scene, and the server is stopped
  even if analysis fails. The product region commands do not create that file.
- Modify `docs/ocr.md`, `docs/qr.md`, `docs/gap-analysis.md` and
  `docs/non-ui-delivery.md` to state support and tested versus untested behavior.

Using temporary PNG input was rejected because it violates the existing in-memory
capture contract. Adding a second capture backend was rejected because existing
native snapshot validation is reusable. Unsupported session/scaling cases fail
before capture rather than silently changing the selected region. Editor actions
remain outside this non-UI slice.

## Verification plan

Check malformed/overflowing/oversized inputs, invalid languages, session/geometry
rejection before capture, changed/shared/oversized returned pixels, exact JSON
results, no-code errors and inert QR URLs. Re-run existing OCR/QR executable
fixtures. Check Windows compilation. Run optimized injected processing fixtures
and, if an isolated X11 server is available, real executable region analysis of
synthetic content there; do not read the user's desktop for test convenience.
Record platform and resource limitations separately from test passes.

Example opt-in native check after building an optimized binary:

```sh
python3 scripts/test-region-analysis.py --binary target/release-fast/screenshots --xvfb /path/to/Xvfb --language deu
```

This test paints only its private virtual display. It recognizes an inert URL
and `HELLO 123` five times by default, reports per-run latency, checks that
redirected user-state paths remain absent, and streams the binary SHA256 in
64 KiB chunks. It does not read the user's display or clipboard. Synthetic
800×600 timing does not establish physical input, mixed-DPI, memory or idle
acceptance. Windows/macOS native execution remains unverified for this entry.
