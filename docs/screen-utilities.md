# Screen color and ruler services

Status: backend implementation and source review completed on 2026-11-16;
optimized pure fixtures and authenticated private X11 checks passed as recorded
below. Native hardware/profile acceptance remains separate. No new UI controls or
interactive overlay are added. These explicit commands add no idle services,
watchers, caches, optional runtime or dependency.

## Commands and coordinate meaning

`screenshots screen-color DISPLAY X Y` samples one pixel from an explicitly
selected native display. X and Y are nonnegative **physical raster pixels**
relative to that display's top-left, not GPUI logical coordinates. The isolated
native capture worker validates the selected source and bounds. The default
capture policy excludes the cursor. The result contains RGBA bytes and RGB/RGBA
hex strings. It creates no screenshot/history entry, writes no clipboard data
and makes no network request. The single result frame is released before stdout.

`--global` changes X/Y to signed physical desktop coordinates. The command first
loads bounded native display metadata, selects one exact identity, validates its
physical coordinate policy and converts global coordinates with 64-bit bounds
arithmetic. It sends an ExactRegion request, retaining the admitted geometry for
the native before/after checks; changed or out-of-display coordinates fail.
Negative monitor origins are supported. This mode requires the adapter to prove
physical metadata. X11 obtains raw RandR origin/size without applying Xft DPI;
Windows uses its calibrated physical geometry. Uncalibrated macOS scaled/rotated
metadata is refused rather than guessed. Display-local raster sampling does not
require that logical-to-physical metadata conversion. Named Wayland display capture uses
the existing explicit unsupported result; it does not silently capture XWayland.

The returned `encoding` is `capture_backend_rgba8`. The sample follows that
platform's current capture conversion/profile policy. The utility does not claim
HDR sampling, independently calibrated colorimetric values or an ICC profile
when its source has supplied none. RGB hex is a convenience representation of
those exact bytes, not evidence that an unknown source was converted to sRGB.
Native permissions and color/backend acceptance remain separate.

`screenshots ruler X1 Y1 X2 Y2` measures two supplied physical pixel centers. It
returns signed X/Y deltas, unsigned rectangle width/height, the rectangle's area,
Euclidean distance, and clockwise angle in the range -180 to180 degrees in a
desktop coordinate system whose Y axis increases downward. Rectangle dimensions are distances between the centers;
they do not count both endpoint pixels. Coincident points have zero distance and
no angle. Coordinates support the full signed 32-bit range. Integer differences
and area remain exact; distance and angle use finite floating-point arithmetic.
It performs no capture, enumeration, filesystem or clipboard access.

`--dpi X_DPI Y_DPI` also returns inches, millimeters and square millimeters using
the two **explicit user-supplied physical DPI values**. GUI scale is never treated
as physical DPI. Each value must be finite and within 0.000001–1000000; differing
horizontal and vertical values are supported. Without these values the command
makes no physical-length claim. Supplied DPI must apply to the whole measured
segment; a span across monitors with different physical DPI requires separate
segment measurements.

The bounded clipboard viewer foundation is the existing `clipboard inspect`
(format names only), `text`, `files` and `image-info` commands; see
[clipboard import/output](clipboard-import-output.md). It remains explicit,
operation-scoped and separately bounded, with no duplicate clipboard backend in
these utilities.

## Resources, cancellation and checks

One color request uses the existing admitted native helper and a 20-second
operation deadline, with existing helper cleanup/process limits. `--global`
adds one bounded enumeration before the capture and shares the same overall
20-second cancellation check. Unix Ctrl-C/termination participates through the
existing command cancellation guard. The returned output contains four pixel
bytes and bounded identity/JSON metadata. Native backends may acquire a larger
source internally; their existing source limits and isolation still apply.
There is no PNG encode/decode or disk round trip in this path. Ruler calculation
uses constant-size numeric state plus one small JSON result.

Fixtures cover malformed arguments before native work, exact one-pixel request,
changed identity/count/dimensions/backing refusal, late cancellation, pixel lease
release before result, negative global origins/bounds, signed coordinate overflow,
zero/directed distances, anisotropic DPI and full i32-span area/distance. These
owned synthetic cases do not read the user's desktop or clipboard. The optimized
and private native execution evidence below covers these bounded paths. It does
not establish native hardware/profile acceptance.

## X11 physical bounds and selected-source privacy

The Linux-only vendored `Monitor::physical_geometry` reads raw RandR raster
origin/size. Existing Xcap logical getters keep their prior behavior for other
consumers. Application display metadata and Display/Region allocation admission
use the raw dimensions; a 192-DPI resource setting cannot halve those bounds and
a 48-DPI setting cannot expand them into another monitor. The vendor region path
uses one current raw monitor snapshot for both selected-source bounds and the
root read's origin, with checked coordinate addition and native protocol ranges.
It rejects an outside pixel before issuing that root read. ExactRegion and local
Region calls also compare before/after native geometry. Those are endpoint
checks, not a topology-generation witness: an A/B/A transient or source-ID reuse
with identical endpoint metadata can remain undetected. No Windows generation
or physical-hotplug guarantee is claimed for X11.

Private native fixtures explicitly set Xft DPI48 and192. A split RandR monitor
owns only the left48×64 pixels of a96×64 root, with an independently drawn right
half. CLI local/global calls and the direct vendor API must admit x40 and refuse
x70, while physical metadata remains48×64. The direct ignored test requires the
fixture's guarded display, private owned authority file and explicit marker; it
is never run against a default desktop. The same private server tests full-raster
far-edge samples at both DPI settings. Both optimized native DPI cases passed.

## Executed evidence

Six optimized desktop pure fixtures and two exact production vendor-bound
fixtures passed. The private E2E run completed59 CLI invocations:16 arithmetic
or invalid-argument cases,28 independently drawn native color results, plus
inventory/refusal checks. Two additional guarded direct-Xcap native tests passed
under Xft DPI48 and192. The selected left48×64 monitor admitted x40 and refused
x70 even though a real adjacent right-half pixel existed in the96×64 root.
The legacy vendor logical getter still reported DPI-scaled dimensions, proving
that the new native bounds did not change that public getter's behavior.

The CLI SHA-256 was
`d65064c2c4c30d32ed0dbcc3bb1c1c21e645ddbcb96f1713f61c89c3c05f30df`.
Local records are `target/verification/screen-tools-e2e.json`, its matching log,
`screen-tools-unit-tests.log` and `linux-physical-bounds-unit-tests.log` in that
verification directory. These are local ignored evidence, not shipped artifacts
or proof of a later source head. No user desktop, clipboard or account was read.
Fresh EGM source reviews2 and3 found no Blocker, Major or Nit; source review
converged before this factual evidence append. Named Wayland capture, native
macOS scaled global geometry and physical/profile/performance acceptance retain
the explicit limits above.
