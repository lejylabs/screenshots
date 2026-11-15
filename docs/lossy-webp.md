# Explicit lossy WebP export

`screenshots image convert INPUT OUTPUT.webp --webp-quality 1..100` explicitly
selects lossy WebP. Omit the flag to keep the existing lossless WebP behavior.
PNG and JPEG defaults and other image commands are unchanged. Input uses the
shared PNG/JPEG/WebP and [truecolor BMP](bmp-input.md) loader and transformations; the final WebP contains only
encoded pixels, without copied source metadata. Quality is not a file-size
guarantee and quality 100 is still a lossy RGB encoding.

The dedicated core `save_lossy_webp` API accepts already prepared RGBA pixels,
an output path, quality and an operation-local cancellation callback. It must
run on a worker, never GPUI's foreground executor. Output dimensions must be
1–16383 pixels per side and the RGBA input is limited to 64 MiB. A 3840×2160
frame fits. The encoded output is capped at 32 MiB; exceeding the cap fails
rather than publishing a truncated result. No image is implicitly resized to
fit these limits.

The encoder uses lossy RGB with lossless alpha (`alpha_quality=100`), method 4,
the low-memory mode and one calling thread (`thread_level=0`). Transparent RGB
values are allowed to change through lossy compression; alpha values are
preserved. It does not add a background service, queue, timer or upload.

Encoded chunks go through one 64 KiB write buffer into the existing atomic
publisher's unique temporary file. There is no full encoded-output copy or
PNG intermediate. Success requires encoding, buffer flush, file sync and
no-overwrite publication; an existing output remains unchanged. Failure or
cancellation removes the temporary file, subject to filesystem cleanup
success. Source pixels and files are never modified.

Cancellation and the 30-second deadline are checked before/after pixel import,
in encoder progress/output callbacks, and before publication. These are
cooperative limits: libwebp allocations and pixel import, time between progress
callbacks, and blocking filesystem operations cannot be interrupted mid-call.
No hard 30-second process termination guarantee is made. Callback failures and
panics become fixed errors; no unwind crosses the C boundary. Rust's installed
panic hook may still report a custom callback panic before it is caught. The
CLI supplies a non-panicking callback and does not install a logging hook here.

The 64 MiB input cap, 32 MiB output cap and single operation are hard admission
bounds, not a total memory bound. libwebp imports/converts pixels into its own
YUV/alpha storage and allocates encoder scratch internally. Low-memory mode
reduces some scratch at a CPU cost; it does not provide a configurable allocator
ceiling. Optimized 4K latency and process-memory measurements remain separate
acceptance evidence, not inferred from these caps.

The pinned `libwebp-sys` 0.14.4 dependency statically builds bundled libwebp 1.6.0
with the C toolchain. Bindings are shipped; CMake and bindgen are not needed for
this default build. Its optional build parallelism, threading and forced SIMD
features are not enabled. The wrapper is MIT; bundled libwebp retains its
COPYING and PATENTS notices. Native platform builds and packaging still need
their own verification.

Core fixtures cover lossy VP8 output and alpha round trips, source preservation,
invalid admission, cancellation, encoded-byte limits, callback panic cleanup and
no overwrite. They use synthetic pixels and temporary files only.

An optimized development-machine run used one 3840×2160 opaque patterned image,
one warmup and five timed exports per format. Median encoding plus durable
publication was 42.07 ms for PNG (11,471,538 bytes), 30.08 ms for lossless WebP
(16,220,138 bytes), and 914.08 ms for lossy WebP quality 75 (1,639,422 bytes).
The fixture retained one 33,177,600-byte RGBA input; lossy output used the 65,536-byte
writer buffer. C scratch and process RSS were not measured in this run. These
numbers describe that synthetic image and machine, not reference-hardware,
quality-equivalence or physical UI responsiveness acceptance.
