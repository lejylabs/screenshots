# Local date API maintenance patch

Published gpui-pre-scheduler 0.3.12, original Apache-2.0 source and license retained.
Archive: <https://static.crates.io/crates/gpui-pre-scheduler/gpui-pre-scheduler-0.3.12.crate>
Archive SHA-256: `b1d3fa97eeb4ea233635b756684affe96db9e29fc2af8cdc269268bd7ff649fc`.

Replace the Chrono clock dependency and `Clock::utc_now`/`TestClock` civil
instant types with Jiff Timestamp. The test seed keeps the same UTC instant.
Monotonic clock selection, dedicated threads, native event scheduling and timer
implementation remain unchanged. Jiff default features are disabled; `std` and
browser `js` clock support add no timezone database/cache or worker.

The registry package's original manifest/lock and source are preserved in this
packet's original source snapshot. Packaged Cargo.lock/Cargo.toml.orig under
this vendor directory are historical upstream input, not the application's
resolved graph. Root owns final lockfile resolution, advisory and test evidence.
