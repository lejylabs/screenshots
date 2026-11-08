# screenshots

Open-source desktop capture and sharing application.

## Workspace

```text
apps/desktop      GPUI binary
crates/app-ui     Windows and views
crates/app-core   Application logic, no GPUI
crates/shared     Shared types
```

## Build

Requires Rust 1.99.

```bash
cargo build -p screenshots
cargo run -p screenshots
```

The first run is the Phase 0 spike: overlay PopUps plus XCap still capture.

- Open small overlay: red card at the top-left of each display.
- Every successful capture writes a PNG under `target/captures/`, shows a thumbnail, and automatically copies the last image to the clipboard with RGBA round-trip verification. This replaces the current clipboard contents.
- Copy last capture + verify: copies that PNG as an image, reads it back, and checks dimensions and every RGBA pixel. This replaces the current clipboard contents.
- Escape or Close overlays dismisses overlay windows.
- On Windows, Ctrl+Shift+S captures the primary display even when another app has focus. Registration failures (for example, a shortcut already in use) appear in the control window. The capture is saved and copied automatically.
- On Windows, right-click the blue camera icon in the system tray (possibly under hidden icons): Capture primary, Show control window, or Quit. Capture uses the same save-and-copy path as the buttons and hotkey. Closing the control window still exits the spike; it does not minimize to the tray.

The clipboard button and its result now appear near the top, above the diagnostic lists and thumbnail. Closing the main window or pressing Ctrl+Q closes all GPUI windows through the same cleanup path. Native WM_CLOSE is intercepted so Windows does not destroy the main window ahead of GPUI cleanup. Interactive shutdown still needs verification.

Clipboard verification runs on a background worker. The clipboard owner stays alive while the app runs. Paste into another application still needs a manual check.

Hardware spikes can be run explicitly:

```bash
cargo test -p app-core png_round_trip -- --ignored --nocapture
cargo test -p app-core captures_primary_monitor_to_png -- --ignored --nocapture
cargo test -p app-core registration_is_released -- --ignored --nocapture
cargo test -p app-core tray_creation_and_cleanup -- --ignored --nocapture
```

Windows interactive capture, clipboard paste, and global hotkey are confirmed in the session. Phase 0 remains open: interactive tray verification, other operating systems, and multi-monitor/DPI checks are pending. The current fullscreen overlay has a 16px inset; it does not yet validate edge-to-edge coverage. Quit actions now defer window removal until action dispatch has finished, avoiding an update of the currently borrowed window; the remaining shutdown diagnostics need another interactive check.

Windows session check (2026-11-08): the clipboard round trip passed and the desktop build passed. The live XCap test failed in the agent test environment with `0x80070006` (invalid handle); interactive capture had worked earlier in this session. Both hardware tests require explicit invocation.

The tray smoke test constructs the icon successfully, but tray-icon prints `Error removing system tray icon` on drop in the agent test environment. This test does not establish successful Shell cleanup; verify menu actions and removal in the interactive Windows session.
