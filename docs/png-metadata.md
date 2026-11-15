# PNG metadata inventory

Inspect one PNG without starting the desktop application:

```sh
screenshots image metadata INPUT
```

The command detects PNG from its signature, regardless of filename extension,
and writes one JSON object to stdout. The object contains:

Exactly one native input-path argument is required; options and extra arguments
are rejected. Unix non-UTF-8 paths are supported and never printed. Success is
one newline-terminated JSON object and exit zero. Failure writes a bounded
operation/validation error through the CLI's `screenshots: ...` stderr prefix,
returns exit one and emits no inventory JSON.

| Field | Meaning |
| --- | --- |
| `source_file_bytes` | Size of the inspected encoded file snapshot. |
| `width`, `height` | Physical pixel dimensions declared by IHDR. |
| `bit_depth`, `color_type` | PNG IHDR numeric fields, before any pixel decoding. |
| `metadata_present` | At least one ancillary chunk exists. This deliberately includes color, transparency, animation and unknown ancillary chunks, as well as text/EXIF. |
| `animated` | A correctly sized, nonzero-frame `acTL` animation declaration exists; frames are not decoded or verified. A one-frame declaration also returns `true`. |
| `chunks` | Ordered objects containing only `type` (four-letter PNG chunk name) and `bytes` (payload length, excluding the 12-byte chunk envelope). |

Chunk values, keywords, comments, profile data, timestamps, EXIF and camera/GPS
data are never printed, including on error. Compressed text, ICC profiles and
image/frame streams are never decompressed. The command creates no files,
history entries or application configuration, changes no clipboard, and performs
no network access. The source remains unchanged.

`metadata_present: false` means there are no ancillary chunks in the inspected
stream. It is not a privacy guarantee: pixels, palettes, compressed image data,
or filenames may themselves contain identifying information. This command does
not sanitize or edit images. Use an explicit pixel re-encoding export when
removing ancillary metadata is intended.

## Bounds and validation

Input must be a regular file, at most 32 MiB encoded, with at most 4096 chunks.
The file is checked before opening and again through the opened handle. Unix
opens use nonblocking mode so replacing a checked file with a FIFO cannot cause
a wait for a writer. A read is limited to the admitted file extent; truncation or
a detected length/modification-time change fails. The in-memory snapshot is
immutable during parsing. These checks detect common concurrent changes, not a
transactional filesystem snapshot or a guarantee against an adversarial writer
that restores metadata.

The parser checks the signature, chunk type syntax, chunk extents and CRCs,
first/single IHDR, valid IHDR methods and bit-depth/color-type combinations,
palette requirements, consecutive IDAT chunks and a final zero-length IEND with
no trailing bytes. Unknown critical chunks are rejected. Dimensions must fit
the existing 256 MiB image admission limit for both RGBA8 and declared sample
storage, although no pixel buffer is allocated. Basic APNG control sizes and
placement are checked.

PLTE is allowed once before IDAT, must contain 1–256 RGB entries (length divisible
by three), is forbidden for grayscale types 0/4, and cannot exceed the indexed
bit-depth palette capacity. Type 3 requires PLTE before IDAT. IDAT chunks must
be consecutive, and at least one is required before a zero-length final IEND.
acTL must occur once before IDAT, have eight bytes and declare a nonzero frame
count. fcTL requires acTL and exactly 26 bytes; fdAT requires acTL, preceding
IDAT and at least four bytes. Frame sequencing and payload validity are not
checked. Only these structural APNG conditions are implied by `animated`.
IHDR depth/type pairs are grayscale 0 at 1/2/4/8/16 bits, truecolor 2 at 8/16,
indexed 3 at 1/2/4/8, and alpha types 4/6 at 8/16. Compression/filter methods
must be zero and interlace zero or one. Dimensions must be positive signed-31-bit
values and fit the stated admission budget at both RGBA8 and declared depth.

This is an inventory tool, not a complete PNG/APNG validity checker. It does not
validate ancillary payload semantics, all ancillary ordering rules, animation
sequence numbers/frame counts, image compression or pixel values. A successful
inventory does not establish that an image decoder can render the file.

Resource cost is one operation-local encoded snapshot of at most 32 MiB, at most
4096 small chunk records and streaming JSON serialization. The snapshot is
released before writing stdout. Parsing is linear in encoded bytes and adds no
threads, resident timers, queues, caches, runtime or pixel textures. Read and
parse work use a cooperative 30-second deadline; a stalled operating-system
filesystem call or stdout consumer cannot be interrupted by that deadline. The
headless command does not run on GPUI's foreground executor.

## Verification

`apps/desktop/tests/png_metadata.rs` exercises the real executable with isolated
configuration/data/cache paths. Fixtures cover source preservation, secret
payload suppression, opaque compressed metadata, normal PNGs and animation
declarations, corrupt CRC/length/header/order/trailer data, chunk-count limits,
oversized sparse files, missing/non-file paths and Unix FIFO rejection. Unit
checks cover extent-limited reads, short input and pre-read deadline/size
rejection. Running these tests and recording optimized performance measurements
is part of the enclosing change's verification; these descriptions are not a
claim that a test or hardware acceptance gate has passed.

The structural checks follow the [W3C PNG specification](https://www.w3.org/TR/png-3/).
Full metadata inspection/editing, JPEG/WebP metadata and native editor integration
remain separate work.
