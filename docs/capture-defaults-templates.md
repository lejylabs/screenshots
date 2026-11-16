# Capture defaults and monitor-bound region templates

Author: NewtTheWolf
Date: 2026-11-15
Status: defaults/templates implemented; focused checks and private X11 evidence
below. Broader capture work and native platform acceptance remain open.

The existing capture-task command already supports 0–3600000 millisecond
delays. Finite display sequences, one-shot future starts, 1–16 regions from
shared display snapshots, and pure desktop stitching also exist. Persisted
monitor binding and explicit still-cursor/window policies are implemented in
this slice. Durable repeating schedules remain an actual gap. The current native desktop compositor is Windows
only at 100% scale. Selection magnifier/snapping/keyboard controls are UI work
and remain outside this package. Eight-bit XCap output does not establish HDR
or wide-gamut correctness.

## Defaults and templates

Use one explicit versioned capture catalog, separate from UI settings. Existing
UI settings reconstruct their known fields on save, and their countdown remains
1–60 whole seconds. This catalog provides millisecond defaults without changing
those controls or losing values during unrelated UI saves. Read it only for an
explicit headless action or task source; no watcher, startup probe or resident
worker is added.

The catalog is one regular JSON object, at most 64 KiB, with strict fields and
duplicates rejected. It contains version 1, one bounded capture-options record
and at most 16 uniquely named region templates. Region names have 1–64 UTF-8
bytes without controls; native paths have at most 4096 encoded bytes. Relative
profile paths resolve against the catalog directory and are snapshotted to
absolute canonical regular-file paths. A template refers to a task profile;
the catalog never embeds or invokes a second task graph. Task execution and
general hotkeys continue through the existing Task runner/host, with no reverse
dependency from core into desktop code and no recursive catalog execution.

Monitor binding uses the native persistent identity rather than a session ID,
display ordinal, primary status or friendly name. Windows uses the active
DisplayConfig target device path; macOS uses the display UUID. Linux X11 uses
the RandR connector plus EDID digest. When EDID is absent, connector-only binding
requires an explicit choice and is reported as connector binding, never as
physical-monitor identity. Missing or duplicate matches fail. Store the admitted
pixel dimensions, scale and rotation with the rectangle; reject changes before
capture and again before publication. Desktop position and session ID may change
without rebinding the same monitor. A replacement monitor with a different
physical identity must not receive a saved region automatically.

Capture options retain the existing backend behavior when omitted, except for
the Windows GDI correctness fix recorded below. Explicit
cursor inclusion composes only an operation-owned native cursor into the
in-memory image, with clipping/hotspot handling and bounded cursor bytes. Window
frame, shadow and transparency choices use native semantics where implemented;
declared unsupported policy choices fail before pixels are captured. Native
connection, format/color and cursor-availability errors can still occur during
the operation; no image is published on those failures. No synthetic shadow,
opaque alpha relabeling or desktop crop is presented as transparent window
capture. An explicitly selected opaque export may flatten against a specified
background; native transparency preservation is a separate capability.

All option/catalog/output validation precedes delay, capture or external input.
Delay is a finite operation-owned wait, 0–3600000 ms. Template tasks check their
existing cancellation/deadline callback in at most 20 ms sleep slices while
waiting; no timer runs outside this explicitly pending operation. Direct
capture-task uses its existing one-shot wait and process termination behavior.
Native geometry/identity are re-resolved at
capture time. Keep existing aggregate 256 MiB RGBA admission, metadata-only
templates, one current image and operation-scoped native handles. Cursor images
are capped at 1024×1024/4 MiB and released after composition. No PNG/disk round
trip, implicit save/copy/upload, hidden clipboard owner, texture or image cache
is introduced.

Files: new core `capture_options.rs`, `capture_catalog.rs` and narrow native
identity/cursor adapter modules; core exports; existing `capture.rs` option
integration; desktop capture-catalog CLI
module, capture-task options and Task template source integration;
focused core/executable tests and a private-Xvfb fixture script. Existing
CaptureSettings/RegionPreset and all GPUI controls stay unchanged. Root owns
main/CLI and Task source wiring; exact public APIs are coordinated before edits.

## Commands and policy data

```sh
screenshots capture-catalog monitors
screenshots capture-catalog bind catalog.json crop DISPLAY 20 20 640 480
screenshots capture-catalog bind catalog.json crop DISPLAY 20 20 640 480 --connector true
screenshots capture-catalog validate catalog.json
screenshots capture-catalog capture catalog.json crop result.png
screenshots capture-task window WINDOW result.png --capture-options '{"window_frame":"client"}'
screenshots capture-task display DISPLAY result.png --defaults catalog.json
```

`bind` writes a new catalog; its optional `--profile FILE` and `--hotkey CHORD`
assign a task/hotkey to the template. It validates/canonicalizes the profile's
regular-file path without reading its task source, allowing the authored task
to reference the new catalog being published. `--defaults JSON` sets the saved
options. `add INPUT OUTPUT NAME DISPLAY X Y WIDTH HEIGHT [OPTIONS]` writes a
new catalog containing an additional template. `defaults INPUT OUTPUT JSON`
copies a catalog with changed options. These commands never replace an existing
output or change the input catalog. `capture` exports by PNG/JPEG/WebP extension
through the existing no-overwrite writer; JPEG uses quality 90 and refuses alpha.

Options are a strict JSON object with these optional fields:

```json
{"cursor":"exclude","window_frame":"native","window_shadow":"native","window_transparency":"native","delay_ms":0}
```

Cursor accepts `exclude`/`include`; frame accepts `native`/`client`/`include`;
shadow accepts `native`/`exclude`/`include`; transparency accepts
`native`/`preserve`/`opaque`. Opaque mode requires an explicit `background:[R,G,B]`
with 0–255 channels; other modes reject background. It composites straight alpha
against that background instead of merely changing alpha bytes. Client-only
capture cannot include a shadow. Window-specific fields apply only to window
captures. `capture-task --capture-options JSON` and `--defaults CATALOG` are
mutually exclusive; an explicit `--delay-ms` overrides only the saved delay.
The selected options/recipe are retained before waiting, independent of later
catalog edits. Existing UI settings/countdowns and capture-sequence remain
unchanged.

Use the existing task runner for a bound region:

```json
{"version":1,"source":{"type":"template","catalog":"catalog.json","name":"crop"},"actions":[{"type":"save","path":"capture-{uuid}.png"}]}
```

`capture-catalog run-task CATALOG NAME` explicitly executes the assigned profile.
It reads the selected catalog once, validates one owned task against that
catalog/name and executes that same task with the initial prepared template.
Replacing either file after admission cannot change its source, actions or
template options.
`capture-catalog host-config CATALOG OUTPUT.json` exports its assigned profiles
and stored hotkeys to the existing host format, with IDs `region-01`..`region-16`
and a 90-minute task timeout (covering the one-hour delay bound). Start that
explicit file with `task-host serve OUTPUT.json ENDPOINT.json`. No configuration
watcher or second hotkey host is added. `validate`, `run-task` and `host-config`
require every relevant assigned task to use the exact selected catalog (canonical
path) and template name; an unrelated file/window/task source is rejected before
dispatch. When copying a catalog to another path, update its task source paths
deliberately before using the copied catalog's assigned-task commands. Source
Template never executes catalog profile metadata recursively.

## Current native support

| Policy | Linux X11 | Windows | macOS |
| --- | --- | --- | --- |
| Still cursor | XFixes premultiplied ARGB, clipped by native hotspot | Snapshot icon; premultiplied alpha or native AND/XOR mask semantics | Explicitly unsupported |
| Window frame | Native/client drawable; decorated inclusion unsupported | Native/included DWM frame; client crop uses physical native client bounds | Native/included window; client-only unsupported |
| Shadow | Native/exclude client drawable; inclusion unsupported | Native/exclude DWM frame; inclusion unsupported | Native/include or native framing-effects exclusion |
| Transparency | Native legacy behavior or explicit opaque background; preserve unsupported | Opaque GDI RGB8 or explicit opaque background; preserve unsupported | Explicit RGBA8/sRGB alpha preservation or opaque background |
| Monitor identity | Connector + validated EDID SHA-256; connector-only requires explicit choice | Active DisplayConfig target device path | ColorSync display UUID |

Linux cursor/template operations require an X11 session and one X screen, which
may contain multiple RandR outputs. Native display/region cursor composition and
template capture currently require unrotated 100% physical-pixel coordinates.
Mirrored/ambiguous Windows identity mappings fail. Native macOS options reject
unrecognized component order, unnamed/non-sRGB color spaces, float/high-bit data
and unsupported alpha formats, rather than relabeling those pixels as sRGB.
The default omitted policies preserve the existing XCap path; this does not
correct its broader HDR/color limitations.

The macOS shadow exclusion uses Apple's
[boundsIgnoreFraming](https://developer.apple.com/documentation/coregraphics/cgwindowimageoption/boundsignoreframing)
option for native framing effects, rather than a synthetic shadow crop. Native
cursor decoding retains Microsoft's documented
[alpha versus mask composition](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-drawiconex)
semantics. Inverting an AND/XOR cursor over transparent destination pixels is
explicitly unsupported. A cursor is sampled after native still capture; the
image and cursor are not one atomic compositor snapshot.

## Resources and limitations

Templates retain only bounded identity/geometry/options/path metadata. Explicit
native operations open/release their connections/handles on demand. The added
Linux x11rb dependency was already in the locked graph; Windows uses existing
bindings with Display/HiDpi features. No resident worker, runtime or periodic
native enumeration is added. New code does not initialize clipboard services.

Cursor pixels are at most 4 MiB. Linux additionally holds up to 4 MiB of native
reply pixels while converting them; Windows legacy cursors add a packed mask
of at most 256 KiB plus OS-owned icon bitmaps. A Windows client crop temporarily
holds its full-window image and one cropped output. macOS holds its native image,
one copied provider representation (stride extent capped at 288 MiB) and one
RGBA output (at most 256 MiB), releasing native objects at return. These active
costs and OS/codec allocations are additional to metadata; they are not process
RSS measurements or a hard bound on allocations inside the OS/XCap.

The desktop executable installs the bounded per-operation process executor
described in [native-capture-isolation.md](native-capture-isolation.md).
Cancellation spans the template delay and the native process deadline/cleanup;
unresolved termination retains admission rather than starting another worker.
Core consumers that do not install that executor still run inline and do not
have native process deadlines. Windows/macOS native execution remains a
separate acceptance gate.
Partial JPEG/WebP/PNG save and atomic publication retain the existing exporter
contract. No complete HDR, named Wayland monitor/region binding, Retina/mixed-DPI calibration,
cross-monitor selection, persisted scheduler or physical-monitor hotplug gate is
closed by this slice.

## Following delivery: durable scheduling and desktop regions

Persist a bounded schedule of existing task profiles with an explicit owner,
UTC start, interval/count/end, missed-run policy and no overlap/backlog. Prefer
OS user timers where available; registration must be explicit, inspectable and
removable. Tests stage service/timer/launch/task definitions under temporary
roots and use private process fixtures; they never register a timer in the
user's desktop/session. Do not call one sleeping foreground invocation a
durable scheduler, and do not create an idle clock-polling daemon. Specify the
exact OS adapters, missed-run semantics, ownership and recovery plan before
implementation of this stage.

Extend native desktop composition only after physical coordinate/session
admission, before/after geometry and byte bounds can be verified. Add global
desktop-region capture from admitted source frames without an intermediate
stitched full-desktop image when only a small cross-monitor region is needed.
Multiple output regions must reuse one frame per participating monitor. Linux
and macOS adapters must be implemented or remain explicitly unsupported; mixed
DPI, rotation, native Wayland portal permissions and genuine high-bit HDR remain
separate obligations. No existing supported platform is silently dropped.

## Verification

Pure fixtures cover strict catalog shape/limits, legacy/omitted defaults,
geometry/identity changes, ID/order changes with stable native identity,
ambiguous/missing monitors, path snapshots, cursor hotspot/clipping/alpha,
unsupported policies before capture and unchanged source pixels. Executable
rejections remove all native session access and redirect user state. Actual
native tests use an authenticated TCP-disabled private Xvfb server, only its
painted windows/outputs/cursor, independent decoded pixel oracles and no user
input. Compile Windows/macOS separately and distinguish compile success from
native acceptance. Report optimized active-work observations without claiming
reference-hardware budgets or repeating unrelated exhaustive benchmarks.

Optimized core filtering passed 16 regular checks with one unrelated opt-in
loopback uploader case excluded (`target/capture-options-catalog-final.log`).
The twelve new core checks cover options/cursor bounds, exact alpha/clipping,
opaque flattening, unsupported-before-delay, EDID checksums and template
identity/schema/path/cancellation behavior, 16/17-template and 64-KiB boundaries,
FIFO refusal and native legacy cursor mask/inversion semantics. Four catalog executable checks and
five existing capture-task rejection checks passed
(`target/capture-catalog-cli-final.log`); the stronger assigned-profile checks
reject another valid template, another catalog and an executable file-input
task without running it. Capture-task's final 13 unit checks include defaults
snapshotting and explicit-delay override (`target/capture-options-desktop-final.log`).
The final catalog/host run also passed all four catalog and seven host executable
checks (`target/capture-template-host-final.log`).

`scripts/test-capture-catalog.py` paints only an authenticated TCP-disabled
private Xvfb server. The current run checks 48,259 exact RGBA pixels across
display/window/region captures, native cursor hotspot and one-pixel clipping,
explicit connector binding, saved-template/assigned-task capture, three rapid
physical hotkey taps, stale identity/geometry refusal and live source-display
preservation. Host endpoint cleanup and absent redirected user-state paths are
checked. The rapid taps exposed a real Xlib queue race: XSync could consume
socket bytes into its internal event queue immediately before an indefinite
poll. Rechecking XPending before poll fixes that race without adding a timer.
The report pins a stable executable inode and records the actual binary hash
(`target/verification/capture-catalog-private-final.json`, SHA256
`7b0d36d5c1e3f63f5baf5ab93201fd941ceaebb11e980dfbb30a80d4427b287a`). The 1.76-second
small-fixture duration includes command orchestration, independent FFmpeg decode
and binary hashing; it is not a capture latency distribution or hardware budget.

Windows optimized core compilation passed with the preserved GNU toolchain
(`target/capture-options-windows.log`). Complete application/test cross-compilation
passed with the debug check profile (`target/capture-options-windows-final.log`).
An optimized full-app cross-check on Linux
cannot generate GPUI's Windows release shader include; no shader stubs were
introduced. macOS has no local SDK/runtime here: its new FFI paths need native
CI compilation and actual permission/color/shadow acceptance. The unsupported
platform modes in the table are genuine remaining capabilities, not skipped
tests claimed as implementations.

## Reviewed Windows GDI correction

Fresh review found that upstream xcap 0.9.10 resized its GDI image to the DWM
visible extent before applying positive invisible-border offsets. That crop
could then clamp to a smaller image even for a normal 100%-scale decorated
window, causing explicit policies to fail the required strict geometry check.
Inferring the bitmap extent from the selected DC bitmap could also substitute a
screen-sized surface for the actual window.

The narrowly vendored Windows GDI path now allocates/renders the physical
`GetWindowRect` extent under scoped PMv2 DPI awareness and crops exactly once to
the checked DWM rectangle. It rejects outside/invalid geometry, raw window
buffers above 256 MiB, partial readback and changed raw/DWM/PID observations.
It clears the render target and restores selected GDI objects on every return.
No resize, class-name exception or scale guess remains. Applications that relied
on the prior Dialog/UWP resize heuristics need native calibration; the backend
does not silently resample them. WGC/video/non-Windows rendering is unchanged.
Microsoft documents the distinction between the
[DPI-virtualized window rectangle and physical visible DWM bounds](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getwindowrect).

GDI window output now has alpha 255 because
[BI_RGB32 reserves its high byte](https://learn.microsoft.com/en-us/windows/win32/wmdm/-bitmapinfoheader).
This preserves captured RGB and explicitly describes opaque GDI output; it does
not implement native shadow/alpha preservation or HDR. The raw invisible border
also counts against the 256 MiB source admission, so a near-limit visible frame
can legitimately be refused when its full raw window exceeds the cap. The peak
application raster cost is the raw RGBA image plus one cropped output, in
addition to the native GDI bitmap/DC resources. Native calls remain synchronous.

The exact production crop helper is exercised by two optimized app-core tests,
including negative origins, all source pixels through the right/bottom edges,
opaque RGB semantics and mismatched/outside geometry refusal
(`target/window-gdi-crop-core.log`). The complete GNU Windows application/test
cross-check passed (`target/window-gdi-windows-check.log`). These are source and
compile evidence, not native DWM execution.

`apps/desktop/tests/capture_window_windows.rs` adds an explicit owned-window
native gate. A separate child creates a standard `WS_OVERLAPPEDWINDOW`, paints
an exact client checker pattern and remains alive on its native message loop.
The parent proves PID/handle ownership, requires genuine enabled DWM, 100%
scaling and actual invisible resize borders, then checks full visible frame
pixels against an independent BitBlt from that owned HWND's DC and exact client
pixels. Frame/client/cursor/opaque/default capture policies are covered. No
desktop DC is captured, input injected, pointer moved, clipboard changed or
image written. Native resources/child are cleaned up on completion or failure.

Run this only on an actual Windows desktop/CI runner, with an outer native-call
timeout. DWM unavailability fails the gate; Wine is not accepted as DWM evidence:

```sh
cargo test --locked --profile release-fast -p screenshots --test capture_window_windows decorated_window_policies_preserve_exact_dwm_frame_and_client_pixels -- --ignored --exact --test-threads=1
```

Native execution of this added gate is pending the reviewed push/CI run. The
existing strict caller geometry check was retained; it was not weakened to hide
the backend defect. Provenance and the limited patch are recorded in
`vendor/xcap/LOCAL-PATCH.md` with the original source/license.
