# Bounded WebP file admission

The direct image loader accepts encoded files up to 32 MiB and decoded RGBA up
to the caller's admitted pixel budget, at most 256 MiB. PNG/JPEG retain their
existing streaming/extent-limited behavior. Direct WebP input now takes one
operation-scoped encoded snapshot before validating or decoding it. This adds at
most 32 MiB of encoded RAM, counted in the active-image scratch allowance. The
snapshot is released with the decoder; no disk handoff, persistent buffer,
thread, timer, cache or dependency is introduced.

The pinned `image` 0.25.10 wrapper does not forward its allocation limits to
`image-webp` 0.2.4 metadata reads. The decoder can allocate from a declared EXIF
length before discovering a truncated input. It also decodes embedded VP8
dimensions before comparing them with a VP8X canvas or animation-frame size.
A small advertised canvas therefore cannot serve as the only admission check.

Before constructing that decoder, the loader validates:

- The RIFF/WEBP signature and exact RIFF extent. Truncated data and bytes trailing
  outside the declared RIFF extent are rejected.
- Every top-level and ANMF subchunk's length and padding against the immutable
  snapshot, with at most 4,096 chunks in total and no recursive traversal.
- The VP8 keyframe signature and embedded dimensions, or VP8L signature/version
  and embedded dimensions, against the caller's RGBA limit.
- VP8X canvas dimensions and agreement with its still-image bitstream.
- Every animation-frame rectangle against its canvas, and its bitstream against
  the frame dimensions. ANMF accepts one VP8/VP8L chunk, or ALPH followed by VP8.

The first EXIF chunk's orientation is parsed directly from its admitted borrowed
slice. This preserves orientation without invoking the decoder's allocating
metadata method or retaining a second encoded-size metadata buffer. Existing
PNG/JPEG orientation handling is unchanged. Valid lossy, lossless, alpha and
animated WebP inputs remain supported; animated input still opens the decoder's
first still frame. No animation editing is implied.

Validation and decoding use the same snapshot, so source growth or in-place
modification cannot change the validated chunk lengths/dimensions during decode.
This does not claim a hard bound on every internal decoder scratch allocation,
native backend allocation or physical process peak. Those remain part of the
existing resource acceptance work.

Fixtures cover tiny malicious EXIF extents, oversized/mismatched embedded VP8
dimensions, excessive/truncated chunks, remaining pixel budgets, lossless alpha,
lossy alpha, EXIF rotation, animated still pixels and unchanged source bytes.
Commands and measured test outcomes belong in the delivery evidence; source
fixtures alone do not establish native or performance acceptance.
