# Local window teardown fix

Source: crates.io `gpui-pre-windows` 0.3.12, Apache-2.0. Only
`src/events.rs` differs from the upstream source. Unused package lock/original
manifest and Cargo cache metadata are omitted; the normalized manifest and license
are retained.

The WM_DESTROY handler sends the validated close notification synchronously,
so the platform removes the HWND from its tracked-window list before Windows
finishes destroying it. This preserves the existing application's teardown
safety fix when moving to the GPUI snapshot pinned by GPUI Kit. No worker,
cache or timer is added.

The notification uses the Windows 0.62.2 binding signature: a direct `HWND`
and optional `WPARAM`/`LPARAM` arguments.

Remove this override once the pinned upstream backend contains the equivalent
fix. Keep renderer-detach and repeated native-window lifecycle regressions.
