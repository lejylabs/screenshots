//! Phase 0 spike: GPUI overlay + XCap still capture on Windows.

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use app_core::{CaptureBackend, CapturedImage, DisplayInfo, WindowInfo, XcapBackend};
use gpui::{
    App, Bounds, Context, DisplayId, FocusHandle, Focusable, IntoElement, KeyBinding, Pixels,
    TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind,
    WindowOptions, actions, div, img, point, prelude::*, px, rgb, rgba, size,
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

#[derive(Clone, Copy, Debug)]
enum OverlayMode {
    Probe,
    Fullscreen,
}

pub struct SpikeShell {
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
                shell.tray_status = "Tray: Capture / Show control window / Quit".into();
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
                                    app_core::TrayCommand::Show => {
                                        if let Err(error) = control.update(cx, |_, window, cx| {
                                            window.activate_window();
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
                    shell.hotkey = Some(hotkey);
                    shell.hotkey_status = "Global capture: Ctrl+Shift+S".into();
                    tracing::info!("global capture hotkey registered: Ctrl+Shift+S");
                    cx.spawn(async move |this, cx| {
                        loop {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(50))
                                .await;
                            if this
                                .update(cx, |this, cx| {
                                    if this
                                        .hotkey
                                        .as_ref()
                                        .is_some_and(|hotkey| hotkey.take_pressed())
                                    {
                                        tracing::info!("global capture hotkey pressed");
                                        this.capture_primary(cx);
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
                    shell.hotkey_status = format!("Ctrl+Shift+S unavailable: {error}");
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
            .filter(|handle| handle.read(cx).is_ok())
            .count()
    }

    fn close_overlays(&mut self, cx: &mut Context<Self>) {
        tracing::info!(count = self.overlays.len(), "closing overlays");
        for handle in self.overlays.drain(..) {
            match handle.update(cx, |_, window, _| window.remove_window()) {
                Ok(()) => {}
                Err(error) => tracing::warn!(?error, "overlay handle already gone"),
            }
        }
        cx.notify();
    }

    fn open_overlays(&mut self, mode: OverlayMode, cx: &mut Context<Self>) {
        self.close_overlays(cx);

        let screens: Vec<(DisplayId, Bounds<Pixels>)> = cx
            .displays()
            .iter()
            .map(|display| (display.id(), display.bounds()))
            .collect();

        for (display_id, display_bounds) in screens {
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
                    cx.new(|cx| Overlay::new(display_id, display_bounds, mode, cx))
                },
            ) {
                Ok(handle) => self.overlays.push(handle),
                Err(error) => tracing::error!(?error, "failed to open overlay window"),
            }
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

    fn start_capture(&mut self, jobs: Vec<CaptureJob>, cx: &mut Context<Self>) {
        if self.capturing || self.copying {
            return;
        }
        self.close_overlays(cx);
        self.capturing = true;
        self.capture_status = format!("capturing {} job(s)…", jobs.len());
        cx.notify();

        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let mut last = None;
                    for job in jobs {
                        let shot = match &job {
                            CaptureJob::Display(id) => backend.capture_display(id)?,
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
                            width = shot.width,
                            height = shot.height,
                            elapsed_ms = shot.elapsed_ms,
                            path = %shot.png_path.display(),
                            "captured png"
                        );
                        last = Some(shot);
                    }
                    Ok::<Option<CapturedImage>, app_core::CaptureError>(last)
                })
                .await;

            this.update(cx, |this, cx| {
                this.capturing = false;
                match result {
                    Ok(Some(shot)) => {
                        this.capture_status = format!(
                            "{}×{}  {} ms  {}",
                            shot.width,
                            shot.height,
                            shot.elapsed_ms,
                            shot.png_path.display()
                        );
                        this.last_capture = Some(shot);
                        this.copy_last_capture(cx);
                    }
                    Ok(None) => this.capture_status = "no capture produced".into(),
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
        let window_preview: Vec<String> = self
            .xcap_windows
            .iter()
            .filter(|window| !window.is_minimized && !window.title.is_empty())
            .take(6)
            .map(|window| {
                let title: String = window.title.chars().take(48).collect();
                let title = if window.title.chars().count() > 48 {
                    format!("{title}…")
                } else {
                    title
                };
                format!(
                    "{title}  {}×{}  ({}, {})",
                    window.width, window.height, window.x, window.y
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
            .child(div().text_sm().child(self.hotkey_status.clone()))
            .child(div().text_sm().child(self.tray_status.clone()))
            .child(capture_button(
                "copy-last-capture",
                "Copy last capture + verify",
                self.copying || self.last_capture.is_none(),
                cx.listener(|this, _, _, cx| this.copy_last_capture(cx)),
            ))
            .child(div().text_sm().child(self.clipboard_status.clone()))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xa3a3a3))
                    .child("Probe: red PopUp top-left. Fullscreen: blue tint. Capture writes PNG to target/captures. Escape closes overlays."),
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
                            .id("open-fullscreen")
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .bg(rgb(0x2563eb))
                            .cursor_pointer()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.open_overlays(OverlayMode::Fullscreen, cx);
                            }))
                            .child("Open fullscreen overlay"),
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
                    .flex_col()
                    .gap_1()
                    .text_xs()
                    .text_color(rgb(0xa3a3a3))
                    .children(window_preview.into_iter().map(|line| div().child(line))),
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
    display_id: DisplayId,
    bounds: Bounds<Pixels>,
    mode: OverlayMode,
    focus_handle: FocusHandle,
}

impl Overlay {
    fn new(
        display_id: DisplayId,
        bounds: Bounds<Pixels>,
        mode: OverlayMode,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            display_id,
            bounds,
            mode,
            focus_handle: cx.focus_handle(),
        }
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
            .track_focus(&self.focus_handle)
            .key_context("Overlay")
            .flex()
            .size_full()
            .items_start()
            .justify_start()
            .bg(fill)
            .text_color(rgb(0xffffff))
            .on_action(cx.listener(|this, _: &CloseOverlay, window, cx| {
                tracing::info!(
                    display_id = u64::from(this.display_id),
                    mode = ?this.mode,
                    "CloseOverlay on overlay"
                );
                window.defer(cx, |window, _| window.remove_window());
            }))
            .child(
                div()
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
                            "This window must stay open. Escape or Close overlays to dismiss.",
                        ),
                    ),
            )
    }
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
        OverlayMode::Fullscreen => {
            let inset = px(16.0);
            Bounds {
                origin: display.origin + point(inset, inset),
                size: size(
                    (display.size.width - inset * 2.0).max(px(320.0)),
                    (display.size.height - inset * 2.0).max(px(240.0)),
                ),
            }
        }
    };

    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        display_id: Some(display_id),
        titlebar: None,
        window_background: WindowBackgroundAppearance::Transparent,
        focus: false,
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
    // Let GPUI own native window destruction instead of DefWindowProc's WM_CLOSE path.
    for handle in cx.windows() {
        if let Err(error) = handle.update(cx, |_, window, _| window.remove_window()) {
            tracing::warn!(%error, "failed to close application window");
        }
    }
    cx.defer(|cx| cx.quit());
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
