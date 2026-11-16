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
