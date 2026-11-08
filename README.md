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
- Capture buttons, hotkey and tray write PNGs under `target/captures/`, show a thumbnail, and automatically copy the last image to the clipboard with RGBA round-trip verification. This replaces the current clipboard contents.
- Bereich auswählen (Strg+Umschalt+F), directly below the window heading: hides existing overlays and the control window, captures each display, then opens a fullscreen PopUp showing that display's frozen image. Drag with the left mouse button; release saves the selection as `target/captures/selection-<uuid>.png`, restores the control window, and copies the selection. Escape cancels without changing the clipboard. Full preview captures remain saved on disk.
- Copy last capture + verify: copies that PNG as an image, reads it back, and checks dimensions and every RGBA pixel. This replaces the current clipboard contents.
- Escape or Close overlays dismisses overlay windows.
- On Windows, Ctrl+Shift+S captures the primary display even when another app has focus. Registration failures (for example, a shortcut already in use) appear in the control window. The capture is saved and copied automatically.
- On Windows, right-click the blue camera icon in the system tray (possibly under hidden icons): Capture primary, Show control window, or Quit. Capture uses the same save-and-copy path as the buttons and hotkey. Closing the control window still exits the spike; it does not minimize to the tray.

The clipboard button and its result now appear near the top, above the diagnostic lists and thumbnail. Closing the main window or pressing Ctrl+Q closes all GPUI windows through the same cleanup path. Native WM_CLOSE is intercepted so Windows does not destroy the main window ahead of GPUI cleanup. Interactive shutdown still needs verification.

Clipboard verification runs on a background worker. The clipboard owner stays alive while the app runs. Paste into another application still needs a manual check.

On Windows, all capture paths hide the control window with native `ShowWindow(SW_HIDE)` and verify its visibility. The capture worker calls `DwmFlush` before XCap so the path no longer relies on a 300ms minimize-animation delay. Failures stop the capture and restore the control window. Normal captures restore it after success; selection overlays keep it hidden until dismissed. GPUI's app-level `hide()` is a no-op on Windows, so it is not used. The explicit compositor synchronization test passes in the agent environment; interactive visibility and image cleanliness still need verification. Other platforms retain the unvalidated minimize-and-delay prototype until their native integration is tested.

Region selection crops the frozen PNG, not a new live capture. Window-local coordinates are scaled to that snapshot's pixel dimensions, with floor/ceil bounds and clipping at the image edges. Dragging in either direction works; a click with no area keeps the overlay open. Selection is limited to one monitor. Dragging across monitors is not supported; if a release is missed outside the originating window, returning without the left button held cancels that drag. Interactive selection, physical DPI calibration, and edge alignment remain to be checked on real hardware.

On Windows, dismissing an overlay now hides its native window and clears the drag/snapshot state. The next invocation reuses the window for the same display and overlay mode. This avoids repeatedly destroying GPUI windows after each selection; native destruction still occurs when quitting. A capture stops if an existing overlay could not be hidden. The diagnostic `overlay geometry after presentation` logs bounds and viewport after two frames, instead of assuming the initial creation bounds are the final fullscreen position. Repeated selection, Escape, and final shutdown still require interactive verification with this lifecycle change.

Hardware spikes can be run explicitly:

```bash
cargo test -p app-core png_round_trip -- --ignored --nocapture
cargo test -p app-core captures_primary_monitor_to_png -- --ignored --nocapture
cargo test -p app-core registration_is_released -- --ignored --nocapture
cargo test -p app-core tray_creation_and_cleanup -- --ignored --nocapture
cargo test -p app-core synchronizes_desktop_compositor -- --ignored --nocapture
```

Windows interactive capture, clipboard paste, global hotkey and tray are confirmed in the session. Phase 0 remains open: fullscreen frozen-image coverage (including taskbar), hiding the control window during capture, other operating systems, and multi-monitor/DPI checks. The 16px inset is removed: overlays request native fullscreen bounds for each display. Snapshot matching currently uses the equal GPUI/XCap display IDs observed on Windows; it has not been validated on other platforms. Quit actions defer window removal until action dispatch has finished; remaining shutdown diagnostics need another interactive check.

Windows session check (2026-11-08): the clipboard round trip passed and the desktop build passed. The live XCap test failed in the agent test environment with `0x80070006` (invalid handle); interactive capture had worked earlier in this session. Both hardware tests require explicit invocation.

The tray smoke test constructs the icon successfully, but tray-icon prints `Error removing system tray icon` on drop in the agent test environment. This test does not establish successful Shell cleanup; verify menu actions and removal in the interactive Windows session.
