# Explicit region OCR and QR

Author: NewtTheWolf
Date: 2026-11-15
Status: source implementation; the added portal route and source/cancellation
checks require the focused verification below. Earlier native evidence applies
to its original image only, not automatically to this revision.

## Contract

Add `screenshots ocr-region DISPLAY X Y WIDTH HEIGHT [--language LANG]` and
`screenshots qr-region DISPLAY X Y WIDTH HEIGHT`. Coordinates are nonnegative,
display-local native coordinates. Both commands validate arguments before any
native access, capture once, verify its source and transfer owned pixels to the existing processor,
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
The installed operation-owned NativeCaptureJob uses the existing five-second
enumeration and twenty-second still policies with cancellation and bounded
cleanup ownership. Kernel-uninterruptible cleanup can remain pending; no absolute
OS termination guarantee is added. This is not native acceptance.

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
is not admitted by this named-display command. Use the separate portal command
below to analyze selected-frame raster coordinates without changing this meaning.
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

## Explicit portal-selected region

Two distinct Linux commands preserve the named-display region API:

```text
screenshots ocr-portal-region display|window X Y WIDTH HEIGHT [--language LANG]
screenshots qr-portal-region display|window X Y WIDTH HEIGHT
```

Arguments and crop limits are admitted before permission access. The source token
must be exactly `display` or `window`; named monitor identifiers are not accepted.
The existing explicit ScreenCast/PipeWire still source obtains one user-selected,
normalized SDR frame using its authorized remote FD, serial, negotiated natural
raster and color/stride proof. Cursor exclusion is required. The existing
120-second overall permission policy and twenty-second post-permission frame
policy apply; no permission token, stream or runtime survives completion.

`X/Y` are top-left, nonnegative coordinates in the actual returned raster. Portal
logical position/size are not a physical display binding. The source can be a
window, its dimensions become known only after permission selection, and an
out-of-raster crop fails after the frame has arrived with no partial JSON.
The full selected source is capped at 256 MiB tightly packed RGBA; the crop is
capped at 16 MiB (QR also at 4096 pixels per side). The CLI cannot request a
smaller compositor raster or establish arbitrary physical multi-monitor
coordinates. It does not gain named Wayland display/region support.

Before either analysis route processes pixels it requires the exact expected
source identifier, no foreign window/physical desktop metadata, normalized
RGBA8 sRGB, nonzero exact raster dimensions/backing length and sole pixel
ownership. Native named sources retain RememberedRegion/ExactRegion geometry
checks before and after capture, reject ambiguous display identities and require
exactly one returned frame. Windows calibrated and macOS UUID/Quartz/filter proof
paths remain in the shared backend; the X11 scale/rotation and narrowed native
coordinate policies are preserved.

The selected portal frame moves directly to analysis when the crop covers it.
A smaller crop allocates only its admitted RGBA bytes and explicitly drops the
full source before analysis. OCR makes the existing single in-memory PNG handoff
to installed Tesseract; QR reuses quircs. No app UI, output image, history,
clipboard, URL action, automatic upload, engine installation, new dependency,
worker thread, permanent timer, queue or cache is added. These standalone JSON
commands do not add or modify Task actions, sources or sealed execution.

Unix routes reuse the scoped signal-safe cancellation flags already used by
recording. Checks precede capture, follow capture, precede analysis, follow
analysis and precede JSON output; the native check reaches the existing worker
supervisor. OCR retains its own thirty-second engine lifecycle and signal cleanup, while
carrying this same latched request through in-memory PNG encoding, the check
immediately after encoding, pre-spawn admission and every existing active engine
wait. Encoding cancellation cannot be lost when the engine installs independent
signal registrations. Windows uses the same callback for its job/start/resume and
active wait checks. The file OCR command keeps its existing wrapper/API.
QR remains a bounded synchronous decoder: cancellation is observed when it
returns, rather than interrupting its internal detector. Windows retains the
existing console/process-exit Job Object cleanup behavior, without adding a new
console handler. Once explicit output begins a signal can terminate a blocked
write; no promise of atomic pipe writes or rollback after partial stdout failure
is made.

## Resource cost and verification gates for this revision

Idle cost is zero. Named-region pixel admission stays at 16 MiB. Portal capture
uses the existing worker's bounded parent/child source representations (each up
to 256 MiB decoded RGBA), its existing PipeWire mapping/driver allocations, then
at most one additional 16 MiB crop while the parent source is alive. The parent
source is released before OCR's at-most-32-MiB encoded handoff or QR's at-most-
4-MiB grayscale plus bounded detector buffers. Existing trusted Tesseract process
memory is additional. Scoped Unix cancellation adds two shared atomic flags and
four registrations only during this headless operation, all removed at scope
exit. These are source-level bounds, not measured RSS/private-memory results or
a relaxation of PLAN's active-image budget.

Added pure tests cover source/backing/ownership proof, native geometry policies,
portal parser/bounds, exact natural-raster crops, whole-frame ownership transfer,
and cancellation at every pipeline boundary including suppression of late
success. Executable preflight tests reject invalid arguments before session,
engine or permission access and verify absent user state. The private generated
portal fixture reuses the existing isolated D-Bus/PipeWire server and publisher:
its 512×384 opaque raster contains synthetic `HELLO 123` and an inert QR URL;
the portal deliberately reports 1×1 logical metadata. Both display and window
routes must return the real QR/OCR results, reject blank/out-of-bounds crops,
close sessions and leave user state absent. Denial and active SIGTERM must emit
no JSON and close the owned session. The SIGTERM fixture waits for a delivered
authorized remote FD, a fresh source-consumer link and a publisher-confirmed
streaming/stalled marker armed after those admissions; CreateSession alone is
not permission-completion evidence. Tesseract plus `eng` must already exist;
missing tools fail this opt-in gate rather than producing a fake pass.

No build, Rust test, native API, Xvfb, D-Bus/PipeWire service or OCR engine was run
by the source-delivery agent. Root owns execution and compiler serialization.
After applying the frozen files and formatting, run focused optimized checks:

```sh
cargo test -p screenshots --profile release-fast --bin screenshots region_analysis::tests -- --test-threads=1
cargo test -p screenshots --profile release-fast --bin screenshots ocr:: -- --test-threads=1
cargo test -p screenshots --profile release-fast --test region_analysis -- --test-threads=1
cargo test -p screenshots --profile release-fast --test ocr -- --test-threads=1
cargo test -p screenshots --profile release-fast --test qr -- --test-threads=1
SCREENSHOTS_PORTAL_ANALYSIS=1 cargo test -p screenshots --profile release-fast --test capture_portal portal_memory_capture_uses_the_authorized_fd_and_exact_serial -- --exact --ignored --test-threads=1 --nocapture
python3 scripts/test-region-analysis.py --binary target/release-fast/screenshots --xvfb /path/to/Xvfb --language eng
cargo check -p screenshots --tests --target x86_64-pc-windows-gnu
```

Run the private portal gate with other SCREENSHOTS_PORTAL fixture mode variables
unset and tools `dbus-daemon`, `pipewire`, installed `tesseract`/`eng` available.
It creates only owned services and never connects to the user desktop, bus,
PipeWire or permission store. The existing authenticated, TCP-disabled Xvfb
fixture remains the named-X11 positive gate and never uses the user's display.
Record binary hashes and actual optimized timings/memory separately. Synthetic
source tests and compilation do not establish Windows/macOS native OCR/QR,
physical capture/DPI/topology, native compositor permission/cancellation,
reference-hardware latency/private-memory or end-to-end UI acceptance. Those
platform and PLAN gates remain open until run on their actual environments.
