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

The Linux ashpd requirement is also updated from 0.13 to 0.14 for the maintained
zbus 6 family. The selected portal settings feature and async-io compatibility
feature remain enabled. This native dependency migration has separate runtime
and acceptance evidence in `docs/dependency-maintenance.md`; the no-runtime-cost
statement above applies to the time-crate replacement.

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
