# Local GPUI Component time dependency patch

Base: published `gpui-component` 0.9.1 from crates.io, commit
`39ab849a93b5f15507b969164d49891db8d5b470`, path `crates/component` in
<https://github.com/longbridge/gpui-kit>. The Apache-2.0 license and complete
published source, tests, locales and build script are retained. Cargo cache
metadata, `Cargo.toml.orig` and the packaged lockfile are omitted.

The unmaintained `instant` dependency is replaced by `web-time` 1.1.0, following
the migration recommended in the
[instant README](https://github.com/sebcrozet/instant#readme). Only crate paths
and that dependency entry change, including paths inside optional speech code.
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
