# Capture selection and native visibility lifecycle

Integrated optimized topology/controller tests, the cached-display regression
and the owned X11 server-visibility check pass; exact evidence is recorded below.
Physical presentation, real-session and performance gates remain open.

## Selection ownership

The controller retains one GPUI display-event subscription. Added/removed outputs,
changed bounds and unknown change flags advance a topology generation, cancel an
owned desktop/crop token, invalidate the remembered region and defer preview
retirement until native dispatch releases its window borrow. A refresh-rate-only
notification preserves selection. Admission stays owned until the canceled worker
actually retires. File loads, existing frozen image editors and already completed
output are not canceled by these display events.

Each overlay retains its session/topology pair and its original cached native display identity,
window origin/extent, viewport extent and scale. Bounds/display callbacks and
selection release compare this metadata before mapping logical drag coordinates
onto the immutable source. Incompatible geometry immediately clears selection,
pixels and the render image; deferred owner callbacks must still own the same
session/topology pair. A topology event between release and deferred crop dispatch
cannot publish the old selection. Crop completion checks its cancellation token
again before output. Obsolete Windows overlay shells are retired instead of
reusing old output bounds.

Selection geometry reads GPUI's cached `Window::display_id` directly. It does not
call `Window::display(&App)`, which enumerates the platform display inventory in
this pinned toolkit. Native display callbacks update the cached identity before
observers run; viewport and scale are also cached, and the required short window
bounds read stays thread-correct. The accessor adds no inventory, cache or event
subscription of its own.

When annotation preparation is canceled or fails after the overlay closes, the
selection completion restores an existing control window and creates one when
absent. The existing session check precedes cancellation validation and all
restoration; Quit and Show-drain behavior remains in the existing lifecycle path.
A test-only hook injects topology invalidation after a real synthetic crop/render
returns, so the regression follows drag release through rejected delivery and the
native test-window visibility event rather than setting terminal state directly.

## Visibility evidence, by platform

- **Windows:** the existing native hidden-window and desktop-compositor wait stays
  in place. This slice does not upgrade its presentation claim.
- **X11:** a metadata-only request runs through the existing isolated native
  worker. It verifies the control's `_NET_WM_PID` against the authenticated parent,
  subscribes to native structure events, asks the WM to minimize, and waits for
  `UnmapNotify`. It verifies `UNMAPPED` and ownership again before returning a typed
  acknowledgment. The event deadline is two seconds and the owned controller
  deadline is three seconds. Missing WM acknowledgment, identity mismatch, destroy,
  disconnect or timeout returns an error and no pixels are captured. No permanent
  timer, server query polling, screenshot/PNG handoff or foreground provider call
  is added. An already unmapped owned window is also a valid server-state proof.
  `UnmapNotify` plus the state round trip proves X-server unmapping; it does **not**
  prove that a compositing WM has finished an animation or physically presented a
  new output frame. The current X11 root capture remains available; compositing-WM
  pixel correctness and animation/frame presentation remain explicit native gates.
- **macOS:** the live AppKit control receives `orderOut:` on the main thread and
  checks `isVisible`, avoiding miniaturization animation and a guessed delay. The
  isolated capture helper scopes the authenticated originating PID across source
  creation. Every ScreenCaptureKit display filter excludes the originating and
  helper applications and takes a fresh shareable source. Source dimensions,
  coordinates, topology and deadlines keep their existing checks. Legacy
  `CGDisplayCreateImage` remains available where SCK is absent, but refuses a shot
  while originating windows are still reported onscreen by WindowServer. AppKit
  visibility/WindowServer source state are not physical-presentation timing. A
  signed packaged SCK and legacy source/pixel fixture remains required.
- **Wayland:** this adds no custom overlay, invented global coordinates, X11 hide
  proof or universal frame-exclusion claim. `set_minimized` is a compositor request
  without a portable acknowledgment. The existing portal-owned source/image and
  frozen-image editor are the explicit fallback; the user/portal owns source
  selection. A portal-selected full display can include this application if the
  compositor keeps it visible. Direct output/global-region capture remains
  rejected by the existing Wayland authority and capability checks, including
  descriptor-only and XWayland contexts. Source selection is not evidence that
  this application has been excluded. Per-compositor capture UX and pixel exclusion
  remain acceptance gates; unsupported custom-overlay modes stay unavailable.

The unconditional 300 ms delay is removed from the three supported OS paths.
None of the native visibility acknowledgments above is labeled a universal
compositor-presented-frame fence.

## Resource cost

One controller display subscription and two small subscriptions plus fixed-size
geometry metadata per live overlay; no new image copy or cache, idle worker,
runtime, polling timer, background stream, network request or production language.
A visible X11 control adds one serialized metadata-only native helper before the
existing capture helper, with one operation-scoped X11 connection and bounded
native event wait. Capture admission remains one. macOS SCK retains at most two
excluded application references and checks a bounded 4096-item native application
inventory; legacy checks the existing bounded 1024-item WindowServer inventory in
the isolated helper. The existing `raw-window-handle` dependency is enabled for
Linux/macOS as well as Windows; no new package/version is added.

## Unrun checks

- Optimized cached-display accessor and native-display-notification toolkit tests,
  plus the actual annotation release/crop/render/topology-cancel/restore transition.
- Optimized core/UI/desktop tests for topology classifications, stale token and
  crop boundaries, geometry changes, typed visibility metadata and process/source
  exclusion. Cross-compile the macOS/Windows paths.
- Run `scripts/test-capture-hide.py --core-test-binary <optimized app-core tests>`
  against its authenticated private Xvfb. It creates only an owned 8x8 window and
  checks unmapped success, foreign/destroyed failure and missing-WM timeout. It
  captures no pixels and does not touch the user's display or clipboard.
- Add real-WM mapped-to-UnmapNotify success and canceled-helper cases; the private
  Xvfb fixture without a WM deliberately exercises refusal, not WM success.
- Exercise native hotplug, DPI/scale/rotation changes during capture/drag/release,
  no-change and refresh-only events, reused Windows shells, Show/Quit and late
  callbacks on each supported platform. Keep file-editor/frozen-preview and Unix
  resident retirement/reopen regressions in the run.
- Use owned opaque source windows to verify exclusion on compositing X11 WMs,
  signed macOS SCK/legacy and the supported Wayland portal sessions. Record source
  server state separately from actual physical/compositor presentation.
- Measure optimized native hide stages, end-to-end overlay/clipboard latency,
  memory and return to idle before/after this change. The extra X11 process may
  affect the warm-capture budget; no latency or memory acceptance is claimed.

## Integrated software checks — 2026-11-18

Optimized checks pass both pure topology/geometry cases, five connected topology UI cases and the resized-overlay regression, including actual synthetic crop/render followed by topology cancellation and restoration of the same hidden control. The 17 resident lifecycle cases remain green. Typed visibility metadata is validated independently of pixels. The authenticated private Xvfb hide gate passed its one exact opt-in case, including already-unmapped success, foreign/destroyed refusal and the missing-WM timeout. The executable SHA256 was `05077fee3f2746c8c14e4d94769c5088cc1ff876ae5bb3f64ad57d7d9d84d92d`. It proves X-server state only; mapped real-WM/compositor presentation and physical topology/performance gates above remain open. Workspace/all-target Linux and Windows GNU checks pass; the ARM macOS native SDK source check also passes. No hardware/resource budget is inferred.

The toolkit's exact cached-display getter regression also executed and passed once in its optimized package unit binary. Its initial standalone package build exposed a missing test-only DisplayId import, which was corrected; the production getter/callback behavior was unchanged. The first attempted feature flag was rejected because this path dependency lies outside the workspace; that invocation is not counted as a compiled or passing test.
