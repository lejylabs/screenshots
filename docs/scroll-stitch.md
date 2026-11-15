# Exact scroll-image assembly

Status: explicit file utility. This assembles existing images; it does not scroll
applications, inject input, capture screens or implement automatic scrolling
capture.

```sh
screenshots stitch-scroll OUTPUT MIN_OVERLAP INPUT INPUT [INPUT...]
```

Supply 2–16 PNG/JPEG/WebP inputs in top-to-bottom order and a minimum overlap of
1–4096 rows. Images must have equal widths and heights of 2–16,384 rows. For each
adjacent pair, the command finds an exact suffix of the earlier image matching
the prefix of the next. An overlap must be at least `MIN_OVERLAP` and less than
both image heights, so each input contributes new rows. Exactly one overlap must
match; no match, multiple matches, identical complete images or excess matching
work fail before any output is created. Repeating flat regions can therefore
cause an explicit ambiguity error. There is no guessed seam or fuzzy matching.

Matching compares raw decoded RGBA8 values after EXIF orientation. Different RGB
values under transparent pixels count as different, and lossy JPEG differences
can prevent a match. Fixed headers/footers, animations, scaling or horizontal
movement are not corrected. Row hashes select candidates only; every accepted
overlap is verified against all original bytes, so a hash collision cannot
approve a wrong seam. The output contains the first image followed by the
nonoverlapping rows of each next image. JSON reports width, height, input count
and the overlap row count for each adjacent pair.

PNG, JPEG or lossless WebP is selected by the output extension through the
existing atomic exporter. PNG/WebP preserve the assembled RGBA values; JPEG is
lossy and requires fully opaque pixels. Existing outputs, including broken
symlinks, are never replaced. All source bytes remain unchanged. Source paths
use native arguments without shell expansion. No history, clipboard, network,
application control or background activity is added.

## Resource contract

The existing decoder admits at most 32 MiB encoded bytes per file. Every next
decode uses the remaining combined 256 MiB source-pixel budget. Output dimensions
are checked against a separate 256 MiB RGBA budget before allocation. Source
buffers are released during assembly and all are gone before export. Retained
pixel buffers therefore peak at at most 512 MiB, plus decoder/encoder scratch
and bounded encoded input; this is not a total-process memory measurement.

Rows are hashed once per input and retained as at most 262,144 `u64` values
(2 MiB). The prefix-function workspace is at most 16,384 `usize` values
(128 KiB on a 64-bit target). Hash input bytes total at most the 256 MiB admitted
sources. Exact-byte comparisons, including checks for identical full images,
share a 256 MiB comparison budget; a request that would exceed it fails rather
than continuing unbounded matching. Candidate discovery is linear in row count,
with at most 15 adjacent seams. No dependencies, threads, timers, queues or
resident caches are added.

Execution is synchronous in the explicitly started headless process. Ctrl+C
terminates the process; decoding, hashing and exporting do not promise mid-call
cooperative cancellation. An interrupted export may leave the existing
unpublished `.part` artifact; successful publication uses the existing no-replace
contract. JSON output is explicitly flushed, and a broken output pipe after
publication reports that the assembled image was saved.

## Verification

Actual-executable fixtures use generated overlapping pixel patterns and check
exact output, multiple seams, alpha preservation, unchanged sources, invalid
counts/minimums/widths, no-match, ambiguity, identical frames, oversized input,
existing destinations and absent user-state creation. Optimized execution and
resource observations are recorded separately after running them. Native
scrolling behavior and real application capture are outside this utility's scope.
