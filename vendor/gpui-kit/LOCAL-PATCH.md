# Local date API maintenance patch

Published gpui-kit 0.9.1, original Apache-2.0 source and license retained.
Archive: <https://static.crates.io/crates/gpui-kit/gpui-kit-0.9.1.crate>
Archive SHA-256: `aca1dedfea73645b60b5893c9c16de0b1396f2cf4251d19e5f03a295c1b61879`.

The one date-picker integration-test helper uses Jiff civil DateTime and the
dev dependency changes accordingly. Normal Kit dependencies, backend features,
UI structure and asset handling stay intact. Vendoring this crate keeps the
existing affected integration tests type-compatible with the maintained Base/
Component civil types. Kit-assets has no direct Chrono edge and is not copied.

The registry package's original manifest/lock and source are preserved in this
packet's original source snapshot. Packaged Cargo.lock/Cargo.toml.orig under
this vendor directory are historical upstream input, not the application's
resolved graph. Root owns final lockfile resolution, advisory and test evidence.
