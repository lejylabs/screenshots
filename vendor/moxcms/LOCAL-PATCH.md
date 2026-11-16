# Preserve independent RGB transfer curves

This is the exact published `moxcms 0.8.1` crate from crates.io, except for the two
predicate replacements described below and these local provenance files.

- Upstream: https://github.com/awxkee/moxcms
- Published VCS revision: `c4affa196727d17f21a7f6f148a558867decb470`, repository root.
- Original archive SHA-256 / Cargo registry checksum:
  `bb85c154ba489f01b25c0d36ae69a87e4a1c73a72631fc6c0eb6dde34a73e44b`.
- Published archive: 188,252 bytes; 127 regular files, 1,553,179 uncompressed bytes.
- Licenses retained verbatim: `LICENSE.md` (BSD-3-Clause) and
  `LICENSE-APACHE.md` (Apache-2.0); published manifest keeps their OR expression.
- `UPSTREAM-SHA256.txt` records each original published file's hash. The archive
  contains no CRLF files. Root `.gitattributes` preserves the copied bytes on
  every checkout platform. The registry cache was never modified.

`ColorProfile::are_all_trc_the_same` in `src/trc.rs` used
`red == green || green == blue`, separately for tabulated and parametric curves.
That predicate wrongly accepts a profile where only two neighboring channels
share a curve. `src/transform.rs` then selects an optimized converter that uses
red's table for all three channels. The patch changes exactly these two `||`
operators to `&&`. Ordinary per-channel profiles remain supported. Existing
all-equal fast paths, formats, precision, intent, feature flags and public APIs
remain unchanged. No new converter, runtime, dependency version, thread or cache
is added. Root Cargo excludes this package from the four-member workspace and
patches both the direct and image crate uses to this one source.

For sRGB primaries with red/green gamma1 and blue gamma2, RGB128 should produce
about sRGB `[188,188,137]`; the faulty common-red path produces blue188 instead.
The analogous green/blue-equal case also failed. Red/blue-equal and all-equal
profiles are controls for selecting the existing paths correctly.

The corrected float converter can use its existing three 65536-entry source and
three 65536-entry destination tables, 1.5 MiB of f32 table data. The
all-equal path uses one of each, 512 KiB. The extra 1 MiB for independently
encoded channels is required conversion data and fits the existing 4 MiB color
scratch reserve. There are no full-image float buffers or retained tables after
an import job. Actual private allocations/performance remain subject to the
application's measurement and codec-private acceptance limitations.

Independent application fixtures live in
`crates/app-core/tests/fixtures/color/channels/`. Their generator,
`scripts/generate-color-channel-oracle.py`, uses LittleCMS 2.19 to create and
reopen the exact serialized ICC profiles before computing RGB16 -> sRGB8
oracles. Both ICC parametric function0 and 257-entry `curv` profiles cover
`r=g!=b`, `r!=g=b`, `r=b!=g`, and all-equal gamma1/gamma2 controls. Application
regressions also retain straight alpha, hidden RGB, 16-bit source admission and
all EXIF orientations. No moxcms function computes expected values.

This source checkpoint has not been compiled or tested by the vendor author.
The root agent owns Cargo lock resolution, application/Windows checks and
optimized numerical/resource verification. The unrelated upstream test suite is
not claimed run.
