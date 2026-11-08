# Phase 0 spike status — 2026-11-08

Phase 0 is open. The Windows prototype is usable; the cross-platform exit gate has not passed.

Performance rework: native hotkey/menu callbacks now feed a bounded 32-event async channel instead of two 50ms polling loops. The receiver sleeps until an event arrives and does not need a Tokio runtime. Queue overflow ignores additional actions with a warning; callbacks never block waiting for capacity. Build, routing/bounded-queue tests and the explicit GPUI repeat/cancel test pass. Native hotkey/tray interaction and release-mode CPU/wakeup measurements remain to be checked; this is not a measured idle-budget pass. File-based frame transfers and clipboard read-back remain prototype debt.

## Evaluated versions

- `gpui-unofficial` and `gpui-platform-gpui-unofficial`: exactly 1.27.0, the named Zed v1.27.0 snapshot. Cargo uses exact requirements because Cargo.lock is intentionally not versioned in this repository.
- XCap: exactly 0.9.8.
- arboard 3.7.0, global-hotkey 0.9.0, tray-icon 0.26.1 in the current local build; their Cargo ranges are not exact pins.

## Evidence on Windows

The interactive session reports display 65537, MPG271QX OLED, 2560×1440, scale 1.0 and origin (0,0). GPUI and XCap report the same display ID on this machine.

- PopUp overlays remained visible until dismissed.
- Full-display captures succeeded in approximately 22–25ms; 400×300 captures in approximately 4ms. These log timings cover the native capture, not PNG encoding or clipboard work.
- The user confirmed clipboard paste, global primary-display capture and the initial tray menu.
- Drag selection saved 782×521 and 937×716 PNGs and passed RGBA clipboard round trips in interactive logs.
- Direct window capture saved window 591508 at 1108×628 in 11ms and passed the clipboard RGBA round trip. That run also logged `RefCell already borrowed` on restoration and `window not found` on exit.
- Ordinary tests cover coordinate conversion, reversed drags, clipping, snapshot cropping, invalid native/window handles and hotkey event routing. Explicit hotkey registration/release and DwmFlush tests passed in the agent environment.
- A native visibility regression test creates an off-screen window, requests show/hide, checks that WM_SHOWWINDOW does not synchronously re-enter the request, pumps the UI message queue while a worker waits for hiding, and destroys the window. It passed on this Windows environment. This test does not exercise GPUI's own state or final application shutdown.
- The explicit GPUI regression test opened/canceled region selection twice, verified reuse of the same overlay handles, checked that snapshots/drag state were cleared and the control window became visible, and confirmed an unchanged clipboard sequence. The test returned through normal shutdown with no errors logged. This exercises the cancellation handler directly, not physical Escape input; it covers the attached single display, not mixed-DPI hardware.

Native hiding, overlay reuse, repeated cancellation, restoration and normal shutdown have the GPUI test evidence above. Physical Escape input and the global region shortcut/tray entry still need interactive verification. Direct window capture has the successful example above; additional window types and failure cases remain open.

Synchronous ShowWindow could re-enter borrowed GPUI state, so native visibility now uses ShowWindowAsync. Before capturing, the worker verifies that the control window and cached overlays are hidden, with a timeout, then synchronizes the compositor. Quit now uses GPUI's normal application shutdown instead of manually removing all windows before quitting. These are implementation changes awaiting an interactive regression check, not confirmation that the logged errors are eliminated.

Earlier logs contain `window not found` and invalid-HWND errors during destruction. Reusing overlays avoids repeated destruction during selection, but is not evidence that the toolkit shutdown bug is fixed. Creation logs report y=-4 before fullscreen placement; `overlay geometry after presentation` now records the later bounds. Final edge alignment and taskbar coverage are unconfirmed.

## Capture and region paths

| Target session | Capture approach | Region UI | Validation |
| --- | --- | --- | --- |
| Windows | XCap still capture | GPUI PopUp over frozen monitor images | Interactive evidence above; remaining checks open |
| macOS | XCap candidate | GPUI overlay candidate | Not built/tested here; permissions and stable signed bundle required |
| Linux X11 | XCap candidate | GPUI overlay candidate | Not built/tested here |
| Linux Wayland | XDG Desktop Portal still-capture candidate | Portal selection; editor crop if only a full image is returned | Portal, cancellation and permissions not implemented/tested |

These are intended paths, not capability claims for untested platforms. Scap/PipeWire evaluation remains open. The current XCap adapter disables window listing/capture on Linux and does not constitute a validated Wayland backend. Wayland global shortcuts require the GlobalShortcuts portal; the Windows integration does not provide that path.

## Remaining gate checks

- Repeated Windows selection, cancel, native UI hiding and shutdown; inspect the saved image and post-presentation bounds.
- Direct window capture, including closed windows, minimized windows, protected content and covered windows.
- Second monitor with negative origin, fractional/mixed DPI, different resolutions, rotation, and monitor disconnect/reconnect. Selection currently stays within one monitor and snapshot matching uses the Windows IDs observed above.
- Build and exercise GPUI, clipboard, hotkeys and tray on macOS/X11/Wayland. Record unavailable capabilities explicitly.
- Evaluate Scap/PipeWire and the portal, then confirm each target's capture and region paths before Phase 1.
