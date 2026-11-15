# Headless canvas and resizing tools

These commands run without initializing GPUI. They do not alter the editor UI,
clipboard, history, configuration or network. Source files stay unchanged and
existing destinations are never overwritten. PNG/JPEG/WebP input uses the shared
bounded decoder and EXIF orientation handling; output uses the shared PNG/JPEG/
lossless WebP exporter. JPEG still rejects transparency instead of flattening it
implicitly. The shared exporter creates missing output parent directories.

`image convert INPUT OUTPUT.bmp` additionally supports explicit
[RGBA BMP output](bmp-export.md), including its existing rotate/flip/downscale
options. BMP remains output-only and is not added to editor/capture defaults or
the other commands' format selection. JPEG/WebP quality flags are rejected for
BMP output.

`image convert` also accepts `.tif` and `.tiff` for explicit
[uncompressed TIFF output](tiff-export.md), with a tagged straight-alpha fourth
channel. This output-only option uses the same transforms/downscale and does
not add TIFF input support or alter editor/capture format preferences.

```sh
screenshots image compose input.png padded.webp --padding 24 --background 202020
screenshots image compose input.png marked.png --watermark logo.png --at 20 30
screenshots image resize input.png larger.png 1920 1080
```

`compose INPUT OUTPUT` creates one canvas with uniform `--padding N` (default 0).
`--background RRGGBB` is opaque; `RRGGBBAA` supplies alpha. The default background
is transparent black. The source is composited at `(padding, padding)` using
straight-alpha source-over blending. Fully transparent source pixels reveal the
background, including its chosen alpha.

An optional `--watermark FILE` requires `--at X Y`, in nonnegative output-pixel
coordinates. Its top-left origin must be inside the canvas; its right/bottom
edges are clipped. The watermark is composited after the source. No watermark
file is modified or retained. Repeated options and unsupported options fail.

`resize INPUT OUTPUT WIDTH HEIGHT` supports both upscaling and downscaling,
including independent dimensions that change the aspect ratio. Dimensions must
be positive. It uses **center-sampled nearest neighbour**, preserving the exact
RGBA value of the chosen source pixel. No smoothing or averaging occurs, so
hidden RGB in transparent pixels cannot bleed into neighbouring visible pixels.
Pixel art remains sharp; photographs may look blocky when enlarged. The separate
`image convert` command retains its existing downscale-only behavior.

For output coordinate `x`, the sampled source coordinate is
`floor((2*x+1)*source_width/(2*output_width))`, and likewise for `y`.
Successful commands print one JSON object with `format`, `width` and `height`.
The shared exporter matches extensions case-insensitively; `.jpg` and `.jpeg`
both report `jpg`, PNG reports `png`, and WebP reports `webp`. JPEG quality is
the existing default of 90.

Resource cost: one operation, decoded source pixels and one final RGBA buffer.
Composition additionally retains one watermark, with combined source/watermark
RGBA admission limited to 256 MiB; the final canvas independently has the same
limit. Each encoded input retains the shared 32 MiB limit. Nearest resizing writes
directly into its result without a full-image float intermediate. Shared decoder
and encoder scratch allocations still apply. No cache, undo snapshots, threads,
timer, runtime or idle work is added.

These are explicit file utilities. They do not implement editor canvas controls,
annotation transforms, reusable effect presets or general watermark workflows.
