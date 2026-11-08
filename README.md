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

See [Phase 0 evidence and remaining checks](docs/phase-0.md).

- Open small overlay: red card at the top-left of each display.
- Fenster aufnehmen: click a listed window near the top of the control window to capture it directly, save it and copy it. Use Fensterliste aktualisieren after opening or closing windows. Minimized, untitled and own-process windows are excluded from the picker. A window that closes before capture produces an error and restores the control window.
- Capture buttons, hotkey and tray write PNGs under `target/captures/`, show a thumbnail, and automatically copy the last image to the clipboard directly from shared RGBA pixels. This replaces the current clipboard contents.
- Bereich auswählen (Strg+Umschalt+F), directly below the window heading: hides existing overlays and the control window, captures each display, then opens a fullscreen PopUp showing that display's frozen image. Drag with the left mouse button; release saves the selection as `target/captures/selection-<uuid>.png`, restores the control window, and copies the selection. Escape cancels without changing the clipboard. Canceled previews are never written to disk.
- Copy last capture: uses live pixels if still retained, otherwise loads the successfully saved PNG on demand. This replaces the current clipboard contents.
- Escape or Close overlays dismisses overlay windows.
- On Windows, Ctrl+Shift+S captures the primary display even when another app has focus. Registration failures (for example, a shortcut already in use) appear in the control window. The capture is saved and copied automatically.
- On Windows, Ctrl+Shift+R starts region selection even when another app has focus. Ctrl+Shift+F remains a local shortcut in the control window. Each global shortcut registers independently: a conflict with one does not disable the other. The status line reports unavailable keys.
- On Windows, right-click the blue camera icon in the system tray (possibly under hidden icons): Bereich auswählen, Capture primary, Show control window, or Quit. Region selection uses the same frozen-image workflow as the button. Capture primary uses the same save-and-copy path as the buttons and hotkey. Closing the control window still exits the spike; it does not minimize to the tray.

The clipboard button and its result now appear near the top, above the diagnostic lists and thumbnail. Closing the main window or pressing Ctrl+Q closes all GPUI windows through the same cleanup path. Native WM_CLOSE is intercepted so Windows does not destroy the main window ahead of GPUI cleanup. Interactive shutdown still needs verification.

Clipboard copying runs on a background worker; pixel round-trip verification runs only in the explicit test. The clipboard owner stays alive while the app runs. Windows paste was confirmed in the interactive session; other platforms still need a paste check.

On Windows, all capture paths request native hiding with `ShowWindowAsync(SW_HIDE)`. Visibility changes are queued to avoid re-entering borrowed GPUI state. The capture worker checks that the control window and cached overlays actually become hidden (up to two seconds per window), then calls `DwmFlush` before XCap. This replaces the 300ms minimize-animation delay. Failures stop the capture and restore the control window. Normal captures restore it after success; selection overlays keep it hidden until dismissed. GPUI's app-level `hide()` is a no-op on Windows, so it is not used. The explicit compositor synchronization test passes in the agent environment; interactive visibility and image cleanliness still need verification. Other platforms retain the unvalidated minimize-and-delay prototype until their native integration is tested.

Region selection crops the frozen PNG, not a new live capture. Window-local coordinates are scaled to that snapshot's pixel dimensions, with floor/ceil bounds and clipping at the image edges. Dragging in either direction works; a click with no area keeps the overlay open. Selection is limited to one monitor. Dragging across monitors is not supported; if a release is missed outside the originating window, returning without the left button held cancels that drag. Interactive selection, physical DPI calibration, and edge alignment remain to be checked on real hardware.

On Windows, dismissing an overlay hides its native window and clears the drag/snapshot state. The next invocation reuses the window for the same display and overlay mode. This avoids repeatedly destroying GPUI windows after each selection; native destruction still occurs when quitting. A capture stops if an existing overlay could not be hidden. The diagnostic `overlay geometry after presentation` logs bounds and viewport after two frames; the interactive single-display run confirmed (0,0), 2560×1440. The explicit GPUI test covers repeated cancellation, reuse, restoration and normal shutdown. Physical Escape input and other display setups remain to be checked.

Hardware spikes can be run explicitly:

```bash
cargo test -p app-core png_round_trip -- --ignored --nocapture
cargo test -p app-core captures_primary_monitor_to_png -- --ignored --nocapture
cargo test -p app-core registration_is_released -- --ignored --nocapture
cargo test -p app-core tray_creation_and_cleanup -- --ignored --nocapture
cargo test -p app-core synchronizes_desktop_compositor -- --ignored --nocapture
cargo test -p app-core visibility_requests_do_not_reenter_and_worker_observes_hide -- --ignored --nocapture
cargo test -p app-ui repeated_region_cancel_preserves_clipboard_and_reuses_windows -- --ignored --nocapture --test-threads=1
```

The GPUI repeat/cancel test opens real overlays twice and checks that no preview PNGs are added under `target/captures/region-cancel-test/`. Run it with no other instance of screenshots open and leave the clipboard alone while it runs. It checks overlay reuse, cancellation cleanup, control-window restoration, unchanged clipboard sequence, and returns through normal shutdown. Error logging is enabled for the test. The explicit Windows run passed without logged errors; it invokes the same cancellation handler as Escape but does not inject a physical keypress.

Windows interactive capture, clipboard paste, global hotkey and tray are confirmed in the session. Phase 0 remains open: fullscreen frozen-image coverage (including taskbar), hiding the control window during capture, other operating systems, and multi-monitor/DPI checks. The 16px inset is removed: overlays request native fullscreen bounds for each display. Snapshot matching currently uses the equal GPUI/XCap display IDs observed on Windows; it has not been validated on other platforms. Quit actions request GPUI's normal shutdown after action dispatch; remaining shutdown diagnostics need another interactive check.

Windows session check (2026-11-08): the clipboard round trip passed and the desktop build passed. The live XCap test failed in the agent test environment with `0x80070006` (invalid handle); interactive capture had worked earlier in this session. Both hardware tests require explicit invocation.

The tray smoke test constructs the icon successfully, but tray-icon prints `Error removing system tray icon` on drop in the agent test environment. This test does not establish successful Shell cleanup; verify menu actions and removal in the interactive Windows session.

Capture, crop, overlay and clipboard now use in-memory pixels. Cloning a capture shares its immutable RGBA buffer; GPUI gets one worker-prepared BGRA render copy. Only finished normal/window/region captures call PNG export. Copy now completes before PNG encoding; clipboard and save outcomes are shown independently. Successful save releases the full-resolution source; explicit recopy loads the saved file on demand. Release-mode resource acceptance remains open.

Quitting while copying/saving waits for the current output job; new captures are blocked. Save failure keeps the app open and visible with the error. Late capture results do not reopen overlays during quit. Explicit test: cargo test -p app-ui quit_waits_for_copy_and_png_save -- --ignored --nocapture --test-threads=1 (replaces clipboard contents with a small test image).

PNG exports now use an exclusive .part file beside the destination, then publish only after encoding/flush succeeds. Existing destinations are never replaced. Windows uses native no-replace move; non-Windows exports currently require hard-link support. Normal failures clean up temporary files; crashes may leave .part files pending a future recovery pass.

Display/window lists load in the background at startup and on manual refresh; repeated refresh requests coalesce while one is active. Captures no longer refresh diagnostic metadata automatically. Use Fensterliste aktualisieren for newly opened/closed windows; monitor-hotplug refresh is not yet automatic.

Release idle measurement (Windows PowerShell 5.1):

```powershell
cargo build --release -p screenshots
powershell -NoProfile -File scripts/measure-idle.ps1 -Seconds 30
```

Close other screenshots instances first and do not trigger captures or menu actions during sampling. The probe owns only the process it starts, hides its control window, waits five seconds, then samples CPU time/Private Bytes/working set once per second and closes the process normally. Results, binary hash and hardware/source context are written to `target/performance/idle-normal.json`; logs are beside it. CPU is expressed as a percentage of one logical core. Use `-Seconds 300` for the planned five-minute idle duration. A short run on the development PC is exploratory, not a reference-hardware acceptance pass; GPU resources, wakeups, energy and post-capture retention need separate measurement. The script never force-kills an unresponsive process.

Resource attribution probes use the same executable without the normal app services:

```powershell
powershell -NoProfile -File scripts/measure-idle.ps1 -Seconds 30 -Mode headless
powershell -NoProfile -File scripts/measure-idle.ps1 -Seconds 30 -Mode empty
powershell -NoProfile -File scripts/measure-idle.ps1 -Seconds 30 -Mode normal
```

Headless explicitly skips attaching the GPU/window system and creates no tray/hotkeys/capture shell; empty attaches GPUI and creates an empty control-sized window but no app services. These are diagnostics, not fully functional alternative application modes. Each probe exits on a bounded one-shot diagnostic deadline. Windowed modes get one second to render before hiding; all modes then warm for five seconds before sampling. Reports are `target/performance/idle-<mode>.json`, with thread/handle counts and the binary hash. Run modes sequentially; differing first-frame/hide timing and driver warmup can affect results, so do not attribute all differences to one allocation source.

Normal output thumbnails are now at most 400x240 BGRA. Replacing thumbnails and dismissing overlays explicitly removes their image atlas entries. This does not prove the renderer/driver releases atlas backing pages; GPU memory must still be measured. Successful save releases the retained source RGBA buffer; recopy explicitly reloads the saved file on a worker with decode/RGBA limits. A failed save retains pixels so the unsaved image remains available. This on-demand recopy is separate from the capture -> selection -> clipboard pipeline, which remains in memory.
