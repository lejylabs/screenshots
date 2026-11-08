# Local patch to gpui-windows-gpui-unofficial 1.27.0

Source: crates.io package `gpui-windows-gpui-unofficial` 1.27.0, Apache-2.0 (see LICENSE-APACHE).

Only `src/events.rs` differs from upstream code: WM_DESTROY synchronously sends the validated platform close notification instead of posting it. The handler only removes the HWND from the tracked-window list under its write lock. The VSync thread uses the same list under a read lock. This removes tracking before Windows finishes destroying the HWND, avoiding redraw requests against a destroyed handle. It adds no worker, cache or idle timer.

Regression: `cargo test --release -p app-ui resident_control_render_latency -- --ignored --nocapture --test-threads=1`, plus `resident_capture_reopen_and_graphics_detach`. The former fails on logged ERROR events. Remove this override when the pinned upstream backend contains an equivalent fix. Do not modify the Cargo registry copy.
