# screenshots

Open-source desktop capture and sharing application. English is the default UI language. Native system dialogs and OS error messages follow the operating system language.

Windows renderer lifecycle and its explicit release probe are documented in [control-window latency](docs/control-open-latency.md). The pinned Windows GPUI backend includes a small [local teardown patch](vendor/gpui-windows-gpui-unofficial/LOCAL-PATCH.md); build through this workspace so Cargo applies the override.

The [region-selection release probe](docs/region-latency.md) measures native Ctrl+Shift+R delivery through capture and compositor synchronization, with 101 real capture/cancel cycles. Its explicit test injects keyboard input; leave keyboard and clipboard untouched while running it. Normal launches contain no probe instrumentation.

On Windows, normal launches share one instance per user/session. Starting the app again signals the running instance to show its control window and exits before creating another tray icon or registering hotkeys. Instance setup/notification failures do not fall back to launching a second owner. A newly created control window is explicitly shown, including after a launcher started the resident app hidden. Explicit resource-probe modes and isolated GUI tests bypass this product lifecycle.

## Capture home

Open the control window with `cargo run -p screenshots -- --show` (Windows otherwise starts in the tray). The home screen offers region selection, primary/all displays, individual displays and window capture. The last capture has a bounded preview, recopy and **Show file**. Technical controls are under **Diagnostics**.

New captures use the user's Pictures/Screenshots folder by default (application config storage is the fallback if no Pictures directory is available). **Change folder…** opens the native directory picker and persists the selection in the platform application's `settings.json`. A canceled or failed change preserves the previous folder. Settings load/save run on workers; capture is disabled while settings are loading/changing. Existing files under `target/captures` are left in place. Hardware tests keep their isolated test directories and do not read or alter user settings.

**Save as…** exports the last capture through a native save dialog. PNG is the export format; existing files are never replaced, including after an overwrite confirmation from the native picker. Choose a free name if a destination already exists. Saved PNGs stream directly to the new destination without image decoding/re-encoding; unsaved captures export from their retained pixels. Canceling preserves the capture, clipboard and default folder. Quit waits for an active export, and export failure keeps the source available.

**Region in 3 seconds** starts a cancellable countdown before freezing the desktop, useful for opening menus first. Use **Cancel countdown** or Escape while the control window has focus. Closing the control window or quitting cancels the pending countdown. Other captures and folder/export actions are blocked during the delay. Its task is dropped on cancellation; no periodic timer remains when idle.

**Capture history** shows saved captures across restarts, 20 entries per page. Use **Older** and **Latest**, then **Open**, **Show file**, or **Copy** on an entry. **Remove entry** removes only its history metadata and keeps the PNG unchanged. Copying runs on a worker with the same decode/size limits as last-capture recopy; it does not change the selected last capture. It records newly saved captures and Save As exports; existing image folders are not automatically imported. The local SQLite database stores metadata and file references, not image blobs. Opening history does not decode thumbnails or scan folders. Missing or externally moved images remain referenced; automatic reconciliation and retention controls are still pending. Database errors are reported separately from PNG save results.

**Add PNGs…** inside Capture history adds references to up to 100 selected existing PNG files through the native picker. This is useful for older files under 	arget/captures. It checks PNG headers and the decoded-size admission limit without decoding full image frames; it does not copy, move or alter originals. Exact paths already listed are skipped. The result reports added/existing/skipped counts and up to three file errors. Valid selections are committed in one metadata transaction; invalid files are skipped with errors. Moving or deleting an original later makes its history reference unavailable. Full pixel decoding happens only when explicitly copying an image.

**Edit a copy** opens the last capture or a selected history entry (including imported PNGs) in the basic editor. Drag rectangles over sensitive content; **Undo** removes the last rectangle, **Redo** restores it, and **Reset** clears crop, annotations and undo/redo state. A new rectangle after Undo discards the redo branch. Editor shortcuts: Ctrl+Z, Ctrl+Shift+Z / Ctrl+Y (Command+Z / Command+Shift+Z on macOS). **Save and copy** applies opaque black RGBA pixels on a worker, exports a new flattened edacted-<uuid>.png in the configured save folder and copies that result. Existing original files and history entries remain unchanged and may still contain sensitive content; the editor states this explicitly. **Cancel**/Escape closes without exporting. Quit discards unexported editor changes. The first tool supports up to 100 rectangles, fits the image to the window and cancels an in-progress drag on resize. Annotation undo stores rectangle metadata, not full-image snapshots. New captures wait until editing is finished or canceled. The UI and interaction design remain provisional.

**Frame** in the editor draws three-source-pixel rectangular outlines. Choose Red, Yellow or Green; the selected color applies to new frames. Frames, redactions and crop share Undo/Redo and the combined 100-annotation limit. The preview and PNG export use the same four border segments. Frames are drawn below opaque redactions, and crop excludes all pixels outside the selected output. Frame-only exports use nnotated-<uuid>.png. No raster image is copied while placing a frame.

**Arrow** draws from the drag start toward the release position, including horizontal, vertical and reversed drags. It shares Red/Yellow/Green with Frame, crop clipping and the combined 100-annotation/100-undo-step limits. Preview and raster export use the same polygon vertices. Arrows render above frames and below opaque redactions. Arrow export temporarily allocates one output-sized premultiplied RGBA overlay in addition to the final output buffer, composites it onto straight RGBA while preserving untouched pixels, then releases it before PNG/clipboard work. This stays within the planned two-output-buffer active-image allocation allowance; no updated latency/memory acceptance is claimed from unit tests.

Editor view controls: **+ / −** zoom, **100%** maps an image pixel to a physical display pixel using the window DPI, and **Fit** restores the centered full-image view. Use the wheel/trackpad to pan enlarged images and Shift+wheel for horizontal movement. Panning is clamped to the image edges and disabled during an annotation drag. Zoom/resize cancels an unfinished drag; document coordinates, undo history and export dimensions are unaffected by view changes. The workspace clips the enlarged image below the toolbar. Zoom uses the existing texture and adds no decoded image copies or background work.

**Ellipse** adds red/yellow/green elliptical outlines, with a three-source-pixel inset border. Very small ellipses become filled when no interior remains. Preview and export share bounded polygon contours (32–256 segments per contour), crop clipping and the existing output-sized raster overlay. Ellipses participate in the same 100-annotation limit and Undo/Redo. Layer order is frames, ellipses, arrows, then opaque redactions. Like other highlights, an ellipse does not conceal its interior; use Redact for sensitive content.

**Capture and annotate** opens a frozen fullscreen editing overlay. Crop is selected initially; redact, frame, arrow, ellipse, undo/redo and zoom work directly on the desktop preview before final output. **Move toolbar** switches the toolbar between the top and bottom so every part of the image remains reachable. The source stays in RAM: opening, cropping, annotating or canceling creates no PNG/history entry and does not change the clipboard. **Copy only** exports pixels to the clipboard; **Save only** publishes a PNG and history entry; **Save and copy** does both. Without a crop, explicit output uses the entire captured display. Cancellation preserves the previous last-capture state. The existing **Select region** action retains its immediate save/copy behavior.

On Windows, **Ctrl+Shift+E** and the tray's **Capture and annotate** action open this overlay without the control window. One display is edited per session: the monitor under the pointer, with the primary display as fallback. Ctrl+Shift+S/R retain their immediate capture behavior. Each shortcut registers independently and unavailable shortcuts appear in the status line. The overlay reuses its worker-prepared image; annotations and undo store geometry rather than full-image snapshots. Canceling or finishing output without an open control window removes the overlay and detaches graphics again. A short real-desktop regression checks cancellation, no image/database output, unchanged clipboard and no remaining GPUI windows. Mixed-DPI interaction and physical input/presentation performance still require manual acceptance checks.

**Select** picks existing redactions, frames, arrows and ellipses in the overlay and standalone editor. A cyan outline identifies the selected mark; drag it to move it within the image, or use **Delete**, Delete/Backspace to remove it. Hollow frames/ellipses are selected at their outline. Selection follows the existing visible layer order. Movement is previewed from metadata and becomes one undo step on release; deletion is undoable too. Clicking empty space deselects. Selection outlines and handles are never exported. Drag one of the four corner handles to resize a redaction, frame or ellipse; drag either endpoint handle to change an arrow. The opposite corner/endpoint stays fixed. Rectangle dimensions stay at least one source pixel, arrow endpoints stay distinct, and changes remain within the source image. Handle sizes remain constant on screen while zooming. Like movement, resizing previews metadata only and commits one undo step on release.
Editor/overlay keyboard tools: **V** Select, **X** Redact, **C** Crop, **R** Frame, **A** Arrow and **E** Ellipse. Tool changes discard an unfinished drag. In Select mode, arrow keys move the selected mark by one source-image pixel; **Shift+arrow** moves ten pixels, independent of zoom/DPI. Moves clamp to image boundaries and remain undoable. Control/Alt/Command/Windows combinations keep their existing meanings. The palette in Select mode shows and changes the selected frame/arrow/ellipse color, with Undo/Redo; selecting the same color adds no undo step. Opaque redactions remain black and cannot be recolored. The chosen color also becomes the color for new marks.
**Pixelate (P)** works in the frozen overlay and standalone editor. Choose it, then drag a region after the one-time background preparation completes. Pixelate regions share Select, move/resize, Delete and Undo/Redo, and count toward the combined 100-mark limit. The effect uses fixed 16-source-pixel blocks aligned to the original image; moving a region reveals the blocks at its new position. It affects source pixels below frames, arrows, ellipses and opaque redactions. It does not pixelate those annotation layers. Pixelation is visual obscuring, not reliable removal of sensitive information; use **Redact** for that.

A source-aligned color grid is computed once per editor document on a worker, using alpha-weighted colors. Preview uses a single BGRA image bounded to 2048 pixels per side (at most 16 MiB), clipped to each region, with no rerasterization during dragging. Large-image preview can have softer block boundaries than the full-resolution export. The grid is retained outside undo states; no new image copies are stored per edit. Closing cancels preparation and removes its atlas image. Export flattens source-aligned blocks at full resolution, clips them to the crop and merges overlapping regions so each affected output pixel is written once. It never saves the unedited original as an intermediate step.
Editor output choices: **Copy only** renders and copies the edited pixels without PNG/history output; **Save only** saves a PNG and records history without touching the clipboard; **Save and copy** does both, keeping independent result statuses. Copy-only output retains at most the last edited RGBA image in RAM for recopy, further editing or Save As, replacing the previous last capture. This is a live document representation, not the empty-resident baseline; it is released when replaced or the app exits. Save success releases the full-resolution source as before. Quit waits for the chosen active output job. A short isolated fixture test verifies Save-only clipboard preservation and Copy-only absence of PNG/history output.

The current home layout is provisional. Final visual design is still planned.

The sections below include historical spike controls and evidence; their old default `target/captures` path applies to the spike/test setup, not the normal application's new default.
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

On Windows the app starts in the system tray without a control window or GPU renderer. Right-click the camera icon (possibly under hidden icons) and choose **Show control window**. Use `cargo run -p screenshots -- --show` to open it immediately. Closing the control window returns to the tray and detaches the renderer; **Quit** in the tray or Ctrl+Q exits. Other platforms retain the windowed Phase 0 spike.

See [Phase 0 evidence and remaining checks](docs/phase-0.md).

- Open small overlay: red card at the top-left of each display.
- Capture a window: click a listed window near the top of the control window to capture it directly, save it and copy it. Use Fensterliste aktualisieren after opening or closing windows. Minimized, untitled and own-process windows are excluded from the picker. A window that closes before capture produces an error and restores the control window.
- Capture buttons, hotkey and tray write PNGs under `target/captures/`, show a thumbnail, and automatically copy the last image to the clipboard directly from shared RGBA pixels. This replaces the current clipboard contents.
- Select region (Strg+Umschalt+F), directly below the window heading: hides existing overlays and the control window, captures each display, then opens a fullscreen PopUp showing that display's frozen image. Drag with the left mouse button; release saves the selection as `target/captures/selection-<uuid>.png`, restores the control window, and copies the selection. Escape cancels without changing the clipboard. Canceled previews are never written to disk.
- Copy last capture: uses live pixels if still retained, otherwise loads the successfully saved PNG on demand. This replaces the current clipboard contents.
- Escape or Close overlays dismisses overlay windows.
- On Windows, Ctrl+Shift+S captures the primary display even when another app has focus. Registration failures (for example, a shortcut already in use) appear in the control window. The capture is saved and copied automatically.
- On Windows, Ctrl+Shift+R starts region selection even when another app has focus. Ctrl+Shift+F remains a local shortcut in the control window. Each global shortcut registers independently: a conflict with one does not disable the other. The status line reports unavailable keys.
- On Windows, right-click the blue camera icon in the system tray (possibly under hidden icons): Select region, Capture primary, Show control window, or Quit. Region selection uses the same frozen-image workflow as the button. Capture primary uses the same save-and-copy path as the buttons and hotkey. Closing the control window returns to the resident tray state.

The clipboard button and its result appear near the top, above the diagnostic lists and thumbnail. On Windows, closing the control window releases its windows and renderer; Ctrl+Q quits through the output-safe cleanup path. Native WM_CLOSE is intercepted so Windows does not destroy the window ahead of GPUI cleanup.

Clipboard copying runs on a background worker; pixel round-trip verification runs only in the explicit test. The clipboard owner stays alive while the app runs. Windows paste was confirmed in the interactive session; other platforms still need a paste check.

On Windows, all capture paths request native hiding with `ShowWindowAsync(SW_HIDE)`. Visibility changes are queued to avoid re-entering borrowed GPUI state. The capture worker checks that the control window and cached overlays actually become hidden (up to two seconds per window), then calls `DwmFlush` before XCap. This replaces the 300ms minimize-animation delay. Failures stop the capture and restore the control window. Normal captures restore it after success; selection overlays keep it hidden until dismissed. GPUI's app-level `hide()` is a no-op on Windows, so it is not used. The explicit compositor synchronization test passes in the agent environment; interactive visibility and image cleanliness still need verification. Other platforms retain the unvalidated minimize-and-delay prototype until their native integration is tested.

Region selection crops the frozen in-memory image, not a new live capture. Window-local coordinates are scaled to that snapshot's pixel dimensions, with floor/ceil bounds and clipping at the image edges. Dragging in either direction works; a click with no area keeps the overlay open. Selection is limited to one monitor. Dragging across monitors is not supported; if a release is missed outside the originating window, returning without the left button held cancels that drag. Interactive selection, physical DPI calibration, and edge alignment remain to be checked on real hardware.

On Windows, dismissing an overlay hides its native window and clears the drag/snapshot state. While the control window remains open, the next invocation reuses the window for the same display and overlay mode. With no control window open, dismissal removes the windows after native hiding/compositor synchronization and detaches graphics. This avoids repeatedly destroying GPUI windows after each selection; native destruction still occurs when quitting. A capture stops if an existing overlay could not be hidden. The diagnostic `overlay geometry after presentation` logs bounds and viewport after two frames; the interactive single-display run confirmed (0,0), 2560×1440. The explicit GPUI test covers repeated cancellation, reuse, restoration and normal shutdown. Physical Escape input and other display setups remain to be checked.

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

Windows resident lifecycle verification:

```powershell
cargo test -p app-ui resident_capture_reopen_and_graphics_detach -- --ignored --nocapture --test-threads=1
powershell -NoProfile -File scripts/measure-idle.ps1 -Seconds 300 -Mode resident
powershell -NoProfile -File scripts/measure-idle.ps1 -Seconds 30 -Mode resident-after-ui
```

The explicit GUI test captures the real desktop, replaces clipboard contents, reopens/closes the control window (including native WM_CLOSE), and cancels region selection. Run with no other app instance and leave the clipboard untouched. `resident` measures the actual tray/hotkey controller; `resident-after-ui` opens and closes its control window before sampling. Both use an explicit diagnostic exit deadline, absent from ordinary launches. See [resident measurements](docs/performance-resident.md).
Capture admission: all requested displays/windows/regions must fit a combined 256 MiB decoded RGBA source limit before the first native capture. Actual returned dimensions are checked again before retaining frames or rendering. A failed group publishes no partial captures and leaves the clipboard unchanged. Individual capture APIs, crop and saved-image recopy share the limit. Empty, negative or out-of-display regions are rejected. This bounds application admission, not XCap's internal/native allocations during a monitor/window size-change race. No new idle work is added; checks run on capture/image workers.
