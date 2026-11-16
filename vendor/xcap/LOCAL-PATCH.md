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

The existing non-Windows, video and WGC paths are unchanged. Monitor rendering
is unchanged; the shared GDI readback now rejects incomplete scanline returns.
The only
manifest feature addition is the existing windows dependency's `Win32_UI_HiDpi`.
