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

The published crate archive omits the repository-root license file. The exact
upstream Apache license/copyright notice is included unchanged as LICENSE-APACHE
from commit 39ab849a93b5f15507b969164d49891db8d5b470 (the archive's VCS identity):
<https://github.com/longbridge/gpui-kit/blob/39ab849a93b5f15507b969164d49891db8d5b470/LICENSE-APACHE>.
Git blob: `1e9e81e7dc6d9c0a7490c9c5debd67697284ee25`; SHA-256:
`d1b0449e5478c574ba4f686c2656df7fe77d66821a61f8b6ed3378a58ed9a811`.
This adds 10,305 bytes of license text, no code/runtime resource. The original
55-file archive-derived snapshot/cost remains recorded separately.
