# Third-party notices

## Rust Windows batch argument serialization

`crates/app-core/src/windows_child_batch.rs` adapts the Windows batch argument
serialization in Rust 1.100.0's standard library under its MIT license option:
https://github.com/rust-lang/rust/blob/1.100.0/library/std/src/sys/args/windows.rs

Copyright (c) The Rust Project Contributors

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies
of the Software, and to permit persons to whom the Software is furnished to do
so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

The vendored GPUI core 0.3.12 and cosmic-text 0.19.0 retain their upstream
licenses. Their maintained SVG/font dependency migration and glyph-coverage
adapter are documented in `vendor/gpui-pre/LOCAL-PATCH.md` and
`vendor/cosmic-text/LOCAL-PATCH.md`. The GPUI Component Windows menu uses the
same upstream resvg family; no application layout/design was changed.

The narrowly vendored GPUI Linux 0.3.12, AccessKit Unix 0.22.1,
AccessKit AT-SPI common 0.19.1, AT-SPI 0.29.0/common and proxies 0.13.0,
and zbus-lockstep/macros 0.5.2 retain their published versions and original
MIT and/or Apache license files. Their `LOCAL-PATCH.md` files describe the
maintained zbus 6 consumer migration. The active application graph removes the
standalone abandoned wire/name crates; this does not replace their source with
renamed copies. The lockfile retains upstream dormant optional backends.

The specialized cancellable history-thumbnail reduction is derived from the
image crate 0.25.10 `src/imageops/sample.rs` and `src/math/utils.rs`, under its
MIT license option. Its dimension, sampling and channel-rounding behavior is
kept compatible with that version. The upstream MIT notice follows:

MIT License

Permission is hereby granted, free of charge, to any
person obtaining a copy of this software and associated
documentation files (the "Software"), to deal in the
Software without restriction, including without
limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of
the Software, and to permit persons to whom the Software
is furnished to do so, subject to the following
conditions:

The above copyright notice and this permission notice
shall be included in all copies or substantial portions
of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF
ANY KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED
TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A
PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT
SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY
CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION
OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR
IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.

---

Explicit lossy WebP export uses libwebp 1.6.0 through libwebp-sys 0.14.4.
The archive includes Google's BSD license and patent grant as
`licenses/libwebp-sys-0.14.4/COPYING` and `PATENTS`, together with dependency
metadata. The full source notices reside in the crate's `vendor` directory.

The application uses GPUI Kit (Apache-2.0) and the matching GPUI snapshot
(Apache-2.0). Their source, license and provenance are available through Cargo.lock:
https://github.com/longbridge/gpui-kit and https://github.com/zed-industries/zed.
The vendored Windows backend retains its upstream Apache-2.0 license.

The vendored glib 0.18.5 retains its upstream MIT license in
`vendor/glib/LICENSE` and copyright notice in `vendor/glib/COPYRIGHT`.
Its two-line VariantStrIter safety backport and upstream commit are documented
in `vendor/glib/LOCAL-PATCH.md`; the package version is unchanged.

The vendored gpui-pre-reqwest 0.12.15 keeps its MIT and Apache-2.0 licenses.
Its PEM-parser dependency migration is documented in
`vendor/gpui-pre-reqwest/LOCAL-PATCH.md`.

The vendored GPUI Base and GPUI Component 0.9.1 crates retain their Apache-2.0
licenses. Their `instant` to `web-time` dependency migration is documented in
each crate's `LOCAL-PATCH.md`.

The vendored rav1e 0.8.1 keeps its upstream BSD license and PATENTS file. Its
compile-time token-pasting dependency alias is documented in
`vendor/rav1e/LOCAL-PATCH.md`; encoder/assembly sources are unchanged.

The vendored GLib macros 0.18.5 and GTK3 macros 0.18.2 retain their MIT licenses
and copyright notices. Their compile-time diagnostic-helper migration is
documented in their respective `LOCAL-PATCH.md` files.

The selected embedded UI icons are provided by Lucide. Their notices follow.

ISC License

Copyright (c) 2026 Lucide Icons and Contributors

Permission to use, copy, modify, and/or distribute this software for any
purpose with or without fee is hereby granted, provided that the above
copyright notice and this permission notice appear in all copies.

THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR
ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN
ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF
OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.

---

The following Lucide icons are derived from the Feather project:

airplay, alert-circle, alert-octagon, alert-triangle, aperture, arrow-down-circle, arrow-down-left, arrow-down-right, arrow-down, arrow-left-circle, arrow-left, arrow-right-circle, arrow-right, arrow-up-circle, arrow-up-left, arrow-up-right, arrow-up, at-sign, calendar, cast, check, chevron-down, chevron-left, chevron-right, chevron-up, chevrons-down, chevrons-left, chevrons-right, chevrons-up, circle, clipboard, clock, code, columns, command, compass, corner-down-left, corner-down-right, corner-left-down, corner-left-up, corner-right-down, corner-right-up, corner-up-left, corner-up-right, crosshair, database, divide-circle, divide-square, dollar-sign, download, external-link, feather, frown, hash, headphones, help-circle, info, italic, key, layout, life-buoy, link-2, link, loader, lock, log-in, log-out, maximize, meh, minimize, minimize-2, minus-circle, minus-square, minus, monitor, moon, more-horizontal, more-vertical, move, music, navigation-2, navigation, octagon, pause-circle, percent, plus-circle, plus-square, plus, power, radio, rss, search, server, share, shopping-bag, sidebar, smartphone, smile, square, table-2, tablet, target, terminal, trash-2, trash, triangle, tv, type, upload, x-circle, x-octagon, x-square, x, zoom-in, zoom-out

The MIT License (MIT) (for the icons listed above)

Copyright (c) 2013-present Cole Bemis

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.

The vendored HarfRust 0.15.0 retains its upstream MIT license and published
version in `vendor/harfrust/LICENSE`. Its observational preprocessing-budget
exhaustion patch, opt-in legacy-kern admission bounds and exact upstream
provenance are documented in `vendor/harfrust/LOCAL-PATCH.md` and
`vendor/harfrust/KERN-BOUND.md`.
Existing upstream shaping defaults remain unchanged; the editor refuses
exhausted preprocessing or legacy caches without a bounded admission proof.

The vendored moxcms 0.8.1 retains its published BSD-3-Clause OR Apache-2.0
licenses in `vendor/moxcms/LICENSE.md` and `LICENSE-APACHE.md`. The local patch
changes two equality predicates so profiles with different channel transfer
curves use the existing per-channel converter. The original crate checksum,
upstream revision and byte inventory are in `vendor/moxcms/LOCAL-PATCH.md` and
`UPSTREAM-SHA256.txt`. No crate version, runtime or color-profile restriction is
added by this correction.
