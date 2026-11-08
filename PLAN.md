# Project Plan – Open-Source Capture & Sharing Application

> **Simple for beginners. Powerful for professionals.**

## 1. Project Vision

Build a modern, open-source desktop application for screenshots, screen recording, image editing, and sharing.

The goal is to create a serious alternative to ShareX with a significantly more modern user experience, a clean architecture, and long-term extensibility.

Rather than simply copying ShareX's feature set, the application should make powerful functionality accessible without overwhelming new users.

### Core principles

- **Balanced UX:** Simple for beginners, powerful for professionals.
- **Cross-platform:** Windows, Linux, and macOS from day one.
- **Performance:** Native desktop performance with low resource consumption.
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

- Pin an exact GPUI source in Phase 0, either a Zed revision or a named crates.io snapshot of upstream GPUI. Upstream GPUI is pre-1.0. Forks that bundle a system tray, global hotkeys, or overlay windows are different projects and are not adopted just because they advertise those features.
- System tray, global hotkeys, clipboard, notifications, and file dialogs are platform crates. They are not GPUI features.
- GPUI's foreground executor is the UI runtime. Tokio is not. See the execution model below.
- Still images go through the `image` crate. tiny-skia draws vector annotations only. Blur and pixelate are pixel operations, applied when the image is exported.
- FFmpeg stays out of the MVP. Before a later version bundles it, check the license. A GPL FFmpeg build cannot be statically linked into this MIT application. Prefer a separate FFmpeg process, or a build whose license fits, and record that decision before writing the integration.

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
- Add schema migrations from the beginning.
- Keep database access off the GPUI foreground executor. The render path is part of that executor.
- Execute blocking operations through a dedicated database worker or controlled blocking tasks.
- Start with a single database connection; introduce additional connections only when justified by actual workloads.

Suggested migration dependency: `rusqlite_migration`.

---

## 3. Architecture

Use a modular Rust workspace with clear boundaries between the UI, application logic, platform integrations, and infrastructure.

### Architectural principles

1. GPUI must not contain application business logic.
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

This is the intended long-term structure, not a requirement to create every crate immediately.

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

GPUI runs the interface on its own foreground executor. That executor is not a Tokio runtime. Do not start the process with `#[tokio::main]`, and do not call Tokio from the foreground executor.

Use three places for work:

- **Foreground:** GPUI state, rendering, and short input handling. No capture, database, filesystem, image encoding, or HTTP here.
- **Blocking workers:** SQLite, image encode and decode, thumbnail generation, and native capture calls that are synchronous. One database connection on one worker is enough to start.
- **Tokio runtime:** HTTP uploads and similar network I/O, owned by the infrastructure layer. Completed work is delivered back to the foreground executor.

Native capture APIs that must run on a specific thread stay on that thread. The core talks to them with a message. It does not pretend that every platform API is async.

### Process lifecycle

A capture tool should run as a single instance. A second launch forwards its request to the running instance and exits. Two instances would register the same hotkeys and write the same history.

The running instance owns the tray icon, the hotkeys, and the database worker. Closing the main window hides the window. Quitting is explicit.

---

## 4. Screenshot Capture Engine

The capture engine is one of the most important and technically challenging parts of the application.

It should be designed around a common API with platform-specific implementations.

### Required capture functionality

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

### Initial implementation candidate: Scap

Repository: https://github.com/CapSoftware/scap

Evaluate Scap first because it provides a potential cross-platform abstraction for screen capture.

It advertises support for:

- Windows Graphics Capture.
- macOS ScreenCaptureKit.
- Linux PipeWire.

However, do not assume that every feature works identically across all platforms.

Scap is built around capture streams. A stream API is only a good fit for a still screenshot if the pinned revision can also return one frame cleanly. On Linux, a README line that says PipeWire is not evidence. Confirm that the revision builds against current PipeWire and returns a frame on each compositor you intend to support.

Test its actual capabilities before deciding whether to build additional native backends.

### Alternative: XCap

Repository: https://github.com/nashaofu/xcap

Evaluate XCap as an alternative or complementary implementation for still images on Windows, macOS, and X11.

On Wayland, measure it before trusting it. Expect incomplete window lists, limited screen capture, and monitor positions that cannot be used to crop a region. A reported origin of `(0, 0)` for every output is a known class of failure. Do not build region selection on those coordinates.

### Wayland still capture

Also evaluate the XDG Desktop Portal directly, through a Rust portal client such as `ashpd`.

The portal is the supported still-image path on GNOME, and it works on other compositors that ship a portal backend. It can be the Wayland backend even when Scap or XCap is used on Windows, macOS, and X11.

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

`capabilities` is part of the contract. A method the platform cannot perform returns a typed unsupported error, and the UI does not offer that action. On many Wayland sessions, window listing and window capture are unavailable. Display capture may also be available only through a portal flow.

The example is synchronous on purpose. Windows Graphics Capture, ScreenCaptureKit, and X11 grabs are synchronous or callback-based, and several of those types are `Send` without being `Sync`. Do not require `Sync`, and do not wrap them in an async trait just to look uniform.

Portal calls are the exception: they wait on D-Bus. Await that wait in the infrastructure runtime, or run it on the blocking worker. Do not await it on the GPUI foreground executor.

The final type can be a trait or an enum. Prefer an enum if a trait object cannot be made object-safe without hiding platform types. The application core depends on the common result types either way.

### Region selection

Region selection is not one portable workflow. Use the path the platform actually allows.

**Custom overlay.** Windows, macOS, X11, and Wayland compositors that support layer-shell. The pinned UI toolkit must be able to create that surface. A transparent toplevel is enough on Windows, macOS, and X11. On Wayland it is not. Upstream GPUI does not speak layer-shell. If the spike cannot show a layer-shell surface, Wayland uses portal selection even on compositors that support the protocol.

1. Hide this application's own windows, or exclude them from the capture, so the overlay is not part of the image.
2. Capture the selected display when the platform provides a frame and a usable coordinate space.
3. Show a transparent, frameless, always-on-top selection surface on that display.
4. Convert the selection from logical coordinates into physical pixels.
5. Crop the captured frame.
6. Return the image.

**Portal selection.** Wayland sessions where a custom overlay cannot cover the desktop. GNOME is in this group, because Mutter does not implement wlr-layer-shell. An XWayland fullscreen window is not a substitute: it is not a desktop overlay, and it does not reliably receive the keyboard when another application is focused.

1. Ask the XDG Desktop Portal for an interactive screenshot, or for a source the user picks.
2. Treat cancellation as a normal outcome.
3. Use the image the portal returns. Do not invent a global monitor origin the compositor did not provide.

**Editor crop.** If neither overlay nor interactive portal is available, capture a display and let the user crop in the editor.

A normal toplevel window is not an overlay. On Wayland, covering the screen requires layer-shell or the portal's own UI. There is often no global coordinate space, so a capture-then-crop overlay must not be the default Wayland design.

### Still-image format

Store still captures as PNG. Width and height in the database are physical pixels.

Keep the pixel format explicit at the capture boundary: channel order, premultiplied alpha, and color space. Convert once into the editor's working image. macOS frames may be wide-gamut. Do not assume every source is sRGB, and do not drop the color space silently. The default export is PNG in the working color space. JPEG can be added later as an explicit export option.

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
- Portal cancellation and a denied or not-yet-granted permission. An empty or black frame is a failure to report, not a successful capture.
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
CaptureFailed

UploadStarted
UploadCompleted
UploadFailed

CaptureSaved
CaptureDeleted
```

The UI sends commands to the application core and receives state updates or events.

The application core handles the actual workflows and communicates with the required services.

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

Crop is not an annotation. It changes the document canvas and is stored as a crop rectangle so it can be undone. Arrow, rectangle, ellipse, and text are vector objects drawn with tiny-skia. Blur and pixelate are pixel filters applied to a region at export time. The original pixels stay available until the user exports.

Undo and redo record document changes. They do not snapshot the full bitmap on every edit.

### MVP editing tools

- Crop.
- Arrow.
- Rectangle.
- Ellipse.
- Text.
- Blur.
- Pixelate.
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

- Clean and modern interface.
- Minimal unnecessary UI elements.
- Clear visual hierarchy.
- Consistent design system.
- Keyboard-first workflows for power users.
- Discoverable functionality for beginners.
- Native platform behavior where appropriate.
- Dark and light themes.

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

The following timeline is a rough estimate for an experienced solo developer. Phase 0 is unscheduled on purpose. The weeks below start only after Phase 0 has passed.

If the spike shows that a platform needs its own capture backend, Phase 2 takes as long as that backend takes. Do not drop DPI handling, permissions, or Wayland behavior to protect the two-week estimate.

### Phase 0 – Technical Spike

**Goal: Validate the riskiest technical decisions before committing to the architecture.**

The prototype may be thrown away. Phase 1 starts the application workspace only after the gate below passes. If the chosen GPUI pin is accepted, Phase 1 may keep that crate setup.

Tasks:

- Pin a GPUI source and record the exact revision.
- Build a minimal GPUI window on Windows, Linux X11, Linux Wayland, and macOS.
- Test transparent, frameless, and always-on-top windows on the pinned source.
- Test placing that window on a non-primary monitor, including a negative origin and a scale factor other than 1.
- Evaluate Scap for a one-shot frame, including a current PipeWire on Linux.
- Evaluate XCap for a still image on Windows, macOS, and X11, and measure its Wayland coordinates and window list.
- Evaluate an XDG Desktop Portal still capture, including interactive selection and user cancellation.
- Decide the region-selection path per session type: custom overlay, portal UI, or editor crop.
- Put a PNG on the clipboard and paste it back on each session type.
- Register a global hotkey on Windows, macOS, and X11, and record the Wayland result through the GlobalShortcuts portal.
- Show a system tray icon and a menu on each platform.
- On macOS, repeat the capture check from a stable signed bundle, not only from `cargo run`.

**Exit gate:** Continue only if the notes name a capture approach for each target and a UI approach for the region overlay. If the pinned GPUI cannot host the overlay on Windows, macOS, or X11, switch the UI before Phase 1. If the only gap is Wayland layer-shell, Wayland region selection uses the portal. Do not leave that choice implicit.

**Deliverable:** A short spike note with the pinned revisions, what worked, what failed, and the capture decision.

### Phase 1 – Foundation

**Estimated duration: Weeks 1–2**

Tasks:

- Create the Cargo workspace.
- Set up the desktop application.
- Integrate GPUI.
- Build the main window.
- Implement basic navigation for the sections that exist.
- Establish a small design system: color, type, and spacing. Leave the editor's visual design until Phase 4.
- Create the application core and the foreground, worker, and Tokio split.
- Implement logging to a file in the application data directory, and error handling.
- Add settings infrastructure without a secret store yet.
- Set up CI for all three platforms. macOS hosted runners are a cost; record that choice instead of assuming they are free.

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
- Preserve pixel format and color space into the PNG.
- Implement permission handling, including denial and portal cancellation.
- Test X11 and Wayland as separate sessions.

**Deliverable:** A working screenshot engine whose unsupported modes are explicit.

### Phase 3 – Desktop Integration

**Estimated duration: Week 5**

Tasks:

- Single-instance lifecycle, tray ownership, and hide-on-close.
- Global hotkeys, with an explained unavailable state on Wayland when the portal cannot bind them.
- System tray.
- Clipboard integration for a PNG round trip.
- Save the PNG into the configured folder. This path does not need SQLite yet.
- Quick-access overlay on the monitor that was captured.
- Native file dialogs.
- Notifications.
- Multi-window behavior.

**Deliverable:** A screenshot workflow that feels like a native desktop application.

### Phase 4 – Image Editor

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

**Deliverable:** A functional screenshot editor.

### Phase 5 – History and Sharing

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

**Deliverable:** A complete local capture and sharing workflow.

### Phase 6 – Stabilization and Release

**Estimated duration: Weeks 10–12**

Tasks:

- Performance optimization.
- Memory usage optimization.
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

---

## 13. Future Development

After the MVP, gradually expand the application.

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

- [ ] Reproducible builds on Windows.
- [ ] Reproducible builds on Linux.
- [ ] Reproducible builds on macOS.
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
- [ ] A denied permission or a cancelled portal request is shown as such, and does not save a black image.
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

### Sharing and storage

- [ ] Copying a capture places a PNG on the clipboard, and paste works on each supported session.
- [ ] Local saving works.
- [ ] Generic HTTP upload works.
- [ ] Uploaded URLs can be copied automatically.
- [ ] Capture history survives application restarts.
- [ ] Database migrations work.

### Desktop integration

- [ ] Global hotkeys work where the platform supports them.
- [ ] Where a global hotkey cannot be registered, the settings screen says so.
- [ ] System tray works.
- [ ] Quick-access overlay works.
- [ ] Native dialogs work.
- [ ] Notifications work.

### Quality

- [ ] No capture, database, filesystem, or image-encoding work runs on the GPUI foreground executor.
- [ ] Coordinate conversion and annotation export are tested without a physical display.
- [ ] No unnecessary background infrastructure.
- [ ] Errors are handled gracefully.
- [ ] Upload credentials are not stored in the TOML file and do not appear in logs.
- [ ] Resource consumption is reasonable.
- [ ] Installers or documented packages are available.
- [ ] macOS builds that request Screen Recording are signed and notarized.
- [ ] Basic documentation is complete, including the Wayland limitations.

---

## 15. Important Architectural Decisions

These decisions should guide development throughout the project.

| Decision | Direction |
|---|---|
| Main language | Rust |
| UI framework | Pinned upstream GPUI, not a feature fork by default |
| UI runtime | GPUI foreground executor |
| Network I/O | Tokio, off the UI executor |
| Application architecture | Modular Cargo workspace |
| Database | SQLite |
| Database access | rusqlite |
| Migrations | rusqlite_migration |
| Image storage | Filesystem, PNG, physical pixel size |
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
5. Evaluate Scap, XCap, and a portal still-capture on the session types above.
6. Implement one minimal screenshot path that returns a PNG.
7. Validate clipboard, hotkeys, and the system tray, and write down where each one is unavailable.
8. Pass or fail the Phase 0 exit gate.
9. Only then create the application workspace and the shell described in Phase 1.

**Do not start with the full UI or the editor.**

The capture engine and desktop integration are the biggest technical risks. Resolve those first.

---

## Final Direction

The goal is not to build another bloated screenshot utility.

The goal is to build a fast, modern, reliable, and extensible desktop capture platform that combines the flexibility of ShareX with a significantly better user experience.

**Rust + GPUI + native platform integrations + SQLite/rusqlite** is the chosen starting direction.

Keep the architecture clean, the implementation pragmatic, and the application useful even without cloud services or an account.
