# Capture and process one image

```sh
screenshots list displays
screenshots capture-task display DISPLAY_ID output.webp --recipe recipe.json --delay-ms 1500
screenshots capture-task window WINDOW_ID output.png
screenshots capture-task region DISPLAY_ID 20 30 640 480 output.png --upload upload.json
```

This explicit headless command captures one display, window or display-relative
region, optionally applies an
[offline workflow recipe](workflows.md) to the in-memory pixels, and saves only
the final image. It does not create a raw capture file, history entry, clipboard
image or application window. An upload occurs only with explicit `--upload`.
Stitched-desktop capture is outside this command.

The display or window ID must contain 1–64 UTF-8 bytes. Region coordinates are
nonnegative whole numbers relative to the display; origins must fit signed
32-bit coordinates, dimensions must be positive and fit the existing image
budget, and coordinate addition must not overflow. Native capture checks the
rectangle against the actual display. Linux window capture requires an X11 or
XWayland window; platform permissions and capture limitations still apply.
Output uses the extension: PNG,
JPEG (`.jpg` or `.jpeg`) or lossless WebP. Existing files, directories and output
symlinks are never replaced. The shared exporter can create missing parent
directories. Relative native paths resolve from the invoking working directory.
Image output does not copy source metadata.

Optional arguments may appear in any order, at most once:

- `--recipe FILE` reads and validates a version-1 workflow with 1–16 operations
  and the existing 64 KiB recipe limit. That validated recipe is retained for
  this invocation; changes to the recipe file during the delay do not change the
  task. Coordinates apply to the current image after prior operations.
- `--delay-ms MS` accepts an integer from 0 through 3600000. Omitted or zero means
  capture immediately after validation. A positive value causes one blocking
  sleep on the invoking headless thread before native capture initialization.
- `--jpeg-quality N` accepts 1–100 and requires a JPEG output path. JPEG defaults
  to quality 90 and rejects remaining transparent pixels; it does not flatten
  them. PNG and WebP retain alpha losslessly.
- `--upload PROFILE` reads a native upload JSON profile or `.sxcu` ShareX profile
  using the existing uploader. The profile is validated and retained before the
  delay. Only after local save succeeds are the finalized PNG bytes
  uploaded once. The uploaded representation is lossless PNG even if the
  local file is JPEG or WebP. Profiles are not saved; there is no automatic retry.
  PNG output reuses the same encoded bytes for local publication and upload;
  JPEG/WebP output requires a separate PNG representation for upload.

Unknown, duplicate or incomplete options are rejected. Output format,
no-overwrite checks, recipe parsing and upload profile loading run before the delay or capture. The
output path is checked again immediately before native capture so a new
collision stops without capturing. The exporter also refuses collisions at
publication, including a file that appears later. Native permissions, display
availability and recipe rectangles relative to the actual capture are checked
when that information becomes available. An invalid post-capture rectangle
stops without publishing an image.

Recipe operations run in order on flattened pixels, using the same implementation
as `screenshots workflow`. Redaction removes original pixels before later steps;
blur/pixelation are appearance effects and are not substitutes for opaque
redaction of secrets. JSON stdout requires a successful final export and, when
requested, a successful upload:

```json
{"format":"webp","width":640,"height":480,"steps":3,"delay_ms":1500}
```

`steps` is zero without a recipe. When explicitly requested and successful,
upload adds `"upload":{"url":"https://…","thumbnail":null,"deletion":null}`.
Thumbnail and deletion links are included when supplied by the validated
profile response; the command does not open or follow them.
Errors use stderr and exit status 1. If the file was saved but upload fails,
the error says the local file remains and a remote file may exist. If stdout
fails after upload, it reports that the image was saved and uploaded.
No success JSON is printed on capture, recipe or export failure. No transaction
can undo a file already published when the process is forcibly interrupted.
No upload is attempted after capture, recipe or local-save failure. The uploader
retains its existing request/response bounds, credentials, redirect policy and
timeouts. Credentials are resolved only for the explicit upload; an unavailable
credential can therefore fail after the local file has been saved.

This is one foreground command, not a persisted schedule. The process must stay
alive during the delay; it does not wake the computer or guarantee precise
wall-clock timing. Ctrl+C/process termination uses the existing headless OS
behavior. There is no cooperative native-capture timeout. An interrupted export
can leave one unpublished UUID `.part` file; it retains the existing exporter
cleanup and storage-inspection behavior.

Capture and every workflow image retain the existing 256 MiB decoded RGBA
admission limit. The command owns one current image and, during replacing
operations, at most one output image; prior pixels are released after each step.
Blur scratch is below 300 KiB; pixelation grid storage is
`ceil(width/16) × ceil(height/16) × 4` bytes, at most 16 MiB under the existing
pixel limit. Native capture, codec scratch and runtime baseline are additional
costs. No full-image undo history, frame queue, extra worker, service, runtime,
dependency or idle activity is added by capture-task. Explicit upload creates
the existing operation-scoped network runtime and a bounded encoded PNG buffer;
decoded final pixels are released before transfer. The uploader currently admits
at most the 256 MiB capture limit plus 16 MiB encoded PNG overhead. These bounds are not process-memory or
latency measurements.

Synthetic unit fixtures cover delay-before-capture ordering, recipe snapshotting,
final-only pixels, unchanged alpha, released source frames, delay/output failures,
JPEG transparency rejection, display/window/region routing, upload-after-save
ordering, final PNG content, upload failure and option boundaries. Executable rejection fixtures
use isolated temporary paths with native display/session access removed. They
never capture the real desktop. Native capture permissions, display loss, actual
delay timing and hardware resource acceptance remain separate checks.

The optimized focused checks passed 10 unit and 4 CLI fixtures, including a synthetic capture
and redaction followed by the real uploader to an explicitly allowed loopback
HTTP server. It verified the local file existed before the request and that the
uploaded PNG matched the saved final pixels. This uses no external endpoint and
does not establish TLS or native screen-capture acceptance.
