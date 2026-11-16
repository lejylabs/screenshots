# Physical Windows GDI window capture

Published `xcap 0.9.10`, upstream commit
`6b4ad603b01bb18a6170f9ff6b5aebcebab174ec`, with original Apache-2.0 license and
source. The source version is unchanged; this is a local source patch.

Windows GDI window capture now renders into the physical `GetWindowRect`
extent under a scoped per-monitor-v2 DPI context, then performs exactly one
validated pixel crop to `DWMWA_EXTENDED_FRAME_BOUNDS`. The prior implementation
guessed a bitmap size, resized it to the DWM extent, then cropped by positive
invisible-border offsets, shrinking normal decorated windows and interpolating
their pixels. No class-name exceptions, interpolation or guessed scale remain.
Invalid/outside DWM rectangles, more than 256 MiB of raw window RGB32 pixels,
and changed raw/DWM/process metadata fail explicitly.

Selected GDI objects and the thread DPI context are restored on every return.
The bitmap is cleared before asking the target to paint. Window output is opaque
RGB8: BI_RGB32's high byte is reserved, so it is not presented as preserved alpha.
Native shadow/alpha preservation, arbitrary applications' WM_PRINT behavior,
protected/minimized windows and mixed-DPI runtime acceptance are not established
by this patch. Dialog/UWP resize heuristics were removed; native calibration for
those classes remains a separate gate rather than silently resampling them.

`src/windows/window_crop.rs` is the pure helper used by production GDI and by
app-core's cross-platform exact-pixel regression tests. A separate Windows
owned-window fixture requires genuine DWM and is an explicit native CI gate;
Wine is not DWM evidence. See `docs/capture-defaults-templates.md` in the parent
repository for the feature contract and measured verification state.

The video and WGC paths are unchanged. Windows monitor rendering
is unchanged; the shared GDI readback now rejects incomplete scanline returns.
The only
manifest feature addition is the existing windows dependency's `Win32_UI_HiDpi`.

## Linux Wayland still-image disk fallback removal

The Linux still path no longer invokes GNOME ScreenshotArea or the portal
Screenshot interface, stages/deletes PNG files or holds a global D-Bus mutex.
Its existing in-memory wlroots implementation remains, with positive dimensions
and a 256-MiB RGBA admission bound. Unused PNG/URI helpers and the ashpd
`screenshot` feature are removed. Callers must still establish real Wayland
logical coordinates; the application refuses named XWayland monitor/region
captures instead of substituting a picker. Its explicit portal source lives in
the application's bounded ScreenCast/PipeWire worker adapter.


## Linux X11 physical region bounds

A Linux-only `Monitor::physical_geometry` reports raw RandR raster origin/size;
existing logical geometry getters remain unchanged. Native region validation
and root-read origin now use one raw monitor snapshot, independent of Xft DPI,
with checked bounds and X11 coordinate narrowing. This prevents low Xft DPI from
admitting adjacent-monitor pixels and high Xft DPI from rejecting valid far-half
pixels. `physical_region_bounds.rs` is the exact production helper exercised by
app-core fixtures. Private authenticated Xvfb tests additionally cover DPI48/192
and a selected left monitor beside another root raster region; their execution
status is recorded by the parent repository, not inferred from this patch.
The application uses physical metadata for display/region admission. X11
endpoint checks do not establish detection of restored A/B/A topology changes.
