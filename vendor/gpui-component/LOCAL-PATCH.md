# Local GPUI Component dependency patches

Base: published `gpui-component` 0.9.1 from crates.io, commit
`39ab849a93b5f15507b969164d49891db8d5b470`, path `crates/component` in
<https://github.com/longbridge/gpui-kit>. The Apache-2.0 license and complete
published source, tests, locales and build script are retained. Cargo cache
metadata, `Cargo.toml.orig` and the packaged lockfile are omitted.

The unmaintained `instant` dependency is replaced by `web-time` 1.1.0, following
the migration recommended in the
[instant README](https://github.com/sebcrozet/instant#readme). That migration only
changes crate paths and the dependency entry, including optional speech code.
The obsolete `instant/wasm-bindgen` dependency feature is removed; all other
features and default behavior stay unchanged.

On native targets both libraries expose the exact `std::time::Instant` and
`std::time::Duration` types, preserving arithmetic, clock source, timer inputs
and native type identity. Browser WASM uses `web-time`'s `Performance.now()`
implementation and normal wasm-bindgen dependencies. It requires the browser
Performance API; no broader WASM platform support is claimed.

No runtime, timer, task, cache, worker or idle activity is added. `web-time` is
already in the application dependency graph. Remove this local patch when the
pinned upstream version includes the migration.

The unmaintained `paste` dependency is also replaced by `pastey` 0.2.3 under the
same `paste` dependency alias. Its two color helper macros remain byte-for-byte
unchanged; `pastey::paste!` supports their identifier generation. No feature or
generated API is intentionally changed. This is a build-time helper only and
adds no application runtime work. Remove this alias patch once upstream adopts
a maintained token-pasting crate.

The Windows native menu SVG dependency is updated from resvg 0.45.1 to 0.50,
matching the maintained GPUI SVG/font family. Existing menu construction and
image conversion stay unchanged. Source/native-target checks and rendering
fixtures are recorded separately in `docs/dependency-maintenance.md`.

## Chrono maintenance migration (2026-11-17)

Replace the unmaintained Chrono dependency with Jiff 0.2.43, default features
disabled (`std` plus browser `js` clock support). Calendar/date/time public Rust
types use Jiff civil types, and scheduler clocks use Jiff Timestamp. UI layout,
controls, events and the existing month-grid algorithms stay intact. GPUI's
on-demand local conversion uses native OS timezone/DST rules and a fixed-offset
Jiff value; it owns no timezone database/cache, refresh timer, worker or runtime.
The existing Apache-2.0 source/license and earlier local patches remain intact.

Activation, optimized regression tests, five native-target metadata checks and
default advisory-policy verification belong to the root integration gate; this
held source packet alone establishes none of those passing results.
