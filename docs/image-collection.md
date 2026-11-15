# Image collection utilities

Status: implementation contract for explicit headless file operations. These
commands do not start the desktop UI or implement live scrolling capture.

## Commands and output

```sh
screenshots image combine horizontal OUTPUT INPUT INPUT [INPUT...]
screenshots image combine vertical OUTPUT INPUT INPUT [INPUT...]
screenshots image compare FIRST SECOND OUTPUT.png
screenshots image split INPUT DIRECTORY --columns N --rows N
```

Combine accepts 2–16 PNG/JPEG/WebP files in command-line order. Horizontal places
them left to right, aligned at the top; vertical places them top to bottom,
aligned at the left. Remaining canvas pixels are transparent black. Source RGBA
bytes are copied unchanged, including RGB values under zero alpha. There is no
resizing or alpha compositing. PNG and lossless WebP outputs retain these pixels;
JPEG uses the existing default quality and requires completely opaque output.
Output format comes from its extension. JSON reports format, width, height and
input count.

Compare requires identical dimensions after the shared decoder applies EXIF
orientation. It counts a pixel as changed when any decoded RGBA channel differs.
The output is an opaque PNG: each RGB channel is the greater of its absolute
difference and the absolute alpha difference. Alpha-only changes therefore show
as grayscale. Hidden RGB differences under transparent pixels are deliberately
counted and shown; this is raw decoded-pixel comparison, not perceived visual
similarity. JSON reports `changed_pixels`, `total_pixels`, width, height and
format. Equal images produce an opaque black difference image.

Split requires an existing empty directory and explicit `--columns` and `--rows`
once each, in either order. Each value is 1–16 and their product is at most 64.
The decoded image dimensions must divide evenly by the grid; no pixels are
rounded, dropped or resized. PNG tiles are named `tile-001.png`, `tile-002.png`,
and so on, in row-major order. JSON reports the grid, tile dimensions, completed
count and relative file names. The destination must exist and be empty at
admission. The shared exporter may recreate parent directories removed during
the operation; the directory is not pinned by an open handle. Every tile is
published separately without replacement. A later error keeps completed tiles
and reports their count in stderr; there is no batch rollback or false claim of
all-or-nothing publication. Files appearing concurrently are never overwritten.

All paths use native operating-system path arguments without a shell. Existing
destinations, including broken symlinks, are rejected. Sources remain untouched.
Commands never write history or clipboard contents, upload, fetch URLs or run a
background job.

## Resource contract

The existing decoder admits at most 32 MiB encoded bytes per file, supports only
PNG/JPEG/WebP and validates dimensions before pixel allocation. Combine and
compare admit each next decoder against the remaining 256 MiB total decoded
source budget. Combine separately admits the complete bounding canvas against
256 MiB, then releases source buffers before encoding. Its peak retained pixel
data is at most 512 MiB; decoder/encoder scratch and bounded encoded input are
additional. Compare transforms the first input buffer in place, so no separate
full difference buffer is allocated; retained source pixels total at most
256 MiB. Split decodes once and keeps only the source plus one cropped tile,
at most 512 MiB of retained pixels. The exporter can require format-specific
scratch; these pixel limits are not total-process memory measurements.

There are no new dependencies, threads, runtimes, idle tasks, queues or caches.
Work occurs synchronously in the explicitly launched headless process and uses
the existing atomic file exporter. Ctrl+C uses process termination; decode,
copy and encoding do not promise cooperative mid-call cancellation. Completed
files survive interruption; an unpublished `.part` may remain under the existing
storage-recovery contract. No intermediate image is written to disk.

## Verification

Actual-executable fixtures cover unequal dimensions and transparent padding,
exact raw RGBA copying/comparison including alpha-only differences, row-major
split reassembly, malformed options/counts/grids, dimension budgets, existing
outputs, nonempty directories, unusual native filenames, source preservation
and absent user-state creation. Tests use generated local images only. Optimized
test and resource observations will be recorded after execution; the limits
above are design bounds, not measured whole-process or native-platform acceptance.
