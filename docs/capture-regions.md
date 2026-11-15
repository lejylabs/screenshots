# Multiple regions from shared display snapshots

Author: NewtTheWolf
Date: 2026-11-15
Status: implemented; release verification in progress.

## User contract

`screenshots capture-regions DIRECTORY DISPLAY X Y WIDTH HEIGHT
[DISPLAY X Y WIDTH HEIGHT]...` accepts 1–16 explicit display-local rectangles.
It captures each referenced display once, then writes each requested crop as
`region-01.png` through `region-16.png` in argument order. Overlapping regions
on the same display contain pixels from the same captured frame. Distinct
displays are captured sequentially; this is not simultaneous desktop capture.

The output directory must already exist, be a real directory rather than a
symlink, and be empty before capture and again before publication. Files are
published with the existing atomic no-overwrite PNG writer. The command never
creates history, changes the clipboard, uploads, opens an editor or changes
settings. All captures/crops stay in memory until their explicit PNG output.

Display IDs are 1–64 UTF-8 bytes without control characters. Coordinates/dimensions are whole nonnegative
`u32` values, dimensions must be nonzero, and both endpoint additions must fit
`u32`. All rectangles must fit the corresponding display. Full source RGBA
across distinct displays is capped at 256 MiB; aggregate requested crop RGBA is
also capped at 256 MiB. Invalid arguments fail before any native capture.

## Native and resource contract

Use the existing XCap full-display batch, then `crop_snapshot` for each output.
Reject rotated displays, scale other than 100% and the current Wayland path.
Linux requires X11 with a nonempty UTF-8 DISPLAY and no nonempty WAYLAND_DISPLAY.
Native X11 origins/dimensions must fit the backend's signed/unsigned 16-bit
fields. Windows uses the same scoped per-monitor-v2 thread DPI context as
region analysis. macOS/Windows native acceptance remains open.

Compare every selected display's position, dimensions, scale and rotation
before and after the capture batch. Require one result per distinct display,
correct identity, no window target, matching pixel dimensions/backing length
and the aggregate source byte cap before any save. On Windows, the backend's
frame geometry must also agree with the admitted display. These are topology
race checks, not an atomic capture guarantee.

Retain at most the admitted full-display frames plus one crop and the existing
bounded PNG writer/encoder scratch. Drop each crop after its save and release a
display after its final requested crop. A duplicate/overlapping output counts
against aggregate crop admission. No new runtime, thread, timer, queue, texture,
cache or dependency is added. XCap/OS allocations precede returned-frame checks;
the source cap is not a hard process-RSS cap. Native calls and filesystem writes
have no hard interrupt deadline; Ctrl+C terminates the headless process.

If native capture or geometry validation fails, no files are written. If a later
save fails, earlier published PNGs remain. The error states the number of earlier
files confirmed and the failed ordinal, preserving the underlying error. The
current file may also exist if its publication failed ambiguously; there is no
retry or rollback delete. Success prints one bounded JSON summary containing
`completed`, `captured_displays`, `directory`, ordered `files` and
`path_encoding: "lossy-native"`. Output write/flush failure after saves
must say the images were already published. A partial filesystem result is not
reported as a complete capture group.

## Files and verification

- Create `apps/desktop/src/capture_regions.rs` for admission, capture/crop/save
  orchestration, result schema and injected deterministic tests.
- Create `apps/desktop/tests/capture_regions.rs` for real executable invalid
  arguments, existing output and output-free failure checks.
- Create `scripts/test-capture-regions.py` for private authenticated Xvfb capture
  of a 96×64 synthetic scene, independent FFmpeg PNG decoding, overlap equality
  and source/state preservation. It requires installed Python, libX11, Xvfb,
  xauth, xdpyinfo, FFmpeg and an optimized application binary; it installs nothing.
- Modify `apps/desktop/src/main.rs` and `cli.rs` to route before GPUI and add help.
- Modify `region_analysis.rs` only to reuse session, coordinate and scoped-DPI
  checks; preserve the existing OCR/QR behavior.
- Update this document, `docs/non-ui-delivery.md` and `docs/gap-analysis.md` with
  actual tests, final schema and support limits after implementation.

Injected tests must prove one capture per distinct display, exact overlapping
pixels, stable argument order, byte/count bounds, topology/returned-frame errors
before publication, source release and truthful partial-save results. Existing
Xvfb fixtures can verify actual CLI PNG outputs on a private synthetic display;
never capture the user's desktop just for test convenience. Compare optimized
results and Windows compilation separately from hardware acceptance.

Alternative: separate native calls for every region were rejected because they
would not share one frozen frame. Capturing one stitched multi-monitor canvas
would allocate additional unnecessary pixels and change coordinates; retain
one frame per selected display instead.

Seven injected unit tests and three executable rejection tests passed. A native
private-Xvfb run captured three overlapping regions from one 96×64 display:
every PNG decoded to the expected pixels, 1,344 overlap comparisons matched,
the source frame stayed unchanged and no user-state paths were created. Windows
GNU compilation and Clippy passed. These checks do not establish native
Windows/macOS, multiple physical monitors, power-loss or reference-hardware
memory/latency acceptance.

Run the explicit isolated native fixture after building:

```sh
python3 scripts/test-capture-regions.py --binary target/release-fast/screenshots --xvfb /path/to/Xvfb
```

The destination check rejects a symlink in the final path component; it does not
claim a descriptor-based sandbox against concurrent renaming of parent paths.
Trailing separators and `.` components are normalized before that check;
`..` keeps its native path semantics and is not flattened across symlinks.
PNG publication reuses the existing writer: synced image contents and atomic
no-replace publication, without a new parent-directory power-loss guarantee.
The command intentionally captures complete selected displays into memory, even
when every requested crop is small. Unselected pixels are released with the
source frame and are not implicitly saved, copied or uploaded.
