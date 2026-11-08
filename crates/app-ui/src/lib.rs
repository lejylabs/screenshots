//! Phase 0 spike: GPUI overlay + XCap still capture on Windows.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use app_core::{CaptureBackend, CapturedImage, DisplayInfo, WindowInfo, XcapBackend};
use gpui::{
    AnyWindowHandle, App, Bounds, Context, DisplayId, FocusHandle, Focusable, IntoElement,
    KeyBinding, Pixels, TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds,
    WindowHandle, WindowKind, WindowOptions, actions, div, img, point, prelude::*, px, rgb, rgba,
    size,
};

actions!(
    spike,
    [
        OpenProbeOverlays,
        OpenFullscreenOverlays,
        CloseOverlay,
        CapturePrimary,
        CaptureAll,
        CaptureCenterRegion,
        Quit
    ]
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OverlayMode {
    Probe,
    Fullscreen,
}

pub struct SpikeShell {
    control: AnyWindowHandle,
    preview_active: bool,
    focus_handle: FocusHandle,
    overlays: Vec<WindowHandle<Overlay>>,
    backend: XcapBackend,
    xcap_displays: Vec<DisplayInfo>,
    xcap_windows: Vec<WindowInfo>,
    capture_status: String,
    last_capture: Option<CapturedImage>,
    capturing: bool,
    clipboard: Arc<Mutex<app_core::ImageClipboard>>,
    copying: bool,
    clipboard_status: String,
    #[cfg(target_os = "windows")]
    hotkey: Option<app_core::CaptureHotkey>,
    hotkey_status: String,
    #[cfg(target_os = "windows")]
    tray: Option<app_core::SpikeTray>,
    tray_status: String,
}

impl SpikeShell {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let backend = XcapBackend::new(app_core::capture_output_dir());
        let caps = backend.capabilities();
        tracing::info!(
            list_displays = caps.list_displays,
            list_windows = caps.list_windows,
            capture_display = caps.capture_display,
            capture_region = caps.capture_region,
            "xcap capabilities"
        );

        let mut shell = Self {
            control: window.window_handle(),
            preview_active: false,
            focus_handle: cx.focus_handle(),
            overlays: Vec::new(),
            backend,
            xcap_displays: Vec::new(),
            xcap_windows: Vec::new(),
            capture_status: "idle".into(),
            last_capture: None,
            capturing: false,
            clipboard: Arc::new(Mutex::new(app_core::ImageClipboard::default())),
            copying: false,
            clipboard_status: "Clipboard: capture an image first".into(),
            #[cfg(target_os = "windows")]
            hotkey: None,
            hotkey_status: "Global hotkey: not tested on this platform".into(),
            #[cfg(target_os = "windows")]
            tray: None,
            tray_status: "Tray: not tested on this platform".into(),
        };
        shell.refresh_xcap_lists();
        #[cfg(target_os = "windows")]
        match app_core::SpikeTray::new() {
            Ok(tray) => {
                shell.tray = Some(tray);
                shell.tray_status =
                    "Tray: Bereich auswählen / Capture / Show control window / Quit".into();
                tracing::info!("tray icon created");
                let control = window.window_handle();
                cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
                        if this.update(cx, |this, cx| {
                            let commands = this.tray.as_ref().map(|tray| tray.take_commands()).unwrap_or_default();
                            for command in commands {
                                tracing::info!(?command, "tray command");
                                match command {
                                    app_core::TrayCommand::Capture => this.capture_primary(cx),
                                    app_core::TrayCommand::Region => this.open_overlays(OverlayMode::Fullscreen, cx),
                                    app_core::TrayCommand::Show => {
                                        if this.capturing || this.preview_active { continue; }
                                        if let Err(error) = control.update(cx, |_, window, cx| {
                                            if let Err(error) = show_control_window(window) {
                                                tracing::error!(%error, "could not show control window");
                                            }
                                            cx.activate(true);
                                        }) { tracing::warn!(%error, "could not show control window"); }
                                    }
                                    app_core::TrayCommand::Quit => cx.defer(close_application),
                                }
                            }
                        }).is_err() { break; }
                    }
                }).detach();
            }
            Err(error) => {
                tracing::warn!(%error, "tray creation failed");
                shell.tray_status = format!("Tray unavailable: {error}");
            }
        }
        #[cfg(target_os = "windows")]
        {
            match app_core::CaptureHotkey::register() {
                Ok(hotkey) => {
                    shell.hotkey_status = hotkey.status().to_string();
                    tracing::info!(status = %shell.hotkey_status, "global hotkey registration complete");
                    shell.hotkey = Some(hotkey);
                    cx.spawn(async move |this, cx| {
                        loop {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(50))
                                .await;
                            if this
                                .update(cx, |this, cx| {
                                    let commands = this
                                        .hotkey
                                        .as_ref()
                                        .map(|hotkey| hotkey.take_commands())
                                        .unwrap_or_default();
                                    for command in commands {
                                        tracing::info!(?command, "global capture hotkey pressed");
                                        match command {
                                            app_core::CaptureCommand::Primary => {
                                                this.capture_primary(cx)
                                            }
                                            app_core::CaptureCommand::Region => {
                                                this.open_overlays(OverlayMode::Fullscreen, cx)
                                            }
                                        }
                                    }
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    })
                    .detach();
                }
                Err(error) => {
                    tracing::warn!(%error, "global capture hotkey registration failed");
                    shell.hotkey_status = format!("Global hotkeys unavailable: {error}");
                }
            }
        }
        shell
    }

    fn refresh_xcap_lists(&mut self) {
        match self.backend.list_displays() {
            Ok(displays) => {
                for info in &displays {
                    tracing::info!(
                        id = %info.id,
                        name = %info.name,
                        x = info.x,
                        y = info.y,
                        width = info.width,
                        height = info.height,
                        scale = info.scale_factor,
                        rotation = info.rotation,
                        primary = info.is_primary,
                        "xcap display"
                    );
                }
                self.xcap_displays = displays;
            }
            Err(error) => {
                tracing::error!(%error, "xcap list_displays failed");
                self.capture_status = format!("list displays failed: {error}");
            }
        }

        match self.backend.list_windows() {
            Ok(windows) => {
                tracing::info!(count = windows.len(), "xcap windows");
                self.xcap_windows = windows;
            }
            Err(error) => {
                tracing::warn!(%error, "xcap list_windows failed");
                self.xcap_windows = Vec::new();
            }
        }
    }

    fn live_overlay_count(&self, cx: &App) -> usize {
        self.overlays
            .iter()
            .filter(|handle| handle.read(cx).is_ok_and(|overlay| overlay.active))
            .count()
    }

    fn close_overlays(&mut self, cx: &mut Context<Self>) {
        tracing::info!(count = self.overlays.len(), "closing overlays");
        for handle in &self.overlays {
            match handle.update(cx, |overlay, window, cx| {
                #[cfg(target_os = "windows")]
                if let Some(native) = &overlay.native {
                    if let Err(error) = native.request_visible(false) {
                        tracing::error!(%error, "could not hide overlay");
                        return;
                    }
                    overlay.active = false;
                    overlay.drag_start = None;
                    overlay.snapshot = None;
                    cx.notify();
                    return;
                }
                window.remove_window();
            }) {
                Ok(()) => {}
                Err(error) => tracing::warn!(?error, "overlay handle already gone"),
            }
        }
        self.overlays.retain(|handle| handle.read(cx).is_ok());
        if self.preview_active && self.live_overlay_count(cx) == 0 {
            self.preview_active = false;
            self.restore_control(cx);
        }
        cx.notify();
    }

    fn open_overlays(&mut self, mode: OverlayMode, cx: &mut Context<Self>) {
        if self.capturing || self.copying {
            return;
        }
        if matches!(mode, OverlayMode::Fullscreen) {
            let jobs = cx
                .displays()
                .iter()
                .map(|display| CaptureJob::Display(u64::from(display.id()).to_string()))
                .collect();
            self.start_capture_with_preview(jobs, true, cx);
            return;
        }
        self.show_overlays(mode, &[], cx);
    }

    fn restore_control(&self, cx: &mut Context<Self>) {
        let control = self.control;
        cx.defer(move |cx| {
            let result = control
                .update(cx, |_, window, _| show_control_window(window))
                .map_err(|error| error.to_string())
                .and_then(|result| result);
            if let Err(error) = result {
                tracing::warn!(%error, "could not restore control window");
            }
        });
    }

    fn show_overlays(
        &mut self,
        mode: OverlayMode,
        shots: &[CapturedImage],
        cx: &mut Context<Self>,
    ) {
        self.close_overlays(cx);
        if self.live_overlay_count(cx) > 0 {
            self.capture_status = "Could not hide the previous overlay".into();
            cx.notify();
            return;
        }

        let screens: Vec<(DisplayId, Bounds<Pixels>)> = cx
            .displays()
            .iter()
            .map(|display| (display.id(), display.bounds()))
            .collect();

        for (display_id, display_bounds) in screens {
            let snapshot = shots
                .iter()
                .find(|shot| shot.display_id == u64::from(display_id).to_string())
                .cloned();
            if matches!(mode, OverlayMode::Fullscreen) && snapshot.is_none() {
                self.capture_status =
                    format!("No captured image for display {}", u64::from(display_id));
                tracing::error!(
                    display_id = u64::from(display_id),
                    "missing overlay snapshot"
                );
                continue;
            }
            #[cfg(target_os = "windows")]
            if let Some(handle) = self
                .overlays
                .iter()
                .find(|handle| {
                    handle.read(cx).is_ok_and(|overlay| {
                        overlay.display_id == display_id
                            && overlay.mode == mode
                            && overlay.native.is_some()
                    })
                })
                .copied()
            {
                let result = handle.update(cx, |overlay, window, cx| {
                    overlay.snapshot = snapshot.clone();
                    overlay.bounds = display_bounds;
                    overlay.drag_start = None;
                    overlay.active = true;
                    if let Some(native) = &overlay.native {
                        if let Err(error) = native.request_visible(true) {
                            overlay.active = false;
                            tracing::error!(%error, "could not show overlay");
                        }
                    }
                    window.focus(&overlay.focus_handle, cx);
                    if matches!(mode, OverlayMode::Fullscreen) {
                        window.activate_window();
                    }
                    log_overlay_geometry(window, display_bounds);
                    cx.notify();
                });
                if let Err(error) = result {
                    tracing::error!(%error, "could not reuse overlay");
                }
                continue;
            }
            let owner = cx.entity().downgrade();
            tracing::info!(
                display_id = u64::from(display_id),
                mode = ?mode,
                origin_x = %display_bounds.origin.x,
                origin_y = %display_bounds.origin.y,
                width = %display_bounds.size.width,
                height = %display_bounds.size.height,
                "opening overlay"
            );

            match cx.open_window(
                overlay_options(mode, display_id, display_bounds),
                |window, cx| {
                    tracing::info!(
                        display_id = u64::from(display_id),
                        mode = ?mode,
                        scale = window.scale_factor(),
                        actual_origin_x = %window.bounds().origin.x,
                        actual_origin_y = %window.bounds().origin.y,
                        actual_width = %window.bounds().size.width,
                        actual_height = %window.bounds().size.height,
                        "overlay window ready"
                    );
                    window.on_window_should_close(cx, move |_, _| {
                        tracing::warn!(
                            display_id = u64::from(display_id),
                            mode = ?mode,
                            "native WM_CLOSE on overlay; refusing so the spike stays open"
                        );
                        false
                    });
                    let overlay = cx.new(|cx| {
                        Overlay::new(
                            display_id,
                            display_bounds,
                            mode,
                            snapshot,
                            owner,
                            window,
                            cx,
                        )
                    });
                    let focus = overlay.read(cx).focus_handle.clone();
                    window.focus(&focus, cx);
                    log_overlay_geometry(window, display_bounds);
                    overlay
                },
            ) {
                Ok(handle) => self.overlays.push(handle),
                Err(error) => tracing::error!(?error, "failed to open overlay window"),
            }
        }

        self.preview_active =
            matches!(mode, OverlayMode::Fullscreen) && self.live_overlay_count(cx) > 0;
        if matches!(mode, OverlayMode::Fullscreen) && !self.preview_active {
            self.restore_control(cx);
        }

        cx.notify();
    }

    fn capture_primary(&mut self, cx: &mut Context<Self>) {
        let Some(display) = self
            .xcap_displays
            .iter()
            .find(|display| display.is_primary)
            .or_else(|| self.xcap_displays.first())
            .cloned()
        else {
            self.capture_status = "no XCap display".into();
            cx.notify();
            return;
        };
        self.start_capture(vec![CaptureJob::Display(display.id)], cx);
    }

    fn capture_all(&mut self, cx: &mut Context<Self>) {
        let jobs: Vec<CaptureJob> = self
            .xcap_displays
            .iter()
            .map(|display| CaptureJob::Display(display.id.clone()))
            .collect();
        if jobs.is_empty() {
            self.capture_status = "no XCap display".into();
            cx.notify();
            return;
        }
        self.start_capture(jobs, cx);
    }

    fn capture_center_region(&mut self, cx: &mut Context<Self>) {
        let Some(display) = self
            .xcap_displays
            .iter()
            .find(|display| display.is_primary)
            .or_else(|| self.xcap_displays.first())
            .cloned()
        else {
            self.capture_status = "no XCap display".into();
            cx.notify();
            return;
        };
        let width = 400.min(display.width);
        let height = 300.min(display.height);
        let x = ((display.width.saturating_sub(width)) / 2) as i32;
        let y = ((display.height.saturating_sub(height)) / 2) as i32;
        self.start_capture(
            vec![CaptureJob::Region {
                display_id: display.id,
                x,
                y,
                width,
                height,
            }],
            cx,
        );
    }

    fn copy_last_capture(&mut self, cx: &mut Context<Self>) {
        if self.copying {
            return;
        }
        let Some(shot) = &self.last_capture else {
            return;
        };
        let path = shot.png_path.clone();
        tracing::info!(path = %path.display(), "copying capture to clipboard");
        let clipboard = self.clipboard.clone();
        self.copying = true;
        self.clipboard_status = "Clipboard: copying and verifying…".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    clipboard
                        .lock()
                        .map_err(|error| error.to_string())?
                        .copy_png_and_verify(&path)
                        .map_err(|error| format!("{error:#}"))
                })
                .await;
            this.update(cx, |this, cx| {
                this.copying = false;
                this.clipboard_status = match result {
                    Ok((width, height)) => {
                        tracing::info!(width, height, "clipboard RGBA round trip passed");
                        format!("Clipboard: {width}×{height} copied, RGBA round trip verified")
                    }
                    Err(error) => {
                        tracing::error!(%error, "clipboard round trip failed");
                        format!("Clipboard failed: {error}")
                    }
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn save_selection(
        &mut self,
        source: CapturedImage,
        region: app_core::PixelRegion,
        cx: &mut Context<Self>,
    ) {
        if self.capturing || self.copying {
            return;
        }
        self.close_overlays(cx);
        self.capturing = true;
        self.capture_status = "Saving selection…".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { app_core::crop_snapshot(&source, region) }).await;
            this.update(cx, |this, cx| {
                this.capturing = false;
                match result {
                    Ok(shot) => {
                        tracing::info!(width = shot.width, height = shot.height, path = %shot.png_path.display(), "saved selection");
                        this.capture_status = format!("Selection {}×{}  {}", shot.width, shot.height, shot.png_path.display());
                        this.last_capture = Some(shot);
                        this.copy_last_capture(cx);
                    }
                    Err(error) => {
                        tracing::error!(%error, "selection failed");
                        this.capture_status = format!("Selection failed: {error:#}");
                    }
                }
                cx.notify();
            }).ok();
        }).detach();
    }

    fn start_capture(&mut self, jobs: Vec<CaptureJob>, cx: &mut Context<Self>) {
        self.start_capture_with_preview(jobs, false, cx);
    }

    fn start_capture_with_preview(
        &mut self,
        jobs: Vec<CaptureJob>,
        preview: bool,
        cx: &mut Context<Self>,
    ) {
        if self.capturing || self.copying {
            return;
        }
        if jobs.is_empty() {
            return;
        }
        // A new capture replaces the preview; keep the control window hidden.
        self.preview_active = false;
        self.close_overlays(cx);
        if self.live_overlay_count(cx) > 0 {
            self.preview_active = true;
            self.capture_status = "Capture stopped: could not hide the previous overlay".into();
            cx.notify();
            return;
        }
        self.capturing = true;
        self.capture_status = format!("capturing {} job(s)…", jobs.len());
        cx.notify();

        let backend = self.backend.clone();
        let control = self.control;
        #[cfg(target_os = "windows")]
        let mut hidden_windows: Vec<_> = self
            .overlays
            .iter()
            .filter_map(|handle| handle.read(cx).ok().and_then(|overlay| overlay.native))
            .collect();
        cx.spawn(async move |this, cx| {
            // Defer until the button/action has released its borrow of the control window.
            let hidden = control
                .update(cx, |_, window, _| {
                    #[cfg(target_os = "windows")]
                    {
                        let native = native_window(window)?;
                        native
                            .request_visible(false)
                            .map_err(|error| error.to_string())?;
                        Ok(native)
                    }
                    #[cfg(not(target_os = "windows"))]
                    {
                        window.minimize_window();
                        Ok::<(), String>(())
                    }
                })
                .map_err(|error| error.to_string())
                .and_then(|result| result);
            let _hidden_control = match hidden {
                Ok(control) => control,
                Err(error) => {
                    this.update(cx, |this, cx| {
                        this.capturing = false;
                        this.capture_status = format!("could not hide control window: {error}");
                        this.restore_control(cx);
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            #[cfg(target_os = "windows")]
            hidden_windows.push(_hidden_control);
            // shortcut: other platforms still need a native visibility/compositor spike.
            #[cfg(not(target_os = "windows"))]
            cx.background_executor()
                .timer(std::time::Duration::from_millis(300))
                .await;
            let result = cx
                .background_spawn(async move {
                    #[cfg(target_os = "windows")]
                    {
                        for window in hidden_windows {
                            window.wait_until_hidden().map_err(|error| {
                                app_core::CaptureError::Backend(error.to_string())
                            })?;
                        }
                        app_core::wait_for_desktop_compositor()
                            .map_err(|error| app_core::CaptureError::Backend(error.to_string()))?;
                    }
                    let mut shots = Vec::new();
                    for job in jobs {
                        let shot = match &job {
                            CaptureJob::Display(id) => backend.capture_display(id)?,
                            CaptureJob::Window(id) => backend.capture_window(id)?,
                            CaptureJob::Region {
                                display_id,
                                x,
                                y,
                                width,
                                height,
                            } => backend.capture_region(display_id, *x, *y, *width, *height)?,
                        };
                        tracing::info!(
                            display_id = %shot.display_id,
                            window_id = ?shot.window_id,
                            width = shot.width,
                            height = shot.height,
                            elapsed_ms = shot.elapsed_ms,
                            path = %shot.png_path.display(),
                            "captured png"
                        );
                        shots.push(shot);
                    }
                    Ok::<Vec<CapturedImage>, app_core::CaptureError>(shots)
                })
                .await;

            this.update(cx, |this, cx| {
                this.capturing = false;
                if !preview || result.is_err() {
                    this.restore_control(cx);
                }
                match result {
                    Ok(shots) => {
                        let Some(shot) = shots.last().cloned() else {
                            this.capture_status = "no capture produced".into();
                            this.restore_control(cx);
                            cx.notify();
                            return;
                        };
                        this.capture_status = format!(
                            "{}×{}  {} ms  {}",
                            shot.width,
                            shot.height,
                            shot.elapsed_ms,
                            shot.png_path.display()
                        );
                        this.last_capture = Some(shot);
                        if preview {
                            this.show_overlays(OverlayMode::Fullscreen, &shots, cx);
                        } else {
                            this.copy_last_capture(cx);
                        }
                    }
                    Err(error) => {
                        tracing::error!(%error, "capture failed");
                        this.capture_status = format!("capture failed: {error}");
                    }
                }
                this.refresh_xcap_lists();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

enum CaptureJob {
    Display(String),
    Window(String),
    Region {
        display_id: String,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
}

impl Focusable for SpikeShell {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SpikeShell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let displays: Vec<(u64, Bounds<Pixels>)> = cx
            .displays()
            .iter()
            .map(|display| (u64::from(display.id()), display.bounds()))
            .collect();
        let live_count = self.live_overlay_count(cx);
        let handle_count = self.overlays.len();
        let capturing = self.capturing || self.copying;
        let capture_status = self.capture_status.clone();
        let last_path: Option<PathBuf> =
            self.last_capture.as_ref().map(|shot| shot.png_path.clone());
        let xcap_displays = self.xcap_displays.clone();
        let window_count = self.xcap_windows.len();
        let window_preview: Vec<(String, String)> = self
            .xcap_windows
            .iter()
            .filter(|window| {
                !window.is_minimized && !window.title.is_empty() && window.pid != std::process::id()
            })
            .map(|window| {
                let title: String = window.title.chars().take(48).collect();
                let title = if window.title.chars().count() > 48 {
                    format!("{title}…")
                } else {
                    title
                };
                (
                    window.id.clone(),
                    format!(
                        "{title}  {}×{}  ({}, {})",
                        window.width, window.height, window.x, window.y
                    ),
                )
            })
            .collect();

        div()
            .id("spike-shell")
            .track_focus(&self.focus_handle)
            .key_context("SpikeShell")
            .flex()
            .flex_col()
            .size_full()
            .p_6()
            .gap_3()
            .overflow_y_scroll()
            .bg(rgb(0x1a1a1a))
            .text_color(rgb(0xf5f5f5))
            .on_action(cx.listener(|this, _: &OpenProbeOverlays, _, cx| {
                this.open_overlays(OverlayMode::Probe, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenFullscreenOverlays, _, cx| {
                this.open_overlays(OverlayMode::Fullscreen, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseOverlay, _, cx| {
                tracing::info!("CloseOverlay on spike shell");
                this.close_overlays(cx);
            }))
            .on_action(cx.listener(|this, _: &CapturePrimary, _, cx| {
                this.capture_primary(cx);
            }))
            .on_action(cx.listener(|this, _: &CaptureAll, _, cx| {
                this.capture_all(cx);
            }))
            .on_action(cx.listener(|this, _: &CaptureCenterRegion, _, cx| {
                this.capture_center_region(cx);
            }))
            .on_action(|_: &Quit, _, cx| cx.defer(close_application))
            .child("Phase 0 — overlay + XCap spike")
            .child(capture_button(
                "select-region",
                "Bereich auswählen (Strg+Umschalt+F)",
                capturing,
                cx.listener(|this, _, _, cx| this.open_overlays(OverlayMode::Fullscreen, cx)),
            ))
            .child(div().text_sm().child(self.hotkey_status.clone()))
            .child(div().text_sm().child(self.tray_status.clone()))
            .child(capture_button(
                "copy-last-capture",
                "Copy last capture + verify",
                self.copying || self.last_capture.is_none(),
                cx.listener(|this, _, _, cx| this.copy_last_capture(cx)),
            ))
            .child(div().text_sm().child(self.clipboard_status.clone()))
            .child(capture_button(
                "refresh-windows",
                "Fensterliste aktualisieren",
                capturing,
                cx.listener(|this, _, _, cx| { this.refresh_xcap_lists(); cx.notify(); }),
            ))
            .child(div().text_sm().child("Fenster aufnehmen — zum Aufnehmen anklicken"))
            .child(
                div().id("window-picker").flex_none().max_h(px(180.0)).overflow_y_scroll()
                    .flex().flex_col().gap_1().text_sm()
                    .children(window_preview.into_iter().enumerate().map(|(index, (id, label))| {
                        div().id(("capture-window", index)).flex_none().px_3().py_2()
                            .rounded_md().bg(rgb(0x27272a)).cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !capturing { this.start_capture(vec![CaptureJob::Window(id.clone())], cx); }
                            })).child(label)
                    })),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xa3a3a3))
                    .child("Fullscreen: freeze each desktop without the control window. Escape returns. Capture saves PNG and copies it."),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_sm()
                    .children(displays.into_iter().map(|(id, bounds)| {
                        div().child(format!(
                            "display {id}  origin=({}, {})  size={}×{}",
                            bounds.origin.x,
                            bounds.origin.y,
                            bounds.size.width,
                            bounds.size.height
                        ))
                    })),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xa3a3a3))
                    .child(format!(
                        "{live_count} live overlay(s), {handle_count} handle(s)"
                    )),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .id("open-probe")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(0x2563eb))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_overlays(OverlayMode::Probe, cx);
                            }))
                            .child("Open small overlay"),
                    )
                    .child(
                        div()
                            .id("close-overlays")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(0x3f3f46))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.close_overlays(cx);
                            }))
                            .child("Close overlays"),
                    ),
            )
            .child(
                div()
                    .mt_2()
                    .text_sm()
                    .text_color(rgb(0xa3a3a3))
                    .child("XCap displays (compare origin/size with GPUI above)"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_sm()
                    .children(xcap_displays.into_iter().map(|display| {
                        div().child(format!(
                            "xcap {}  {}  origin=({}, {})  size={}×{}  scale={:.2}  rot={}{}",
                            display.id,
                            display.name,
                            display.x,
                            display.y,
                            display.width,
                            display.height,
                            display.scale_factor,
                            display.rotation,
                            if display.is_primary { "  primary" } else { "" }
                        ))
                    })),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xa3a3a3))
                    .child(format!("{window_count} XCap window(s)")),
            )
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(capture_button(
                        "capture-primary",
                        "Capture primary",
                        capturing,
                        cx.listener(|this, _, _, cx| this.capture_primary(cx)),
                    ))
                    .child(capture_button(
                        "capture-all",
                        "Capture all displays",
                        capturing,
                        cx.listener(|this, _, _, cx| this.capture_all(cx)),
                    ))
                    .child(capture_button(
                        "capture-region",
                        "Capture 400×300 center",
                        capturing,
                        cx.listener(|this, _, _, cx| this.capture_center_region(cx)),
                    )),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xa3a3a3))
                    .child(capture_status),
            )
            .children(last_path.map(|path| {
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        img(path)
                            .id("last-capture")
                            .h(px(200.0))
                            .max_w_full()
                            .rounded_md(),
                    )
                    .into_any_element()
            }))
    }
}

fn capture_button(
    id: &'static str,
    label: &'static str,
    disabled: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex_none()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(if disabled {
            rgb(0x3f3f46)
        } else {
            rgb(0x16a34a)
        })
        .cursor_pointer()
        .on_click(move |event, window, cx| {
            if !disabled {
                on_click(event, window, cx);
            }
        })
        .child(label)
}

struct Overlay {
    active: bool,
    #[cfg(target_os = "windows")]
    native: Option<app_core::DesktopWindow>,
    display_id: DisplayId,
    bounds: Bounds<Pixels>,
    mode: OverlayMode,
    focus_handle: FocusHandle,
    snapshot: Option<CapturedImage>,
    drag_start: Option<gpui::Point<Pixels>>,
    drag_end: gpui::Point<Pixels>,
    owner: gpui::WeakEntity<SpikeShell>,
}

impl Overlay {
    fn new(
        display_id: DisplayId,
        bounds: Bounds<Pixels>,
        mode: OverlayMode,
        snapshot: Option<CapturedImage>,
        owner: gpui::WeakEntity<SpikeShell>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            active: true,
            #[cfg(target_os = "windows")]
            native: native_window(window)
                .map_err(|error| tracing::error!(%error, "could not obtain overlay HWND"))
                .ok(),
            display_id,
            bounds,
            mode,
            focus_handle: cx.focus_handle(),
            snapshot,
            drag_start: None,
            drag_end: point(px(0.0), px(0.0)),
            owner,
        }
    }

    fn finish_drag(&mut self, end: gpui::Point<Pixels>, window: &Window, cx: &mut Context<Self>) {
        let Some(start) = self.drag_start.take() else {
            return;
        };
        cx.notify();
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        let viewport = window.viewport_size();
        let Some(region) = app_core::PixelRegion::from_drag(
            [start.x.into(), start.y.into()],
            [end.x.into(), end.y.into()],
            [viewport.width.into(), viewport.height.into()],
            [snapshot.width, snapshot.height],
        ) else {
            return;
        };
        let owner = self.owner.clone();
        cx.defer(move |cx| {
            owner
                .update(cx, |this, cx| this.save_selection(snapshot, region, cx))
                .ok();
        });
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        tracing::info!(
            display_id = u64::from(self.display_id),
            mode = ?self.mode,
            "overlay view dropped"
        );
    }
}

impl Focusable for Overlay {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Overlay {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let scale = window.scale_factor();
        let id = u64::from(self.display_id);
        let (label, fill) = match self.mode {
            OverlayMode::Probe => ("small PopUp", rgba(0xdc2626ff)),
            OverlayMode::Fullscreen => ("fullscreen PopUp", rgba(0x1d4ed8cc)),
        };

        div()
            .id("overlay")
            .relative()
            .track_focus(&self.focus_handle)
            .key_context("Overlay")
            .flex()
            .size_full()
            .items_start()
            .justify_start()
            .bg(fill)
            .text_color(rgb(0xffffff))
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                if this.snapshot.is_some() {
                    this.drag_start = Some(event.position);
                    this.drag_end = event.position;
                    cx.notify();
                }
            }))
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                if this.drag_start.is_some() {
                    if !event.dragging() {
                        this.drag_start = None;
                        cx.notify();
                        return;
                    }
                    this.drag_end = event.position;
                    cx.notify();
                }
            }))
            .on_mouse_up(gpui::MouseButton::Left, cx.listener(|this, event: &gpui::MouseUpEvent, window, cx| {
                this.finish_drag(event.position, window, cx);
            }))
            .on_mouse_up_out(gpui::MouseButton::Left, cx.listener(|this, event: &gpui::MouseUpEvent, window, cx| {
                this.finish_drag(event.position, window, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseOverlay, _, cx| {
                tracing::info!(
                    display_id = u64::from(this.display_id),
                    mode = ?this.mode,
                    "CloseOverlay on overlay"
                );
                let owner = this.owner.clone();
                cx.defer(move |cx| {
                    owner.update(cx, |this, cx| this.close_overlays(cx)).ok();
                });
            }))
            .children(self.snapshot.clone().map(|shot| {
                img(shot.png_path).absolute().top_0().left_0().size_full()
            }))
            .children(self.drag_start.map(|start| {
                let end = self.drag_end;
                div().absolute().left(start.x.min(end.x)).top(start.y.min(end.y))
                    .w((end.x - start.x).abs()).h((end.y - start.y).abs())
                    .border_2().border_color(rgb(0x60a5fa)).bg(rgba(0x60a5fa30))
            }))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .m_6()
                    .p_6()
                    .rounded_lg()
                    .bg(rgb(0x111827))
                    .child(format!("{label}  display {id}"))
                    .child(div().text_sm().child(format!(
                        "display origin=({}, {})  size={}×{}  scale={scale:.2}",
                        self.bounds.origin.x,
                        self.bounds.origin.y,
                        self.bounds.size.width,
                        self.bounds.size.height
                    )))
                    .child(
                        div().text_sm().text_color(rgb(0xa3a3a3)).child(
                            "Drag to select. Release to save and copy. Escape cancels. Selection stays on one monitor.",
                        ),
                    ),
            )
    }
}

#[cfg(target_os = "windows")]
fn native_window(window: &Window) -> Result<app_core::DesktopWindow, String> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::Win32(handle)) => {
            Ok(app_core::DesktopWindow::from_handle(handle.hwnd.get()))
        }
        other => Err(format!("could not obtain Windows handle: {other:?}")),
    }
}

fn show_control_window(window: &Window) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    native_window(window)?
        .request_visible(true)
        .map_err(|error| error.to_string())?;
    window.activate_window();
    Ok(())
}

fn log_overlay_geometry(window: &Window, expected: Bounds<Pixels>) {
    window.on_next_frame(move |window, _| {
        window.on_next_frame(move |window, _| {
            let actual = window.bounds();
            let viewport = window.viewport_size();
            tracing::info!(
                expected_x = %expected.origin.x, expected_y = %expected.origin.y,
                actual_x = %actual.origin.x, actual_y = %actual.origin.y,
                width = %viewport.width, height = %viewport.height,
                scale = window.scale_factor(),
                "overlay geometry after presentation"
            );
        });
    });
}

fn overlay_options(
    mode: OverlayMode,
    display_id: DisplayId,
    display: Bounds<Pixels>,
) -> WindowOptions {
    // Match gpui's window_positioning example: no titlebar, no client
    // decorations, PopUp (WS_EX_TOOLWINDOW | WS_EX_TOPMOST), no focus steal.
    let bounds = match mode {
        OverlayMode::Probe => Bounds {
            origin: display.origin + point(px(48.0), px(48.0)),
            size: size(px(420.0), px(180.0)),
        },
        OverlayMode::Fullscreen => display,
    };

    WindowOptions {
        window_bounds: Some(match mode {
            OverlayMode::Probe => WindowBounds::Windowed(bounds),
            OverlayMode::Fullscreen => WindowBounds::Fullscreen(bounds),
        }),
        display_id: Some(display_id),
        titlebar: None,
        window_background: WindowBackgroundAppearance::Transparent,
        focus: matches!(mode, OverlayMode::Fullscreen),
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_min_size: None,
        window_decorations: None,
        tabbing_identifier: None,
        ..Default::default()
    }
}

fn close_application(cx: &mut App) {
    // GPUI shutdown clears its windows; avoid removing them while callbacks are still queued.
    cx.quit();
}

pub fn run() {
    tracing::info!(
        version = app_core::version(),
        "starting overlay + capture spike"
    );

    gpui_platform::application().run(|cx: &mut App| {
        cx.on_action(|_: &Quit, cx| cx.defer(close_application));
        cx.bind_keys([
            KeyBinding::new("escape", CloseOverlay, None),
            KeyBinding::new("ctrl-shift-o", OpenProbeOverlays, Some("SpikeShell")),
            KeyBinding::new("ctrl-shift-f", OpenFullscreenOverlays, Some("SpikeShell")),
            KeyBinding::new("ctrl-shift-c", CapturePrimary, Some("SpikeShell")),
            KeyBinding::new("ctrl-q", Quit, None),
        ]);

        let bounds = Bounds::centered(None, size(px(820.0), px(720.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("screenshots — phase 0 spike".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                let shell = cx.new(|cx| SpikeShell::new(window, cx));
                window.on_window_should_close(cx, |_, cx| {
                    cx.defer(close_application);
                    false
                });
                let focus = shell.read(cx).focus_handle.clone();
                window.focus(&focus, cx);
                shell
            },
        )
        .expect("failed to open spike window");
        cx.activate(true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fullscreen_overlay_keeps_entire_display_bounds() {
        for bounds in [
            Bounds {
                origin: point(px(0.0), px(0.0)),
                size: size(px(2560.0), px(1440.0)),
            },
            Bounds {
                origin: point(px(-1536.0), px(-200.0)),
                size: size(px(1536.0), px(864.0)),
            },
        ] {
            let options = overlay_options(OverlayMode::Fullscreen, DisplayId::from(1), bounds);
            assert!(
                matches!(options.window_bounds, Some(WindowBounds::Fullscreen(actual)) if actual == bounds)
            );
            assert!(options.focus);
            assert!(matches!(options.kind, WindowKind::PopUp));
        }
    }
}
