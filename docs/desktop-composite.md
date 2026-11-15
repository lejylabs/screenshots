# Stitched physical-pixel desktop

`screenshots capture desktop OUTPUT` captures one frozen image per currently
enumerated display and exports one stitched image. This is a Windows-only CLI
entry, currently requiring 100% scaling on every monitor until scaled XCap
coordinates have passed native calibration. Other sessions remain unsupported. It does
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
succeed. A topology change that returns to the same values between observations
cannot be detected; this remains native acceptance work. The pure compositor
accepts mixed scales only when supplied positions already describe nonoverlapping
physical pixels; the native entry requires 100% scale. Rotations must be 0, 90,
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
