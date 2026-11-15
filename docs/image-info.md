# Image information and pixel sampling

Status: explicit headless file utilities. These commands do not start the
desktop UI, sample the screen, modify files or change clipboard/history state.

```sh
screenshots image info INPUT
screenshots image pixel INPUT X Y
```

`info` returns one JSON object with `width`, `height`, `decoded_rgba_bytes` and
`source_file_bytes`. Dimensions describe the decoded image after EXIF
orientation, and decoded bytes are the actual RGBA8 buffer length. Source file
bytes are a filesystem metadata observation at the start of the command; if
another program changes/replaces the file concurrently, this count can differ
from the bytes later seen by the decoder. The command does not claim a snapshot
of a file being concurrently edited.

`pixel` accepts zero-based unsigned integer X and Y coordinates in that oriented
image, with the origin at its top-left corner. It returns width, height, X/Y,
`rgba` as four 0–255 channel values, and uppercase `hex_rgba` as `#RRGGBBAA`.
Coordinates outside the image fail without sampling. Transparent pixels retain
their stored RGB channel values; this is raw decoded-pixel sampling rather than
a color composited against a background.

Both operations reuse the shared PNG/JPEG/WebP and [truecolor BMP](bmp-input.md) loader, with at most 32 MiB
encoded input and 256 MiB decoded RGBA. The input format is recognized by the
decoder rather than inferred from the extension, but is not reported because
the shared loader does not return a trustworthy encoding label. These are basic
decoded-image details, not a full EXIF/ancillary-metadata inspector. No embedded
GPS, comments, thumbnails, ICC profile, or arbitrary metadata payload is emitted.
Color values are the decoder's RGBA8 output used by the existing editor; there
is no additional ICC/wide-gamut/HDR conversion or claim of colorimetric accuracy.

The source buffer is released before output. Result JSON is small and flushed
explicitly; output errors are reported as failures. No new dependency, runtime,
thread, timer, cache or background task is introduced. Full decoding is required
even for information/single-pixel commands; peak decoder scratch and encoded
input are additional to the 256 MiB retained-pixel bound. Existing synchronous
decoder calls cannot be canceled mid-call; Ctrl+C terminates the explicit CLI
process. No file, directory or image output is created on success or failure.

Tests invoke the actual executable against generated images to check oriented
dimensions, exact alpha/hidden RGB samples, Unicode/native paths, strict argument
counts, invalid coordinates, malformed and oversized images, unchanged source
bytes and absent user-state writes. Optimized results and measurements are
recorded after execution; design bounds alone do not establish whole-process
memory or native-platform acceptance.
