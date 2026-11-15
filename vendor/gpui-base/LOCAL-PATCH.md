# Local GPUI Base time dependency patch

Base: published `gpui-base` 0.9.1 from crates.io, commit
`39ab849a93b5f15507b969164d49891db8d5b470`, path `crates/base` in
<https://github.com/longbridge/gpui-kit>. The Apache-2.0 license and complete
published source, tests and benches are retained. Cargo cache metadata,
`Cargo.toml.orig` and the packaged lockfile are omitted.

The unmaintained `instant` dependency is removed. Its crate paths now use the
existing `web-time` dependency, with minimum version 1.1.0. The obsolete
`instant/wasm-bindgen` dependency feature is removed; other features and APIs are
unchanged. This follows the migration recommended in the
[instant README](https://github.com/sebcrozet/instant#readme).

On native targets both libraries expose the exact `std::time::Instant` and
`std::time::Duration` types. Existing arithmetic, clock source and timer inputs
therefore keep their native type identity and behavior. Browser WASM uses
`web-time`'s `Performance.now()` implementation and its normal wasm-bindgen
dependencies. It requires the browser Performance API; no broader WASM platform
support is claimed.

No new runtime, timer, task, cache, worker or idle activity is introduced.
`web-time` was already present in this crate and the application graph. Remove
the local patch when the pinned upstream version includes this migration.
