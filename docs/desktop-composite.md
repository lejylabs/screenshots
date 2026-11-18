# Stitched desktop output

`screenshots capture desktop OUTPUT` captures one frozen image per currently
enumerated display and exports one stitched image. Windows uses operation-scoped
physical mode/rectangle/DPI calibration, including mixed scales and already
oriented rotations. macOS uses the explicitly derived Quartz layout grid below.
Linux X11 uses the independently checked RandR root-raster policy below. It does
not copy the clipboard, create history, initialize the UI or start a stream.

The pure `stitch_desktop` core operation accepts 1–32 display descriptions and
exactly one frame per unique display ID. Positions are physical desktop pixels;
negative origins are normalized against the upper-left bounding box. Rows are
copied exactly, including alpha; holes between displays remain transparent.
Overlapping/mirrored layouts, mismatched frames, invalid scale/rotation and
oversized bounding boxes fail before the output allocation. Layout metadata,
total source bytes and final output each keep their existing admission limits.
The source group is at most 256 MiB and the output is at most 256 MiB; no persistent
buffer, queue, worker, cache or idle task is introduced. The pure compositor
retains its supplied sources only during the call and allocates one output.

The native entry enumerates displays, preflights the layout, calls the existing
single capture batch, requires each frame's before/after-validated display geometry
to equal the original description, re-enumerates and rejects changed geometry before assembly.
No output is published unless capture, topology comparison and assembly all
succeed. The entire request runs in one isolated native helper with an outer
topology witness. Windows 11's dynamic topology generation also detects restored
layouts; Windows 10 uses global scale notifications, known native events and
endpoint checks with its explicitly weaker guarantee. Physical reconfiguration
acceptance remains open. The pure compositor
accepts mixed scales only when supplied positions already describe nonoverlapping
physical pixels; native Windows positions are independently calibrated. Rotations must be 0, 90,
180 or 270 degrees. This feature does not convert logical display coordinates.

Synthetic core fixtures cover reversed frame order, negative origins, unequal
scale factors, transparent gaps, duplicate IDs, overlaps, missing/mismatched
frames and overflowing/sparse layouts. These do not establish native multi-DPI
or monitor-disconnect acceptance. Cross-monitor interactive drag selection is
separate UI work and is not claimed here.

The pure function trusts the caller's physical-pixel coordinate convention; it
cannot detect a nonoverlapping logical-coordinate layout. Display IDs must be
nonempty and at most 64 bytes. Window frames, differing declared/pixel dimensions
and non-RGBA format markers are rejected. Rotation metadata describes the already
rotated captured pixel dimensions; the compositor never rotates pixels itself.

macOS uses `quartz_max_native_scale`: one output grid at the highest admitted
native pixels-per-Quartz-point density, with exact row copies at that density and
explicit nearest-neighbor expansion of lower-density displays. It retains the
Quartz arrangement without inventing an OS-global physical-pixel plane. The
isolated child returns the actual density, point origin and bounded placements
from the same snapshot. Complete 1–32-display inventory, UUID/geometry and topology
checks precede publication; source/output limits remain 256 MiB each. See the
[Mac coordinate contract](macos-coordinate-consumers.md) for integer-grid limits,
memory admission, legacy restrictions and current SDK/hardware acceptance gates.


Linux X11 uses the existing raw `Monitor::physical_geometry()` and full-display
batch, never the legacy getters divided by Xft DPI. One operation-scoped RandR
1.5 connection checks the complete native output/CRTC/monitor inventory before
capture and after assembly. Every active CRTC must have exactly one output and
one matching monitor record; outputless, cloned, omitted, duplicate, reflected,
out-of-root or custom partial-monitor records fail before pixel capture.
Rotations 0/90/180/270 use the already oriented root pixels. Xft UI DPI (including
48/96/192) does not rescale the raster. CRTC projective transforms and panning
currently fail with an explicit unsupported error; supporting these requires a
separate calibrated mapping. Multiple X11 screens also fail, rather than dropping
screens from a successful desktop. Standard multiple-output layouts on one root
are admitted; X11 root rectangles must be nonnegative and within that root.
The pure compositor still supports caller-provided negative physical origins.

A finite RandR event subscription plus timestamp/geometry checks rejects observed
changes even when the arrangement is restored before the endpoint check. No
background observer survives the request. Frames remain sequential snapshots;
this does not promise simultaneous scanout or atomic content across monitors.
The existing helper deadline/process teardown, cancellation, source and output
limits apply. New resource cost is one temporary X11 connection and at most 32
source descriptions, with at most 256 native output/CRTC metadata records per
snapshot; there are no new threads, streams, timers, dependencies or caches.

Wayland `desktop` remains explicitly unsupported. The public
[ScreenCast portal contract](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html#org-freedesktop-portal-screencast-start)
provides optional monitor position and size in compositor coordinates, which need
not equal pixel positions or decoded stream dimensions. It does not promise a
complete physical desktop plane or that the selected sources include every
monitor. The existing explicit `portal` source captures the user's selected
composed source if their portal offers one; a compositor-selected whole-desktop
stream needs no invented inter-monitor positions. Arbitrary selected stream
composition would need a separately named derived coordinate grid and a finite
multi-stream decoder, and is an unimplemented software feature, not hardware
acceptance. XWayland coordinates are never substituted for native Wayland output
positions. See the [RandR root/CRTC model](https://www.x.org/releases/current/doc/randrproto/randrproto.txt)
for the X11 coordinate basis.

Saved Task profiles accept `{"type":"desktop"}` through the same isolated
operation and apply their recipe before explicit output. Validation and schedule
waiting do not capture, and native failure publishes no partial image.

Verification source is `scripts/test-linux-desktop-composite.py`: it owns an
authenticated 96×64 Xvfb, independently draws and compares every output pixel at
three Xft DPIs, runs the real desktop CLI and finalized Task recipe, checks
Wayland/outputless-split refusal and a real restored RandR-monitor change.
Xvfb's outputless second `--setmonitor` record is a negative fixture, not proof
of two captured outputs. Pure geometry/provenance and scheduled-Task tests are
separate. All newly added tests and optimized timing/resource checks were unrun
when this held change was authored; native two-output, rotation, panel scaling,
hotplug, process-memory and reference-hardware acceptance remain open.
