# Project Plan – Open-Source Capture & Sharing Application

> **Simple for beginners. Powerful for professionals.**

**Status (2026-11-08):** This is a production delivery plan, not a claim that the current application is production-ready. Phase 0 remains open; see [the measured spike evidence](docs/phase-0.md). The performance contract below applies to the entire application and every future feature. Budgets are acceptance targets, not measurements of the current prototype.

## 1. Project Vision

Build a modern, open-source desktop application for screenshots, screen recording, image editing, and sharing.

The goal is to create a serious alternative to ShareX with a significantly more modern user experience, a clean architecture, and long-term extensibility.

Rather than simply copying ShareX's feature set, the application should make powerful functionality accessible without overwhelming new users.

### Core principles

- **Balanced UX:** Simple for beginners, powerful for professionals.
- **Cross-platform:** Windows, Linux, and macOS from day one.
- **Performance:** Near-zero idle activity, small bounded memory use, and immediate visible input response. Performance is a feature and a release gate from Phase 0 onward, including all future extensions.
- **Open source:** Transparent, community-driven development.
- **Local-first:** Fully usable without an account or cloud service.
- **Extensible:** Modular architecture for future integrations and workflows.
- **Privacy:** Users retain control over their screenshots, recordings, and uploads.

### Platform strategy

All three desktop platforms are first-class targets from the beginning:

- Windows
- Linux (X11 and Wayland)
- macOS

Linux must be treated as multiple environments rather than one uniform platform. Different desktop environments and compositors can have different capture capabilities and restrictions.

First-class support means each platform is tested and ships the capture modes it can actually perform. It does not mean every mode exists on every compositor in the first release. Where a mode is missing, the interface hides it and reports the limitation. It does not offer a control that fails later.

---

## 2. Technology Stack

The application will primarily be written in Rust.

| Area | Technology | Purpose |
|---|---|---|
| Programming language | Rust | Main application language |
| Desktop UI | GPUI | Native desktop interface |
| Async runtime | GPUI executor + Tokio | UI on GPUI's executor; Tokio only for HTTP and similar I/O |
| HTTP client | Reqwest | Uploads and HTTP communication |
| Serialization | Serde | Data serialization |
| Database | SQLite | Local metadata and history |
| Database access | **rusqlite** | Lightweight, synchronous SQLite access |
| Database migrations | rusqlite_migration | Schema versioning and migrations |
| Image processing | image | Decode, encode, crop, and pixel filters such as blur and pixelate |
| 2D rendering | tiny-skia | Vector annotations such as arrows, shapes, and text |
| Video processing | FFmpeg | Recording and encoding after the MVP. Not an MVP dependency |
| Clipboard | arboard / native APIs | Clipboard integration |
| Global hotkeys | global-hotkey / native APIs | Keyboard shortcuts |
| File dialogs | rfd / native APIs | Native file selection |
| Logging | tracing | Structured logging |
| Error handling | thiserror + anyhow | Application and library errors |
| Configuration | TOML | User settings |
| IDs | UUID | Unique identifiers |
| Time | Chrono | Date and time handling |
| Build system | Cargo | Rust workspace and builds |
| CI/CD | GitHub Actions | Cross-platform builds and testing |

Dependencies are initial candidates, not unconditional commitments. Check maintenance status, licenses, platform support, and actual performance before adopting them.

### Stack constraints

- Pin an exact GPUI source in Phase 0, either a Zed revision or a reviewed crates.io snapshot. The current evaluated packages are `gpui-unofficial` / `gpui-platform-gpui-unofficial` 1.27.0. Record source provenance, packaging/platform patches, licenses and maintenance ownership; a version label is not proof of an unmodified or officially maintained upstream build. Forks that bundle tray/hotkeys/overlays are not adopted solely for those features.
- System tray, global hotkeys, clipboard, notifications, and file dialogs are platform crates. They are not GPUI features.
- GPUI's foreground executor is the UI runtime. Tokio is not. See the execution model below.
- Still-image codecs/CPU operations use the `image` crate. tiny-skia is a vector rasterization candidate; text requires a shaping/font pipeline. Blur and pixelate have an immediate bounded-resolution preview and are flattened in document order during export.
- FFmpeg stays out of the MVP. Before bundling it, review the exact build, enabled components and distribution obligations. A GPL-enabled build can impose GPL obligations on the combined distributed program; the MIT license on our own source does not remove those obligations. A separate executable is not a blanket exemption. Record a compatible distribution decision before integration.
- Keep default features and platform dependencies narrow. Load optional services only when used. Measure release profiles, LTO and dependency changes; do not assume Rust, a GPU renderer or more threads automatically make the application fast.

### Database decision: rusqlite

Use `rusqlite` instead of SQLx.

The database is primarily responsible for local metadata: captures, uploads, history, and application state. This does not justify the additional asynchronous database abstraction and infrastructure that SQLx introduces.

Reasons for choosing rusqlite:

- Lightweight and direct SQLite integration.
- No unnecessary async database layer.
- No connection pool required for the initial use case.
- Direct access to SQLite functionality.
- Simple and predictable database architecture.
- Less infrastructure and fewer moving parts.

Implementation guidelines:

- Enable SQLite WAL mode.
- Use prepared statements.
- Use transactions for related database changes.
- Add migrations with the first real database schema (Phase 5), not an otherwise unused database service during startup.
- Keep database access off the GPUI foreground executor. The render path is part of that executor.
- Execute blocking operations through a dedicated database worker or controlled blocking tasks.
- Start with a single database connection; introduce additional connections only when justified by actual workloads.

Suggested migration dependency: `rusqlite_migration`.

---

## 3. Architecture

Use a modular Rust workspace with clear boundaries between the UI, application logic, platform integrations, and infrastructure.

### Architectural principles

1. GPUI views contain presentation and interaction, not business logic.
2. The application core must not depend on GPUI.
3. Platform-specific implementations must be hidden behind common interfaces.
4. Database and filesystem operations must be isolated from UI rendering.
5. Upload providers must be interchangeable.
6. Features should be modular without introducing unnecessary abstraction.
7. Keep the architecture suitable for a single application rather than prematurely introducing microservices.
8. Capture, clipboard, hotkeys, tray, and notifications sit behind platform interfaces. The UI calls those interfaces and does not call native APIs itself.
9. The capture interface must be implementable by a fake backend, so UI and editor tests do not need a physical display.

### Proposed workspace structure

```text
project/
├── apps/
│   └── desktop/
│       └── src/
│           └── main.rs
│
├── crates/
│   ├── app-core/
│   ├── app-ui/
│   │
│   ├── capture/
│   ├── capture-platform/
│   ├── capture-windows/
│   ├── capture-linux/
│   ├── capture-macos/
│   │
│   ├── image-editor/
│   ├── media/
│   │
│   ├── clipboard/
│   ├── hotkeys/
│   ├── system-tray/
│   │
│   ├── storage/
│   ├── history/
│   │
│   ├── uploader/
│   ├── uploader-providers/
│   │
│   ├── settings/
│   ├── commands/
│   └── shared/
│
├── assets/
│   ├── icons/
│   ├── fonts/
│   └── themes/
│
├── packaging/
│   ├── windows/
│   ├── linux/
│   └── macos/
│
├── docs/
│
├── Cargo.toml
└── README.md
```

This is a responsibility map, not a list of crates to create. The existing four-crate workspace is the starting point; most responsibilities should initially remain modules.

Start with a smaller number of crates and split modules when their responsibilities and complexity justify it.

### Main architectural layers

**Application UI**
- GPUI components and views.
- Navigation and window management.
- User interaction and presentation.
- No direct platform or database implementation dependencies.

**Application Core**
- Application state.
- Commands and events.
- Feature orchestration.
- Capture, editing, storage, and upload workflows.

**Platform Layer**
- Native screenshot capture.
- Window and display enumeration.
- Global hotkeys.
- Clipboard.
- System tray.
- Platform-specific permissions and integrations.

**Infrastructure Layer**
- SQLite database.
- Filesystem storage.
- HTTP uploads.
- Configuration.
- Logging.

### Execution model

GPUI runs the interface on its own foreground executor. That executor is not a Tokio runtime. Do not start the process with `#[tokio::main]`; do not block it or poll futures that require a Tokio reactor there. Sending a request to an infrastructure-owned runtime is allowed.

Use three places for work:

- **Foreground:** GPUI state, rendering, short input handling, and native event registration/window calls that require this thread. No blocking enumeration, capture, database, filesystem, image encode/decode, filters, synchronous logging or HTTP here. Defer native work that can synchronously re-enter borrowed toolkit state.
- **Blocking workers:** SQLite, image encode and decode, thumbnail generation, and native capture calls that are synchronous. One database connection on one worker is enough to start.
- **Tokio runtime:** HTTP uploads and similar network I/O, owned by the infrastructure layer. Completed work is delivered back to the foreground executor.

Native capture APIs that must run on a specific thread stay on that thread. The core talks to them with a message. It does not pretend that every platform API is async.

### Performance contract — all versions and features

The application should feel absent when idle and immediate when used. Zero RAM use and zero physical latency are impossible; hidden work, unbounded caches and visibly stalled input are not acceptable substitutes. Do not lower budgets silently to make a benchmark pass. A failing backend or UI toolkit must be reduced, replaced, or explicitly reconsidered before its gate passes.

#### Reference workloads and initial budgets

Use a documented modest reference machine: four CPU cores, 8 GiB RAM, an integrated GPU, SSD, and a 60 Hz display. Record actual model, OS/compositor, power mode, display resolution/DPI and driver. Run on AC and repeat responsiveness checks in the normal battery power mode. Also exercise 120/144 Hz input/rendering where available. A high-end development GPU is not the release baseline.

The following are initial release acceptance targets. Establish actual baselines in Phase 0 and retain results for each supported platform/session; these numbers are not claimed to be achieved today.

| Scenario | Acceptance target |
|---|---|
| Resident tray, all windows hidden, no jobs | Mean process CPU <=0.1% of one logical core over 5 minutes after warmup; no application polling timers, periodic enumeration, idle redraw loop or idle disk/network traffic |
| Resident tray memory, no live image consumers | <=50 MiB process private committed/footprint memory and <=16 MiB app-owned GPU resources; also report resident memory and driver allocations separately, without hiding them by trimming the working set |
| Visible empty control/history shell | <=100 MiB private memory and <=32 MiB app-owned GPU resources; no frames when unchanged; first usable window <=150 ms p95 when resident |
| Cold launch | Tray/hotkeys usable <=1 s p95; database history, upload runtime and editor loading must not gate readiness |
| Input response | Visible acknowledgment <=50 ms p95 for clicks/commands; selection/drag feedback by the next presented frame, without waiting for workers |
| Foreground work during interaction | <=4 ms p95 application CPU work per frame; <=16.7 ms p95 presented-frame intervals at 60 Hz, <=8.3 ms at 120 Hz; no application-caused stall >50 ms in the reference run |
| Warm resident region capture | Hotkey received to correct frozen overlay accepting input <=150 ms p95 at 2560×1440, <=250 ms at 3840×2160, excluding explicit OS permission UI |
| Copy selected region / full display | Release of selection or receipt of capture command to clipboard ready <=300 ms p95 at 1440p, <=500 ms at 4K; local save completion measured separately, target <=1 s p95 on the reference SSD |
| Return to tray | Release session pixel buffers, editor/overlay textures and temporary image references within 5 s after their last consumer; after 30 s, private memory <=idle baseline +16 MiB |
| Repeated use | After cache warmup, 500 capture/cancel/open/close cycles show no persistent upward allocation trend; retained private memory <=warm baseline +10 MiB, with OS/driver caches reported separately |

CPU means accumulated process user+kernel CPU time divided by elapsed wall time, multiplied by 100; it is not divided again by logical processor count. On Windows report Private Bytes as the private-memory budget and private working set separately; on macOS report physical footprint and resident size; on Linux report private mappings and PSS/RSS. These metrics are not interchangeable. Keep per-platform baselines and explain driver/shared-memory costs instead of claiming cross-platform byte equivalence.

Active-image memory is budgeted by pixels, not by a fixed idle number. Let `S` be the total decoded source bytes for the current session (including row stride and every selected display) and `E` the bytes of the selected output. For 8-bit RGBA, one tightly packed 4K image is about 31.6 MiB. Start with a total CPU-image-buffer limit of `2*S + 2*E + 32 MiB` above the shell baseline, and a GPU image allocation limit of `S + E + 32 MiB`. Every pool/cache counts against these limits; HDR/float buffers use their real byte depth. OS-owned clipboard copies and backend/driver allocations must be reported as additional costs, not silently omitted. Set an admission ceiling of 256 MiB decoded source data per job initially; larger desktops/images need an explained limit or a separately measured bounded/tiled path before acceptance. Never allocate to exhaustion or partially capture a desktop without telling the user.

Clipboard ownership can be a live consumer even with every window hidden, particularly on Linux. Preserve paste semantics: never clear the clipboard just to meet an idle number. Retain at most the single required output representation, release the larger source/preview data, and report its actual retained bytes separately; the private-memory ceiling in that state is the 50 MiB idle base plus that representation. Avoid an extra decoded copy when the native clipboard can serve encoded data. The return-to-idle and soak checks must compare identical clipboard ownership states, not disguise a retained image as a leak or exclude a real allocation from the total.

#### Mandatory implementation rules

- Idle is event-driven. Deliver hotkey/tray/IPC events through native callbacks and an executor wakeup/channel. No 20 Hz event-receiver polling, permanent capture streams, busy waits or resident housekeeping timers. A bounded, operation-scoped visibility wait is allowed; long-lived idle polling is not.
- Render only on invalidation. Coalesce mouse moves to the latest position per frame; avoid one task per move. Hidden windows must not schedule frames or retain full-resolution image textures. Reuse a bounded set of native overlay shells only if images/textures are released and measurements justify keeping the shells.
- Keep a captured frame in memory through selection, crop, preview and clipboard. Do not PNG-encode to disk and decode it again to feed the next stage. Store source buffers with explicit ownership; reuse immutable data where possible and copy only when a consumer requires a different representation. Record unavoidable backend/clipboard copies. Do not promise zero-copy without measuring it.
- A canceled selection must not save a full-desktop PNG, create history or change the clipboard. Release its buffers. Full-desktop debug dumps are explicit opt-in diagnostics with short retention, never the production default.
- Copy from pixels and encode once per required export representation. Clipboard round-trip reads and whole-image pixel comparisons belong in tests/diagnostics, not every production copy. Reuse encoded output for saving/uploading when the representation matches.
- Allocate each image/texture once per required representation. No full-bitmap undo snapshots, full-frame re-upload for moving a selection rectangle, per-frame filesystem reads or whole-frame filters on the UI thread. Use cropped/dirty-region work and cancel obsolete results. A reusable buffer pool is bounded in bytes and released when idle; it is not an excuse for permanent peak RAM.
- Every worker queue, cache and undo history has a byte/item bound. Start with one active capture job and no unbounded capture backlog; duplicate triggers coalesce or receive a busy response. Allow at most two uploads and one CPU-heavy image job initially. Inactive workers sleep; optional network/editor services initialize on demand. Benchmark before increasing concurrency.
- Capture and interactive preview work take priority over optional thumbnails/indexing. Do not place a hotkey capture behind a queue of background image jobs. Use the existing executors with bounded admission and measured scheduling before introducing a separate thread pool per feature.
- Add cancellation, deadlines, job/session IDs and stale-result checks to every long operation. A late result must not reopen a canceled overlay, copy an old image, replace a newer document, or dereference a removed window. Blocking APIs that cannot be interrupted need a bounded lifecycle or an isolated fallback; spawning another thread after each timeout does not solve a hung call.
- Enumerate displays/windows on demand or OS topology notifications, off the foreground executor where permitted. Refresh only affected metadata. Never enumerate every window after every capture just to update diagnostics.
- History uses indexed, paged queries and a virtualized list/grid. Decode only visible thumbnails with bounded prefetch and a byte-limited cache. Search cancels obsolete queries. Loading 10,000 history rows must not create 10,000 views or decode their originals.
- Recording, OCR, uploads, plugins and later features follow the same idle/UI/memory contract. They consume resources only when requested; hidden background work, streaming capture and automatic network checks require an explicit product need and their own measured active-work budget.

#### Measurement and regression gate

- Measure optimized packaged/release builds, not `cargo run` debug timings. Keep a profiling build with symbols. Compare LTO/codegen/startup tradeoffs rather than adopting every optimization flag by default.
- Record cold launch separately from warm resident capture. Timestamp input receipt, capture start/end, hide/compositor wait, first correct presented overlay, crop, clipboard completion and durable save separately. Report end-to-end latency as well as stage timings; native capture alone is not the user-facing result.
- Run at least 100 warm captures per latency scenario and 20 cold launches, report p50/p95/p99 and max (cold-launch percentiles are provisional with only 20 samples). Use fixed image fixtures plus real 1440p/4K, mixed-DPI and permission/portal sessions. Exclude user decision time only when it is also reported separately. Consent/backend delays are not excuses to freeze input or hide end-to-end latency.
- Measure idle before and after captures, editing, history scrolling and uploads; include CPU, wakeups, disk/network activity, private/resident memory, allocation high-water marks and GPU usage. Do not keep CPU sampling, frame counters or full-resolution dumps enabled in ordinary production idle.
- Test input while encoding, uploading, paging history and exporting large images. Verify queue admission, cancellation, disk-full and worker-failure behavior under load. Unit tests do not replace hardware presentation/energy measurements.
- Run deterministic allocation/copy/queue tests in CI. Use a stable reference machine for latency/energy comparisons; noisy shared-runner timings are reports, not trustworthy hard latency gates. Any repeatable >10% p95 latency/private-memory regression on the same workload requires a recorded cause and remediation before merge, even if the absolute budget still passes. Report small/noisy differences as inconclusive.
- Every phase has performance evidence for its new path. New dependencies, caches, background services and high-resolution pipelines need a stated resource cost. Performance is not deferred to Phase 6.

#### Prototype debt that must not ship

The initial polling loops and file-based frame handoffs have been replaced with native events and shared in-memory pixels. Canceled previews no longer save PNGs, and clipboard read-back runs only in tests. Windows now keeps controller/tray/hotkeys alive independently of windows, attaches graphics on demand and detaches after closing its UI. Capture groups now admit at most 256 MiB of decoded RGBA source data before native capture and recheck actual returned dimensions; individual capture, crop and recopy share the image limit. XCap still controls its internal allocations, including size-change races; a hard native allocation bound, other-platform resident integration and unmeasured reference-hardware memory/latency budgets remain production debt. See [Windows resident evidence](docs/performance-resident.md) and [control frame-callback timings/teardown regression](docs/control-open-latency.md); these callback timings do not establish physical presentation or usable-input latency. The [1440p native-hotkey region probe](docs/region-latency.md) records 101 real capture/cancel cycles and stage timings; 4K, physical presentation/input and reference-hardware acceptance remain open. Passing the functional spike is not a performance result. See [Phase 0 evidence](docs/phase-0.md).

### Process lifecycle

Windows implementation: normal startup uses SID/session-scoped named kernel objects with Windows default token security. A second launch sends only a bounded/coalesced Show signal, then exits before GPUI/tray/hotkey initialization. The listener starts after event delivery is installed and waits natively without polling. Ownership survives until GPUI shutdown completes. Resource cost: one parked native waiter with a 256 KiB requested stack and a small fixed handle set; updated idle measurements are not claimed here. The kernel-object handoff/restart test and a real two-process release smoke check passed: the secondary exited and native window enumeration confirmed a visible control window in the original process. The smoke processes exited on their diagnostic deadlines with no stderr errors. Cross-platform IPC, foreground-activation policy and shutdown/session/elevation races remain verification work.


A capture tool should run as a single instance. A second launch forwards its request to the running instance and exits. Two instances would register the same hotkeys and write the same history.

The running instance owns the tray icon, the hotkeys, and the database worker. Closing the main window hides the window. Quitting is explicit.

Use authenticated/user-scoped local IPC with bounded messages; a second launch must never silently create another tray/hotkey owner when forwarding fails. Native registrations stay on their required event-loop thread. Tray availability is a capability: sessions without a tray get an accessible window/launcher alternative, never a hidden process the user cannot reopen. Hide-on-close requires such a working reopen path. On quit, stop accepting jobs, cancel or bound outstanding work, release registrations and close storage in order. Test restart, sleep/resume, display disconnect, device loss and session lock; no stale frames or callbacks after shutdown.

---

## 4. Screenshot Capture Engine

The capture engine is one of the most important and technically challenging parts of the application.

It should be designed around a common API with platform-specific implementations.

### Required capture functionality

Implemented convenience workflow: a three-second delayed region selection with a cancel button/Escape in the control window. Closing the UI or quitting discards the countdown task; it adds no resident polling. The existing capture/selection/copy/save pipeline is reused after the delay.


- Fullscreen capture.
- Display capture.
- Window capture.
- Region capture.
- Multi-monitor support.
- Display and window enumeration.
- High-DPI support.
- Correct coordinate handling.
- Permission handling.
- Clipboard integration.

Define modes precisely: primary-display capture and explicitly selected-display capture return one image; all-displays capture returns one image per output in the MVP. Region selection is confined to one selected output initially. A stitched virtual desktop or a drag spanning mixed-DPI monitors is separate scope and must not be implied by the phrase multi-monitor support. Display/window IDs are session-scoped and can become stale; validate them and the topology at capture time and cancel selection on an incompatible topology change.

### Stream candidate for evaluation: Scap

Repository: https://github.com/CapSoftware/scap

The existing Windows still-image spike uses XCap. Evaluate Scap only where a stream API or missing backend capability justifies it, especially future recording/PipeWire work. Do not replace the measured still-image path just to use one library everywhere.

It advertises support for:

- Windows Graphics Capture.
- macOS ScreenCaptureKit.
- Linux PipeWire.

However, do not assume that every feature works identically across all platforms.

Scap is built around capture streams. A stream API is only a good fit for a still screenshot if the pinned revision can also return one frame cleanly. On Linux, a README line that says PipeWire is not evidence. Confirm that the revision builds against current PipeWire and returns a frame on each compositor you intend to support.

Test its actual capabilities before deciding whether to build additional native backends.

### Alternative: XCap

Repository: https://github.com/nashaofu/xcap

XCap 0.9.8 is the evaluated Windows still-image candidate. Measure its suitability on macOS and X11 before claiming those targets work. Pin accepted backend revisions and record the exact platform/session in the spike notes.

On Wayland, measure it before trusting it. Expect incomplete window lists, limited screen capture, and monitor positions that cannot be used to crop a region. A reported origin of `(0, 0)` for every output is a known class of failure. Do not build region selection on those coordinates.

### Wayland still capture

Also evaluate the XDG Desktop Portal directly, through a Rust portal client such as `ashpd`.

Use the portal as the Wayland still-image candidate on GNOME and other compositors with an appropriate backend. Introspect the available interface version and runtime capabilities; installed portal software is not proof that a particular capture mode works. It can be the Wayland backend while XCap handles Windows/macOS/X11.

Portal cancellation is a normal outcome, not an application error. Permission denial must be shown to the user with the next step, not swallowed as an empty image.

### Native platform implementations

**Windows**
- Windows.Graphics.Capture.
- Display and window enumeration.
- Multi-monitor support.
- DPI-aware coordinate handling.

**Linux – Wayland**
- XDG Desktop Portals.
- PipeWire.
- Permission-aware capture workflows.
- Compositor-dependent functionality.

**Linux – X11**
- Native X11 capture.
- Window and display enumeration.
- Multi-monitor support.

**macOS**
- ScreenCaptureKit.
- Native screen recording and capture permissions.
- Retina-aware coordinate handling.

### Common API

Expose a consistent interface to the application core.

Illustrative interface:

```rust
pub trait CaptureBackend: Send {
    fn capabilities(&self) -> CaptureCapabilities;

    fn list_displays(&self) -> Result<Vec<DisplayInfo>, CaptureError>;

    fn list_windows(&self) -> Result<Vec<WindowInfo>, CaptureError>;

    fn capture_display(&self, display_id: &str) -> Result<CapturedImage, CaptureError>;

    fn capture_window(&self, window_id: &str) -> Result<CapturedImage, CaptureError>;
}
```

`capabilities` is part of the contract and reflects the current session/API/permissions, not just `cfg!(target_os)`. Distinguish direct window enumeration/capture from portal-mediated user selection: a portal may offer a user-picked window without exposing a window list to the application. Unsupported actions stay absent or unavailable with an explanation. Display capture may be available only through a portal flow.

The example is synchronous on purpose. Native grabs may be synchronous or callback-based. The `Send` contract applies to a core-facing handle; thread-affine native objects stay on their owner thread behind that handle. Do not require `Sync`, transfer objects that are not transferable, or wrap an operation in an async trait just to look uniform.

Portal calls wait on D-Bus. Poll their runtime-dependent futures on the integration's executor and return a completion to GPUI. GPUI may await a compatible completion receiver without blocking; it must not block a worker for the entire time a user is deciding in a consent dialog. Portal requests support cancellation and do not occupy the image-processing queue while waiting.

The final type can be a trait or an enum. The application core depends on common result types either way. A production capture result owns or references validated in-memory pixels with dimensions, stride, channel/alpha format, color metadata and source identity; a filesystem path is an export result, not the transport between capture, overlay, crop and clipboard. Check byte-size arithmetic before allocation.

### Region selection

Region selection is not one portable workflow. Use the path the platform actually allows.

**Custom overlay.** Windows, macOS and X11 use validated native overlay windows. A Wayland custom overlay requires a compositor advertising the appropriate layer-shell protocol and a validated capture coordinate/input path. The pinned `gpui-unofficial` 1.27.0 source contains `WindowKind::LayerShell` and a `layer_shell` example; upstream also includes it. The old claim that GPUI has no layer-shell support is incorrect. An API/example alone does not validate output targeting, exclusive keyboard input, fractional scaling or our capture path. Layer-shell does not grant screen-capture permission or a usable global image coordinate system. Until those checks pass for a session, use portal selection/editor crop.

1. Hide this application's own windows, or exclude them from the capture, so the overlay is not part of the image.
2. Capture the selected display when the platform provides a frame and a usable coordinate space.
3. Show a transparent, frameless, always-on-top selection surface on that display.
4. Convert the selection from logical coordinates into physical pixels.
5. Crop the captured frame.
6. Return the image.

**Portal selection.** Use this for Wayland sessions where our custom overlay/capture path is unavailable or unvalidated, including the planned GNOME path. Detect protocol availability on the running compositor rather than hardcoding support from a desktop name. An XWayland fullscreen window is not a substitute for a validated native Wayland overlay with correct keyboard access.

1. Inspect the portal version. With Screenshot interface version 3 or later, request a `target` only when advertised in `AvailableTargets`. On older backends, `interactive` is a customization hint, not a guaranteed area/window picker; validate the actual UX and provide editor crop where necessary.
2. Treat cancellation as a normal outcome.
3. Use the image the portal returns. Do not invent a global monitor origin the compositor did not provide.

**Editor crop.** If neither overlay nor interactive portal is available, capture a display and let the user crop in the editor.

A normal toplevel window is not an overlay. On Wayland, covering the screen requires layer-shell or the portal's own UI. There is often no global coordinate space, so a capture-then-crop overlay must not be the default Wayland design.

### Still-image format

Store still captures as PNG. Width and height in the database are physical pixels.

Keep channel order, stride, alpha representation and color space explicit at the capture boundary. MVP export targets SDR sRGB PNG with appropriate color metadata. Convert supported tagged sources into that space once; validate HDR/tone mapping and wide-gamut inputs per backend or mark the mode unsupported. Unknown color metadata is an explicit limitation, not proof of sRGB. Do not claim HDR preservation for an 8-bit RGBA backend. Budget conversion buffers. JPEG can be added later as an explicit export option.

### Important edge cases

- Multiple monitors.
- Negative monitor coordinates.
- Different monitor resolutions.
- Mixed DPI scaling.
- Retina displays.
- Monitor rotation.
- Wayland permission restrictions.
- Compositor-specific limitations.
- Window movement during capture.
- Display changes during application runtime.
- The application's own overlay, editor, or main window appearing in the shot.
- Portal cancellation and denied/not-yet-granted permission are typed outcomes. Empty or structurally invalid frames fail. All-black pixels alone are not a reliable failure detector: a valid selection can be black. Use backend status/permissions to distinguish protected or unavailable content; never fabricate a successful frame after a backend error.
- No global coordinate space, including Wayland outputs reported at `(0, 0)`.
- Windows.Graphics.Capture consent UI and capture border.
- macOS Screen Recording permission that behaves differently for `cargo run` than for a signed, stable bundle. The spike must include the packaged form, not only a debug binary.

The application must expose available capabilities rather than promising identical functionality on every platform.

---

## 5. Application Core

The application core coordinates the individual features.

Keep the command and event architecture simple. A large event-bus framework is unnecessary at this stage.

### Example commands

```text
CaptureFullscreen
CaptureRegion
CaptureWindow
CaptureDisplay

SaveCapture
CopyCapture
EditCapture

UploadCapture
DeleteCapture

OpenHistory
OpenSettings
```

### Example events

```text
CaptureStarted
CaptureCompleted(CaptureId)
CaptureCanceled
CaptureFailed

UploadStarted
UploadCompleted
UploadFailed

CaptureSaved
CaptureDeleted
```

The UI sends commands to the application core and receives state updates or events.

The application core handles the actual workflows and communicates with the required services.

Model a capture session explicitly: idle, preparing, awaiting permission, selecting, processing, completed, canceled or failed. A terminal state restores or hides UI according to the initiating context and releases ownership exactly once. Give each session an ID; topology changes, cancellation and shutdown invalidate late callbacks. Restore UI on every failure path without forcing focus away from another application for a background capture unless the workflow needs it.

---

## 6. Local Storage and Database

The application should work completely offline without requiring an account or external service.

### Storage strategy

Use SQLite for metadata and the filesystem for actual image and video files.

Do not store large image or video files as database BLOBs.

### Database tables

Initial schema:

**captures**

```text
id
file_path
thumbnail_path
mime_type
width
height
file_size
created_at
source
```

**uploads**

```text
id
capture_id
provider
url
status
created_at
```

Additional tables can be introduced when necessary for recordings, workflows, or other functionality.

### Database implementation

- SQLite.
- rusqlite.
- rusqlite_migration.
- WAL mode.
- Prepared statements.
- Explicit transactions.
- Dedicated database worker or controlled blocking execution.
- No database access from the GPUI foreground executor, including during render.

When the first database schema is introduced in Phase 5, include migrations, foreign keys and indexes from its first version. Keep the database on a local application-data filesystem: SQLite WAL is not a network-filesystem design. The user-configurable export directory is separate. Bound busy waits and checkpoint work on the database worker; document the chosen synchronous/durability policy and test power-loss behavior. Do not disable durability merely to improve benchmark numbers. Back up before destructive migrations, test upgrades from every released schema, and open newer unknown schemas without destructive changes (or explain incompatibility).

### Filesystem

Use platform-appropriate application data directories.

Keep the storage implementation independent of the UI and database implementation.

Support:
- Original captures.
- Thumbnails.
- Recordings.
- Temporary files.
- Exported images.

The user should be able to configure the storage location.

File saving and SQLite are not one atomic transaction. Write to a uniquely named temporary file on the destination filesystem, finish/flush according to the stated durability policy, atomically publish without unintended overwrite, then commit ready metadata. Track pending operations and reconcile interrupted saves/deletes with bounded recovery work. Mark thumbnails as derived data that can be rebuilt lazily. A disk-full, permission or rename failure must not create a success notification or a history entry pointing at a missing file. Report Copy and Save independently when only one succeeds.

Retention is explicit and bounded for history, thumbnails, temporary files and logs. Cleanup may delete only app-owned files under validated storage roots; it must not traverse symlinks or delete arbitrary user exports. Changing folders, unplugging an export drive, and deleting records/files are tested workflows. Export deletion and metadata deletion have a documented order and recoverable partial-failure state. Do not keep sensitive full-desktop preview files after a canceled selection.

MVP history search covers metadata only: file name, date, capture source, and upload URL. Searching the pixels waits for OCR.

### Secrets

Upload tokens, passwords, and authorization headers do not belong in the TOML settings file. Store them in the operating system secret store: Windows Credential Manager, macOS Keychain, or the Linux Secret Service.

The TOML file may store a non-secret profile: URL, method, form field, and which secret to read. If no secret store is available, refuse to save the credential. Do not fall back to writing it into the settings file. Logging must not print secrets or authorization headers.

---

## 7. Upload and Sharing System

Sharing should be modular and independent of any particular hosting provider.

### UploadProvider abstraction

Create a common provider interface.

Illustrative concept:

```rust
pub trait UploadProvider {
    async fn upload(&self, file: UploadFile) -> Result<UploadResult, UploadError>;
}
```

The final implementation should support provider-specific configuration and authentication without exposing provider internals to the UI. The upload call runs on the Tokio runtime, not on the GPUI foreground executor. The `async` here is real network I/O. It is not a model for the capture interface.

### First implementation: Custom HTTP uploader

The first upload provider should be a generic HTTP uploader.

Configuration options:

- Request URL.
- HTTP method.
- Multipart form field.
- Custom headers.
- Authentication, read from the secret store rather than from the settings file.
- Request parameters.
- Response parsing.
- URL extraction.

URL extraction stays declarative: HTTP status, a response header, or a JSON pointer. The MVP does not embed a scripting language in the uploader. That would be a plugin runtime under another name.

This makes it possible to integrate many existing upload services without implementing a dedicated provider for every service.

Uploads are explicit opt-in actions, disabled until a destination is configured; capture and editing never upload implicitly. Default to HTTPS. Support user-configured local/self-hosted destinations deliberately, not through URLs taken from images or untrusted responses. Do not forward credentials or replay captured content to a different host through redirects; disable redirects by default. Stream files rather than loading a second full file into RAM. Start with bounded connect/transfer timeouts (5 s / 60 s), a 1 MiB response-body limit and the concurrency limits in the performance contract. Expose cancel and progress without per-packet UI updates. Retry with a finite policy only when safe; a POST is not automatically idempotent. Never auto-open extracted URLs; validate http/https before offering them, and treat returned text/HTML as data.

### Future providers

- S3-compatible storage.
- Cloudflare R2.
- Custom self-hosted upload servers.
- Community-maintained providers.
- Additional image and file hosting services.

Do not implement a large plugin runtime in the MVP.

Keep the provider architecture extensible so that a plugin system can be introduced later if it becomes useful.

---

## 8. Image Editor

The editor should be powerful but approachable.

A major architectural decision is to use non-destructive editing.

Instead of modifying the original image whenever the user adds an annotation, store annotations as individual objects and render them during export.

Crop changes the document canvas and is stored as a crop rectangle so it can be undone. Arrow, rectangle and ellipse are vector objects; tiny-skia is a rasterization candidate, not a text layout engine. Choose and test text shaping, font fallback and glyph rendering explicitly, with consistent preview/export layout. Blur and pixelate are ordered image effects: preview them during editing and flatten them into the exported pixels. Draw and apply effects in document order, not in separate passes that silently reorder overlapping objects. Immutable original pixels remain available in memory for undo until the document closes.

Undo and redo record document changes. They do not snapshot the full bitmap on every edit.

Render an immediate bounded-resolution preview while dragging effects; perform expensive refinement/export off-thread with stale-result checks. Limit undo by bytes and commands. Use the same crop/transforms, text layout, alpha handling and layer order in preview and export, verified by deterministic image fixtures. Zoom/pan/selection must not rerasterize the whole full-resolution document on each mouse move.

Only a finalized, flattened document may be copied, saved or uploaded after edits. Never upload the original as a shortcut when redaction/export is still processing or has failed. Raw unredacted originals, undo data and thumbnails must not survive in app-managed history or temporary files by default after a sanitized export; retaining originals is an explicit choice with a clear privacy implication. Previously exported user-owned originals are not silently deleted; explain that sanitizing a new export does not sanitize those earlier copies. Blur/pixelation are visual obscuring tools, not a guarantee of irreversible removal: offer an opaque redaction rectangle and explain the distinction when choosing the tool. Do not promise forensic secure deletion on SSDs or external filesystems.

### MVP editing tools

- Crop.
- Arrow.
- Rectangle.
- Ellipse.
- Text.
- Blur.
- Pixelate.
- Opaque redaction rectangle.
- Undo.
- Redo.

### Later editing tools

- Freehand pen.
- Numbering.
- Spotlight.
- Highlighting.
- Additional shapes.
- More advanced text options.
- Advanced image transformations.

### Editing model

Represent each annotation as an independent object with its own properties.

Examples:

```text
Arrow
Rectangle
Ellipse
Text
Blur
Pixelate
```

The editor should support:

- Selection.
- Movement.
- Resizing.
- Deletion.
- Undo and redo.
- Exporting the final rendered image.

Initially, annotations can remain in memory during editing.

A persistent editable project format can be introduced later.

---

## 9. User Interface and UX

The UI should feel like a modern desktop application rather than a collection of disconnected tools.

GPUI is the selected UI framework.

The application core must remain independent of GPUI. That boundary keeps a later UI replacement possible. It is not a promise that overlay behavior written for GPUI will move unchanged to another toolkit.

### Main navigation

Proposed sections:

```text
Captures
Uploads
Settings
```

Recordings appears when recording exists. Workflows appears when the workflow engine exists. An empty Home page is not part of the shell.

Not every section needs to be fully implemented in the MVP. Uploads can ship as soon as the HTTP uploader exists; until then the navigation does not show an empty section.

### Quick Access Overlay

After taking a screenshot, display a small floating interface with the most important actions:

- Copy.
- Save.
- Edit.
- Upload.
- Delete.
- Open in History.

The overlay should be quick, unobtrusive, and easy to use.

### UX principles

English is the default application language at launch, independently of the OS locale. All app-owned labels, menus, prompts, status text and errors must start in English. Localization may be added later; native OS dialog chrome and system error messages follow the OS language.


- Clean and modern interface.
- Minimal unnecessary UI elements.
- Clear visual hierarchy.
- Consistent design system.
- Keyboard-first workflows for power users.
- Discoverable functionality for beginners.
- Native platform behavior where appropriate.
- Dark and light themes.
- Immediate pressed/selection feedback; no debounce on direct manipulation. Search may coalesce input and cancel stale requests without blocking typing.
- No layout-blocking spinners, ornamental perpetual animations or simulated loading delays. Load fonts/assets on demand; respect reduced motion.
- Keyboard navigation, visible focus, scalable text and accessible labels. Verify actual assistive-technology behavior on each target; GPU rendering does not establish accessibility by itself.

### GPUI technical evaluation

Before building the full application, verify the pinned GPUI source itself:

- Transparent windows.
- Frameless windows.
- Always-on-top windows.
- Multiple windows.
- Placing a window on a specific monitor, including a monitor with a negative origin or a scale factor other than 1.
- Platform-specific window behavior.

Verify these separately. They are not GPUI features:

- Global hotkeys.
- System tray.
- Clipboard, including placing a PNG on the clipboard.
- Native file dialogs.
- Notifications.

If the pinned GPUI source cannot show a frameless, transparent, always-on-top surface on the monitor being captured, stop and decide on the UI again before Phase 1. On Wayland, also check whether that source can create a layer-shell surface. A transparent toplevel does not count. Do not build the design system on a toolkit that cannot host the region overlay, unless the product decision is to use the portal's selection UI for every session where the overlay is impossible.

Do not assume that every feature is equally mature on every operating system.

---

## 10. Desktop Integration

Native desktop integration is essential for an application of this type.

### Required functionality

- Global keyboard shortcuts.
- System tray integration.
- Clipboard access.
- Native file dialogs.
- Notifications.
- Floating quick-access overlay.
- Multi-window support.
- Startup behavior and application lifecycle handling.

Each platform should have its own implementation where necessary.

Avoid forcing one generic implementation onto platforms with fundamentally different APIs.

Global hotkeys are one of those splits:

- Windows: `RegisterHotKey`.
- macOS: a Carbon or Cocoa global hotkey, which needs an Accessibility permission depending on the API.
- X11: an X grab while the real X11 session is in use. An X grab inside XWayland does not deliver keys when a native Wayland window is focused.
- Wayland: the GlobalShortcuts portal, and only where that compositor's portal backend implements it. There is no general Wayland equivalent of `RegisterHotKey`.

The settings screen must be able to say that a global hotkey is unavailable here, and how the user can bind one through the desktop. A silent registration failure is not acceptable.

Clipboard images have the same kind of split. Writing image bytes that work on Windows does not mean the Wayland clipboard will offer them as `image/png`. Test the round trip on each session type.

The quick-access overlay is a small window, not the full-screen region overlay. It still has to position itself on the monitor that was captured, including mixed DPI and negative origins. On Wayland it only needs to be a normal window, so it must not depend on layer-shell.

---

## 11. MVP Scope

The MVP should already be useful as a daily screenshot and sharing application.

It should not attempt to implement every feature of ShareX immediately.

A platform that cannot list windows, register a global hotkey, or show a custom region overlay still meets this scope when the modes it does support work and the missing ones stay hidden.

### Capture

- Fullscreen screenshots.
- Display screenshots.
- Window screenshots.
- Region selection.
- Multi-monitor support.
- Global hotkeys.

### Editor

- Crop.
- Arrow.
- Rectangle.
- Ellipse.
- Text.
- Blur.
- Pixelate.
- Undo and redo.

### Sharing

- Copy to clipboard.
- Save locally.
- Generic HTTP uploader.
- Automatic URL copying.
- Upload history.

### History

- Capture history.
- Thumbnails.
- Metadata search by file name, date, capture source, and upload URL.
- Metadata.
- Delete captures.
- Open captures.

### Desktop integration

- System tray.
- Quick-access overlay.
- Global hotkeys.
- Native dialogs.
- Notifications.

### Settings

- Theme.
- Hotkeys.
- Capture defaults.
- Storage location.
- Upload configuration, with credentials in the OS secret store.

---

## 12. Development Roadmap

The following timeline is a rough estimate, not a production deadline. Phase 0 is unscheduled; platform, correctness and performance evidence determine the gates. The weeks below start only after Phase 0 has passed. A functional Windows demo does not establish cross-platform readiness or the resource budgets.

If the spike shows that a platform needs its own capture backend, Phase 2 takes as long as that backend takes. Do not drop DPI handling, permissions, or Wayland behavior to protect the two-week estimate.

### Phase 0 – Technical Spike

**Goal: Validate the riskiest technical decisions before committing to the architecture.**

The prototype may be thrown away. Phase 1 starts the application workspace only after the gate below passes. If the chosen GPUI pin is accepted, Phase 1 may keep that crate setup.

Tasks:

- Pin a GPUI source and record the exact revision.
- Build a minimal GPUI window on Windows, Linux X11, Linux Wayland, and macOS.
- Test transparent, frameless, and always-on-top windows on the pinned source.
- Test placing that window on a non-primary monitor, including a negative origin and a scale factor other than 1.
- Evaluate Scap where needed to resolve stream/PipeWire questions; a redundant backend is not a gate requirement when an accepted still-image path already meets the target.
- Evaluate XCap for a still image on Windows, macOS, and X11, and measure its Wayland coordinates and window list.
- Evaluate an XDG Desktop Portal still capture, including interactive selection and user cancellation.
- Decide the region-selection path per session type: custom overlay, portal UI, or editor crop.
- Put a PNG on the clipboard and paste it back on each session type.
- Register a global hotkey on Windows, macOS, and X11, and record the Wayland result through the GlobalShortcuts portal.
- Show a system tray icon and a menu on each platform.
- On macOS, repeat the capture check from a stable signed bundle, not only from `cargo run`.
- Measure release-mode idle, startup, UI/input latency, end-to-end capture and CPU/GPU memory against the performance contract on modest reference hardware. Test the idle cost of the pinned GPUI renderer before building on it.
- Compare an in-memory still-image path against the spike's disk/PNG handoff and count allocations/copies. Prove bounded ownership and cancellation; record any capability or budget failure.

**Exit gate:** The notes must name and demonstrate a capture approach and region UI on each declared target/session, distinguish unavailable modes, and include measured performance results or an explicitly unresolved failure. A candidate name or documented failure is not a passing result. Measure the implemented slices (empty resident shell, reference capture and overlay), not future history/editor features. Replace prototype event polling before accepting its idle results. The UI/renderer must meet the idle/input budgets and the capture design must demonstrate a feasible bounded in-memory path; functional correctness alone does not pass the gate. If GPUI cannot support the required overlays or performance, reconsider it before Phase 1. If the only gap is Wayland layer-shell, use validated portal selection/editor crop. Changing supported release targets or relaxing budgets requires an explicit product decision, not an unchecked box silently removed.

**Deliverable:** A short spike note with the pinned revisions, what worked, what failed, and the capture decision.

### Phase 1 – Foundation

Implementation progress (Windows-first, not a completed cross-platform phase gate): the usable capture home now groups region/display/window actions, last-output controls and optional diagnostics. Native folder selection persists the capture destination with worker-side bounded settings IO. Remaining phase/release gates stay open; feature work proceeds without repeating exploratory performance runs for every UI change.

UI direction: the current capture home is a functional placeholder. Its layout, typography, visual hierarchy and component styling are not the final design; a deliberate, polished UI pass remains required. Functional progress should continue without treating this temporary appearance as a design commitment. One-off PNG export now uses a native Save As dialog, preserving the default capture folder and existing files.



**Estimated duration: Weeks 1–2**

Tasks:

- Create the Cargo workspace.
- Set up the desktop application.
- Integrate GPUI.
- Build the main window.
- Implement basic navigation for the sections that exist.
- Establish a small design system: color, type, and spacing. Leave the editor's visual design until Phase 4.
- Create the application core and the foreground, worker, and Tokio split.
- Replace spike polling with event-driven hotkey/tray/IPC delivery and lazy service startup. Implement bounded queues, session IDs, cancellation and a repeatable performance harness now.
- Implement bounded asynchronous logging in the application-data directory; rotate/cap files, redact secrets and avoid logging paths/window titles/content by default. Crash reports/full-frame diagnostics require explicit opt-in. Error logging must not become a render-path lock or idle disk writer.
- Add settings infrastructure without a secret store yet.
- Set up CI for all three platforms. macOS hosted runners are a cost; record that choice instead of assuming they are free.
- Record shell idle/private-memory and cold/warm startup baselines; fail this phase if the framework alone exceeds the contract without an agreed replacement/rework.

**Deliverable:** A functioning cross-platform application shell.

### Phase 2 – Capture Engine

**Estimated duration: Weeks 3–4, longer when a platform needs a native backend**

Tasks:

- Implement the capture interface, including capability flags and a fake backend for tests.
- Add display enumeration where the platform provides it.
- Add window enumeration where the platform provides it.
- Implement fullscreen and display capture.
- Implement window capture where the capability exists.
- Implement region selection with the path chosen in Phase 0. Hide the application's own UI before a custom-overlay capture.
- Add multi-monitor support, including negative origins and mixed scale factors.
- Handle DPI and coordinate conversion with tests that do not need a display.
- Preserve source metadata, implement the declared SDR sRGB conversion/export policy, and explicitly reject unsupported HDR/wide-gamut paths.
- Implement permission handling, including denial and portal cancellation.
- Test X11 and Wayland as separate sessions.
- Replace file-based frame handoffs with the bounded in-memory ownership model. Test valid all-black images, malformed/oversized frames, late callbacks, device loss and monitor hotplug during selection.
- Meet overlay/clipboard latency and active-memory budgets; do not encode a canceled preview to disk or retain peak-resolution textures in idle overlay caches.

**Deliverable:** A working screenshot engine whose unsupported modes are explicit.

### Phase 3 – Desktop Integration

**Estimated duration: Week 5**

Tasks:

- Single-instance lifecycle, tray ownership, and hide-on-close.
- Global hotkeys, with an explained unavailable state on Wayland when the portal cannot bind them.
- System tray.
- Native clipboard representations from the in-memory frame, including PNG where supported. Run RGBA/PNG round-trip checks in tests rather than reading/comparing the clipboard after every production copy.
- Save the PNG into the configured folder. This path does not need SQLite yet.
- Quick-access overlay on the monitor that was captured.
- Native file dialogs.
- Notifications.
- Multi-window behavior.
- Validate hotkey conflict handling, hide/restore, no-tray fallback, IPC/single-instance races, sleep/resume and clean shutdown. Quitting must release registrations and not lose an already acknowledged save.
- Verify end-to-end latency and return-to-tray memory after repeated use; no polling loops or background capture streams may remain.

**Deliverable:** A screenshot workflow that feels like a native desktop application.

### Phase 4 – Image Editor

Initial implemented editing slice: last-capture opaque redaction in a separate basic editor, up to 100 rectangles and 100 bounded metadata-only undo/redo states including crop, Undo/Redo/Reset and scoped keyboard shortcuts, fit-to-window preview and worker-side flattened PNG/clipboard export. The editor can also open selected history entries, including imported PNGs, independently of the last capture. This explicit Edit a copy workflow keeps existing original files/history entries and says so in the editor. It does not claim original-file sanitization or deletion. Closing removes the editor image atlas entry and releases its document; production driver-memory/physical-input checks remain open. Editor crop now outlines the export area, shades excluded pixels and reports output dimensions; Full image restores the uncropped selection. Crop and redaction are rendered into one output buffer on the worker, without intermediate full-image snapshots. Arrows, shapes, text, blur/pixelate, zoom and final visual design are still pending.


**Estimated duration: Weeks 6–7**

Tasks:

- Build the editor interface.
- Implement the annotation model.
- Add crop.
- Add arrows.
- Add rectangles and ellipses.
- Add text.
- Add blur and pixelate.
- Implement undo and redo.
- Implement image export.
- Add opaque redaction and consistent text shaping/export. Test layer ordering, preview/export parity, sanitized clipboard/upload/history paths, and bounded undo memory.
- Meet drag/frame-time budgets while background export/filter work is running; release document buffers/textures on close.

**Deliverable:** A functional screenshot editor.

### Phase 5 – History and Sharing

Implemented initial slice: new successful PNG saves and Save As exports enter a migrated local SQLite/WAL metadata store. Capture history loads lazily in bounded pages (20 visible rows plus one lookahead), with Open/Show file, bounded on-demand Copy, and metadata-only Remove entry. Listing performs no image decoding or eager startup query. Removing an entry keeps its image file; image deletion and retention are separate pending workflows. Metadata writes follow completed file publication and report failures independently. Search, thumbnails, missing-file reconciliation, deletion/retention, crash recovery and the full phase gate remain open. Source classification currently uses capture for app outputs and import for manually added PNGs. Manual PNG import is limited to 100 explicit selections, validates bounded image metadata, skips duplicate exact paths, reports file failures and commits references without modifying original files.


**Estimated duration: Weeks 8–9**

Tasks:

- Integrate SQLite and rusqlite on the database worker.
- Implement database migrations.
- Move the Phase 3 save path onto the shared filesystem API if that path was temporary.
- Add thumbnails, generated off the foreground executor.
- Build capture history.
- Add metadata search.
- Implement generic HTTP uploads.
- Add URL extraction and automatic copying.
- Add upload history.
- Add paged/indexed history with a virtualized viewport, bounded thumbnail decoding/cache, and tests with at least 10,000 records.
- Test interrupted saves, migrations, partial delete recovery, disk-full, unavailable storage and retention. Test upload timeout/cancel, redirects, response limits and safe retry behavior.
- Meet idle and input budgets while history/search/uploads are active; no eager history load or network-service initialization on resident startup. A lazily initialized runtime may remain parked only if its retained threads/memory meet the idle budgets.

**Deliverable:** A complete local capture and sharing workflow.

### Phase 6 – Stabilization and Release

**Estimated duration: Weeks 10–12**

Tasks:

- Verify the accumulated performance budgets and remedy remaining regressions; the architecture and measurements have been required in every preceding phase.
- Run the memory/cancellation soak and resource-admission tests, not just a fast-path demo.
- Multi-monitor testing.
- DPI testing.
- Wayland testing.
- macOS permission testing on a signed and notarized build. Screen Recording permission is part of that package, not a last-minute check on an unsigned binary.
- Error handling improvements.
- UI polish.
- Installer creation.
- Windows code signing, or an explicit decision to ship an unsigned first release and document the SmartScreen warning.
- Linux packaging as a normal binary and a `.desktop` file, or distro packages. A Flatpak is a separate decision because the sandbox changes portals, the filesystem, and the secret store. Do not add it in the same step as the first native build.
- Documentation.
- Release preparation.

**Deliverable:** A usable first public release.

### Production verification and release engineering

- Pin the Rust toolchain and release dependency resolution. The user's current development policy leaves Cargo.lock unversioned; keep that policy unchanged in this planning edit, but do not call floating dependency builds reproducible. Prefer versioning the application lockfile before release, or archive the exact resolved lockfile with immutable release inputs, restore it in every release build, and use `--locked`. Exact GPUI/XCap pins alone do not freeze transitive dependencies. Document repeatable dependency/toolchain builds separately from any unproven bit-for-bit build claim.
- CI runs formatting, linting, deterministic unit/component tests, migration/recovery tests, dependency vulnerability/license checks and builds for each declared target. Isolate hardware tests as explicit jobs; skipped hardware tests must not count as platform validation. Package/install/upgrade/uninstall smoke tests use the actual artifacts, with published checksums, version and dependency/license notices. Audit the real dependency graph, including GPUI packaging, not just top-level crate names.
- Treat imported images, clipboard images, portal-returned files, IPC messages and uploader responses as untrusted boundaries. Enforce dimension/decoded-byte/stride/path/response limits before allocation or use. Fuzz coordinate/size parsing and uploader response extraction; maintain deterministic regression fixtures. No shell execution from user-provided filenames, URLs or upload response text.
- Test cancellation and failures at every asynchronous boundary with fake backends and controlled faults: permission revocation, hung/failed capture, renderer/device loss, stale callbacks, full disk, database busy/corruption and shutdown while exporting/uploading. Keep UI available and ownership bounded. Recovery must preserve user-owned files and explain what was or was not saved.
- Test packaged apps as standard users. Document supported OS/architecture and Linux session/portal versions, signed macOS permissions, Windows signing/SmartScreen status, accessibility and startup/tray fallback. An update mechanism is out of MVP unless explicitly scoped; any later auto-updater must authenticate artifacts, handle failed installs/rollback and meet the same idle/privacy contract.
- Final release evidence includes the platform capability matrix, hardware/performance reports, storage/migration recovery results, security/privacy review and known limitations. A document or green compile alone does not establish production readiness.

---

## 13. Future Development

After the MVP, gradually expand the application. Every future feature inherits the performance contract, bounded-resource/cancellation rules and production gates. Add an explicit active-work budget for recording, OCR, plugins or synchronization before implementation; no feature may add a permanent idle cost by default.

### V1.1 – Recording

- Screen recording.
- GIF recording.
- Audio recording.
- An FFmpeg integration only after the license choice in the stack constraints is written down.
- Advanced capture options.
- Additional editor tools.

### V1.2 – Advanced Capture and Integrations

- OCR.
- Scrolling screenshots.
- QR code recognition.
- S3-compatible storage.
- Cloudflare R2.
- Additional upload providers.

### V2 – Automation and Extensibility

- Workflow engine.
- Custom actions.
- Plugin system.
- CLI.
- Scripting.
- Custom sharing server.

### V3 – Optional Cloud Ecosystem

- Optional user accounts.
- Synchronization.
- Team sharing.
- Self-hosted infrastructure.
- Public API.
- Community plugin ecosystem.

The local-first application must remain fully usable without any of these services.

---

## 14. Definition of Done

The MVP is ready for its first public release when the following requirements are met:

### Cross-platform

- [ ] Windows release builds use the documented frozen dependency/toolchain inputs.
- [ ] Linux release builds use the documented frozen dependency/toolchain inputs.
- [ ] macOS release builds use the documented frozen dependency/toolchain inputs.
- [ ] Linux X11 tested independently.
- [ ] Linux Wayland tested independently.
- [ ] Platform-specific permissions handled correctly.

### Capture

- [ ] Fullscreen capture works.
- [ ] Display capture works.
- [ ] Window capture works where the platform can list and capture windows.
- [ ] Where window capture is unavailable, the action is absent.
- [ ] Region capture uses the custom overlay where that overlay can cover the captured display.
- [ ] Wayland sessions that cannot host that overlay use portal selection or editor crop.
- [ ] Denied permission/canceled requests produce typed outcomes and no fabricated image, history entry or clipboard change; a legitimate all-black image is accepted.
- [ ] The application's own overlay and windows are not part of the captured image.
- [ ] Multi-monitor support works, including a monitor with a negative origin.
- [ ] DPI and coordinate handling are correct on mixed scale factors.

### Editing

- [ ] Crop works.
- [ ] Arrows work.
- [ ] Shapes work.
- [ ] Text works.
- [ ] Blur and pixelate work.
- [ ] Undo and redo work.
- [ ] Export and preview agree on text, alpha, layer order and crop/effect coordinates.
- [ ] Sanitized copy/save/upload/history do not leak the unredacted source; original retention is explicit.

### Sharing and storage

- [ ] Copying a capture places a PNG on the clipboard, and paste works on each supported session.
- [ ] Local saving works.
- [ ] Generic HTTP upload works.
- [ ] Uploaded URLs can be copied automatically.
- [ ] Capture history survives application restarts.
- [ ] Database migrations work.
- [ ] Interrupted save/delete/migration, disk-full and unavailable storage recovery are tested; success is acknowledged only for completed stages.
- [ ] Upload timeouts, cancellation, response limits, redirect policy and credential handling pass tests.

### Desktop integration

- [ ] Global hotkeys work where the platform supports them.
- [ ] Where a global hotkey cannot be registered, the settings screen says so.
- [ ] Tray works where available; other sessions retain an accessible reopen/quit path.
- [ ] Quick-access overlay works.
- [ ] Native dialogs work.
- [ ] Notifications work.
- [ ] Single-instance forwarding, hotkey conflicts, hide/restore, sleep/resume and shutdown are exercised.

### Quality

- [ ] No blocking enumeration/capture/database/filesystem/image decode/encode/filter/network work runs on the foreground executor; required native UI-thread callbacks stay short and reentrancy-safe.
- [ ] Coordinate conversion and annotation export are tested without a physical display.
- [ ] No unnecessary background infrastructure.
- [ ] Errors are handled gracefully.
- [ ] Upload credentials are not stored in the TOML file and do not appear in logs.
- [ ] The performance contract passes on documented reference hardware/session types; results include idle CPU/wakeups, private/resident/GPU memory, input/presentation, startup and end-to-end capture latency.
- [ ] No idle polling, hidden redraw loop, production clipboard read-back verification or PNG/disk round trip in the hot path remains.
- [ ] Queues, caches, decoded images, undo and concurrency are bounded; the 500-cycle test returns to the measured idle budget.
- [ ] Canceled sessions create no persisted desktop previews and stale results cannot mutate later sessions.
- [ ] Untrusted input limits, dependency/license checks, recovery tests and packaged install/upgrade/uninstall checks pass.
- [ ] Installers or documented packages are available.
- [ ] macOS builds that request Screen Recording are signed and notarized.
- [ ] Basic documentation is complete, including the Wayland limitations.

---

## 15. Important Architectural Decisions

These decisions should guide development throughout the project.

| Decision | Direction |
|---|---|
| Main language | Rust |
| UI framework | Pinned, reviewed GPUI source/snapshot; packaging provenance and platform patches recorded |
| Performance | Mandatory idle/latency/memory budgets, measured from Phase 0 and inherited by every future feature |
| UI runtime | GPUI foreground executor |
| Network I/O | Tokio, off the UI executor |
| Application architecture | Modular Cargo workspace |
| Database | SQLite |
| Database access | rusqlite |
| Migrations | rusqlite_migration |
| Image storage | Filesystem, PNG, physical pixel size |
| Hot path | In-memory frames with bounded ownership; encoding/storage only for required outputs |
| Scheduling | Native events, invalidation-driven rendering, bounded queues and cancelable jobs |
| Secrets | OS secret store, never the TOML file |
| Capture architecture | Common API with native backends and capability flags |
| Region selection | Custom overlay, portal UI, or editor crop, per session |
| Linux support | X11 and Wayland tested as separate sessions |
| Wayland hotkeys | GlobalShortcuts portal where it exists; explained fallback otherwise |
| Upload architecture | Provider abstraction, declarative HTTP uploader first |
| Editing | Non-destructive annotations; crop is a document change |
| Execution | Single-instance native desktop application |
| Cloud dependency | None required |
| FFmpeg | After the MVP, and only after a license decision |
| Plugin system | Later, not in MVP |

### Avoid premature complexity

Do not introduce:

- Microservices.
- Electron or a WebView-based UI.
- An unnecessarily complex event bus.
- An ORM-heavy database architecture.
- A large plugin runtime in the MVP, including a scripting language inside the HTTP uploader.
- A GPUI fork adopted only because it bundles tray, hotkeys, or overlay windows.
- Unnecessary C/C++ dependencies.
- Multiple implementations of the same functionality without a technical reason.

Rust should remain the primary implementation language.

C/C++ may be used selectively for native integrations when there is a demonstrable technical advantage, but the entire application should not become a mixed-language project without a clear reason.

---

## 16. First Development Steps

The most important thing is to validate the technical foundation before implementing the complete application.

Recommended starting order:

1. Keep this repository. The plan, license, and README are already here.
2. Run Phase 0 in a small prototype, with a pinned GPUI revision.
3. Build and run that prototype on Windows, Linux X11, Linux Wayland, and macOS.
4. Test transparent, frameless, always-on-top, and multi-monitor window placement before writing application screens.
5. Validate XCap/portal still-capture paths; evaluate Scap only for unresolved stream/backend requirements.
6. Implement a bounded in-memory screenshot path and export its PNG off-thread.
7. Validate clipboard, hotkeys, and the system tray, and write down where each one is unavailable.
8. Measure release-mode idle/startup/input/capture/memory behavior and pass or fail the Phase 0 gate with evidence.
9. Only then create the application workspace and the shell described in Phase 1.

**Do not start with the full UI or the editor.**

The capture engine and desktop integration are the biggest technical risks. Resolve those first.

---

## Final Direction

The goal is not to build another bloated screenshot utility.

The goal is to build a fast, modern, reliable, and extensible desktop capture platform that combines the flexibility of ShareX with a significantly better user experience.

**Rust + GPUI + native platform integrations + SQLite/rusqlite** is the chosen starting direction.

Keep the architecture clean, the implementation pragmatic, and the application useful even without cloud services or an account.

## Technical references for the production review

These sources inform the contracts above; benchmark budgets are project requirements, not numbers taken from these documents.

- [Cargo: manifests versus lockfiles](https://doc.rust-lang.org/cargo/guide/cargo-toml-vs-cargo-lock.html) — freezing resolved dependencies for release inputs.
- [XDG Screenshot portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Screenshot.html) — versioned target capabilities and the older interactive hint.
- [Upstream GPUI layer-shell example](https://github.com/zed-industries/zed/blob/main/crates/gpui/examples/layer_shell.rs) — confirms that a blanket claim of no GPUI layer-shell support is obsolete; the local 1.27.0 package also includes the example/API. Runtime validation remains necessary. Do not copy the example's timer-driven clock redraw into an idle capture application.
- [SQLite WAL](https://www.sqlite.org/wal.html) and [atomic commit](https://www.sqlite.org/atomiccommit.html) — local-filesystem constraints and database durability; not a transaction across external PNG files.
- [tiny-skia API](https://docs.rs/tiny-skia/0.12.0/tiny_skia/) — low-level rasterization; text layout must be designed separately.
- [FFmpeg license and legal considerations](https://ffmpeg.org/legal.html) — obligations depend on the build and distribution.
