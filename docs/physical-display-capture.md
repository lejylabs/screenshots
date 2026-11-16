# Physical display capture and topology

Status: implementation in progress, 2026-11-16. This is backend work; no
selection controls or final UI design are added. Physical mixed-DPI hardware
acceptance remains separate from model, Wine and native CI checks.

The Windows GDI adapter must use physical desktop coordinates for enumeration,
display/region capture, cursor placement, remembered regions and desktop
composition. Scaling and rotation describe the already oriented raster; do not
multiply its origin/size by the scale or rotate its pixels a second time.

Create `crates/app-core/src/capture_native_windows_geometry.rs` and
`crates/app-core/src/display_pixels.rs`. Modify these existing paths:

- `crates/app-core/src/capture_native_windows.rs`, `capture_native.rs`,
  `capture.rs`, `lib.rs`, `desktop_composite.rs`, `capture_catalog.rs` and
  `native_capture_job.rs` for calibration, coordinate policy and isolation.
- `apps/desktop/src/capture_catalog.rs`, `region_analysis.rs`, `cli.rs` and
  `capture_worker_protocol.rs` for existing consumers and the typed Desktop call.
- `.github/workflows/rust.yml` and the existing Windows capture fixtures for
  explicit native CI checks; this document and `docs/desktop-composite.md`
  record evidence and limits.

Reuse the current isolated native
worker and existing decoded-source/output limits. No backend, dependency,
resident thread, timer, queue or cache is added.

For each Windows geometry observation, enter a thread-local per-monitor DPI scope,
resolve the XCap display's GDI device name through its physical mode center and
native monitor name, and compare `EnumDisplaySettingsW` physical mode coordinates with
`GetMonitorInfoW`'s physical rectangle. Obtain effective DPI through a temporary,
invisible per-monitor-aware window on that monitor (`GetDpiForWindow`), including custom
scales; do not trust XCap's process-awareness-dependent DPI fallback. Require
the independently observed mode/rectangle/raster to agree before publishing.
All windows are created and destroyed on the calling worker thread. Source
regions remain integer physical pixels relative to the oriented monitor.
Prefer PMv2 on Windows 10 1703+, falling back to PMv1 on 1607; neither path uses
system/unaware virtualization. Failure to enter either scope is an explicit
unsupported capture operation. This adapter does not establish earlier Windows
support or change the product's remaining OS acceptance matrix.

Use an operation-scoped invisible top-level window to witness native display,
device, relevant settings and power messages. Drain its native messages before
and after capture. Limit retrieved posted messages to 256 per check. Native
PeekMessage may also dispatch nonqueued sent messages, which are not bounded by
that counter: the whole native operation runs in the existing owned helper,
under its parent deadline/cancellation and process-family teardown. The helper
thread owns only short app witness/calibration window procedures; it runs no
GPUI or other user callbacks. No message pumping is added to the parent.
A change rejects the result even
if the observed geometry returns to the same values. On Windows 11 additionally
resolve public `GetCurrentMonitorTopologyId` dynamically from the already
loaded system User32 module; a changed generation also rejects the result.
Missing optional generation support retains the Windows 10 event witness.
One outer witness spans the entire native request, including template initial
enumeration/physical identity resolution, capture and final identity checks.
Desktop enumeration, all monitor acquisitions and assembly run in one typed
Desktop helper call under that witness, returning one final raw frame. The
public desktop entry requires the installed isolated executor; only the private
worker implementation performs inline composition. It still rejects mirrored/overlapping layouts,
missing or unverified frames and oversized/sparse bounds. It never publishes
partial output. Rendering changes within a stable topology are normal content
changes; sequential display acquisition is not an atomic cross-monitor frame.

The temporary geometry calibration and topology windows retain metadata only;
no image, texture or frame copy is added. The pure compositor retains at most
256 MiB source rasters plus one output of at most 256 MiB. It must check
cancellation per bounded row of copy work. Existing capture admission and
native process deadline contain blocking drivers. No application idle work is
introduced. Optimized measurements must distinguish native acceptance from
pure-model/copy timings and disclose any repeatable regression over 10%.

Verify independent physical-mode/rectangle mismatches, negative origins, custom
scales, all four rotations, exact crop/cursor coordinates, before/after drift,
bounded message storms, operation teardown and unchanged source pixels. Native
Windows CI can verify API/handle lifecycle against its actual desktop; it
cannot establish physical mixed-DPI/rotation, reconnect, sleep, device-loss or
presentation acceptance. Linux/macOS coordinate policies remain their existing
explicit policies until their own adapters establish physical coordinates.

Implementation checkpoints are independently buildable: (1) pure calibration
model and GNU Windows all-target metadata; (2) native calibration/witness and
typed Desktop helper round-trip, retaining existing platform guards; (3) switch
existing template/region/composition consumers to calibrated geometry, run
optimized regressions and isolated executable checks; (4) fresh source review,
native CI and evidence reconciliation before the feature commit.

The pure calibration seam takes independently supplied mode, PM-aware native
rectangle, backend raster geometry and effective window DPI; tests must vary
these independently. A private test-only witness constructor supplies a missing
or changing topology-generation resolver. Native tests post/send changes to
owned invisible windows without changing any real monitor, test message-storm
refusal and check destruction/class release. Phase fixtures retain the outer
witness while synthetic physical identity changes A/B/A between resolve/capture/
verify; source review checks that the real template has that exact lifetime.
Use an owned test child plus the existing parent timeout/cancel/process-family
fixtures for nonqueued-message flood containment. Queue bounds alone never
count as a hard native deadline. Record which checks actually ran separately.

Primary API contracts: [physical mode coordinates](https://learn.microsoft.com/en-us/windows/desktop/api/winuser/nf-winuser-enumdisplaysettingsw),
[window DPI](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getdpiforwindow),
[monitor topology generation](https://learn.microsoft.com/en-us/windows/win32/winmsg/winuser/nf-winuser-getcurrentmonitortopologyid).
These establish API behavior, not successful native app acceptance.
