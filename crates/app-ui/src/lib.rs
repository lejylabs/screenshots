//! Phase 0 spike: GPUI overlay + XCap still capture on Windows.

use std::{
    collections::HashMap,
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
    refreshing: bool,
    list_status: String,
    capture_status: String,
    last_capture: Option<CapturedImage>,
    last_saved: Option<PathBuf>,
    last_render: Option<Arc<gpui::RenderImage>>,
    capturing: bool,
    clipboard: Arc<Mutex<app_core::ImageClipboard>>,
    copying: bool,
    saving: bool,
    quitting: bool,
    save_status: String,
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
            refreshing: false,
            list_status: String::new(),
            capture_status: "idle".into(),
            last_capture: None,
            last_saved: None,
            last_render: None,
            capturing: false,
            clipboard: Arc::new(Mutex::new(app_core::ImageClipboard::default())),
            copying: false,
            saving: false,
            quitting: false,
            save_status: String::new(),
            clipboard_status: "Clipboard: capture an image first".into(),
            #[cfg(target_os = "windows")]
            hotkey: None,
            hotkey_status: "Global hotkey: not tested on this platform".into(),
            #[cfg(target_os = "windows")]
            tray: None,
            tray_status: "Tray: not tested on this platform".into(),
        };
        shell.refresh_xcap_lists(cx);
        #[cfg(target_os = "windows")]
        match app_core::SpikeTray::new() {
            Ok(tray) => {
                shell.tray = Some(tray);
                shell.tray_status =
                    "Tray: Bereich auswählen / Capture / Show control window / Quit".into();
                tracing::info!("tray icon created");
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
                }
                Err(error) => {
                    tracing::warn!(%error, "global capture hotkey registration failed");
                    shell.hotkey_status = format!("Global hotkeys unavailable: {error}");
                }
            }
        }
        #[cfg(target_os = "windows")]
        {
            let mut events = app_core::desktop_events();
            let control = window.window_handle();
            cx.spawn(async move |this, cx| {
                while let Some(event) = events.recv().await {
                    if this.update(cx, |this, cx| {
                        match event {
                            app_core::DesktopEvent::Hotkey(event) => {
                                match this.hotkey.as_ref().and_then(|hotkey| hotkey.command(event)) {
                                    Some(app_core::CaptureCommand::Primary) => this.capture_primary(cx),
                                    Some(app_core::CaptureCommand::Region) => this.open_overlays(OverlayMode::Fullscreen, cx),
                                    None => {},
                                }
                            }
                            app_core::DesktopEvent::Menu(event) => {
                                match this.tray.as_ref().and_then(|tray| tray.command(event)) {
                                    Some(app_core::TrayCommand::Capture) => this.capture_primary(cx),
                                    Some(app_core::TrayCommand::Region) => this.open_overlays(OverlayMode::Fullscreen, cx),
                                    Some(app_core::TrayCommand::Show) if !this.capturing && !this.preview_active => {
                                        if let Err(error) = control.update(cx, |_, window, cx| {
                                            if let Err(error) = show_control_window(window) { tracing::error!(%error, "could not show control window"); }
                                            cx.activate(true);
                                        }) { tracing::warn!(%error, "could not show control window"); }
                                    }
                                    Some(app_core::TrayCommand::Quit) => cx.defer(close_application),
                                    _ => {},
                                }
                            }
                        }
                    }).is_err() { break; }
                }
            }).detach();
        }
        shell
    }

    fn refresh_xcap_lists(&mut self, cx: &mut Context<Self>) {
        if self.refreshing || self.quitting {
            return;
        }
        self.refreshing = true;
        self.list_status = "Displays/Fenster werden geladen…".into();
        cx.notify();
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let (displays, windows) = cx
                .background_spawn(async move { (backend.list_displays(), backend.list_windows()) })
                .await;
            this.update(cx, |this, cx| {
                this.refreshing = false;
                if this.quitting {
                    return;
                }
                let mut errors = Vec::new();
                match displays {
                    Ok(displays) => this.xcap_displays = displays,
                    Err(error) => {
                        tracing::warn!(%error, "display enumeration failed");
                        errors.push(format!("Displays: {error}"));
                        this.xcap_displays.clear();
                    }
                }
                match windows {
                    Ok(windows) => this.xcap_windows = windows,
                    Err(error) => {
                        tracing::warn!(%error, "window enumeration failed");
                        errors.push(format!("Fenster: {error}"));
                        this.xcap_windows.clear();
                    }
                }
                this.list_status = if errors.is_empty() {
                    format!(
                        "{} Display(s), {} Fenster geladen",
                        this.xcap_displays.len(),
                        this.xcap_windows.len()
                    )
                } else {
                    errors.join("; ")
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
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
                    if let Some(render) = overlay.render_image.take() {
                        if let Err(error) = window.drop_image(render) {
                            tracing::warn!(%error, "could not release overlay image atlas entry");
                        }
                    }
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
        if self.capturing || self.copying || self.saving || self.quitting {
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
        self.show_overlays(mode, &[], &HashMap::new(), cx);
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
        renders: &HashMap<PathBuf, Arc<gpui::RenderImage>>,
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
                    overlay.render_image = snapshot
                        .as_ref()
                        .and_then(|shot| renders.get(&shot.png_path))
                        .cloned();
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
                            snapshot.clone(),
                            snapshot
                                .as_ref()
                                .and_then(|shot| renders.get(&shot.png_path))
                                .cloned(),
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

    fn replace_last_render(&mut self, render: Arc<gpui::RenderImage>, cx: &mut Context<Self>) {
        if let Some(previous) = self.last_render.replace(render) {
            let control = self.control;
            cx.defer(move |cx| {
                control
                    .update(cx, |_, window, _| {
                        if let Err(error) = window.drop_image(previous) {
                            tracing::warn!(%error, "could not release previous thumbnail");
                        }
                    })
                    .ok();
            });
        }
    }

    fn finish_quit_if_idle(&self, cx: &mut Context<Self>) {
        if self.quitting && !self.capturing && !self.copying && !self.saving {
            cx.defer(close_application);
        }
    }

    fn copy_last_capture(&mut self, cx: &mut Context<Self>) {
        self.copy_and_save(Vec::new(), cx);
    }

    fn copy_and_save(&mut self, saves: Vec<CapturedImage>, cx: &mut Context<Self>) {
        if self.copying || self.saving || self.quitting {
            return;
        }
        let Some(path) = self
            .last_capture
            .as_ref()
            .map(|shot| shot.png_path.clone())
            .or_else(|| self.last_saved.clone())
        else {
            return;
        };
        let pixels = self.last_capture.as_ref().map(|shot| shot.pixels.clone());
        tracing::info!(path = %path.display(), "copying capture to clipboard");
        let clipboard = self.clipboard.clone();
        self.copying = true;
        self.saving = !saves.is_empty();
        if self.saving {
            self.save_status = "Save: pending clipboard completion".into();
        }
        self.clipboard_status = "Clipboard: copying…".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let pixels = match pixels {
                        Some(pixels) => pixels,
                        None => {
                            let mut reader = image::ImageReader::open(&path)
                                .map_err(|error| error.to_string())?;
                            let mut limits = image::Limits::default();
                            limits.max_alloc = Some(256 * 1024 * 1024);
                            reader.limits(limits);
                            let decoded = reader.decode().map_err(|error| error.to_string())?;
                            let bytes = u64::from(decoded.width())
                                .checked_mul(u64::from(decoded.height()))
                                .and_then(|pixels| pixels.checked_mul(4));
                            if bytes.is_none_or(|bytes| bytes > 256 * 1024 * 1024) {
                                return Err("saved image exceeds the 256 MiB RGBA limit".into());
                            }
                            Arc::new(decoded.into_rgba8())
                        }
                    };
                    clipboard
                        .lock()
                        .map_err(|error| error.to_string())?
                        .copy_image(&pixels)
                        .map_err(|error| format!("{error:#}"))
                })
                .await;
            this.update(cx, |this, cx| {
                this.copying = false;
                this.clipboard_status = match result {
                    Ok((width, height)) => {
                        tracing::info!(width, height, "clipboard image copied");
                        format!("Clipboard: {width}×{height} copied")
                    }
                    Err(error) => {
                        tracing::error!(%error, "clipboard copy failed");
                        format!("Clipboard failed: {error}")
                    }
                };
                this.finish_quit_if_idle(cx);
                cx.notify();
            })
            .ok();
            if !saves.is_empty() {
                let results = cx
                    .background_spawn(async move {
                        saves
                            .into_iter()
                            .map(|shot| {
                                (
                                    shot.png_path.clone(),
                                    shot.save_png().map_err(|error| error.to_string()),
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                    .await;
                this.update(cx, |this, cx| {
                    this.saving = false;
                    let errors: Vec<_> = results
                        .iter()
                        .filter_map(|(path, result)| {
                            result
                                .as_ref()
                                .err()
                                .map(|error| format!("{}: {error}", path.display()))
                        })
                        .collect();
                    this.save_status = if errors.is_empty() {
                        this.last_saved =
                            this.last_capture.as_ref().map(|shot| shot.png_path.clone());
                        this.last_capture = None;
                        tracing::info!(count = results.len(), "capture outputs saved");
                        format!("Save: {} PNG(s) saved", results.len())
                    } else {
                        this.quitting = false;
                        this.restore_control(cx);
                        tracing::error!(errors = %errors.join("; "), "capture output save failed");
                        format!("Save failed: {}", errors.join("; "))
                    };
                    this.finish_quit_if_idle(cx);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn save_selection(
        &mut self,
        source: CapturedImage,
        region: app_core::PixelRegion,
        cx: &mut Context<Self>,
    ) {
        if self.capturing || self.copying || self.saving || self.quitting {
            return;
        }
        self.close_overlays(cx);
        self.capturing = true;
        self.capture_status = "Preparing selection…".into();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { app_core::crop_snapshot(&source, region).map(|shot| { let render = render_thumbnail(&shot); (shot, render) }) }).await;
            this.update(cx, |this, cx| {
                this.capturing = false;
                if this.quitting { this.finish_quit_if_idle(cx); return; }
                match result {
                    Ok((shot, render)) => {
                        this.replace_last_render(render, cx);
                        tracing::info!(width = shot.width, height = shot.height, path = %shot.png_path.display(), "selection prepared");
                        this.capture_status = format!("Selection {}×{}  {}", shot.width, shot.height, shot.png_path.display());
                        this.last_capture = Some(shot.clone());
                        this.copy_and_save(vec![shot], cx);
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
        if self.capturing || self.copying || self.saving || self.quitting {
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
                        if this.quitting {
                            this.finish_quit_if_idle(cx);
                            return;
                        }
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
                    let mut renders = HashMap::new();
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
                            "captured frame"
                        );

                        renders.insert(
                            shot.png_path.clone(),
                            if preview {
                                render_frame(&shot)
                            } else {
                                render_thumbnail(&shot)
                            },
                        );
                        shots.push(shot);
                    }
                    Ok::<_, app_core::CaptureError>((shots, renders))
                })
                .await;

            this.update(cx, |this, cx| {
                this.capturing = false;
                if this.quitting {
                    this.finish_quit_if_idle(cx);
                    return;
                }
                if !preview || result.is_err() {
                    this.restore_control(cx);
                }
                match result {
                    Ok((shots, renders)) => {
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
                        if preview {
                            this.show_overlays(OverlayMode::Fullscreen, &shots, &renders, cx);
                        } else {
                            if let Some(render) = renders.get(&shot.png_path).cloned() {
                                this.replace_last_render(render, cx);
                            }
                            this.last_capture = Some(shot);
                            this.copy_and_save(shots, cx);
                        }
                    }
                    Err(error) => {
                        tracing::error!(%error, "capture failed");
                        this.capture_status = format!("capture failed: {error}");
                    }
                }

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
        let capturing = self.capturing || self.copying || self.saving || self.quitting;
        let capture_status = self.capture_status.clone();
        let last_path = self.last_render.clone();
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
                "Copy last capture",
                self.copying || (self.last_capture.is_none() && self.last_saved.is_none()),
                cx.listener(|this, _, _, cx| this.copy_last_capture(cx)),
            ))
            .child(div().text_sm().child(self.clipboard_status.clone()))
            .child(div().text_sm().child(self.save_status.clone()))
            .child(div().text_sm().child(self.list_status.clone()))
            .child(capture_button(
                "refresh-windows",
                "Fensterliste aktualisieren",
                capturing || self.refreshing,
                cx.listener(|this, _, _, cx| { this.refresh_xcap_lists(cx); }),
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

fn render_frame(shot: &CapturedImage) -> Arc<gpui::RenderImage> {
    let mut bgra = shot.pixels.as_ref().clone();
    for pixel in bgra.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(gpui::RenderImage::new(vec![image::Frame::new(bgra)]))
}

fn render_thumbnail(shot: &CapturedImage) -> Arc<gpui::RenderImage> {
    let mut pixels = image::imageops::thumbnail(shot.pixels.as_ref(), 400, 240);
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(gpui::RenderImage::new(vec![image::Frame::new(pixels)]))
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
    render_image: Option<Arc<gpui::RenderImage>>,
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
        render_image: Option<Arc<gpui::RenderImage>>,
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
            render_image,
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
            .children(self.render_image.clone().map(|render| {
                img(render).absolute().top_0().left_0().size_full()
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
    for handle in cx.windows() {
        if let Some(control) = handle.downcast::<SpikeShell>() {
            let waiting = control
                .update(cx, |this, _, cx| {
                    this.quitting = true;
                    let busy = this.capturing || this.copying || this.saving;
                    if busy {
                        this.capture_status = "Quitting: waiting for active work to finish…".into();
                        this.restore_control(cx);
                        cx.notify();
                    }
                    busy
                })
                .unwrap_or(false);
            if waiting {
                return;
            }
        }
    }
    cx.quit();
}

pub fn run() {
    if let Some(mode) =
        std::env::args().find_map(|arg| arg.strip_prefix("--resource-probe=").map(str::to_owned))
    {
        run_resource_probe(&mode);
        return;
    }
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

/// Explicit diagnostic mode, never enabled by an ordinary application launch.
fn run_resource_probe(mode: &str) {
    assert!(
        matches!(mode, "headless" | "empty"),
        "unknown resource probe mode"
    );
    let seconds = std::env::args()
        .find_map(|arg| {
            arg.strip_prefix("--probe-seconds=")
                .and_then(|value| value.parse::<u64>().ok())
        })
        .unwrap_or(45)
        .clamp(10, 660);
    struct Empty;
    impl Render for Empty {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().bg(rgb(0x1a1a1a))
        }
    }
    let application = gpui_platform::application();
    let application = if mode == "headless" {
        application.with_windowing(gpui::WindowingRequest::Headless)
    } else {
        application
    };
    let show_window = mode == "empty";
    application.run(move |cx| {
        if show_window {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                        None,
                        size(px(820.0), px(720.0)),
                        cx,
                    ))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("screenshots resource probe".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    window.on_window_should_close(cx, |_, cx| {
                        cx.defer(|cx| cx.quit());
                        false
                    });
                    cx.new(|_| Empty)
                },
            )
            .expect("open empty probe window");
        }
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_secs(seconds))
                .await;
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "opens GPUI, saves a test image and replaces clipboard contents"]
    fn saved_pixels_are_released_and_explicit_recopy_works() {
        let path = std::env::temp_dir().join(format!(
            "screenshots-release-{}.png",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let output = path.clone();
        let result = Arc::new(Mutex::new(None));
        let report = result.clone();
        gpui_platform::application().run(move |cx| {
            let control = cx
                .open_window(WindowOptions::default(), |window, cx| {
                    cx.new(|cx| SpikeShell::new(window, cx))
                })
                .unwrap();
            let pixels = Arc::new(image::RgbaImage::from_pixel(
                2,
                2,
                image::Rgba([12, 34, 56, 255]),
            ));
            let weak_pixels = Arc::downgrade(&pixels);
            let shell = control
                .update(cx, |this, _, cx| {
                    let shot = CapturedImage {
                        display_id: "test".into(),
                        window_id: None,
                        width: 2,
                        height: 2,
                        pixels,
                        png_path: output,
                        elapsed_ms: 0,
                        source_format: app_core::PixelFormat::Rgba8UnormSrgb,
                    };
                    this.last_capture = Some(shot.clone());
                    this.copy_and_save(vec![shot], cx);
                    cx.entity().downgrade()
                })
                .unwrap();
            cx.spawn(async move |cx| {
                let check = async {
                    for stage in 0..2 {
                        let started = std::time::Instant::now();
                        loop {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(10))
                                .await;
                            let idle = shell
                                .update(cx, |this, _| !this.copying && !this.saving)
                                .map_err(|error| error.to_string())?;
                            if idle {
                                break;
                            }
                            if started.elapsed().as_secs() > 8 {
                                return Err("output timed out".to_string());
                            }
                        }
                        let valid = shell
                            .update(cx, |this, _| {
                                this.last_capture.is_none()
                                    && this.last_saved.is_some()
                                    && this.clipboard_status.contains("copied")
                            })
                            .map_err(|error| error.to_string())?;
                        if !valid || weak_pixels.upgrade().is_some() {
                            return Err("source pixels retained or copy/save failed".into());
                        }
                        if stage == 0 {
                            shell
                                .update(cx, |this, cx| this.copy_last_capture(cx))
                                .map_err(|error| error.to_string())?;
                        }
                    }
                    Ok(())
                }
                .await;
                *report.lock().unwrap() = Some(check);
                cx.update(close_application);
            })
            .detach();
        });
        result
            .lock()
            .unwrap()
            .take()
            .expect("test did not complete")
            .unwrap();
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn thumbnail_is_bounded_bgra_without_changing_source_pixels() {
        let source = CapturedImage {
            display_id: "test".into(),
            window_id: None,
            width: 800,
            height: 480,
            pixels: Arc::new(image::RgbaImage::from_pixel(
                800,
                480,
                image::Rgba([12, 34, 56, 255]),
            )),
            png_path: PathBuf::from("unused.png"),
            elapsed_ms: 0,
            source_format: app_core::PixelFormat::Rgba8UnormSrgb,
        };
        let render = render_thumbnail(&source);
        let bytes = render.as_bytes(0).unwrap();
        assert_eq!(bytes.len(), 400 * 240 * 4);
        assert_eq!(&bytes[..4], &[56, 34, 12, 255]);
        assert_eq!(source.pixels.get_pixel(0, 0).0, [12, 34, 56, 255]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "opens GPUI and replaces clipboard contents with a small test image"]
    fn quit_waits_for_copy_and_png_save() {
        let path = std::env::temp_dir().join(format!(
            "screenshots-quit-{}.png",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let output = path.clone();
        let waited = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed = waited.clone();
        gpui_platform::application().run(move |cx| {
            let control = cx
                .open_window(WindowOptions::default(), |window, cx| {
                    cx.new(|cx| SpikeShell::new(window, cx))
                })
                .unwrap();
            control
                .update(cx, |this, _, cx| {
                    let shot = CapturedImage {
                        display_id: "test".into(),
                        window_id: None,
                        width: 2,
                        height: 2,
                        png_path: output,
                        pixels: Arc::new(image::RgbaImage::from_pixel(
                            2,
                            2,
                            image::Rgba([12, 34, 56, 255]),
                        )),
                        elapsed_ms: 0,
                        source_format: app_core::PixelFormat::Rgba8UnormSrgb,
                    };
                    this.last_capture = Some(shot.clone());
                    this.copy_and_save(vec![shot], cx);
                })
                .unwrap();
            close_application(cx);
            observed.store(
                control
                    .read(cx)
                    .is_ok_and(|shell| shell.quitting && shell.saving),
                std::sync::atomic::Ordering::SeqCst,
            );
            cx.spawn(async move |cx| {
                cx.background_executor()
                    .timer(std::time::Duration::from_secs(10))
                    .await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
        assert!(
            waited.load(std::sync::atomic::Ordering::SeqCst),
            "quit did not wait for output"
        );
        let decoded = image::open(&path)
            .expect("PNG must be complete before quit")
            .into_rgba8();
        assert_eq!(decoded.get_pixel(0, 0).0, [12, 34, 56, 255]);
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(target_os = "windows")]
    #[test]
    #[ignore = "opens real GPUI overlays and saves desktop captures; do not use the clipboard during this test"]
    fn repeated_region_cancel_preserves_clipboard_and_reuses_windows() {
        use windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber;
        let capture_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/captures/region-cancel-test");
        let file_count = || {
            std::fs::read_dir(&capture_dir)
                .map(|entries| entries.count())
                .unwrap_or(0)
        };
        let files_before = file_count();
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::ERROR)
            .with_test_writer()
            .try_init()
            .expect("initialize regression-test logging");
        let outcome = Arc::new(Mutex::new(None));
        let reported = outcome.clone();
        gpui_platform::application().run(move |cx| {
            let opened = cx.open_window(WindowOptions::default(), |window, cx| {
                cx.new(|cx| {
                    let mut shell = SpikeShell::new(window, cx);
                    shell.backend = XcapBackend::new(
                        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                            .join("../../target/captures/region-cancel-test"),
                    );
                    shell
                })
            });
            let control = match opened {
                Ok(handle) => handle,
                Err(error) => {
                    *reported.lock().unwrap() = Some(Err(format!("open control window: {error}")));
                    cx.quit();
                    return;
                }
            };
            let shell = control
                .update(cx, |_, _, cx| cx.entity().downgrade())
                .unwrap();
            cx.spawn(async move |cx| {
                let result = async {
                    let clipboard_before = unsafe { GetClipboardSequenceNumber() };
                    let mut first_handles = None;
                    for _ in 0..2 {
                        shell
                            .update(cx, |this, cx| {
                                this.open_overlays(OverlayMode::Fullscreen, cx)
                            })
                            .map_err(|error| error.to_string())?;
                        let started = std::time::Instant::now();
                        loop {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(25))
                                .await;
                            let (capturing, active, status, handles) = shell
                                .update(cx, |this, _| {
                                    (
                                        this.capturing,
                                        this.preview_active,
                                        this.capture_status.clone(),
                                        this.overlays
                                            .iter()
                                            .map(|handle| handle.window_id())
                                            .collect::<Vec<_>>(),
                                    )
                                })
                                .map_err(|error| error.to_string())?;
                            if !capturing {
                                if !active {
                                    return Err(format!("preview did not open: {status}"));
                                }
                                if let Some(first) = &first_handles {
                                    if *first != handles {
                                        return Err("overlay windows were recreated".into());
                                    }
                                } else {
                                    first_handles = Some(handles);
                                }
                                break;
                            }
                            if started.elapsed() > std::time::Duration::from_secs(8) {
                                return Err("capture timed out".into());
                            }
                        }
                        // Same handler used by Escape; no physical mouse/keyboard input is injected.
                        shell
                            .update(cx, |this, cx| this.close_overlays(cx))
                            .map_err(|error| error.to_string())?;
                        let started = std::time::Instant::now();
                        loop {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(25))
                                .await;
                            let restored = control
                                .update(cx, |this, window, cx| {
                                    window.is_visible()
                                        && !this.preview_active
                                        && this.live_overlay_count(cx) == 0
                                        && this.overlays.iter().all(|handle| {
                                            handle.read(cx).is_ok_and(|overlay| {
                                                overlay.snapshot.is_none()
                                                    && overlay.drag_start.is_none()
                                            })
                                        })
                                })
                                .map_err(|error| error.to_string())?;
                            if restored {
                                break;
                            }
                            if started.elapsed() > std::time::Duration::from_secs(3) {
                                return Err("cancel did not restore the control window".into());
                            }
                        }
                        if unsafe { GetClipboardSequenceNumber() } != clipboard_before {
                            return Err("cancel changed the clipboard sequence".into());
                        }
                    }
                    Ok(())
                }
                .await;
                *reported.lock().unwrap() = Some(result);
                cx.update(close_application);
            })
            .detach();
        });
        outcome
            .lock()
            .unwrap()
            .take()
            .expect("regression test did not finish")
            .unwrap();
        assert_eq!(
            file_count(),
            files_before,
            "cancel persisted desktop previews"
        );
    }

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
